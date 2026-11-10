//! What the UI and the overlay read: the connection, the room, the people in it and their pointers.
//!
//! Plain data built from the client by [`crate::Session::collab_view`]. Nothing here can change the session.

use drafftink_collab::{ConnState, Peer};
use drafftink_protocol::UserInfo;
use schemars::JsonSchema;
use serde::Serialize;

/// Where the connection to the server is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    /// No server chosen.
    Disconnected,
    /// A server is chosen (`collab.connect` without a room); no socket is open and no room joined.
    Ready,
    /// Opening the connection, joining, catching up, or waiting to retry.
    Connecting,
    /// In a room and in sync.
    Connected,
    /// Sharing stopped and will not resume by itself. `error` says why.
    Error,
}

impl ConnectionState {
    /// How a client state shows to the user.
    pub(super) fn of(state: ConnState) -> Self {
        match state {
            ConnState::Idle | ConnState::Connecting | ConnState::Syncing | ConnState::Reconnecting => Self::Connecting,
            ConnState::Online => Self::Connected,
            ConnState::Stopped => Self::Error,
        }
    }
}

/// Why the room is not usable yet. Present from the join until the first catch-up is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Loading {
    /// Opening the connection.
    Connecting,
    /// Joined; waiting for the room's content.
    Syncing,
    /// A large update (a big room's snapshot) is about to be applied. Applying it takes a moment, during which
    /// the app does not respond; draw a progress hint before the next [`crate::Session::collab_poll`].
    Receiving {
        /// Size of the update in bytes.
        bytes: u64,
    },
}

/// A point of a fading trail.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, JsonSchema)]
pub struct TrailDot {
    /// World x.
    pub x: f64,
    /// World y.
    pub y: f64,
    /// Milliseconds since the point was recorded.
    pub age_ms: u64,
}

/// A point of this user's own laser trail.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, JsonSchema)]
pub struct LaserDot {
    /// World x.
    pub x: f64,
    /// World y.
    pub y: f64,
    /// Milliseconds since the point was recorded.
    pub age_ms: u64,
    /// The point is gone when `age_ms` reaches this.
    pub fade_ms: u64,
}

/// One other person in the room.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct PeerView {
    /// The peer's id (32 hex characters). `collab.follow` takes it.
    pub id: String,
    /// Display name, or a short form of the id.
    pub name: String,
    /// `#rrggbb`.
    pub color: String,
    /// Pointer in world coordinates; absent when it is outside the canvas.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pointer: Option<[f64; 2]>,
    /// Ids of the elements the peer has selected.
    pub selection: Vec<String>,
    /// The peer's visible world rectangle `[x, y, w, h]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub viewport: Option<[f64; 4]>,
    /// The recent path of the pointer, oldest first. Drawn as a fading trail (the laser).
    pub trail: Vec<TrailDot>,
    /// True while this user follows the peer.
    pub followed: bool,
}

/// The colour of a person who chose none, picked from the nine presets by id so it stays the same.
const PRESETS: [u32; 9] = [0xef4444, 0xf97316, 0xeab308, 0x22c55e, 0x06b6d4, 0x3b82f6, 0x6366f1, 0xa855f7, 0xec4899];

/// `#rrggbb` of a colour in `0xRRGGBB`.
pub(super) fn hex(color: u32) -> String {
    format!("#{:06x}", color & 0x00ff_ffff)
}

impl PeerView {
    pub(super) fn of(peer: &Peer, now_ms: u64, followed: bool) -> Self {
        let id = peer.id.to_string();
        let preset = id.bytes().fold(0usize, |acc, b| acc.wrapping_add(usize::from(b)));
        let color = match &peer.user {
            Some(UserInfo { color, .. }) => *color,
            None => PRESETS.get(preset % PRESETS.len()).copied().unwrap_or(0x6366f1),
        };
        Self {
            name: peer.display_name(),
            color: hex(color),
            pointer: peer.cursor.map(|p| [p.x, p.y]),
            selection: peer.selection.clone(),
            viewport: peer.viewport.map(|v| [v.x, v.y, v.width, v.height]),
            trail: peer.trail().points(now_ms).into_iter().map(|(age_ms, p)| TrailDot { x: p.x, y: p.y, age_ms }).collect(),
            followed,
            id,
        }
    }
}

/// Everything the UI shows about collaboration.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct CollabView {
    /// The connection.
    pub state: ConnectionState,
    /// The server address, once one is chosen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The room this session is in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    /// The tab bound to the room, as `tab.list` shows it (`t1`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    /// One sentence for a status bar.
    pub text: String,
    /// Why sharing stopped, in `Error` state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// True while local changes are not yet confirmed by the server.
    pub unsynced: bool,
    /// Present while the room is not yet usable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loading: Option<Loading>,
    /// The other people in the room.
    pub peers: Vec<PeerView>,
    /// This user's laser trail.
    pub laser: Vec<LaserDot>,
}

impl CollabView {
    /// The view of a session that shares nothing.
    pub(super) fn disconnected() -> Self {
        Self {
            state: ConnectionState::Disconnected,
            url: None,
            room: None,
            tab: None,
            text: "Not shared. Working on this device only.".to_owned(),
            error: None,
            unsynced: false,
            loading: None,
            peers: Vec::new(),
            laser: Vec::new(),
        }
    }
}
