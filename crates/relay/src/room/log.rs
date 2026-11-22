//! The ordered log: sequence numbers, the segment buffer and its flush rules, snapshots by compare-and-set,
//! catch-up and the advice to compact.

use drafftink_protocol::v2::{CatchupSnapshot, ErrorCode, RecordHeader, ServerMessage, StoredRecord};
use drafftink_protocol::{CATCHUP_TARGET_BYTES, MAX_CATCHUP_RECORDS, MAX_FRAME_BYTES, PUT_SNAPSHOT_MIN_INTERVAL_MS};

use super::{ConnId, MetaState, Output, Relay, StoreOp, retry_error};
use crate::config::FlushRules;
use crate::store::{BlobStore, CasOutcome, RelayStore, Segment, StoreError, StoredSnapshot};

/// Records that were sequenced and fanned out and wait for the next write, and the acknowledgements that wait with
/// them: `Ack` means the record is stored.
#[derive(Debug, Default)]
pub(super) struct Log {
    pending: Vec<StoredRecord>,
    bytes: usize,
    first_pending_ms: Option<u64>,
    acks: Vec<(ConnId, u64, u64)>,
    retry_at_ms: Option<u64>,
}

impl Log {
    pub(super) fn forget_acks(&mut self, conn: ConnId) {
        self.acks.retain(|(c, _, _)| *c != conn);
    }

    /// When the buffer must be written at the latest.
    pub(super) fn next_flush(&self, rules: &FlushRules) -> Option<u64> {
        let due = self.first_pending_ms?.saturating_add(rules.interval_ms);
        Some(self.retry_at_ms.map_or(due, |retry| retry.max(due)))
    }

    fn last_seq(&self) -> Option<u64> {
        self.pending.last().map(|r| r.seq)
    }

    /// Forgets what waits for a write, and the acknowledgements that wait with it.
    pub(super) fn drop_pending(&mut self) {
        *self = Log::default();
    }
}

/// Why a flush did not write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FlushError {
    /// The store failed. The records stay in the buffer and the write is tried again after the retry time.
    Failed,
    /// The daily limit stopped the write. The room shed everybody and dropped the buffer ([`Relay::shed_everyone`]).
    Shed,
}

/// The client that was asked to compact, and the ones asked before it.
#[derive(Debug)]
pub(super) struct Advice {
    pub(super) conn: ConnId,
    pub(super) sent_ms: u64,
    asked: Vec<ConnId>,
}

impl<S: RelayStore, B: BlobStore> Relay<S, B> {
    /// The newest sequence number handed out, written or not.
    fn head_seq(&self) -> u64 {
        self.log.last_seq().or_else(|| self.meta_now().map(|m| m.head_seq)).unwrap_or(0)
    }

    pub(super) fn append(&mut self, conn: ConnId, now_ms: u64, record: Vec<u8>, out: &mut Output) {
        let Some(client_id) = self.client_of(conn) else { return };
        let Ok(header) = RecordHeader::peek(&record) else { return };
        if header.sender != client_id {
            let text = "a record names another sender than the connection";
            self.refuse(conn, now_ms, ServerMessage::error(ErrorCode::Forbidden, text), true, out);
            return;
        }
        if record.len() > self.cfg.limits.max_record_bytes as usize {
            let text = "the update is larger than this relay accepts";
            self.refuse(conn, now_ms, ServerMessage::error(ErrorCode::TooLarge, text), false, out);
            return;
        }
        if let Err(wait) = self.take_update_tokens(conn, now_ms) {
            let close = self.breach(conn, now_ms);
            let text = if close {
                "too many messages; the connection is closed"
            } else {
                "slow down; this update was dropped"
            };
            self.refuse(conn, now_ms, retry_error(ErrorCode::RateLimited, text, wait), close, out);
            return;
        }
        let Some(meta) = self.meta_now() else { return };
        let queued = u64::try_from(self.log.bytes).unwrap_or(u64::MAX);
        if meta.state_bytes().saturating_add(queued).saturating_add(record.len() as u64) > self.cfg.max_room_state_bytes
        {
            let text = "This room holds as much as a room may. Start a new room from this one.";
            self.refuse(conn, now_ms, ServerMessage::error(ErrorCode::RoomFull, text), true, out);
            return;
        }
        let seq = self.head_seq() + 1;
        let stored = StoredRecord { seq, from: client_id, ctr: header.ctr, record };
        self.broadcast(out, Some(conn), &ServerMessage::Record(stored.clone()));
        self.log.acks.push((conn, header.ctr, seq));
        self.log.bytes += stored.record.len();
        self.log.first_pending_ms.get_or_insert(now_ms);
        self.log.pending.push(stored);
        if self.cfg.flush.interval_ms == 0 || self.log.bytes >= self.cfg.flush.max_bytes {
            let _ = self.flush(now_ms, out);
        }
    }

    /// Takes a token for one update from the person and from the room.
    fn take_update_tokens(&mut self, conn: ConnId, now_ms: u64) -> Result<(), u32> {
        self.room_updates.take(now_ms)?;
        match self.member_mut(conn) {
            Some(member) => member.updates.take(now_ms),
            None => Ok(()),
        }
    }

    /// Writes the buffer as one segment and acknowledges what it held. When the store failed, the records stay in the
    /// buffer, and the write is tried again after [`FlushRules::retry_ms`]. The daily limit is different: a retry
    /// every second until midnight would only burn the host's calls, so [`Relay::shed_everyone`] stops the room.
    pub(super) fn flush(&mut self, now_ms: u64, out: &mut Output) -> Result<(), FlushError> {
        if self.log.pending.is_empty() {
            return Ok(());
        }
        let Some(mut meta) = self.meta_now() else { return Err(FlushError::Failed) };
        let segment = Segment { records: self.log.pending.clone() };
        meta.head_seq = segment.last_seq();
        meta.log_records = meta.log_records.saturating_add(u32::try_from(segment.records.len()).unwrap_or(u32::MAX));
        meta.log_bytes = meta.log_bytes.saturating_add(segment.bytes());
        match self.quota.watch(self.store.append_segment(&segment, &meta)) {
            Ok(()) => {}
            Err(StoreError::DailyLimit) => {
                self.shed_everyone(now_ms, out);
                return Err(FlushError::Shed);
            }
            Err(_) => {
                self.log.retry_at_ms = Some(now_ms.saturating_add(self.cfg.flush.retry_ms));
                return Err(FlushError::Failed);
            }
        }
        out.store_ops.push(StoreOp::AppendSegment {
            records: u32::try_from(segment.records.len()).unwrap_or(u32::MAX),
            bytes: segment.bytes(),
        });
        self.meta = MetaState::Loaded(meta);
        self.log.pending.clear();
        self.log.bytes = 0;
        self.log.first_pending_ms = None;
        self.log.retry_at_ms = None;
        for (conn, ctr, seq) in std::mem::take(&mut self.log.acks) {
            if self.conns.contains_key(&conn) {
                out.send(conn, &ServerMessage::Ack { ctr, seq });
            }
        }
        self.advise_after_write(now_ms, out);
        Ok(())
    }

    /// Answers a connection whose request needed a flush that failed. After a shed there is nobody to answer.
    pub(super) fn refuse_flush(
        &mut self,
        conn: ConnId,
        now_ms: u64,
        failure: FlushError,
        close: bool,
        out: &mut Output,
    ) {
        if failure == FlushError::Failed {
            self.refuse(conn, now_ms, Self::unavailable(), close, out);
        }
    }

    pub(super) fn flush_if_due(&mut self, now_ms: u64, out: &mut Output) {
        if self.log.next_flush(&self.cfg.flush).is_some_and(|due| due <= now_ms) {
            let _ = self.flush(now_ms, out);
        }
    }

    // ---- snapshots --------------------------------------------------------------------------------------------

    pub(super) fn put_snapshot(&mut self, conn: ConnId, now_ms: u64, expected: u64, record: Vec<u8>, out: &mut Output) {
        let Some(client_id) = self.client_of(conn) else { return };
        let Ok(header) = RecordHeader::peek(&record) else { return };
        if header.sender != client_id {
            let text = "a record names another sender than the connection";
            self.refuse(conn, now_ms, ServerMessage::error(ErrorCode::Forbidden, text), true, out);
            return;
        }
        let gap = u64::from(PUT_SNAPSHOT_MIN_INTERVAL_MS);
        // The seed of an empty room is not a compaction: it does not count against the gap.
        let seeding = self.meta_now().is_some_and(|m| m.is_empty());
        if let Some(Err(wait)) = self.member_mut(conn).filter(|_| !seeding).map(|m| m.snapshots.take(now_ms, gap)) {
            let close = self.breach(conn, now_ms);
            let text = "snapshots may be sent once every few seconds";
            self.refuse(conn, now_ms, retry_error(ErrorCode::RateLimited, text, wait), close, out);
            return;
        }
        if record.len() as u64 > self.cfg.max_room_state_bytes {
            let text = "This room holds as much as a room may. Start a new room from this one.";
            self.refuse(conn, now_ms, ServerMessage::error(ErrorCode::RoomFull, text), true, out);
            return;
        }
        // The cover must refer to records that exist, so write the buffer first.
        if let Err(failure) = self.flush(now_ms, out) {
            self.refuse_flush(conn, now_ms, failure, false, out);
            return;
        }
        let covers_seq = header.covers_seq;
        let bytes = record.len() as u64;
        let snapshot = StoredSnapshot { covers_seq, record };
        let Some(meta) = self.meta_now() else { return };
        let swapped = if meta.accepts_snapshot(expected, covers_seq) {
            match self.quota.watch(self.store.put_snapshot_cas(expected, &snapshot)) {
                Ok(CasOutcome::Swapped(_)) => true,
                Ok(CasOutcome::Stale) => false,
                Err(e) => {
                    self.refuse_store(conn, now_ms, &e, false, out);
                    return;
                }
            }
        } else {
            false
        };
        if swapped {
            if let Ok(Some(stored)) = self.quota.watch(self.store.load_meta()) {
                self.meta = MetaState::Loaded(stored);
            }
            out.store_ops.push(StoreOp::PutSnapshot { covers_seq, bytes });
            self.advice = None;
        }
        let meta = self.meta_now().unwrap_or(meta);
        out.send(
            conn,
            &ServerMessage::SnapshotResult {
                ok: swapped,
                covers_seq,
                snapshot_seq: meta.snapshot_seq,
                head_seq: meta.head_seq,
            },
        );
    }

    // ---- catch-up ---------------------------------------------------------------------------------------------

    pub(super) fn catchup_request(&mut self, conn: ConnId, now_ms: u64, after_seq: u64, out: &mut Output) {
        if let Some(Err(wait)) = self.member_mut(conn).map(|m| m.catchup.take(now_ms)) {
            let close = self.breach(conn, now_ms);
            self.refuse(
                conn,
                now_ms,
                retry_error(ErrorCode::RateLimited, "slow down; ask again in a moment", wait),
                close,
                out,
            );
            return;
        }
        if let Err(failure) = self.flush(now_ms, out) {
            self.refuse_flush(conn, now_ms, failure, false, out);
            return;
        }
        match self.build_catchup(after_seq, false) {
            Ok(catchup) => out.send(conn, &catchup),
            Err(e) => self.refuse_store(conn, now_ms, &e, false, out),
        }
    }

    /// What a client that holds the room up to `after_seq` lacks: the snapshot when it covers more than the client
    /// has (or, on a join of an empty copy, whenever the room has one), and the records after that, one page.
    pub(super) fn build_catchup(&mut self, after_seq: u64, joining: bool) -> Result<ServerMessage, StoreError> {
        let meta = self.meta_now().ok_or_else(|| StoreError::backend("the room is not loaded"))?;
        let wants_snapshot = meta.has_snapshot && (meta.snapshot_seq > after_seq || (joining && after_seq == 0));
        let snapshot = if wants_snapshot {
            let stored = self
                .quota
                .watch(self.store.read_snapshot())?
                .ok_or_else(|| StoreError::Corrupt("the snapshot is missing".into()))?;
            Some(CatchupSnapshot { covers_seq: stored.covers_seq, record: stored.record })
        } else {
            None
        };
        let cursor = snapshot.as_ref().map_or(after_seq, |s| s.covers_seq.max(after_seq));
        // A frame holds the snapshot and the records together.
        let room_left =
            MAX_FRAME_BYTES.saturating_sub(snapshot.as_ref().map_or(0, |s| s.record.len())).saturating_sub(64 * 1024);
        let target = CATCHUP_TARGET_BYTES.min(room_left);
        let mut records = self.quota.watch(self.store.read_since(cursor, MAX_CATCHUP_RECORDS, target))?;
        let mut used = 0usize;
        let fits = records
            .iter()
            .take_while(|r| {
                used += r.record.len();
                used <= room_left
            })
            .count();
        // The first record may be left out only when a snapshot already fills the frame.
        records.truncate(fits.max(usize::from(snapshot.is_none())));
        let more = records.last().map_or(cursor, |r| r.seq) < meta.head_seq;
        Ok(ServerMessage::Catchup { snapshot, records, head_seq: meta.head_seq, more })
    }

    // ---- compaction advice ------------------------------------------------------------------------------------

    fn advice_pending(&self, now_ms: u64) -> bool {
        self.advice.as_ref().is_some_and(|a| now_ms < a.sent_ms.saturating_add(self.cfg.compact.advice_timeout_ms))
    }

    fn compaction_due(&self) -> bool {
        self.meta_now()
            .is_some_and(|m| m.log_records >= self.cfg.compact.records || m.log_bytes >= self.cfg.compact.bytes)
    }

    /// After a write: when the log is long, ask the longest-connected person to compact. Anyone asked before and
    /// silent for the timeout is skipped.
    fn advise_after_write(&mut self, now_ms: u64, out: &mut Output) {
        if !self.compaction_due() || self.advice_pending(now_ms) {
            return;
        }
        let asked = self.advice.take().map(|a| a.asked).unwrap_or_default();
        let mut candidates: Vec<(u64, ConnId)> = self.members().map(|(id, m)| (m.order, id)).collect();
        candidates.sort_unstable();
        let next = candidates
            .iter()
            .map(|(_, id)| *id)
            .find(|id| !asked.contains(id))
            .or_else(|| candidates.first().map(|(_, id)| *id));
        if let Some(conn) = next {
            let asked = if asked.contains(&conn) { Vec::new() } else { asked };
            self.send_advice(conn, now_ms, asked, out);
        }
    }

    /// Asks `conn` to compact now, unless somebody was asked a moment ago.
    pub(super) fn advise(&mut self, conn: ConnId, now_ms: u64, out: &mut Output) {
        if !self.advice_pending(now_ms) {
            let asked = self.advice.take().map(|a| a.asked).unwrap_or_default();
            self.send_advice(conn, now_ms, asked, out);
        }
    }

    fn send_advice(&mut self, conn: ConnId, now_ms: u64, mut asked: Vec<ConnId>, out: &mut Output) {
        asked.push(conn);
        out.send(conn, &ServerMessage::CompactAdvice { head_seq: self.head_seq() });
        self.advice = Some(Advice { conn, sent_ms: now_ms, asked });
    }

    pub(super) fn advise_if_due(&mut self, now_ms: u64, out: &mut Output) {
        self.advise_after_write(now_ms, out);
    }
}
