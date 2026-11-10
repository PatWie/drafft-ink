//! A tiny in-memory collaboration server for tests: it joins the loopback ends of several sessions, keeps one
//! document, acknowledges and relays updates, relays presence and announces who joins and leaves. It speaks the
//! same protocol as the real server (`docs/protocol.md`) but has no rooms, limits, rate limits or storage.
//!
//! Drive it by calling [`FakeServer::step`] between the polls of the sessions it serves.

use std::sync::Arc;

use drafftink_collab::LoopbackServer;
use drafftink_doc::{DocError, LoroConfig, LoroStore};
use drafftink_protocol::{ClientId, ClientMessage, Limits, PeerInfo, ServerMessage, SyncKind, UserInfo};

use crate::ports::fake::FakeNet;

/// One connected client.
struct Link {
    end: Arc<LoopbackServer>,
    id: Option<ClientId>,
    user: Option<UserInfo>,
    joined: bool,
}

/// The server. It serves every connection that was attached to it.
pub struct FakeServer {
    store: LoroStore,
    links: Vec<Link>,
}

impl Default for FakeServer {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeServer {
    /// A server with an empty document.
    pub fn new() -> Self {
        Self { store: LoroStore::with_config(LoroConfig { peer_id: Some(u64::MAX - 1), ..LoroConfig::default() }), links: Vec::new() }
    }

    /// Serves the connections `net` opened since the last call.
    pub fn attach(&mut self, net: &FakeNet) {
        for end in net.servers() {
            if !self.links.iter().any(|l| Arc::ptr_eq(&l.end, &end)) {
                self.links.push(Link { end, id: None, user: None, joined: false });
            }
        }
    }

    /// The server's copy of the document.
    pub fn document(&self) -> &LoroStore {
        &self.store
    }

    /// How many clients are in the room now.
    pub fn clients(&self) -> usize {
        self.links.iter().filter(|l| l.joined).count()
    }

    /// Handles every frame the clients sent, and notices connections that closed. Returns true when it did
    /// anything.
    pub fn step(&mut self) -> bool {
        let mut busy = false;
        for i in 0..self.links.len() {
            while let Some(msg) = self.links.get(i).and_then(|l| l.end.recv()) {
                busy = true;
                self.handle(i, msg);
            }
            if self.links.get(i).is_some_and(|l| l.joined && !l.end.is_open()) {
                busy = true;
                self.part(i);
            }
        }
        busy
    }

    fn send(&self, to: usize, msg: &ServerMessage) {
        if let Some(l) = self.links.get(to) {
            l.end.push(msg);
        }
    }

    fn broadcast_except(&self, except: usize, msg: &ServerMessage) {
        for (j, l) in self.links.iter().enumerate() {
            if j != except && l.joined {
                l.end.push(msg);
            }
        }
    }

    fn part(&mut self, i: usize) {
        let Some(l) = self.links.get_mut(i) else { return };
        l.joined = false;
        if let Some(client_id) = l.id {
            self.broadcast_except(i, &ServerMessage::PeerLeft { client_id });
        }
    }

    fn handle(&mut self, i: usize, msg: ClientMessage) {
        match msg {
            ClientMessage::Join { room, client_id, version, user, .. } => self.join(i, room, client_id, &version, user),
            ClientMessage::Update { seq, update } => {
                if self.store.apply_remote(&update).is_err() {
                    return;
                }
                self.send(i, &ServerMessage::Ack { seq });
                if let Some(from) = self.links.get(i).and_then(|l| l.id) {
                    self.broadcast_except(i, &ServerMessage::Update { from, update });
                }
            }
            ClientMessage::RequestSync { version } => self.sync(i, &version),
            ClientMessage::Awareness(state) => {
                if let Some(from) = self.links.get(i).and_then(|l| l.id) {
                    self.broadcast_except(i, &ServerMessage::PeerAwareness { from, state });
                }
            }
            ClientMessage::Ping { nonce } => self.send(i, &ServerMessage::Pong { nonce }),
            ClientMessage::Pong { .. } => {}
            ClientMessage::Leave => self.part(i),
        }
    }

    fn join(&mut self, i: usize, room: drafftink_protocol::RoomId, client_id: ClientId, version: &[u8], user: Option<UserInfo>) {
        let peers: Vec<PeerInfo> = self
            .links
            .iter()
            .enumerate()
            .filter(|(j, l)| *j != i && l.joined)
            .filter_map(|(_, l)| l.id.map(|client_id| PeerInfo { client_id, user: l.user.clone() }))
            .collect();
        if let Some(l) = self.links.get_mut(i) {
            l.id = Some(client_id);
            l.user = user.clone();
            l.joined = true;
        }
        self.send(
            i,
            &ServerMessage::Joined {
                protocol_version: 1,
                server: "fake".to_owned(),
                room,
                client_id,
                peers,
                server_version: self.store.version_vector(),
                limits: Limits::default(),
            },
        );
        self.sync(i, version);
        self.broadcast_except(i, &ServerMessage::PeerJoined { peer: PeerInfo { client_id, user } });
    }

    fn sync(&self, i: usize, version: &[u8]) {
        let (kind, data) = match self.store.export_updates_since(version) {
            Ok(bytes) => (SyncKind::Updates, bytes),
            Err(DocError::HistoryTrimmed) => (SyncKind::Snapshot, self.store.export_snapshot().unwrap_or_default()),
            Err(_) => return,
        };
        self.send(i, &ServerMessage::SyncResponse { kind, data, server_version: self.store.version_vector() });
    }
}
