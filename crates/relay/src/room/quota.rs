//! The daily limit of the host (see [`ms_until_utc_midnight`]). A store call that fails with
//! [`StoreError::DailyLimit`] is noticed wherever it happens, so the host can tell its edge, and is told to a person
//! in plain words with the time of the reset.

use drafftink_protocol::v2::{ErrorCode, ServerMessage};

use super::{ConnId, Output, Relay, retry_error};
use crate::images::{ImageBody, ImageResponse, ImageStatus};
use crate::quota::ms_until_utc_midnight;
use crate::store::{BlobStore, RelayStore, StoreError};

/// Whether a store call failed with the daily limit since the host last got an [`Output`].
#[derive(Debug, Default)]
pub(super) struct QuotaWatch {
    hit: bool,
}

impl QuotaWatch {
    /// Passes a store result through and remembers a daily limit in it.
    pub(super) fn watch<T>(&mut self, result: Result<T, StoreError>) -> Result<T, StoreError> {
        self.hit |= matches!(result, Err(StoreError::DailyLimit));
        result
    }

    /// Whether the limit was hit, and forgets it.
    pub(super) fn take(&mut self) -> bool {
        std::mem::take(&mut self.hit)
    }
}

const DAILY_LIMIT_TEXT: &str = "The free demo server has reached its daily limit and resets at 00:00 UTC. \
Your drawing is saved on this device. Try again later.";

impl<S: RelayStore, B: BlobStore> Relay<S, B> {
    /// The error a client sees when the store fails: the daily limit with the time of the reset, or `Unavailable`.
    pub(super) fn store_failure(error: &StoreError, now_ms: u64) -> ServerMessage {
        match error {
            StoreError::DailyLimit => {
                retry_error(ErrorCode::DailyLimit, DAILY_LIMIT_TEXT, ms_until_utc_midnight(now_ms))
            }
            StoreError::Backend(_) | StoreError::Corrupt(_) => Self::unavailable(),
        }
    }

    /// Answers a failed store call to `conn`. The daily limit always ends the connection (the code says a client
    /// must not retry on it at once); another failure ends it only when `close` says so.
    pub(super) fn refuse_store(
        &mut self,
        conn: ConnId,
        now_ms: u64,
        error: &StoreError,
        close: bool,
        out: &mut Output,
    ) {
        let close = close || *error == StoreError::DailyLimit;
        self.refuse(conn, now_ms, Self::store_failure(error, now_ms), close, out);
    }

    /// The answer of an image route to a failed store call.
    pub(super) fn image_failure(error: &StoreError, now_ms: u64) -> ImageResponse {
        match error {
            StoreError::DailyLimit => ImageResponse {
                status: ImageStatus::DailyLimit,
                retry_after_ms: Some(ms_until_utc_midnight(now_ms)),
                body: ImageBody::Empty,
            },
            StoreError::Backend(_) | StoreError::Corrupt(_) => ImageResponse::status(ImageStatus::Unavailable),
        }
    }

    /// The daily limit stopped a write of the log. A retry every second would be hopeless until midnight, so the room
    /// stops instead: every connection hears the limit and is closed (the host, with nobody left, goes idle), and the
    /// buffered records are dropped. They never got an `Ack`: their senders keep them and send them again after they
    /// rejoin. A peer that already saw one of the dropped sequence numbers rejoins with a `last_seq` above the head,
    /// and the join serves that as a fresh room.
    pub(super) fn shed_everyone(&mut self, now_ms: u64, out: &mut Output) {
        let frame = super::encode(&Self::store_failure(&StoreError::DailyLimit, now_ms));
        let open: Vec<ConnId> = self.conns.keys().copied().collect();
        for conn in open {
            if let Some(frame) = &frame {
                out.send_frame(conn, frame.clone());
            }
            out.close(conn);
        }
        self.conns.clear();
        self.log.drop_pending();
        self.advice = None;
        self.presence = Default::default();
        self.life.connection_closed(now_ms, 0);
    }
}
