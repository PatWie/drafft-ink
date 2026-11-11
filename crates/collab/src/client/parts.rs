//! The state of a client by concern: connection health, uploads of local edits, reconciling with the server.

use std::collections::VecDeque;

use super::{Block, Inflight, PendingSnapshot, RESYNC_MIN_GAP_MS};

/// Liveness of the connection and the schedule of reconnects.
#[derive(Debug)]
pub(super) struct Health {
    pub(super) last_tick_ms: u64,
    pub(super) last_rx_ms: u64,
    pub(super) last_ping_ms: u64,
    pub(super) ping_nonce: u64,
    pub(super) stable_since: Option<u64>,
    pub(super) attempt: u32,
    pub(super) rng: u64,
    pub(super) offline_reason: String,
    pub(super) server_retry_ms: Option<u64>,
}

impl Health {
    pub(super) fn new(rng: u64) -> Self {
        Self {
            last_tick_ms: 0,
            last_rx_ms: 0,
            last_ping_ms: 0,
            ping_nonce: 0,
            stable_since: None,
            attempt: 0,
            rng,
            offline_reason: String::new(),
            server_retry_ms: None,
        }
    }
}

/// Local edits on their way to the server.
#[derive(Debug)]
pub(super) struct Uploads {
    pub(super) next_seq: u32,
    pub(super) inflight: VecDeque<Inflight>,
    pub(super) flush_at: Option<u64>,
    pub(super) want_flush: bool,
    pub(super) last_update_sent_ms: Option<u64>,
    pub(super) send_not_before: u64,
    pub(super) blocked: Option<Block>,
    pub(super) local_dirty: bool,
}

impl Uploads {
    pub(super) fn new() -> Self {
        Self {
            next_seq: 1,
            inflight: VecDeque::new(),
            flush_at: None,
            want_flush: false,
            last_update_sent_ms: None,
            send_not_before: 0,
            blocked: None,
            local_dirty: false,
        }
    }
}

/// Asking the server what this copy lacks, and holding a snapshot that waits for a decision.
#[derive(Debug)]
pub(super) struct Reconcile {
    pub(super) resync_due: Option<u64>,
    pub(super) sync_outstanding: Option<u64>,
    pub(super) resync_gap_ms: u64,
    pub(super) last_resync_ms: Option<u64>,
    pub(super) pending_snapshot: Option<PendingSnapshot>,
}

impl Reconcile {
    pub(super) fn new() -> Self {
        Self {
            resync_due: None,
            sync_outstanding: None,
            resync_gap_ms: RESYNC_MIN_GAP_MS,
            last_resync_ms: None,
            pending_snapshot: None,
        }
    }
}
