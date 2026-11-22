//! drafftink-protocol: the versioned collaboration wire protocol shared by the client
//! (`crates/collab`) and the server (`apps/drafftink-server`, run by `drafftink-headless serve`). See `docs/protocol.md`.
//!
//! This crate is plain data and codecs (and the one hash that names a sealed room). CRDT payloads (Loro updates, snapshots, version
//! vectors) are opaque bytes here, so the crate does not depend on Loro, on a runtime or on a
//! socket, and it builds for `wasm32-unknown-unknown`.
//!
//! - [`ClientMessage`] and [`ServerMessage`] are the two message sets of protocol v1, plaintext rooms.
//! - [`v2`] has the message sets of protocol v2, sealed rooms. A host reads [`JoinVersions`] from a first
//!   frame, negotiates, and picks the decoder of that version.
//! - `encode` / `decode` is the binary form (production). `to_json` / `from_json` is the
//!   debug form (v1 only).
//! - Every decoder takes hostile input: it returns a [`DecodeError`], never panics, and
//!   allocates no more than the input holds.

#![warn(missing_docs)]

mod binary;
mod error;
mod ids;
mod json;
mod limits;
mod message;
pub mod v2;
mod version;
mod wire;

pub use error::{DecodeError, EncodeError, Rule, ValidationError};
pub use ids::{ClientId, RoomId};
pub use limits::{
    AWARENESS_HEARTBEAT_MS, AWARENESS_MIN_INTERVAL_MS, AWARENESS_TIMEOUT_MS, CATCHUP_MIN_INTERVAL_MS,
    CATCHUP_TARGET_BYTES, DEFAULT_MAX_AWARENESS_PER_SEC, DEFAULT_MAX_UPDATES_PER_SEC, IDLE_TIMEOUT_MS, KEEPALIVE_PING,
    KEEPALIVE_PONG, Limits, MAX_AUTH_TOKEN_BYTES, MAX_BUNDLE_ENTRIES, MAX_CAPTCHA_BYTES, MAX_CATCHUP_RECORDS,
    MAX_DIRECT_PER_SEC, MAX_ELEMENT_ID_BYTES, MAX_ERROR_MESSAGE_BYTES, MAX_FRAME_BYTES, MAX_IMAGE_BYTES,
    MAX_JSON_BYTES, MAX_PEERS, MAX_PEERS_PER_ROOM, MAX_PRESENCE_RECORD_BYTES, MAX_PROOF_BYTES, MAX_RECORD_BYTES,
    MAX_ROOM_ID_BYTES, MAX_ROOM_IMAGE_BYTES, MAX_ROOM_IMAGES, MAX_ROOM_STATE_BYTES, MAX_ROOM_UPDATES_PER_SEC,
    MAX_SEALED_IMAGE_BYTES, MAX_SELECTION_IDS, MAX_SERVER_NAME_BYTES, MAX_SNAPSHOT_RECORD_BYTES, MAX_SYNC_BYTES,
    MAX_UPDATE_BYTES, MAX_USER_NAME_BYTES, MAX_VERSION_BYTES, MIN_PROTOCOL_VERSION, PING_INTERVAL_MS, PRESENCE_TICK_MS,
    PROTOCOL_V1, PROTOCOL_V2, PROTOCOL_VERSION, PUT_SNAPSHOT_MIN_INTERVAL_MS, RECONNECT_BASE_MS, RECONNECT_MAX_MS,
    RECORD_HEADER_BYTES, RECORD_LEN_PREFIX_BYTES, RECORD_NONCE_BYTES, RECORD_TAG_BYTES, ROOM_LEAVE_GRACE,
    ROOM_PRESENCE_BUDGET, ROOM_RECHECK, ROOM_TTL, SEAL_OVERHEAD, SELF_HOST_ROOM_STATE_BYTES, reconnect_delay_ms,
};
pub use message::{
    AwarenessState, ClientMessage, ErrorCode, PeerInfo, Point, ServerMessage, SyncKind, UserInfo, Viewport,
};
pub use version::{JoinVersions, negotiate_version, negotiate_version_within};
