//! Message types. These are plain data; encoding lives in `binary` and `json`.

use serde::{Deserialize, Serialize};

use crate::error::ValidationError;
use crate::ids::{ClientId, RoomId};
use crate::json::hex_bytes;
use crate::limits::*;

/// A user's display identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserInfo {
    /// Display name, at most [`MAX_USER_NAME_BYTES`] bytes.
    pub name: String,
    /// Colour as `0xRRGGBB`.
    pub color: u32,
}

/// A point in world coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    /// X in world units.
    pub x: f64,
    /// Y in world units.
    pub y: f64,
}

/// The visible world rectangle of a peer.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Viewport {
    /// Left edge in world units.
    pub x: f64,
    /// Top edge in world units.
    pub y: f64,
    /// Width in world units, not negative.
    pub width: f64,
    /// Height in world units, not negative.
    pub height: f64,
}

/// A peer's ephemeral presence. Always a full replacement of the previous state of that peer.
/// It is never stored by the server and never enters the document.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AwarenessState {
    /// Name and colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<UserInfo>,
    /// Pointer position, or `None` when the pointer is outside the canvas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Point>,
    /// Ids of the selected elements, at most [`MAX_SELECTION_IDS`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selection: Vec<String>,
    /// The visible world rectangle (used for "follow user").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewport: Option<Viewport>,
}

/// A peer as listed in `Joined` and `PeerJoined`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerInfo {
    /// The peer's id.
    pub client_id: ClientId,
    /// Name and colour from the peer's `Join`, or its latest awareness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<UserInfo>,
}

/// What a `SyncResponse` carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncKind {
    /// The updates the client lacks, computed from its version vector.
    Updates,
    /// A full snapshot, sent when the client is far behind or has no history.
    Snapshot,
}

impl SyncKind {
    /// The value used in binary frames.
    pub(crate) const fn as_u8(self) -> u8 {
        match self {
            SyncKind::Updates => 0,
            SyncKind::Snapshot => 1,
        }
    }

    /// Parses the value used in binary frames.
    pub(crate) const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(SyncKind::Updates),
            1 => Some(SyncKind::Snapshot),
            _ => None,
        }
    }
}

/// Why the server sends an `Error`. The numeric values are stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum ErrorCode {
    /// The message could not be decoded or broke a rule.
    Malformed = 1,
    /// No protocol version in common. The `message` names the versions the server speaks.
    UnsupportedVersion = 2,
    /// Auth is on and the token is missing or invalid.
    Unauthorized = 3,
    /// The request is understood but not allowed (for example, a disallowed Origin).
    Forbidden = 4,
    /// The room has reached its peer limit.
    RoomFull = 5,
    /// A frame or field is larger than the limit.
    TooLarge = 6,
    /// The client sends too fast. `retry_after_ms` says when to try again.
    RateLimited = 7,
    /// The server dropped messages for this client. Send `RequestSync` with your version.
    ResyncRequired = 8,
    /// Another connection with the same client id joined, so this one is closed.
    Replaced = 9,
    /// The first message was not `Join`.
    NotJoined = 10,
    /// A `Join` arrived on a connection that already joined.
    AlreadyJoined = 11,
    /// The server could not import the update bytes.
    InvalidUpdate = 12,
    /// The server cannot serve the room now (storage failure, shutdown). Retry later.
    Unavailable = 13,
    /// A bug in the server.
    Internal = 14,
}

impl ErrorCode {
    /// The stable numeric value used in binary frames.
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// Parses the numeric value.
    pub const fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            1 => ErrorCode::Malformed,
            2 => ErrorCode::UnsupportedVersion,
            3 => ErrorCode::Unauthorized,
            4 => ErrorCode::Forbidden,
            5 => ErrorCode::RoomFull,
            6 => ErrorCode::TooLarge,
            7 => ErrorCode::RateLimited,
            8 => ErrorCode::ResyncRequired,
            9 => ErrorCode::Replaced,
            10 => ErrorCode::NotJoined,
            11 => ErrorCode::AlreadyJoined,
            12 => ErrorCode::InvalidUpdate,
            13 => ErrorCode::Unavailable,
            14 => ErrorCode::Internal,
            _ => return None,
        })
    }

    /// Whether the server closes the connection after sending this code.
    /// Clients must not retry on `UnsupportedVersion`, `Unauthorized`, `Forbidden` and
    /// `Replaced` without a change (new build, new token, user action).
    pub const fn closes_connection(self) -> bool {
        matches!(
            self,
            ErrorCode::UnsupportedVersion
                | ErrorCode::Unauthorized
                | ErrorCode::Forbidden
                | ErrorCode::RoomFull
                | ErrorCode::Replaced
                | ErrorCode::NotJoined
                | ErrorCode::AlreadyJoined
                | ErrorCode::Malformed
                | ErrorCode::Internal
        )
    }

    /// Whether a client may reconnect on its own after this code.
    pub const fn retryable(self) -> bool {
        matches!(self, ErrorCode::RoomFull | ErrorCode::RateLimited | ErrorCode::Unavailable | ErrorCode::Internal)
    }
}

/// A message from a client to the server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// The first message on a connection: handshake and room join in one.
    Join {
        /// Lowest protocol version the client speaks.
        min_version: u16,
        /// Highest protocol version the client speaks.
        max_version: u16,
        /// The room to join. The server creates it when it does not exist.
        room: RoomId,
        /// The client's stable id.
        client_id: ClientId,
        /// Bearer token. Ignored unless the server runs with auth on.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth_token: Option<String>,
        /// The client's Loro version vector, opaque to the protocol. Empty means "I have
        /// nothing", and the server answers with a snapshot.
        #[serde(with = "hex_bytes")]
        version: Vec<u8>,
        /// Name and colour to show to the room.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user: Option<UserInfo>,
    },
    /// A local edit: opaque Loro update bytes. The server merges, persists, acknowledges
    /// and relays the bytes to the other peers.
    Update {
        /// Client-chosen number echoed in `Ack`; wraps around.
        seq: u32,
        /// Loro update bytes, at most [`MAX_UPDATE_BYTES`].
        #[serde(with = "hex_bytes")]
        update: Vec<u8>,
    },
    /// Ask for what the client lacks (after `ResyncRequired`, or on a suspected gap).
    RequestSync {
        /// The client's current version vector, opaque.
        #[serde(with = "hex_bytes")]
        version: Vec<u8>,
    },
    /// The client's presence, replacing the previous one.
    Awareness(AwarenessState),
    /// Liveness probe; the receiver answers `Pong` with the same nonce.
    Ping {
        /// Echoed in the answer.
        nonce: u64,
    },
    /// Answer to a server `Ping`.
    Pong {
        /// The nonce of the ping.
        nonce: u64,
    },
    /// Leave the room cleanly.
    Leave,
}

/// A message from the server to a client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Answer to `Join`. A `SyncResponse` follows at once.
    Joined {
        /// The negotiated protocol version.
        protocol_version: u16,
        /// Server software and version, for humans.
        server: String,
        /// The room joined.
        room: RoomId,
        /// The client's id, echoed.
        client_id: ClientId,
        /// The other peers now in the room.
        peers: Vec<PeerInfo>,
        /// The room's version vector, opaque. The client sends the updates the server lacks
        /// (its own changes since this version) as `Update` messages.
        #[serde(with = "hex_bytes")]
        server_version: Vec<u8>,
        /// Limits that apply to this connection.
        limits: Limits,
    },
    /// Catch-up data for a `Join` or `RequestSync`.
    SyncResponse {
        /// Whether `data` holds updates or a snapshot.
        kind: SyncKind,
        /// Loro bytes to import. May be empty when the client is current.
        #[serde(with = "hex_bytes")]
        data: Vec<u8>,
        /// The room's version vector after this data, opaque.
        #[serde(with = "hex_bytes")]
        server_version: Vec<u8>,
    },
    /// A relayed edit from another peer.
    Update {
        /// The sender, stamped by the server.
        from: ClientId,
        /// Loro update bytes.
        #[serde(with = "hex_bytes")]
        update: Vec<u8>,
    },
    /// The server has merged the client's `Update` with this `seq`.
    Ack {
        /// The `seq` of the update.
        seq: u32,
    },
    /// A peer joined.
    PeerJoined {
        /// The new peer.
        peer: PeerInfo,
    },
    /// A peer left or lost its connection.
    PeerLeft {
        /// The peer.
        client_id: ClientId,
    },
    /// A peer's presence.
    PeerAwareness {
        /// The sender, stamped by the server.
        from: ClientId,
        /// The presence.
        state: AwarenessState,
    },
    /// Something went wrong.
    Error {
        /// What kind of failure.
        code: ErrorCode,
        /// Plain words for the user or the log, at most [`MAX_ERROR_MESSAGE_BYTES`] bytes.
        message: String,
        /// For `RateLimited` and `Unavailable`: wait this long before retrying.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_after_ms: Option<u32>,
    },
    /// Liveness probe; the receiver answers `Pong` with the same nonce.
    Ping {
        /// Echoed in the answer.
        nonce: u64,
    },
    /// Answer to a client `Ping`.
    Pong {
        /// The nonce of the ping.
        nonce: u64,
    },
}

impl ServerMessage {
    /// Builds an `Error`, cutting the message to the size limit on a character boundary.
    pub fn error(code: ErrorCode, message: impl Into<String>) -> Self {
        let mut message = message.into();
        message.truncate(message.floor_char_boundary(MAX_ERROR_MESSAGE_BYTES));
        ServerMessage::Error { code, message, retry_after_ms: None }
    }

    /// The `Error` a server sends for a frame it could not decode.
    pub fn error_for_decode(err: &crate::DecodeError) -> Self {
        ServerMessage::error(err.error_code(), err.to_string())
    }
}

// ---- validation ---------------------------------------------------------------------------

fn check_len(field: &'static str, len: usize, max: usize) -> Result<(), ValidationError> {
    if len > max { Err(ValidationError::new(field, "is too long")) } else { Ok(()) }
}

fn check_finite(field: &'static str, v: f64) -> Result<(), ValidationError> {
    if v.is_finite() { Ok(()) } else { Err(ValidationError::new(field, "must be a finite number")) }
}

/// A live update payload: not empty, at most [`MAX_UPDATE_BYTES`].
fn check_update(update: &[u8]) -> Result<(), ValidationError> {
    if update.is_empty() {
        return Err(ValidationError::new("update", "must not be empty"));
    }
    check_len("update", update.len(), MAX_UPDATE_BYTES)
}

impl UserInfo {
    /// Checks the size and colour range.
    pub fn validate(&self) -> Result<(), ValidationError> {
        check_len("user.name", self.name.len(), MAX_USER_NAME_BYTES)?;
        if self.color > 0x00ff_ffff {
            return Err(ValidationError::new("user.color", "must fit in 0xRRGGBB"));
        }
        Ok(())
    }
}

impl AwarenessState {
    /// Checks sizes and that numbers are finite.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if let Some(user) = &self.user {
            user.validate()?;
        }
        if let Some(p) = &self.cursor {
            check_finite("cursor.x", p.x)?;
            check_finite("cursor.y", p.y)?;
        }
        if let Some(v) = &self.viewport {
            check_finite("viewport.x", v.x)?;
            check_finite("viewport.y", v.y)?;
            check_finite("viewport.width", v.width)?;
            check_finite("viewport.height", v.height)?;
            if v.width < 0.0 || v.height < 0.0 {
                return Err(ValidationError::new("viewport", "size must not be negative"));
            }
        }
        check_len("selection", self.selection.len(), MAX_SELECTION_IDS)?;
        for id in &self.selection {
            if id.is_empty() {
                return Err(ValidationError::new("selection", "ids must not be empty"));
            }
            check_len("selection id", id.len(), MAX_ELEMENT_ID_BYTES)?;
        }
        Ok(())
    }
}

impl PeerInfo {
    fn validate(&self) -> Result<(), ValidationError> {
        match &self.user {
            Some(u) => u.validate(),
            None => Ok(()),
        }
    }
}

impl Limits {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.max_frame_bytes as usize > MAX_FRAME_BYTES {
            return Err(ValidationError::new("limits.max_frame_bytes", "above the hard limit"));
        }
        if self.max_update_bytes as usize > MAX_UPDATE_BYTES {
            return Err(ValidationError::new("limits.max_update_bytes", "above the hard limit"));
        }
        Ok(())
    }
}

impl ClientMessage {
    /// Checks every size limit and value rule. Encoders and decoders call this.
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            ClientMessage::Join { min_version, max_version, auth_token, version, user, .. } => {
                if min_version > max_version {
                    return Err(ValidationError::new("version range", "min is above max"));
                }
                if let Some(t) = auth_token {
                    check_len("auth_token", t.len(), MAX_AUTH_TOKEN_BYTES)?;
                }
                check_len("version", version.len(), MAX_VERSION_BYTES)?;
                if let Some(u) = user {
                    u.validate()?;
                }
                Ok(())
            }
            ClientMessage::Update { update, .. } => check_update(update),
            ClientMessage::RequestSync { version } => check_len("version", version.len(), MAX_VERSION_BYTES),
            ClientMessage::Awareness(state) => state.validate(),
            ClientMessage::Ping { .. } | ClientMessage::Pong { .. } | ClientMessage::Leave => Ok(()),
        }
    }
}

impl ServerMessage {
    /// Checks every size limit and value rule. Encoders and decoders call this.
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            ServerMessage::Joined { server, peers, server_version, limits, .. } => {
                check_len("server", server.len(), MAX_SERVER_NAME_BYTES)?;
                check_len("peers", peers.len(), MAX_PEERS)?;
                for p in peers {
                    p.validate()?;
                }
                check_len("server_version", server_version.len(), MAX_VERSION_BYTES)?;
                limits.validate()
            }
            ServerMessage::SyncResponse { data, server_version, .. } => {
                check_len("data", data.len(), MAX_SYNC_BYTES)?;
                check_len("server_version", server_version.len(), MAX_VERSION_BYTES)
            }
            ServerMessage::Update { update, .. } => check_update(update),
            ServerMessage::PeerJoined { peer } => peer.validate(),
            ServerMessage::PeerAwareness { state, .. } => state.validate(),
            ServerMessage::Error { message, .. } => check_len("message", message.len(), MAX_ERROR_MESSAGE_BYTES),
            ServerMessage::Ack { .. }
            | ServerMessage::PeerLeft { .. }
            | ServerMessage::Ping { .. }
            | ServerMessage::Pong { .. } => Ok(()),
        }
    }
}
