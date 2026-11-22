//! What the client sends: the join, local updates, the upload of what the relay lacks, the seed of an empty room,
//! compaction, presence and requests for records.

use drafftink_doc::LoroStore;
use drafftink_protocol::v2::{ClientMessage, ErrorCode, Proof, RecordHeader, RecordKind};
use drafftink_protocol::{MAX_PRESENCE_RECORD_BYTES, MAX_SNAPSHOT_RECORD_BYTES, SEAL_OVERHEAD};
use loro::VersionVector;

use super::problems::explain;
use super::{COMPACT_RETRY_MS, FULL_SNAPSHOT_LIMIT, Inflight, Phase, SHALLOW_KEEP_COMMITS, SealedClient};
use crate::awareness::LocalPresence;
use crate::client::CollabEvent;
use crate::client::{Output, SEND_DEBOUNCE_MS};
use crate::status::Problem;

impl SealedClient {
    pub(super) fn send(&mut self, message: &ClientMessage) {
        match message.encode() {
            Ok(bytes) => self.out.push_back(Output::Send(bytes)),
            // Not `{message:?}`: a `Join` holds the membership proof.
            Err(e) => log::warn!("collab: a message was not sent: {e}"),
        }
    }

    pub(super) fn update_interval_ms(&self) -> u64 {
        1000u64.div_ceil(u64::from(self.limits.max_updates_per_sec.max(1))).max(SEND_DEBOUNCE_MS)
    }

    pub(super) fn presence_interval(&self) -> u64 {
        LocalPresence::min_interval_ms(self.limits.max_presence_per_sec)
    }

    /// Seals `payload` as a record of this client. Returns its counter and the bytes.
    fn seal(&mut self, kind: RecordKind, payload: &[u8], covers_seq: u64) -> Option<(u64, Vec<u8>)> {
        let counter = match kind {
            RecordKind::Update => &mut self.update_ctr,
            RecordKind::Snapshot => &mut self.snapshot_ctr,
            _ => &mut self.presence_ctr,
        };
        *counter += 1;
        let ctr = *counter;
        let header = RecordHeader::new(kind, self.cfg.client_id, ctr, covers_seq).ok()?;
        match self.keys.seal(header, payload) {
            Ok(record) => Some((ctr, record.into_bytes())),
            Err(e) => {
                log::warn!("collab: a record was not sealed: {e}");
                None
            }
        }
    }

    pub(super) fn on_opened(&mut self, now_ms: u64) {
        if !matches!(self.phase, Phase::Connecting { .. }) {
            return;
        }
        self.last_rx_ms = now_ms;
        self.last_ping_ms = now_ms;
        self.phase = Phase::Joining { since: now_ms };
        let join = ClientMessage::Join {
            // A Join lays out its fields for one version, so min == max.
            min_version: drafftink_protocol::PROTOCOL_V2,
            max_version: drafftink_protocol::PROTOCOL_V2,
            room: self.keys.room_id(),
            client_id: self.cfg.client_id,
            mode: self.cfg.mode,
            proof: Proof::token(self.keys.membership_token()),
            last_seq: self.pos.last_seq,
            captcha: None,
        };
        self.send(&join);
        self.refresh_status();
    }

    pub(super) fn ask_catchup(&mut self, now_ms: u64) {
        self.pos.catchup_asked = Some(now_ms);
        self.send(&ClientMessage::Catchup { after_seq: self.pos.last_seq });
    }

    // ---- updates ----------------------------------------------------------------------------------------------

    fn local_version(&mut self, store: &LoroStore) -> Option<VersionVector> {
        match store.version_vector().map(|v| VersionVector::decode(&v)) {
            Ok(Ok(vv)) => Some(vv),
            Ok(Err(_)) => None,
            Err(e) => {
                self.doc_failure(&e);
                None
            }
        }
    }

    /// Sends the document's unsent operations as one update.
    pub(super) fn flush_updates(&mut self, now_ms: u64, store: &mut LoroStore) {
        self.uploads.flush_at = None;
        if self.uploads.inflight.len() >= crate::client::MAX_INFLIGHT {
            self.uploads.want_flush = true;
            return;
        }
        self.uploads.want_flush = false;
        match store.take_unsent_update() {
            Ok(Some(bytes)) => {
                if let Some(end) = self.local_version(store) {
                    self.send_update(now_ms, bytes, end);
                }
            }
            Ok(None) => {
                if self.uploads.inflight.is_empty() {
                    self.uploads.local_dirty = false;
                }
            }
            Err(e) => self.doc_failure(&e),
        }
        self.refresh_status();
    }

    /// Seals and sends an update that holds everything up to `end`.
    fn send_update(&mut self, now_ms: u64, bytes: Vec<u8>, end: VersionVector) {
        if bytes.len() + SEAL_OVERHEAD > self.limits.max_record_bytes as usize {
            let problem = explain(ErrorCode::TooLarge, false);
            self.uploads.blocked = Some(super::Block::Oversize);
            self.problem = Some(problem.clone());
            self.events.push(CollabEvent::Problem(problem));
            return;
        }
        let Some((ctr, record)) = self.seal(RecordKind::Update, &bytes, 0) else {
            self.uploads.blocked = Some(super::Block::Oversize);
            return;
        };
        self.send(&ClientMessage::Append { record });
        self.uploads.inflight.push_back(Inflight { ctr, sent_at: now_ms, end });
        self.uploads.last_update_sent_ms = Some(now_ms);
        self.stats.updates_sent += 1;
    }

    /// Uploads what the relay is not known to hold. This single step repairs reconnects, dropped updates, rate
    /// limits and a crashed tab.
    pub(super) fn upload_missing(&mut self, now_ms: u64, store: &mut LoroStore) {
        self.uploads.inflight.clear();
        self.uploads.flush_at = None;
        self.uploads.want_flush = false;
        let Some(local) = self.local_version(store) else { return };
        if self.relay_vv.includes_vv(&local) {
            // Whatever the document still counts as unsent is already on the relay.
            let _ = store.take_unsent_update();
            self.uploads.local_dirty = false;
            return;
        }
        let bytes = match store.export_updates_since(&self.relay_vv.encode()) {
            Ok(bytes) => bytes,
            Err(drafftink_doc::DocError::HistoryTrimmed) => match store.export_snapshot() {
                Ok(bytes) => bytes,
                Err(e) => return self.doc_failure(&e),
            },
            Err(e) => return self.doc_failure(&e),
        };
        // The export covers everything the document counts as unsent.
        let _ = store.take_unsent_update();
        self.uploads.local_dirty = true;
        if self.uploads.blocked.is_none() {
            self.send_update(now_ms, bytes, local);
        }
    }

    /// The first sync is done: seed an empty room with the document, or upload what the relay lacks.
    pub(super) fn seed_or_upload(&mut self, now_ms: u64, store: &mut LoroStore) {
        let empty = self.pos.head_seq == 0 && !self.pos.saw_content;
        let Some(local) = self.local_version(store) else { return };
        if empty && !local.is_empty() && self.try_seed(store, local) {
            return;
        }
        self.upload_missing(now_ms, store);
    }

    /// Seals the whole document as a snapshot and offers it to an empty room. Returns false when it cannot.
    fn try_seed(&mut self, store: &mut LoroStore, local: VersionVector) -> bool {
        let Ok(snapshot) = store.export_snapshot() else { return false };
        if snapshot.len() + SEAL_OVERHEAD > MAX_SNAPSHOT_RECORD_BYTES {
            return false;
        }
        let Some((_, record)) = self.seal(RecordKind::Snapshot, &snapshot, 0) else { return false };
        // The seed expects no snapshot and covers nothing: the relay takes it only while the room is empty.
        self.send(&ClientMessage::PutSnapshot { expected_snapshot_seq: 0, record });
        let _ = store.take_unsent_update();
        self.uploads.seed = Some(local);
        self.uploads.local_dirty = true;
        self.snapshot_out = Some(0);
        true
    }

    // ---- compaction -------------------------------------------------------------------------------------------

    /// When the relay asked for it and this client holds every record up to the request, seals a snapshot and sends
    /// it with the sequence number it covers and the snapshot it expects to replace.
    pub(super) fn maybe_compact(&mut self, now_ms: u64, store: &mut LoroStore) {
        let Some(target) = self.compact_for else { return };
        if self.snapshot_out.is_some() || now_ms < self.compact_after_ms || self.pos.more {
            return;
        }
        if self.pos.last_seq < target {
            // Records up to the request are still on their way, or missing: ask for them.
            if self.pos.catchup_asked.is_none() {
                self.ask_catchup(now_ms);
            }
            return;
        }
        let covers = self.pos.last_seq;
        if covers <= self.pos.snapshot_seq {
            self.compact_for = None;
            return;
        }
        let exported = match store.export_snapshot() {
            Ok(full) if full.len() > FULL_SNAPSHOT_LIMIT => store.export_shallow_snapshot(SHALLOW_KEEP_COMMITS),
            other => other,
        };
        let Ok(bytes) = exported else { return };
        if bytes.len() + SEAL_OVERHEAD > MAX_SNAPSHOT_RECORD_BYTES {
            self.compact_for = None;
            return;
        }
        let Some((_, record)) = self.seal(RecordKind::Snapshot, &bytes, covers) else { return };
        self.send(&ClientMessage::PutSnapshot { expected_snapshot_seq: self.pos.snapshot_seq, record });
        self.snapshot_out = Some(covers);
        self.compact_after_ms = now_ms.saturating_add(COMPACT_RETRY_MS);
    }

    // ---- presence ---------------------------------------------------------------------------------------------

    pub(super) fn send_presence_if_due(&mut self, now_ms: u64) {
        let interval = self.presence_interval();
        let Some(state) = self.presence.due(now_ms, interval, !self.peers.is_empty()).cloned() else { return };
        self.presence.mark_sent(now_ms);
        let Ok(payload) = serde_json::to_vec(&state) else { return };
        if payload.len() + SEAL_OVERHEAD > MAX_PRESENCE_RECORD_BYTES {
            return;
        }
        if let Some((_, record)) = self.seal(RecordKind::Presence, &payload, 0) {
            self.send(&ClientMessage::Presence { record });
        }
    }

    pub(super) fn problem_event(&mut self, problem: Problem) {
        self.problem = Some(problem.clone());
        self.events.push(CollabEvent::Problem(problem));
    }
}
