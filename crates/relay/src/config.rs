//! The policy of a relay: what it keeps, for how long, and how much it takes. The numbers come from
//! `docs/design/collab-e2e.md` sections 8, 10 and 11. Every limit is at most its ceiling in `drafftink-protocol`.

use std::time::Duration;

use drafftink_protocol as proto;
use drafftink_protocol::v2::RoomLimits;

/// How long a room lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retention {
    /// Rooms are never deleted by the relay (self-host and the private instance).
    Never,
    /// A room is deleted `ttl` after its last visit, which is a `Join`. Edits, presence and image requests do not
    /// move the deadline.
    After {
        /// Time from the last visit to the deadline.
        ttl: Duration,
        /// A room past its deadline is deleted this long after its last connection closed.
        grace: Duration,
        /// A room past its deadline with people connected is looked at again after this long.
        recheck: Duration,
    },
}

impl Retention {
    /// The public demo: [`proto::ROOM_TTL`], [`proto::ROOM_LEAVE_GRACE`] and [`proto::ROOM_RECHECK`].
    pub const DEMO: Retention =
        Retention::After { ttl: proto::ROOM_TTL, grace: proto::ROOM_LEAVE_GRACE, recheck: proto::ROOM_RECHECK };

    /// A deadline `ttl` after the last visit, with the default grace and recheck.
    pub const fn after(ttl: Duration) -> Retention {
        Retention::After { ttl, grace: proto::ROOM_LEAVE_GRACE, recheck: proto::ROOM_RECHECK }
    }
}

/// When buffered records are written to the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlushRules {
    /// Records are written at most this long after the first of them arrived. Zero writes before every `Ack`.
    pub interval_ms: u64,
    /// Records are written at once when they take this many bytes.
    pub max_bytes: usize,
    /// A failed write is tried again after this long.
    pub retry_ms: u64,
}

/// When the relay asks a client to compact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactRules {
    /// Records after the snapshot that trigger advice.
    pub records: u32,
    /// Log bytes that trigger advice.
    pub bytes: u64,
    /// A newcomer is advised when the log it just applied has more records than this.
    pub join_records: u32,
    /// Advice that gets no `SnapshotResult` within this long goes to the next client.
    pub advice_timeout_ms: u64,
}

/// A setting that breaks a limit of the protocol.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ConfigError(pub String);

/// Everything a relay needs to know about its policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayConfig {
    /// How long rooms live.
    pub retention: Retention,
    /// When records are written.
    pub flush: FlushRules,
    /// When to ask for a compaction.
    pub compact: CompactRules,
    /// What `Joined` announces. Every value is at most its ceiling.
    pub limits: RoomLimits,
    /// Updates per second for the whole room.
    pub max_room_updates_per_sec: u16,
    /// Sealed bytes (snapshot plus log) a room may grow to.
    pub max_room_state_bytes: u64,
    /// People in one room.
    pub max_peers: usize,
    /// Presence messages per second for the whole room.
    pub presence_budget: u16,
    /// A connection that sends no `Join` in this time is closed.
    pub join_timeout_ms: u64,
    /// A connection not heard from for this long does not count as live for retention.
    pub stale_after_ms: u64,
    /// Rate-limit breaches in a row (within [`BREACH_MEMORY_MS`](Self::BREACH_MEMORY_MS)) that close a connection.
    pub max_breaches: u32,
    /// Whether a member may delete the room (`DeleteRoom`): the private instance.
    pub allow_delete_room: bool,
    /// Image uploads per second for the whole room.
    pub max_image_puts_per_sec: u16,
}

impl RelayConfig {
    /// Rate-limit breaches further apart than this start counting again from zero.
    pub const BREACH_MEMORY_MS: u64 = 10_000;

    /// The public demo: rooms live 48 hours after the last visit, and a room holds at most 32 MiB.
    pub fn demo() -> Self {
        RelayConfig {
            retention: Retention::DEMO,
            max_room_state_bytes: proto::MAX_ROOM_STATE_BYTES,
            ..Self::self_host()
        }
    }

    /// A self-hosted relay: rooms are kept, and a room may hold 256 MiB.
    pub fn self_host() -> Self {
        RelayConfig {
            retention: Retention::Never,
            flush: FlushRules { interval_ms: 1_000, max_bytes: 256 * 1024, retry_ms: 1_000 },
            compact: CompactRules {
                records: 500,
                bytes: 4 * 1024 * 1024,
                join_records: 100,
                advice_timeout_ms: 30_000,
            },
            limits: RoomLimits::DEFAULT,
            max_room_updates_per_sec: proto::MAX_ROOM_UPDATES_PER_SEC,
            max_room_state_bytes: proto::SELF_HOST_ROOM_STATE_BYTES,
            max_peers: proto::MAX_PEERS_PER_ROOM,
            presence_budget: proto::ROOM_PRESENCE_BUDGET,
            join_timeout_ms: 10_000,
            stale_after_ms: 2 * 60 * 1000,
            max_breaches: 50,
            allow_delete_room: false,
            max_image_puts_per_sec: 10,
        }
    }

    /// Checks the values against the ceilings of the protocol.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let bad = |text: &str| Err(ConfigError(text.to_owned()));
        let l = &self.limits;
        if l.max_record_bytes as usize > proto::MAX_RECORD_BYTES || l.max_record_bytes == 0 {
            return bad("max record bytes must be between 1 and the protocol limit");
        }
        if l.max_frame_bytes as usize > proto::MAX_FRAME_BYTES
            || l.max_frame_bytes < l.max_record_bytes.saturating_add(1024)
        {
            return bad("max frame bytes must hold a record and stay within the protocol limit");
        }
        if l.max_updates_per_sec == 0 || self.max_room_updates_per_sec == 0 || self.presence_budget == 0 {
            return bad("update and presence rates must be at least 1");
        }
        if self.max_peers == 0 || self.max_peers > proto::MAX_PEERS {
            return bad("max peers must be between 1 and 256");
        }
        if l.max_image_bytes as usize > proto::MAX_IMAGE_BYTES
            || l.max_room_image_bytes > proto::MAX_ROOM_IMAGE_BYTES
            || l.max_room_images > proto::MAX_ROOM_IMAGES
        {
            // `Joined` validates the limits it announces against the same ceilings.
            return bad("image limits must stay within the protocol ceilings");
        }
        if self.max_room_state_bytes == 0 || self.compact.records == 0 {
            return bad("room state and compaction limits must be at least 1");
        }
        Ok(())
    }
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self::self_host()
    }
}
