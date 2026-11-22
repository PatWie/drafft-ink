//! The state of one connection.

use drafftink_protocol::ClientId;

use crate::config::RelayConfig;
use crate::rate::{MinGap, TokenBucket};

/// Catch-up requests a connection may make at once: one a second, with a burst for the pages of a long catch-up.
const CATCHUP_BURST: u32 = 4;

/// A connection of the host.
pub(super) struct Conn {
    /// The last time a frame arrived on it.
    pub(super) last_heard_ms: u64,
    pub(super) phase: Phase,
}

impl Conn {
    pub(super) fn new(now_ms: u64, join_by_ms: u64) -> Self {
        Conn { last_heard_ms: now_ms, phase: Phase::Waiting { join_by_ms } }
    }
}

pub(super) enum Phase {
    /// Connected, and the first message must be `Join` before this time.
    Waiting { join_by_ms: u64 },
    /// Joined.
    Member(Member),
}

/// A person in the room.
pub(super) struct Member {
    pub(super) client_id: ClientId,
    /// Join order: the longest-connected person has the smallest.
    pub(super) order: u64,
    pub(super) updates: TokenBucket,
    pub(super) presence: TokenBucket,
    pub(super) direct: TokenBucket,
    pub(super) catchup: TokenBucket,
    pub(super) snapshots: MinGap,
    breaches: u32,
    last_breach_ms: Option<u64>,
}

impl Member {
    pub(super) fn new(client_id: ClientId, order: u64, now_ms: u64, cfg: &RelayConfig, presence_rate: u16) -> Self {
        let updates = u32::from(cfg.limits.max_updates_per_sec);
        Member {
            client_id,
            order,
            updates: TokenBucket::new(updates, updates * 2, now_ms),
            presence: TokenBucket::new(u32::from(presence_rate), u32::from(presence_rate) * 2, now_ms),
            direct: TokenBucket::new(
                u32::from(drafftink_protocol::MAX_DIRECT_PER_SEC),
                u32::from(drafftink_protocol::MAX_DIRECT_PER_SEC) * 2,
                now_ms,
            ),
            catchup: TokenBucket::new(1, CATCHUP_BURST, now_ms),
            snapshots: MinGap::default(),
            breaches: 0,
            last_breach_ms: None,
        }
    }

    /// Counts a rate-limit breach. True when the connection has had enough.
    pub(super) fn breach(&mut self, now_ms: u64, max_breaches: u32) -> bool {
        if self.last_breach_ms.is_some_and(|t| now_ms.saturating_sub(t) > RelayConfig::BREACH_MEMORY_MS) {
            self.breaches = 0;
        }
        self.last_breach_ms = Some(now_ms);
        self.breaches += 1;
        self.breaches >= max_breaches
    }
}
