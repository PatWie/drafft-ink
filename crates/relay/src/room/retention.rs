//! Retention: the deadline policy applied to the room, and deleting it (by the deadline or by a member).

use drafftink_protocol::v2::{ErrorCode, ServerMessage};

use super::{ConnId, Effect, MetaState, Output, Relay, StoreOp};
use crate::lifecycle::Verdict;
use crate::quota::ms_until_utc_midnight;
use crate::store::{BlobStore, RelayStore, StoreError};

impl<S: RelayStore, B: BlobStore> Relay<S, B> {
    /// Asks the retention state machine what to do with the room, and does it. Sets the next retention wake.
    pub(super) fn run_retention(&mut self, now_ms: u64, out: &mut Output) {
        // A room that is not loaded yet (the host resumes stored rooms) is read now, so its deadline is seen.
        let Some(meta) = self.load_meta().ok().flatten() else {
            self.retention_wake = None;
            return;
        };
        if self.delete_retry_ms.is_some_and(|retry| now_ms < retry) {
            return;
        }
        let was_expiring = self.life.expiring();
        let live = self.live_count(now_ms);
        match self.life.check(meta.deadline_ms, now_ms, live) {
            Verdict::Keep { wake_at, expiring } => {
                self.retention_wake = wake_at;
                if expiring && !was_expiring {
                    self.broadcast(
                        out,
                        None,
                        &ServerMessage::Retention { expires_at_ms: meta.deadline_ms, expiring: true },
                    );
                }
            }
            Verdict::Delete => {
                // A failed deletion has set its own retry.
                let _ = self.delete_room(now_ms, out);
            }
        }
    }

    /// Deletes the room: its records, snapshot and images, and closes every connection. When a store fails, the
    /// deletion is tried again after the flush retry time, and after the daily limit resets when that was the failure.
    fn delete_room(&mut self, now_ms: u64, out: &mut Output) -> Result<(), StoreError> {
        let deleted =
            self.quota.watch(self.store.delete_all()).and_then(|()| self.quota.watch(self.blobs.delete_all()));
        if let Err(error) = deleted {
            let wait = match error {
                StoreError::DailyLimit => u64::from(ms_until_utc_midnight(now_ms)),
                StoreError::Backend(_) | StoreError::Corrupt(_) => self.cfg.flush.retry_ms,
            };
            let retry = now_ms.saturating_add(wait);
            self.delete_retry_ms = Some(retry);
            self.retention_wake = Some(retry);
            return Err(error);
        }
        self.delete_retry_ms = None;
        out.store_ops.push(StoreOp::DeleteRoom);
        let open: Vec<ConnId> = self.conns.keys().copied().collect();
        out.effects.extend(open.into_iter().map(|conn| Effect::Close { conn }));
        self.conns.clear();
        self.meta = MetaState::Missing;
        self.log = Default::default();
        self.presence = Default::default();
        self.advice = None;
        self.retention_wake = None;
        self.life = crate::lifecycle::Lifecycle::new(self.cfg.retention);
        out.deleted = true;
        Ok(())
    }

    /// A member asked to delete the room. Only a relay that allows it does so.
    pub(super) fn delete_requested(&mut self, conn: ConnId, now_ms: u64, out: &mut Output) {
        if !self.cfg.allow_delete_room {
            let text = "This server does not allow deleting rooms.";
            self.refuse(conn, now_ms, ServerMessage::error(ErrorCode::Forbidden, text), true, out);
            return;
        }
        if let Err(error) = self.delete_room(now_ms, out) {
            self.refuse_store(conn, now_ms, &error, false, out);
        }
    }
}
