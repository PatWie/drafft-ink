//! Protocol v2: sealed, end-to-end encrypted rooms (`docs/protocol.md`, `docs/design/collab-e2e.md` section 7).
//!
//! The relay in a v2 room is a blind, ordered log. It checks that a joiner holds the room's membership token,
//! numbers sealed records, stores them, and fans them out. It never reads a payload: updates, snapshots, presence
//! and images are sealed by `drafftink-seal` with a key that only the people in the room have.
//!
//! - [`ClientMessage`] and [`ServerMessage`] are the two message sets (binary form only).
//! - [`RecordHeader`] is the plaintext header of a sealed record, the only part a relay reads.
//! - [`RoomId`], [`ImageName`], [`MembershipToken`] and [`Proof`] name a room and prove membership.
//!
//! Every decoder takes hostile input: it returns a [`DecodeError`](crate::DecodeError), never panics, and
//! allocates no more than the input holds.

mod binary;
mod ids;
mod message;
mod record;

pub use ids::{
    IMAGE_NAME_BYTES, ImageName, MEMBERSHIP_TOKEN_BYTES, MembershipToken, Proof, ProofScheme, ROOM_ID_BYTES,
    ROOM_ID_LABEL, RoomId,
};
pub use message::{
    CatchupSnapshot, ClientMessage, ErrorCode, JoinMode, PresenceEntry, RoomLimits, RoomState, ServerMessage,
    StoredRecord,
};
pub use record::{RECORD_FORMAT_VERSION, RecordError, RecordHeader, RecordKind};
