//! Rooms. One task per loaded room owns the Loro document, the peer table and the write queue
//! to the store. Connections talk to it through a bounded command channel, and it talks back
//! through each peer's [`Outbox`], which never blocks it.

mod actor;
mod manager;

pub use manager::RoomManager;

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use drafftink_protocol::{
    AwarenessState, ClientId, ErrorCode, MAX_SYNC_BYTES, PeerInfo, RoomId, ServerMessage, SyncKind, UserInfo,
};
use loro::{ExportMode, Frontiers, LoroDoc, VersionVector};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::time::Instant;
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::outbox::{Kind, Outbox};
use crate::schema;
use crate::store::{RoomStore, StoreError, StoredRoom};

/// A refusal sent to a client. Boxed because `ServerMessage` is large.
pub type Refusal = Box<ServerMessage>;

/// Depth of a room's command channel. A connection waits when it is full, which slows only
/// that sender.
pub(super) const COMMAND_QUEUE: usize = 1024;

/// A short, stable tag for logs. A room id is a capability when auth is off, so it is never
/// logged in full.
pub fn room_tag(room: &RoomId) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    room.hash(&mut h);
    format!("{:08x}", h.finish() as u32)
}

/// What a connection tells a room when it joins.
#[derive(Clone)]
pub struct PeerSetup {
    /// Unique per connection (a reconnect has a new one, the same client id).
    pub conn: u64,
    /// The client's stable id.
    pub client_id: ClientId,
    /// Name and colour from `Join`.
    pub user: Option<UserInfo>,
    /// The client's version vector from `Join`, opaque.
    pub version: Vec<u8>,
    /// The negotiated protocol version.
    pub protocol_version: u16,
    /// Where to send messages to the client.
    pub outbox: Outbox,
}

/// A command from a connection to its room.
pub enum RoomCmd {
    /// A client joins. The reply says whether the room accepted it.
    Join {
        /// The joining client.
        peer: Box<PeerSetup>,
        /// Answered once the join is decided.
        reply: oneshot::Sender<Result<(), Refusal>>,
    },
    /// A client sent a document update.
    Update {
        /// The sending connection.
        conn: u64,
        /// The client's sequence number, echoed in the acknowledgement.
        seq: u32,
        /// The Loro update.
        bytes: Vec<u8>,
    },
    /// A client asks for what it is missing.
    RequestSync {
        /// The asking connection.
        conn: u64,
        /// The client's version vector, opaque.
        version: Vec<u8>,
    },
    /// A client sent its presence.
    Awareness {
        /// The sending connection.
        conn: u64,
        /// The presence state.
        state: AwarenessState,
    },
    /// A client left or its connection closed.
    Leave {
        /// The leaving connection.
        conn: u64,
    },
    /// Save the room and stop.
    Shutdown {
        /// Signalled when the room is saved.
        done: oneshot::Sender<()>,
    },
}

/// A handle to a loaded room.
#[derive(Clone)]
pub struct RoomHandle {
    /// The command channel.
    pub tx: mpsc::Sender<RoomCmd>,
    pub(super) generation: u64,
}

pub(super) fn unavailable(message: &str, retry_after_ms: u32) -> Refusal {
    let mut msg = ServerMessage::error(ErrorCode::Unavailable, message);
    if let ServerMessage::Error { retry_after_ms: slot, .. } = &mut msg {
        *slot = Some(retry_after_ms);
    }
    Box::new(msg)
}

pub(super) fn refuse(code: ErrorCode, message: impl Into<String>) -> Refusal {
    Box::new(ServerMessage::error(code, message))
}
