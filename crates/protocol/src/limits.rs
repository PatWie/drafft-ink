//! Protocol version, hard size limits, timing constants and the reconnect backoff.
//!
//! The constants here are hard ceilings. Decoders enforce them before they allocate.
//! A server may advertise lower, per-deployment values in [`Limits`] (sent in `Joined`).

/// The protocol version this build speaks (highest).
pub const PROTOCOL_VERSION: u16 = 1;
/// The lowest protocol version this build still speaks.
pub const MIN_PROTOCOL_VERSION: u16 = 1;

/// Largest accepted frame (one WebSocket binary message), in bytes.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
/// Largest accepted JSON debug message, in bytes.
pub const MAX_JSON_BYTES: usize = 2 * MAX_FRAME_BYTES;
/// Largest live `Update` payload, in bytes.
pub const MAX_UPDATE_BYTES: usize = 4 * 1024 * 1024;
/// Largest `SyncResponse` payload (updates or snapshot), in bytes. Leaves room for the envelope.
pub const MAX_SYNC_BYTES: usize = MAX_FRAME_BYTES - 1024 * 1024;
/// Largest opaque version-vector blob, in bytes.
pub const MAX_VERSION_BYTES: usize = 256 * 1024;
/// Longest room id, in bytes.
pub const MAX_ROOM_ID_BYTES: usize = 128;
/// Longest auth token, in bytes.
pub const MAX_AUTH_TOKEN_BYTES: usize = 4096;
/// Longest user display name, in bytes.
pub const MAX_USER_NAME_BYTES: usize = 64;
/// Longest element id in a selection, in bytes.
pub const MAX_ELEMENT_ID_BYTES: usize = 64;
/// Most element ids in one awareness selection.
pub const MAX_SELECTION_IDS: usize = 512;
/// Most peers listed in one `Joined`.
pub const MAX_PEERS: usize = 256;
/// Longest error message, in bytes.
pub const MAX_ERROR_MESSAGE_BYTES: usize = 512;
/// Longest server software string, in bytes.
pub const MAX_SERVER_NAME_BYTES: usize = 64;

/// Default most live `Update` messages per second per client (token bucket refill rate).
pub const DEFAULT_MAX_UPDATES_PER_SEC: u16 = 60;
/// Default most awareness messages per second per client.
pub const DEFAULT_MAX_AWARENESS_PER_SEC: u16 = 20;
/// Smallest gap a client should leave between awareness messages, in milliseconds.
pub const AWARENESS_MIN_INTERVAL_MS: u32 = 50;
/// A client resends its awareness at least this often while connected, in milliseconds.
pub const AWARENESS_HEARTBEAT_MS: u32 = 10_000;
/// A receiver forgets a peer's awareness after this much silence, in milliseconds.
pub const AWARENESS_TIMEOUT_MS: u32 = 30_000;
/// Interval between pings, in milliseconds.
pub const PING_INTERVAL_MS: u32 = 20_000;
/// A connection with no frame for this long is dead, in milliseconds.
pub const IDLE_TIMEOUT_MS: u32 = 50_000;

/// First reconnect delay, in milliseconds.
pub const RECONNECT_BASE_MS: u32 = 500;
/// Longest reconnect delay, in milliseconds.
pub const RECONNECT_MAX_MS: u32 = 30_000;

/// Per-deployment limits that the server announces in `Joined`.
/// Every value is at most the matching hard ceiling above.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Limits {
    /// Largest frame the server accepts.
    pub max_frame_bytes: u32,
    /// Largest live update the server accepts.
    pub max_update_bytes: u32,
    /// Updates per second the server accepts from this client.
    pub max_updates_per_sec: u16,
    /// Awareness messages per second the server accepts from this client.
    pub max_awareness_per_sec: u16,
}

// `Limits` stores sizes as u32; the hard ceilings must fit.
const _: () = assert!(MAX_FRAME_BYTES <= u32::MAX as usize && MAX_UPDATE_BYTES <= u32::MAX as usize);

impl Limits {
    /// The hard-ceiling defaults.
    pub const DEFAULT: Limits = Limits {
        max_frame_bytes: MAX_FRAME_BYTES as u32,
        max_update_bytes: MAX_UPDATE_BYTES as u32,
        max_updates_per_sec: DEFAULT_MAX_UPDATES_PER_SEC,
        max_awareness_per_sec: DEFAULT_MAX_AWARENESS_PER_SEC,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Limits::DEFAULT
    }
}

/// Picks the protocol version both sides speak: the highest in both ranges, or `None`.
pub fn negotiate_version(client_min: u16, client_max: u16) -> Option<u16> {
    if client_min > client_max {
        return None;
    }
    let high = client_max.min(PROTOCOL_VERSION);
    let low = client_min.max(MIN_PROTOCOL_VERSION);
    (low <= high).then_some(high)
}

/// Delay before reconnect attempt `attempt` (0 is the first retry), in milliseconds.
///
/// Exponential from [`RECONNECT_BASE_MS`], capped at [`RECONNECT_MAX_MS`], with "equal jitter":
/// half the delay is fixed and the other half is picked by `jitter`, so a crowd of clients
/// that lost the server together does not come back together. `jitter` is any random u32.
pub fn reconnect_delay_ms(attempt: u32, jitter: u32) -> u32 {
    let exp = attempt.min(16);
    let full = RECONNECT_BASE_MS.saturating_mul(1u32 << exp).min(RECONNECT_MAX_MS);
    let half = full / 2;
    // `full` is at most RECONNECT_MAX_MS, so `full - half + 1` cannot overflow.
    half + jitter % (full - half + 1)
}
