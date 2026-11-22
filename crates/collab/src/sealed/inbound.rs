//! What the client does with what the relay sends.

use drafftink_doc::{DocError, LoroStore};
use drafftink_protocol::v2::{
    CatchupSnapshot, ErrorCode, JoinMode, PresenceEntry, RecordKind, RoomLimits, RoomState, ServerMessage, StoredRecord,
};
use drafftink_protocol::{AwarenessState, ClientId, PROTOCOL_V2, PeerInfo};
use drafftink_scene::ChangeSet;
use drafftink_seal::SealedRecord;
use loro::VersionVector;

use super::problems::{explain, stop_reason};
use super::{Block, Phase, SealedClient};
use crate::awareness::Peer;
use crate::client::{CollabEvent, FollowEnd, Output, RemoteBatch, RemoteKind};
use crate::status::{Problem, ProblemAction, StopReason};

/// What came of one received record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Applied {
    /// Applied, or already known.
    Ok,
    /// Unreadable: a damaged record or a malformed update. It is skipped and shown once.
    Skipped,
    /// The document cannot take any more: the client stopped.
    Stopped,
}

impl SealedClient {
    pub(super) fn on_frame(&mut self, now_ms: u64, bytes: &[u8], store: &mut LoroStore) {
        if matches!(self.phase, Phase::Idle | Phase::Stopped(_) | Phase::Backoff { .. } | Phase::Connecting { .. }) {
            return;
        }
        self.last_rx_ms = now_ms;
        let message = match ServerMessage::decode(bytes) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("collab: the server sent a frame that cannot be read: {e}");
                self.fail_connection(now_ms, "The server sent something the app cannot read");
                return;
            }
        };
        match message {
            ServerMessage::Joined {
                protocol_version,
                room_state,
                head_seq,
                snapshot_seq,
                expires_at_ms,
                peers,
                limits,
                ..
            } => {
                let joined =
                    Joined { protocol_version, room_state, head_seq, snapshot_seq, expires_at_ms, peers, limits };
                self.on_joined(now_ms, joined);
            }
            ServerMessage::Catchup { snapshot, records, head_seq, more } => {
                self.on_catchup(now_ms, store, snapshot, &records, head_seq, more);
            }
            ServerMessage::Record(record) => self.on_record(now_ms, store, &record),
            ServerMessage::Ack { ctr, seq } => self.on_ack(now_ms, ctr, seq),
            ServerMessage::PeerJoined { client_id, max_presence_per_sec } => {
                self.on_peer_joined(client_id, max_presence_per_sec);
            }
            ServerMessage::PeerLeft { client_id } => self.on_peer_left(client_id),
            ServerMessage::PresenceBundle(entries) => self.on_presence(now_ms, &entries),
            ServerMessage::Error { code, retry_after_ms, .. } => self.on_server_error(now_ms, code, retry_after_ms),
            ServerMessage::SnapshotResult { ok, covers_seq, snapshot_seq, head_seq } => {
                self.on_snapshot_result(now_ms, ok, covers_seq, snapshot_seq, head_seq);
            }
            ServerMessage::CompactAdvice { head_seq } => {
                self.compact_for = Some(self.compact_for.map_or(head_seq, |t| t.max(head_seq)));
            }
            ServerMessage::Retention { expires_at_ms, expiring } => self.set_retention(expires_at_ms, expiring),
            // The peer-to-peer fallback is not built yet: a request from a peer goes unanswered.
            ServerMessage::Direct { .. } => {}
        }
    }

    fn on_joined(&mut self, now_ms: u64, joined: Joined) {
        if !matches!(self.phase, Phase::Joining { .. }) {
            return;
        }
        if joined.protocol_version != PROTOCOL_V2 {
            log::warn!("collab: the relay answered the join with another protocol version");
            self.fail_connection(now_ms, "The server's answer did not match the request");
            return;
        }
        self.limits = joined.limits;
        self.room_state = Some(joined.room_state);
        if joined.room_state == RoomState::Created || joined.head_seq < self.pos.last_seq {
            // The room is a new one under the old id (it expired and was made again): what this client remembers of
            // the old one is not true of it. It holds no sequence number and the relay holds nothing of its.
            self.pos.last_seq = 0;
            self.pos.held.clear();
            self.pos.sender_ctrs.clear();
            self.relay_vv = VersionVector::default();
        }
        // The room exists now. A reconnect must not make a new one if it is deleted meanwhile.
        self.cfg.mode = JoinMode::Open;
        let infos: Vec<PeerInfo> = joined.peers.iter().map(|id| PeerInfo { client_id: *id, user: None }).collect();
        self.peers.replace(self.cfg.client_id, &infos);
        self.pos.head_seq = joined.head_seq;
        self.pos.snapshot_seq = joined.snapshot_seq;
        self.pos.more = false;
        self.pos.saw_content = joined.head_seq > 0 || joined.snapshot_seq > 0;
        self.set_retention(joined.expires_at_ms, false);
        self.phase = Phase::Syncing { since: now_ms };
        self.uploads.blocked = None;
        self.presence_changed = true;
        self.refresh_status();
    }

    fn set_retention(&mut self, expires_at_ms: u64, expiring: bool) {
        if self.expires_at_ms != expires_at_ms || self.expiring != expiring {
            self.expires_at_ms = expires_at_ms;
            self.expiring = expiring;
            self.events.push(CollabEvent::Retention { expires_at_ms, expiring });
            self.refresh_status();
        }
    }

    // ---- the log ----------------------------------------------------------------------------------------------

    fn on_catchup(
        &mut self,
        now_ms: u64,
        store: &mut LoroStore,
        snapshot: Option<CatchupSnapshot>,
        records: &[StoredRecord],
        head_seq: u64,
        more: bool,
    ) {
        let joining = match self.phase {
            Phase::Syncing { .. } => true,
            Phase::Online => false,
            _ => return,
        };
        self.pos.catchup_asked = None;
        self.pos.head_seq = self.pos.head_seq.max(head_seq);
        if let Some(snapshot) = snapshot {
            self.pos.saw_content = true;
            if !self.apply_snapshot(store, &snapshot) {
                return;
            }
        }
        for record in records {
            self.pos.saw_content = true;
            if self.apply_record(now_ms, store, record, RemoteKind::CatchUp) == Applied::Stopped {
                return;
            }
        }
        self.pos.more = more;
        if more {
            self.ask_catchup(now_ms);
        } else if joining {
            self.after_sync(now_ms, store);
        }
        self.refresh_status();
    }

    /// The relay's content is in the document: finish the join, seed an empty room or upload what the relay lacks.
    fn after_sync(&mut self, now_ms: u64, store: &mut LoroStore) {
        self.stats.joins += 1;
        self.phase = Phase::Online;
        self.stable_since = Some(now_ms);
        self.presence.reset_sent();
        self.problem = None;
        if store.has_pending_remote() {
            self.history_missing();
        }
        self.images.on_online(now_ms);
        self.seed_or_upload(now_ms, store);
        self.refresh_status();
    }

    /// The whole catch-up is applied and some operations still wait for history the relay did not send. Part of the
    /// room cannot be shown: say so instead of looking like an empty or shorter room.
    fn history_missing(&mut self) {
        log::warn!("collab: the catch-up left operations that wait for missing history");
        let text = "Part of this room could not be loaded because some of its saved history is missing. What is shown \
                    may be incomplete. Save a copy of it, and ask for a new link if the drawing looks wrong.";
        let problem = Problem::local(text, ProblemAction::SaveCopy, false);
        self.problem = Some(problem.clone());
        self.events.push(CollabEvent::Problem(problem));
    }

    /// Opens a sealed record of `kind`. `None` for anything that is not a record of this room and kind.
    fn open(&self, kind: RecordKind, record: &[u8]) -> Option<Vec<u8>> {
        let sealed = SealedRecord::from_bytes(record.to_vec()).ok()?;
        self.keys.open(kind, &sealed).ok()
    }

    /// Applies the relay's snapshot. False when the client stopped.
    fn apply_snapshot(&mut self, store: &mut LoroStore, snapshot: &CatchupSnapshot) -> bool {
        let Some(bytes) = self.open(RecordKind::Snapshot, &snapshot.record) else {
            self.stop_unreadable();
            return false;
        };
        let (changes, kind) = match store.apply_remote(&bytes) {
            Ok(changes) => (changes, RemoteKind::Snapshot),
            Err(DocError::SnapshotStartsLater) if self.cfg.replace_when_trimmed => {
                match store.replace_from_snapshot(&bytes) {
                    Ok(changes) => (changes, RemoteKind::Replaced),
                    Err(e) => {
                        self.doc_failure(&e);
                        return false;
                    }
                }
            }
            Err(DocError::SnapshotStartsLater) => {
                let message = "The room's copy starts after the history this copy has. Replacing this copy with the \
                               room's loses changes made here that were never shared."
                    .to_owned();
                self.events.push(CollabEvent::NeedsDecision { message });
                self.out.push_back(Output::Close);
                self.enter_stopped(StopReason::Declined, None);
                return false;
            }
            Err(e @ (DocError::NewerSchema { .. } | DocError::Poisoned)) => {
                self.doc_failure(&e);
                return false;
            }
            Err(e) => {
                log::warn!("collab: the room's snapshot could not be applied: {e}");
                self.stop_unreadable();
                return false;
            }
        };
        self.push_remote(None, kind, changes);
        self.merge_relay_vv(store, &bytes);
        self.pos.snapshot_seq = snapshot.covers_seq;
        self.advance_to(snapshot.covers_seq);
        true
    }

    /// A snapshot that cannot be opened or applied loses everything below its cover, so it is not skipped.
    fn stop_unreadable(&mut self) {
        let text =
            "The saved state of this room could not be read. Save a copy of what you have and ask for the link again.";
        self.out.push_back(Output::Close);
        self.enter_stopped(StopReason::NeedsReload, Some(Problem::local(text, ProblemAction::Reload, true)));
    }

    /// Opens and applies one update of the log.
    fn apply_record(&mut self, now_ms: u64, store: &mut LoroStore, record: &StoredRecord, kind: RemoteKind) -> Applied {
        let seen = record.seq <= self.pos.last_seq || self.pos.held.contains(&record.seq);
        let applied = if seen { Applied::Ok } else { self.apply_update(store, record, kind) };
        if applied == Applied::Stopped {
            return applied;
        }
        let known = self.pos.sender_ctrs.entry(record.from).or_insert(0);
        *known = (*known).max(record.ctr);
        self.mark_seq(now_ms, record.seq);
        applied
    }

    fn apply_update(&mut self, store: &mut LoroStore, record: &StoredRecord, kind: RemoteKind) -> Applied {
        let _span = drafftink_profile::span("collab_apply");
        let Some(bytes) = self.open(RecordKind::Update, &record.record) else {
            return self.skip("a record that could not be opened");
        };
        match store.apply_remote(&bytes) {
            Ok(changes) => {
                self.push_remote(Some(record.from), kind, changes);
                self.merge_relay_vv(store, &bytes);
                Applied::Ok
            }
            Err(e @ (DocError::NewerSchema { .. } | DocError::Poisoned)) => {
                self.doc_failure(&e);
                Applied::Stopped
            }
            Err(e) => {
                log::warn!("collab: a change from the room could not be applied: {e}");
                self.skip("a change that could not be applied")
            }
        }
    }

    /// Notes a change that was skipped and says so once.
    fn skip(&mut self, what: &str) -> Applied {
        log::warn!("collab: skipped {what}");
        self.skipped += 1;
        if self.skipped == 1 {
            let text = "One change from someone in this room could not be read and was skipped.";
            self.events.push(CollabEvent::Problem(Problem::local(text, ProblemAction::Wait, false)));
        }
        Applied::Skipped
    }

    fn on_record(&mut self, now_ms: u64, store: &mut LoroStore, record: &StoredRecord) {
        if !matches!(self.phase, Phase::Syncing { .. } | Phase::Online) {
            return;
        }
        // A counter that jumps means a record of that sender never came.
        if self.pos.sender_ctrs.get(&record.from).is_some_and(|known| record.ctr > known.saturating_add(1)) {
            self.pos.gap_since.get_or_insert(now_ms);
        }
        self.apply_record(now_ms, store, record, RemoteKind::Live);
    }

    /// Marks `seq` as held. A sequence number above the next one opens a gap, which is repaired if it stays.
    fn mark_seq(&mut self, now_ms: u64, seq: u64) {
        match seq.cmp(&self.pos.last_seq.saturating_add(1)) {
            std::cmp::Ordering::Less => {}
            std::cmp::Ordering::Equal => {
                self.pos.last_seq = seq;
                while self.pos.held.remove(&self.pos.last_seq.saturating_add(1)) {
                    self.pos.last_seq = self.pos.last_seq.saturating_add(1);
                }
            }
            std::cmp::Ordering::Greater => {
                self.pos.held.insert(seq);
                self.pos.gap_since.get_or_insert(now_ms);
            }
        }
        if self.pos.held.is_empty() {
            self.pos.gap_since = None;
        }
    }

    /// Everything up to `seq` is held (a snapshot covers it).
    fn advance_to(&mut self, seq: u64) {
        if seq > self.pos.last_seq {
            self.pos.last_seq = seq;
            self.pos.held = self.pos.held.split_off(&seq.saturating_add(1));
            while self.pos.held.remove(&self.pos.last_seq.saturating_add(1)) {
                self.pos.last_seq = self.pos.last_seq.saturating_add(1);
            }
        }
        if self.pos.held.is_empty() {
            self.pos.gap_since = None;
        }
    }

    /// The relay holds what `bytes` (just applied) hold: add its end version to `relay_vv`.
    fn merge_relay_vv(&mut self, store: &LoroStore, bytes: &[u8]) {
        if let Ok(end) = store.blob_end_version(bytes)
            && let Ok(end) = VersionVector::decode(&end)
        {
            self.relay_vv.merge(&end);
        }
    }

    fn push_remote(&mut self, from: Option<ClientId>, kind: RemoteKind, changes: ChangeSet) {
        if changes.is_empty() && kind != RemoteKind::Replaced {
            return;
        }
        let who = from.and_then(|id| self.peers.get(&id)).map(Peer::display_name);
        self.stats.remote_batches += 1;
        self.events.push(CollabEvent::Remote(RemoteBatch { from, who, kind, changes }));
    }

    fn on_ack(&mut self, now_ms: u64, ctr: u64, seq: u64) {
        self.stats.acks += 1;
        if let Some(i) = self.uploads.inflight.iter().position(|f| f.ctr == ctr)
            && let Some(done) = self.uploads.inflight.remove(i)
        {
            self.relay_vv.merge(&done.end);
        }
        // Our own record is in the relay's log under `seq`: nobody sends it back, so it fills its own gap.
        self.mark_seq(now_ms, seq);
        if self.uploads.inflight.is_empty() && self.uploads.flush_at.is_none() && !self.uploads.want_flush {
            self.uploads.local_dirty = self.uploads.seed.is_some();
        }
        if self.uploads.want_flush && self.uploads.inflight.len() < crate::client::MAX_INFLIGHT {
            self.uploads.flush_at = Some(now_ms);
        }
        self.refresh_status();
    }

    fn on_snapshot_result(&mut self, now_ms: u64, ok: bool, covers_seq: u64, snapshot_seq: u64, head_seq: u64) {
        self.snapshot_out = None;
        self.pos.head_seq = self.pos.head_seq.max(head_seq);
        if ok {
            self.pos.snapshot_seq = covers_seq;
            self.compact_for = self.compact_for.filter(|target| *target > covers_seq);
        } else {
            self.pos.snapshot_seq = self.pos.snapshot_seq.max(snapshot_seq);
        }
        if let Some(seeded) = self.uploads.seed.take() {
            if ok {
                self.relay_vv.merge(&seeded);
                self.uploads.local_dirty = false;
            } else {
                // Another copy was restored first. Join the room as it is: its content comes with the next join,
                // and this document merges into it.
                self.pos.saw_content = true;
                self.drop_connection(now_ms, "Another copy of this room was restored first. Joining it", true);
                return;
            }
        }
        self.refresh_status();
    }

    // ---- people -----------------------------------------------------------------------------------------------

    fn on_peer_joined(&mut self, client_id: ClientId, max_presence_per_sec: u16) {
        // A client takes the lower of its old and new rate.
        self.limits.max_presence_per_sec = self.limits.max_presence_per_sec.min(max_presence_per_sec).max(1);
        let was_empty = self.peers.is_empty();
        if self.peers.join(self.cfg.client_id, &PeerInfo { client_id, user: None }) {
            let name = self.peers.get(&client_id).map(Peer::display_name).unwrap_or_default();
            self.events.push(CollabEvent::PeerJoined { id: client_id, name });
            self.presence_changed = true;
        }
        if was_empty && !self.peers.is_empty() {
            // The newcomer has not seen our presence; the heartbeat would take up to 10 s.
            self.presence.reset_sent();
        }
        self.refresh_status();
    }

    fn on_peer_left(&mut self, id: ClientId) {
        if let Some(p) = self.peers.leave(id) {
            self.events.push(CollabEvent::PeerLeft { id, name: p.display_name() });
            self.presence_changed = true;
            if self.following == Some(id) {
                self.following = None;
                self.events.push(CollabEvent::FollowEnded(FollowEnd::PeerLeft));
            }
        }
        self.refresh_status();
    }

    fn on_presence(&mut self, now_ms: u64, entries: &[PresenceEntry]) {
        for entry in entries {
            let Some(bytes) = self.open(RecordKind::Presence, &entry.record) else { continue };
            let Ok(state) = serde_json::from_slice::<AwarenessState>(&bytes) else { continue };
            if state.validate().is_err() {
                continue;
            }
            // A replayed presence record is older than one already seen from the same sender.
            let counter = drafftink_protocol::v2::RecordHeader::peek(&entry.record).map_or(0, |h| h.ctr);
            let newest = self.pos.presence_ctrs.entry(entry.from).or_insert(0);
            if counter <= *newest {
                continue;
            }
            *newest = counter;
            let viewport = state.viewport;
            if self.peers.awareness(self.cfg.client_id, now_ms, entry.from, state) {
                self.presence_changed = true;
                if self.following == Some(entry.from)
                    && let Some(v) = viewport
                {
                    self.events.push(CollabEvent::FollowViewport(v));
                }
            }
        }
    }

    // ---- errors -----------------------------------------------------------------------------------------------

    fn on_server_error(&mut self, now_ms: u64, code: ErrorCode, retry_after_ms: Option<u32>) {
        let joining = !matches!(self.phase, Phase::Online);
        let problem = explain(code, joining);
        let retry = retry_after_ms.map(u64::from);
        match code {
            ErrorCode::RateLimited => {
                let wait = retry.unwrap_or(1000).clamp(50, crate::client::RESYNC_MAX_GAP_MS);
                if self.snapshot_out.take().is_some() && self.uploads.seed.is_none() {
                    // A compaction came too soon after the last snapshot: try again later.
                    self.compact_after_ms = now_ms.saturating_add(wait);
                }
                self.uploads.send_not_before = now_ms.saturating_add(wait);
                // The update that was refused was taken from the document already: upload what the relay lacks.
                self.reupload_at = Some(self.uploads.send_not_before);
            }
            ErrorCode::TooLarge if !joining => {
                self.uploads.blocked = Some(Block::Oversize);
                self.problem_event(problem);
            }
            ErrorCode::RoomFull if !joining => {
                self.out.push_back(Output::Close);
                self.enter_stopped(StopReason::TooLarge, Some(problem));
            }
            _ => {
                if let Some(reason) = stop_reason(code) {
                    self.out.push_back(Output::Close);
                    self.enter_stopped(reason, Some(problem));
                } else {
                    // The relay closes the connection after these; reconnect with its hint, if it gave one.
                    self.server_retry_ms = retry;
                    self.events.push(CollabEvent::Problem(problem.clone()));
                    self.fail_connection(now_ms, &problem.text);
                }
            }
        }
        self.refresh_status();
    }
}

/// The fields of `Joined` that the client uses.
struct Joined {
    protocol_version: u16,
    room_state: RoomState,
    head_seq: u64,
    snapshot_seq: u64,
    expires_at_ms: u64,
    peers: Vec<ClientId>,
    limits: RoomLimits,
}
