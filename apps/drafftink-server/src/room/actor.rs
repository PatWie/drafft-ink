//! The task that owns one loaded room: its Loro document, its peers and its writes to the store.

use super::*;

pub(super) struct Peer {
    client_id: ClientId,
    user: Option<UserInfo>,
    awareness: Option<AwarenessState>,
    outbox: Outbox,
}

pub(super) struct RoomActor {
    manager: Arc<RoomManager>,
    id: RoomId,
    generation: u64,
    doc: LoroDoc,
    peers: HashMap<u64, Peer>,
    by_client: HashMap<ClientId, u64>,
    /// Version of the document as last written to the store.
    flushed: VersionVector,
    dirty: bool,
    flush_at: Option<Instant>,
    log_entries: u64,
    log_bytes: u64,
    snapshot_len: usize,
    idle_since: Option<Instant>,
    poisoned: bool,
}

/// Runs `f`, turning a panic inside the CRDT library into `None`. The room is then shut down
/// instead of serving a document in an unknown state.
fn guarded<T>(poisoned: &mut bool, f: impl FnOnce() -> T) -> Option<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(v) => Some(v),
        Err(_) => {
            *poisoned = true;
            None
        }
    }
}

async fn sleep_until_opt(deadline: Option<Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending().await,
    }
}

impl RoomActor {
    pub(super) fn new(manager: Arc<RoomManager>, id: RoomId, generation: u64) -> Self {
        RoomActor {
            manager,
            id,
            generation,
            doc: LoroDoc::new(),
            peers: HashMap::new(),
            by_client: HashMap::new(),
            flushed: VersionVector::default(),
            dirty: false,
            flush_at: None,
            log_entries: 0,
            log_bytes: 0,
            snapshot_len: 0,
            idle_since: Some(Instant::now()),
            poisoned: false,
        }
    }

    fn cfg(&self) -> &Config {
        &self.manager.config
    }

    fn tag(&self) -> String {
        room_tag(&self.id)
    }

    async fn store_call<T>(
        &self,
        what: &'static str,
        fut: impl std::future::Future<Output = Result<T, StoreError>>,
    ) -> Result<T, StoreError> {
        match tokio::time::timeout(self.cfg().timing.store_timeout, fut).await {
            Ok(r) => r,
            Err(_) => Err(StoreError::Timeout { what }),
        }
    }

    // ---- lifecycle ------------------------------------------------------------------------

    async fn load(&mut self) -> Result<(), Refusal> {
        let stored = match self.store_call("load", self.manager.store.load(&self.id)).await {
            Ok(s) => s,
            Err(StoreError::InvalidRoom(why)) => return Err(refuse(ErrorCode::Malformed, why)),
            Err(e) => {
                error!("room {}: cannot load: {e}", self.tag());
                return Err(unavailable("the room cannot be loaded from storage; try again later", 5000));
            }
        };
        let Some(StoredRoom { snapshot, updates }) = stored else { return Ok(()) };
        let snapshot_len = snapshot.as_ref().map_or(0, Vec::len);
        let result = guarded(&mut self.poisoned, || {
            if let Some(s) = &snapshot {
                self.doc.import(s)?;
            }
            for u in &updates {
                self.doc.import(u)?;
            }
            Ok::<(), loro::LoroError>(())
        });
        let log_bytes: u64 = updates.iter().map(|u| u.len() as u64).sum();
        match result {
            Some(Ok(())) => {}
            Some(Err(e)) => {
                error!("room {}: stored data does not import: {e}", self.tag());
                return Err(unavailable("the stored room data is damaged; it was not changed", 60_000));
            }
            None => {
                error!("room {}: the CRDT library panicked while loading", self.tag());
                return Err(unavailable("the stored room data cannot be read; it was not changed", 60_000));
            }
        }
        self.flushed = self.doc.oplog_vv();
        self.log_entries = updates.len() as u64;
        self.log_bytes = log_bytes;
        self.snapshot_len = snapshot_len;
        debug!("room {} loaded: snapshot {} bytes, {} log entries", self.tag(), snapshot_len, updates.len());
        Ok(())
    }

    pub(super) async fn run(mut self, mut rx: mpsc::Receiver<RoomCmd>) {
        if let Err(msg) = self.load().await {
            self.manager.remove(&self.id, self.generation).await;
            rx.close();
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    RoomCmd::Join { reply, .. } => {
                        let _ = reply.send(Err(msg.clone()));
                    }
                    RoomCmd::Shutdown { done } => {
                        let _ = done.send(());
                    }
                    _ => {}
                }
            }
            return;
        }
        loop {
            let idle_at = self.idle_since.map(|t| t + self.cfg().timing.room_idle);
            tokio::select! {
                cmd = rx.recv() => {
                    let Some(cmd) = cmd else { break };
                    if let RoomCmd::Shutdown { done } = cmd {
                        self.stop(unavailable("the server is shutting down", 5000)).await;
                        self.finish(&mut rx).await;
                        let _ = done.send(());
                        return;
                    }
                    self.handle(cmd).await;
                    if self.poisoned {
                        error!("room {} stopped after a failure inside the CRDT library", self.tag());
                        self.stop(unavailable("the room hit an internal error; reconnect to continue", 2000)).await;
                        self.finish(&mut rx).await;
                        return;
                    }
                }
                () = sleep_until_opt(self.flush_at) => {
                    self.flush().await;
                }
                () = sleep_until_opt(idle_at), if self.peers.is_empty() => {
                    if self.try_unload(&mut rx).await {
                        return;
                    }
                }
            }
        }
        self.finish(&mut rx).await;
    }

    /// Flushes, compacts when worthwhile and unloads if nothing arrived meanwhile.
    async fn try_unload(&mut self, rx: &mut mpsc::Receiver<RoomCmd>) -> bool {
        self.flush().await;
        if self.dirty {
            self.idle_since = Some(Instant::now());
            return false;
        }
        if self.log_entries > 0 {
            self.compact().await;
        }
        if let Ok(cmd) = rx.try_recv() {
            self.handle(cmd).await;
            return false;
        }
        info!("room {} unloaded", self.tag());
        self.finish(rx).await;
        true
    }

    /// Leaves the table and refuses what is still queued; senders retry on a fresh room.
    async fn finish(&mut self, rx: &mut mpsc::Receiver<RoomCmd>) {
        self.manager.remove(&self.id, self.generation).await;
        rx.close();
        while let Some(cmd) = rx.recv().await {
            if let RoomCmd::Shutdown { done } = cmd {
                let _ = done.send(());
            }
        }
    }

    /// Tells every peer to go, then saves the room.
    async fn stop(&mut self, why: Refusal) {
        let why = Arc::new(*why);
        for peer in self.peers.values() {
            peer.outbox.send(why.clone(), Kind::Final);
        }
        self.peers.clear();
        self.by_client.clear();
        self.flush().await;
        if !self.dirty && self.log_entries > 0 && !self.poisoned {
            self.compact().await;
        }
    }

    // ---- commands -------------------------------------------------------------------------

    async fn handle(&mut self, cmd: RoomCmd) {
        match cmd {
            RoomCmd::Join { peer, reply } => {
                let result = self.on_join(*peer);
                let _ = reply.send(result);
            }
            RoomCmd::Update { conn, seq, bytes } => self.on_update(conn, seq, bytes).await,
            RoomCmd::RequestSync { conn, version } => self.on_request_sync(conn, &version),
            RoomCmd::Awareness { conn, state } => self.on_awareness(conn, state),
            RoomCmd::Leave { conn } => self.on_leave(conn),
            RoomCmd::Shutdown { done } => {
                let _ = done.send(());
            }
        }
    }

    fn broadcast(&self, except: u64, msg: ServerMessage, kind: Kind) {
        let msg = Arc::new(msg);
        for (conn, peer) in &self.peers {
            if *conn != except {
                peer.outbox.send(msg.clone(), kind);
            }
        }
    }

    /// The Loro bytes a client with version `client` lacks.
    fn sync_payload(&mut self, client: Option<VersionVector>) -> Result<(SyncKind, Vec<u8>), Refusal> {
        let internal = || refuse(ErrorCode::Internal, "the server could not prepare the room data");
        let too_large = || {
            refuse(
                ErrorCode::TooLarge,
                "the room is larger than one sync message can carry; images embedded in the document add up",
            )
        };
        let snapshot = |doc: &LoroDoc, poisoned: &mut bool| -> Result<Vec<u8>, Refusal> {
            match guarded(poisoned, || doc.export(ExportMode::Snapshot)) {
                Some(Ok(b)) => Ok(b),
                _ => Err(internal()),
            }
        };
        let (kind, data) = match client {
            None => (SyncKind::Snapshot, snapshot(&self.doc, &mut self.poisoned)?),
            Some(vv) => {
                let ours = self.doc.oplog_vv();
                if vv.includes_vv(&ours) {
                    return Ok((SyncKind::Updates, Vec::new()));
                }
                let diff = guarded(&mut self.poisoned, || self.doc.export(ExportMode::updates(&vv)));
                match diff {
                    Some(Ok(d))
                        if self.snapshot_len == 0 || d.len() <= self.snapshot_len && d.len() <= MAX_SYNC_BYTES =>
                    {
                        (SyncKind::Updates, d)
                    }
                    // The diff is larger than a snapshot, too large for a message, or cannot be
                    // computed from this version: send everything.
                    _ => (SyncKind::Snapshot, snapshot(&self.doc, &mut self.poisoned)?),
                }
            }
        };
        if data.len() > MAX_SYNC_BYTES {
            if kind == SyncKind::Updates {
                let all = snapshot(&self.doc, &mut self.poisoned)?;
                if all.len() <= MAX_SYNC_BYTES {
                    return Ok((SyncKind::Snapshot, all));
                }
            }
            return Err(too_large());
        }
        Ok((kind, data))
    }

    fn parse_version(bytes: &[u8]) -> Result<Option<VersionVector>, Refusal> {
        if bytes.is_empty() {
            return Ok(None);
        }
        let vv = VersionVector::decode(bytes)
            .map_err(|_| refuse(ErrorCode::Malformed, "the version vector cannot be read"))?;
        Ok(if vv == VersionVector::default() { None } else { Some(vv) })
    }

    fn on_join(&mut self, setup: PeerSetup) -> Result<(), Refusal> {
        let replaced = self.by_client.get(&setup.client_id).copied();
        if replaced.is_none() && self.peers.len() >= self.cfg().limits.max_peers_per_room {
            return Err(refuse(ErrorCode::RoomFull, "this room has reached its limit of peers"));
        }
        let client = Self::parse_version(&setup.version)?;
        let (kind, data) = self.sync_payload(client)?;
        if self.poisoned {
            return Err(unavailable("the room hit an internal error; try again", 2000));
        }
        if let Some(old_conn) = replaced
            && let Some(old) = self.peers.remove(&old_conn)
        {
            old.outbox.send(
                Arc::new(ServerMessage::error(
                    ErrorCode::Replaced,
                    "another connection joined with the same client id",
                )),
                Kind::Final,
            );
            self.by_client.remove(&old.client_id);
            self.broadcast(setup.conn, ServerMessage::PeerLeft { client_id: old.client_id }, Kind::BestEffort);
        }
        let others: Vec<PeerInfo> = self
            .peers
            .values()
            .map(|p| PeerInfo { client_id: p.client_id, user: p.user.clone() })
            .take(drafftink_protocol::MAX_PEERS)
            .collect();
        let server_version = self.doc.oplog_vv().encode();
        let out = &setup.outbox;
        out.send(
            Arc::new(ServerMessage::Joined {
                protocol_version: setup.protocol_version,
                server: concat!("drafftink-headless/", env!("CARGO_PKG_VERSION")).to_owned(),
                room: self.id.clone(),
                client_id: setup.client_id,
                peers: others,
                server_version: server_version.clone(),
                limits: self.cfg().limits.announced(),
            }),
            Kind::Control,
        );
        out.send(Arc::new(ServerMessage::SyncResponse { kind, data, server_version }), Kind::Control);
        for p in self.peers.values() {
            if let Some(state) = &p.awareness {
                out.send(
                    Arc::new(ServerMessage::PeerAwareness { from: p.client_id, state: state.clone() }),
                    Kind::BestEffort,
                );
            }
        }
        self.broadcast(
            setup.conn,
            ServerMessage::PeerJoined { peer: PeerInfo { client_id: setup.client_id, user: setup.user.clone() } },
            Kind::BestEffort,
        );
        self.by_client.insert(setup.client_id, setup.conn);
        self.peers.insert(
            setup.conn,
            Peer { client_id: setup.client_id, user: setup.user, awareness: None, outbox: setup.outbox },
        );
        self.idle_since = None;
        debug!("room {}: peer joined, {} in room", self.tag(), self.peers.len());
        Ok(())
    }

    fn on_leave(&mut self, conn: u64) {
        let Some(peer) = self.peers.remove(&conn) else { return };
        if self.by_client.get(&peer.client_id) == Some(&conn) {
            self.by_client.remove(&peer.client_id);
        }
        self.broadcast(conn, ServerMessage::PeerLeft { client_id: peer.client_id }, Kind::BestEffort);
        if self.peers.is_empty() {
            self.idle_since = Some(Instant::now());
        }
        debug!("room {}: peer left, {} in room", self.tag(), self.peers.len());
    }

    fn on_request_sync(&mut self, conn: u64, version: &[u8]) {
        let Some(outbox) = self.peers.get(&conn).map(|p| p.outbox.clone()) else { return };
        let result = Self::parse_version(version).and_then(|client| self.sync_payload(client));
        match result {
            Ok((kind, data)) => {
                let server_version = self.doc.oplog_vv().encode();
                outbox.send(Arc::new(ServerMessage::SyncResponse { kind, data, server_version }), Kind::Control);
            }
            Err(msg) => {
                let kind = match &*msg {
                    ServerMessage::Error { code, .. } if code.closes_connection() => Kind::Final,
                    _ => Kind::Control,
                };
                outbox.send(Arc::new(*msg), kind);
            }
        }
    }

    fn on_awareness(&mut self, conn: u64, state: AwarenessState) {
        let Some(peer) = self.peers.get_mut(&conn) else { return };
        if state.user.is_some() {
            peer.user = state.user.clone();
        }
        let from = peer.client_id;
        peer.awareness = Some(state.clone());
        self.broadcast(conn, ServerMessage::PeerAwareness { from, state }, Kind::BestEffort);
    }

    async fn on_update(&mut self, conn: u64, seq: u32, bytes: Vec<u8>) {
        let Some(peer) = self.peers.get(&conn) else { return };
        let outbox = peer.outbox.clone();
        let from = peer.client_id;
        let stored = self.snapshot_len as u64 + self.log_bytes;
        if stored.saturating_add(bytes.len() as u64) > self.cfg().limits.max_room_bytes {
            outbox.send(
                Arc::new(ServerMessage::error(ErrorCode::TooLarge, "this room has reached its storage limit")),
                Kind::Control,
            );
            return;
        }
        let before = self.doc.oplog_vv();
        let before_frontiers = self.doc.oplog_frontiers();
        let imported = guarded(&mut self.poisoned, || self.doc.import(&bytes));
        let status = match imported {
            Some(Ok(status)) => status,
            Some(Err(e)) => {
                outbox.send(
                    Arc::new(ServerMessage::error(
                        ErrorCode::InvalidUpdate,
                        format!("the update cannot be applied: {e}"),
                    )),
                    Kind::Control,
                );
                return;
            }
            None => {
                outbox.send(
                    Arc::new(ServerMessage::error(ErrorCode::InvalidUpdate, "the update cannot be applied")),
                    Kind::Control,
                );
                return;
            }
        };
        if let Some(found) = guarded(&mut self.poisoned, || schema::beyond_served(&self.doc)).flatten() {
            self.roll_back_to(&before_frontiers);
            warn!("room {}: refused an update that raises meta.schema to {found}", self.tag());
            outbox.send(
                Arc::new(ServerMessage::error(
                    ErrorCode::InvalidUpdate,
                    format!(
                        "the update raises the document to schema {found}, and this server serves schema {}. Update the app that sent it",
                        schema::SERVED_SCHEMA
                    ),
                )),
                Kind::Control,
            );
            return;
        }
        let changed = self.doc.oplog_vv() != before;
        if changed {
            self.dirty = true;
            if self.cfg().timing.flush_interval.is_zero() {
                self.flush().await;
            } else if self.flush_at.is_none() {
                self.flush_at = Some(Instant::now() + self.cfg().timing.flush_interval);
            }
        }
        if changed || status.pending.is_some() {
            self.broadcast(conn, ServerMessage::Update { from, update: bytes }, Kind::Relay);
        }
        outbox.send(Arc::new(ServerMessage::Ack { seq }), Kind::BestEffort);
    }

    /// Puts the room back to `frontiers`, dropping what was imported after it. Loro cannot take an import back, so
    /// the document is rebuilt from a snapshot of the older state. That costs the size of the room and only
    /// happens for an update that was refused after the import. If it cannot be done the room is poisoned and
    /// stops, so a document in an unknown state is never served.
    fn roll_back_to(&mut self, frontiers: &Frontiers) {
        let rebuilt = guarded(&mut self.poisoned, || {
            let snapshot = self.doc.export(ExportMode::SnapshotAt { version: Cow::Borrowed(frontiers) }).ok()?;
            let doc = LoroDoc::new();
            doc.import(&snapshot).ok()?;
            Some(doc)
        })
        .flatten();
        match rebuilt {
            Some(doc) => self.doc = doc,
            None => {
                error!("room {}: cannot roll back a refused update", self.tag());
                self.poisoned = true;
            }
        }
    }

    // ---- persistence ----------------------------------------------------------------------

    /// Writes everything since the last flush to the store as one log entry.
    async fn flush(&mut self) {
        self.flush_at = None;
        if !self.dirty {
            return;
        }
        let now = self.doc.oplog_vv();
        let exported = guarded(&mut self.poisoned, || self.doc.export(ExportMode::updates(&self.flushed)));
        let bytes = match exported {
            Some(Ok(b)) => b,
            _ => {
                error!("room {}: cannot export updates for the store", self.tag());
                self.flush_at = Some(Instant::now() + self.cfg().timing.flush_retry);
                return;
            }
        };
        match self.store_call("append", self.manager.store.append_update(&self.id, &bytes)).await {
            Ok(()) => {
                self.flushed = now;
                self.dirty = false;
                self.log_entries += 1;
                self.log_bytes += bytes.len() as u64;
                let t = &self.cfg().timing;
                if self.log_entries >= t.compact_entries || self.log_bytes >= t.compact_bytes {
                    self.compact().await;
                }
            }
            Err(e) => {
                error!("room {}: cannot write to storage, will retry: {e}", self.tag());
                self.flush_at = Some(Instant::now() + self.cfg().timing.flush_retry);
            }
        }
    }

    /// Replaces the stored snapshot with the current document and drops the covered log.
    async fn compact(&mut self) {
        let snapshot = guarded(&mut self.poisoned, || self.doc.export(ExportMode::Snapshot));
        let Some(Ok(snapshot)) = snapshot else {
            error!("room {}: cannot export a snapshot for compaction", self.tag());
            return;
        };
        let covered = self.log_entries;
        match self.store_call("compact", self.manager.store.compact(&self.id, &snapshot, covered)).await {
            Ok(()) => {
                self.snapshot_len = snapshot.len();
                self.log_entries = 0;
                self.log_bytes = 0;
                debug!("room {} compacted to {} bytes", self.tag(), snapshot.len());
            }
            Err(e) => warn!("room {}: compaction failed, the log is kept: {e}", self.tag()),
        }
    }
}
