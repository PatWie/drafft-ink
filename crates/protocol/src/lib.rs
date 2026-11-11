//! drafftink-protocol: the versioned collaboration wire protocol shared by the client
//! (`crates/collab`) and the server (`apps/drafftink-server`). See `docs/protocol.md`.
//!
//! This crate is plain data and codecs. CRDT payloads (Loro updates, snapshots, version
//! vectors) are opaque bytes here, so the crate does not depend on Loro, on a runtime or on a
//! socket, and it builds for `wasm32-unknown-unknown`.
//!
//! - [`ClientMessage`] and [`ServerMessage`] are the two message sets.
//! - `encode` / `decode` is the binary form (production). `to_json` / `from_json` is the
//!   debug form.
//! - Every decoder takes hostile input: it returns a [`DecodeError`], never panics, and
//!   allocates no more than the input holds.

#![warn(missing_docs)]

mod binary;
mod error;
mod ids;
mod json;
mod limits;
mod message;
mod wire;

pub use error::{DecodeError, EncodeError, Rule, ValidationError};
pub use ids::{ClientId, RoomId};
pub use limits::{
    AWARENESS_HEARTBEAT_MS, AWARENESS_MIN_INTERVAL_MS, AWARENESS_TIMEOUT_MS, DEFAULT_MAX_AWARENESS_PER_SEC,
    DEFAULT_MAX_UPDATES_PER_SEC, IDLE_TIMEOUT_MS, Limits, MAX_AUTH_TOKEN_BYTES, MAX_ELEMENT_ID_BYTES,
    MAX_ERROR_MESSAGE_BYTES, MAX_FRAME_BYTES, MAX_JSON_BYTES, MAX_PEERS, MAX_ROOM_ID_BYTES, MAX_SELECTION_IDS,
    MAX_SERVER_NAME_BYTES, MAX_SYNC_BYTES, MAX_UPDATE_BYTES, MAX_USER_NAME_BYTES, MAX_VERSION_BYTES,
    MIN_PROTOCOL_VERSION, PING_INTERVAL_MS, PROTOCOL_VERSION, RECONNECT_BASE_MS, RECONNECT_MAX_MS, negotiate_version,
    reconnect_delay_ms,
};
pub use message::{
    AwarenessState, ClientMessage, ErrorCode, PeerInfo, Point, ServerMessage, SyncKind, UserInfo, Viewport,
};
