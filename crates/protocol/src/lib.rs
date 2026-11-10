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

mod binary;
mod error;
mod ids;
mod json;
mod limits;
mod message;
mod wire;

pub use error::{DecodeError, EncodeError, ValidationError};
pub use ids::{ClientId, RoomId};
pub use limits::*;
pub use message::{
    AwarenessState, ClientMessage, ErrorCode, PeerInfo, Point, ServerMessage, SyncKind, UserInfo,
    Viewport,
};
