//! Protocol versions, hard size limits, timing constants and the reconnect backoff.
//!
//! The constants here are hard ceilings. Decoders enforce them before they allocate.
//! A server may advertise lower, per-deployment values in [`Limits`] (sent in a v1 `Joined`) or in
//! [`RoomLimits`](crate::v2::RoomLimits) (a v2 `Joined`).

use std::time::Duration;

/// The plaintext protocol: Loro updates in the clear, the server merges them (`docs/protocol.md`).
pub const PROTOCOL_V1: u16 = 1;
/// The end-to-end encrypted protocol: sealed records the relay cannot read ([`crate::v2`]).
pub const PROTOCOL_V2: u16 = 2;
/// The highest protocol version this build speaks.
pub const PROTOCOL_VERSION: u16 = PROTOCOL_V2;
/// The lowest protocol version this build still speaks.
pub const MIN_PROTOCOL_VERSION: u16 = PROTOCOL_V1;

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

// ---- protocol v2 (sealed rooms): see `docs/design/collab-e2e.md` sections 7, 9.4, 10 and 11.4 ----

/// Size of the plaintext part of a sealed record: format version, kind, sender, counter, snapshot cover.
pub const RECORD_HEADER_BYTES: usize = 34;
/// Size of the random nonce of a sealed record (XChaCha20-Poly1305).
pub const RECORD_NONCE_BYTES: usize = 24;
/// Size of the length prefix in front of the sealed payload.
pub const RECORD_LEN_PREFIX_BYTES: usize = 4;
/// Size of the Poly1305 tag at the end of a sealed record.
pub const RECORD_TAG_BYTES: usize = 16;
/// Largest sealed record in one `Append`, `Record` or `Direct`, in bytes.
pub const MAX_RECORD_BYTES: usize = 4 * 1024 * 1024;
/// Largest sealed snapshot record in a `PutSnapshot` or a `Catchup`, in bytes.
pub const MAX_SNAPSHOT_RECORD_BYTES: usize = MAX_SYNC_BYTES;
/// Largest sealed presence record, in bytes. A selection of 512 ids of 64 bytes is about 33 KiB.
pub const MAX_PRESENCE_RECORD_BYTES: usize = 64 * 1024;
/// What sealing adds to a payload: the 34 byte header, the 24 byte nonce, the 4 byte length prefix inside the
/// ciphertext and the 16 byte tag. It is also the size of the smallest sealed record (an empty payload).
///
/// The design (section 6.3) counts 74 bytes and leaves the length prefix out, but the prefix is part of the
/// sealed plaintext, so a sealed image of [`MAX_IMAGE_BYTES`] is `MAX_IMAGE_BYTES + 78` bytes.
pub const SEAL_OVERHEAD: usize = RECORD_HEADER_BYTES + RECORD_NONCE_BYTES + RECORD_LEN_PREFIX_BYTES + RECORD_TAG_BYTES;
/// Most records in one `Catchup`. The relay sets `more` and the client asks again.
pub const MAX_CATCHUP_RECORDS: usize = 64;
/// A relay stops adding records to a `Catchup` once their bytes pass this, and sets `more`. It always adds at
/// least one record, so a single record can exceed it. The frame limit bounds the rest.
pub const CATCHUP_TARGET_BYTES: usize = 2 * 1024 * 1024;
/// Longest proof in a v2 `Join`, in bytes. A membership token is 32.
pub const MAX_PROOF_BYTES: usize = 256;
/// Longest bot-check token in a v2 `Join`, in bytes.
pub const MAX_CAPTCHA_BYTES: usize = 4096;
/// Most presence records in one `PresenceBundle`.
pub const MAX_BUNDLE_ENTRIES: usize = MAX_PEERS;
/// The text frame a client sends every [`PING_INTERVAL_MS`]. Text frames other than the two keepalives are refused.
pub const KEEPALIVE_PING: &str = "ping";
/// The text frame the relay answers with.
pub const KEEPALIVE_PONG: &str = "pong";

/// Largest image before sealing, in bytes. The user may lower it (to 2 MiB) in a deployment.
pub const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;
/// Largest sealed image on the relay: [`MAX_IMAGE_BYTES`] plus [`SEAL_OVERHEAD`].
pub const MAX_SEALED_IMAGE_BYTES: usize = MAX_IMAGE_BYTES + SEAL_OVERHEAD;
/// Most sealed image bytes in one room.
pub const MAX_ROOM_IMAGE_BYTES: u64 = 50 * 1024 * 1024;
/// Most images in one room. Bounds the row count of the relay's store.
pub const MAX_ROOM_IMAGES: u32 = 200;

/// Most people in one sealed room on a default relay. The protocol maximum is [`MAX_PEERS`].
pub const MAX_PEERS_PER_ROOM: usize = 64;
/// Presence messages per second that a whole room may send. Each person gets a share, at most
/// [`DEFAULT_MAX_AWARENESS_PER_SEC`].
pub const ROOM_PRESENCE_BUDGET: u16 = 40;
/// The relay sends at most one `PresenceBundle` per this many milliseconds.
pub const PRESENCE_TICK_MS: u32 = 100;
/// Most `Append` messages per second for a whole room.
pub const MAX_ROOM_UPDATES_PER_SEC: u16 = 200;
/// Largest sealed state of a room (snapshot plus log) on the public demo, in bytes.
pub const MAX_ROOM_STATE_BYTES: u64 = 32 * 1024 * 1024;
/// Largest sealed state of a room on a self-hosted relay by default, in bytes.
pub const SELF_HOST_ROOM_STATE_BYTES: u64 = 256 * 1024 * 1024;
/// Shortest gap between two `PutSnapshot` messages of one connection, in milliseconds.
pub const PUT_SNAPSHOT_MIN_INTERVAL_MS: u32 = 10_000;
/// Shortest gap between two `Catchup` requests of one connection, in milliseconds.
pub const CATCHUP_MIN_INTERVAL_MS: u32 = 1_000;
/// Most `Direct` messages per second per connection.
pub const MAX_DIRECT_PER_SEC: u16 = 5;

/// A room is deleted this long after its most recent visit (an accepted `Join`): 48 hours. Edits, presence and
/// image requests do not move the deadline. A private instance sets no deadline.
pub const ROOM_TTL: Duration = Duration::from_secs(48 * 60 * 60);
/// A room past its deadline is deleted this long after its last connection closes. A page reload looks like
/// leave and join, and a join inside the grace is a visit.
pub const ROOM_LEAVE_GRACE: Duration = Duration::from_secs(60);
/// A room that is past its deadline with people connected is looked at again after this long. Only connections
/// heard from within [`IDLE_TIMEOUT_MS`] count as live.
pub const ROOM_RECHECK: Duration = Duration::from_secs(60 * 60);

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

// `Limits` stores sizes as u32, and an update must fit in a frame.
const _: () = assert!(MAX_UPDATE_BYTES < MAX_FRAME_BYTES && MAX_FRAME_BYTES <= u32::MAX as usize);
// A sealed update fits in a frame, a sealed snapshot is larger than an update, and a sealed image fits in a frame.
const _: () = assert!(MAX_RECORD_BYTES < MAX_SNAPSHOT_RECORD_BYTES && MAX_SNAPSHOT_RECORD_BYTES < MAX_FRAME_BYTES);
const _: () = assert!(MAX_PRESENCE_RECORD_BYTES <= MAX_RECORD_BYTES);

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
