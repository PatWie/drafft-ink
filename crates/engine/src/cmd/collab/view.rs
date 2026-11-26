//! What the UI and the overlay read: the connection, the room, the people in it and their pointers.
//!
//! Plain data built from the client by [`crate::Session::collab_view`]. Nothing here can change the session.

use drafftink_canvas::{RemoteUser, TrailPoint};
use drafftink_collab::{ConnState, Peer};
use drafftink_geom::kurbo::Point;
use drafftink_protocol::UserInfo;
use drafftink_scene::{Color, ElementId};

use super::access::AccessView;
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
    /// The room's content is in, and the scene is being filled in slices, a few milliseconds at a time. The app keeps
    /// answering; elements appear as they are decoded.
    Importing {
        /// Items (elements, files) shown so far.
        done: usize,
        /// All items of the room.
        total: usize,
    },
    /// The room is usable, and pictures in the visible area are still on their way. They show as empty pictures
    /// until they arrive.
    Images {
        /// Pictures of the visible area that are here.
        done: u32,
        /// Pictures the visible area shows.
        total: u32,
    },
}

/// Where the pictures of an encrypted room stand. All zero for a plaintext room, which embeds them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ImagesView {
    /// Pictures being sent to the room.
    pub uploading: u32,
    /// Pictures being fetched from the room.
    pub downloading: u32,
    /// Pictures waiting for a free slot, a retry time, or the room's list (a picture whose upload has not finished
    /// yet is retried every few seconds).
    pub waiting: u32,
    /// Pictures that will not be transferred: refused uploads, damaged downloads.
    pub failed: u32,
    /// Pictures the room holds, as the server last reported.
    pub room_images: u32,
    /// Bytes the room's pictures take on the server (sealed), as the server last reported.
    pub room_image_bytes: u64,
}

impl ImagesView {
    pub(super) fn of(progress: &drafftink_collab::ImageProgress) -> Self {
        Self {
            uploading: progress.uploading,
            downloading: progress.downloading,
            waiting: progress.waiting,
            failed: progress.failed,
            room_images: progress.room_images,
            room_image_bytes: progress.room_image_bytes,
        }
    }
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

/// The colour of a peer whose `color` is not a hex colour (it always is; this keeps the conversion total).
const FALLBACK_COLOR: Color = Color::rgb(0x63, 0x66, 0xf1);

impl PeerView {
    /// The peer as the canvas overlay describes a collaborator, for [`drafftink_canvas::remote_parts`]: the pointer
    /// with the name, the selection (ids that are not element ids are dropped) and the trail. See `docs/render.md`.
    pub fn to_remote_user(&self) -> RemoteUser {
        RemoteUser {
            id: self.id.clone(),
            name: Some(self.name.clone()),
            color: Color::parse(&self.color).unwrap_or(FALLBACK_COLOR),
            cursor: self.pointer.map(|[x, y]| Point::new(x, y)),
            selection: self.selection.iter().filter_map(|s| ElementId::new(s).ok()).collect(),
            trail: self
                .trail
                .iter()
                .map(|d| TrailPoint { pos: Point::new(d.x, d.y), age_ms: d.age_ms as f64 })
                .collect(),
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
    /// Where the pictures of an encrypted room stand.
    pub images: ImagesView,
    /// True for an end-to-end encrypted room: the server cannot read it. `room` is its id, never its key.
    pub e2e: bool,
    /// When the server deletes the room if nobody visits, in milliseconds since the epoch. Absent: never, or not
    /// known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<u64>,
    /// The room is past its time limit and is deleted when the last person leaves.
    pub expiring: bool,
    /// Whether a service token is stored for the server (a private instance): `service_token`, `none`, or `browser`
    /// in the web app, where the browser carries the sign-in. Never a value.
    pub access: AccessView,
    /// The room this tab was in was deleted on request (`collab.delete_room`). The tab is a local copy now. Cleared by
    /// the next `collab.connect` or `collab.disconnect`.
    pub room_deleted: bool,
    /// The active tab is the copy of a private room that is not connected, and `collab.resume_room` reconnects it. The
    /// server it is on, as the person reads it (`https://canvas.example.com`). A page does not reconnect a room on
    /// another server by itself: the person confirms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconnect: Option<String>,
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
            images: ImagesView::default(),
            e2e: false,
            expires_at_ms: None,
            expiring: false,
            access: AccessView::None,
            room_deleted: false,
            reconnect: None,
        }
    }
}
