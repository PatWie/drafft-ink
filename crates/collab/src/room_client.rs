//! One client type for the engine: a plaintext room ([`CollabClient`]) or an end-to-end encrypted one
//! ([`SealedClient`]). Every method forwards, so the session and the engine do not care which one runs.

use std::sync::Arc;

use drafftink_doc::LoroStore;
use drafftink_protocol::{ClientId, Point, UserInfo, Viewport};
use drafftink_scene::FileId;

use crate::awareness::{LocalPresence, PeerTable};
use crate::blobs::{BlobRequest, BlobResponse};
use crate::client::{AuthToken, CollabClient, CollabEvent, Output, Stats, TrimmedChoice};
use crate::sealed::{ImageProgress, SealedClient};
use crate::status::Status;
use crate::transport::TransportEvent;
use crate::url::ServerUrl;

/// A client of either kind of room.
#[derive(Debug)]
pub enum RoomClient {
    /// A plaintext room: the server reads and merges updates (protocol v1).
    Plain(Box<CollabClient>),
    /// An end-to-end encrypted room: the relay reads nothing (protocol v2).
    Sealed(Box<SealedClient>),
}

impl From<CollabClient> for RoomClient {
    fn from(client: CollabClient) -> Self {
        RoomClient::Plain(Box::new(client))
    }
}

impl From<SealedClient> for RoomClient {
    fn from(client: SealedClient) -> Self {
        RoomClient::Sealed(Box::new(client))
    }
}

macro_rules! forward {
    ($self:ident.$method:ident($($arg:expr),*)) => {
        match $self {
            RoomClient::Plain(c) => c.$method($($arg),*),
            RoomClient::Sealed(c) => c.$method($($arg),*),
        }
    };
}

impl RoomClient {
    /// The plaintext client, when this is one.
    pub fn plain(&self) -> Option<&CollabClient> {
        match self {
            RoomClient::Plain(c) => Some(c),
            RoomClient::Sealed(_) => None,
        }
    }

    /// The sealed client, when this is one.
    pub fn sealed(&self) -> Option<&SealedClient> {
        match self {
            RoomClient::Sealed(c) => Some(c),
            RoomClient::Plain(_) => None,
        }
    }

    /// The server address the person gave.
    pub fn url(&self) -> &ServerUrl {
        match self {
            RoomClient::Plain(c) => &c.config().url,
            RoomClient::Sealed(c) => c.url(),
        }
    }

    /// The access token of the server, for a plaintext room. A sealed room has none: the key is the proof.
    pub fn auth_token(&self) -> Option<&AuthToken> {
        self.plain().and_then(|c| c.config().auth_token.as_ref())
    }

    /// The current status.
    pub fn status(&self) -> &Status {
        forward!(self.status())
    }

    /// The other people in the room.
    pub fn peers(&self) -> &PeerTable {
        forward!(self.peers())
    }

    /// This client's own presence.
    pub fn presence(&self) -> &LocalPresence {
        forward!(self.presence())
    }

    /// The peer being followed, if any.
    pub fn following(&self) -> Option<ClientId> {
        forward!(self.following())
    }

    /// True while connected and joined.
    pub fn is_connected(&self) -> bool {
        forward!(self.is_connected())
    }

    /// Counters of what happened so far.
    pub fn stats(&self) -> Stats {
        forward!(self.stats())
    }

    /// Starts connecting.
    pub fn start(&mut self, now_ms: u64) {
        forward!(self.start(now_ms));
    }

    /// Tries again at once instead of waiting out the backoff.
    pub fn retry_now(&mut self, now_ms: u64) {
        forward!(self.retry_now(now_ms));
    }

    /// Leaves the room.
    pub fn leave(&mut self, now_ms: u64, store: &mut LoroStore) {
        forward!(self.leave(now_ms, store));
    }

    /// The document got a local commit.
    pub fn local_changed(&mut self, now_ms: u64) {
        forward!(self.local_changed(now_ms));
    }

    /// Sets the pointer position in world units.
    pub fn set_cursor(&mut self, now_ms: u64, cursor: Option<Point>) {
        forward!(self.set_cursor(now_ms, cursor));
    }

    /// Sets the selected element ids.
    pub fn set_selection(&mut self, ids: impl IntoIterator<Item = String>) {
        forward!(self.set_selection(ids));
    }

    /// Sets the visible world rectangle.
    pub fn set_viewport(&mut self, viewport: Option<Viewport>) {
        forward!(self.set_viewport(viewport));
    }

    /// Sets name and colour.
    pub fn set_user(&mut self, user: Option<UserInfo>) {
        forward!(self.set_user(user));
    }

    /// Turns the local laser pointer on or off.
    pub fn set_laser(&mut self, on: bool) {
        forward!(self.set_laser(on));
    }

    /// Follows a peer.
    pub fn follow(&mut self, id: ClientId) -> bool {
        forward!(self.follow(id))
    }

    /// Stops following.
    pub fn stop_following(&mut self) {
        forward!(self.stop_following());
    }

    /// Answers [`CollabEvent::NeedsDecision`].
    pub fn resolve_trimmed(&mut self, now_ms: u64, store: &mut LoroStore, choice: TrimmedChoice) {
        forward!(self.resolve_trimmed(now_ms, store, choice));
    }

    /// Advances time.
    pub fn tick(&mut self, now_ms: u64, store: &mut LoroStore) {
        forward!(self.tick(now_ms, store));
    }

    /// Handles what the transport reported.
    pub fn on_event(&mut self, now_ms: u64, event: TransportEvent, store: &mut LoroStore) {
        forward!(self.on_event(now_ms, event, store));
    }

    /// The next frame to send, connect or close request.
    pub fn poll_output(&mut self) -> Option<Output> {
        forward!(self.poll_output())
    }

    /// Everything that happened since the last call.
    pub fn take_events(&mut self) -> Vec<CollabEvent> {
        forward!(self.take_events())
    }

    /// When the owner must call `tick` next.
    pub fn next_wake_ms(&self) -> Option<u64> {
        forward!(self.next_wake_ms())
    }

    // ---- pictures (encrypted rooms only; a plaintext room embeds them in the document) ---------------------------

    /// "I hold these bytes": uploads them unless the relay has the name. A plaintext client ignores it.
    pub fn offer_image(&mut self, now_ms: u64, id: &FileId, bytes: Arc<[u8]>) {
        if let RoomClient::Sealed(c) = self {
            c.offer_image(now_ms, id, bytes);
        }
    }

    /// "Show me this picture": downloads it. A plaintext client ignores it.
    pub fn want_image(&mut self, now_ms: u64, id: &FileId) {
        if let RoomClient::Sealed(c) = self {
            c.want_image(now_ms, id);
        }
    }

    /// The next HTTP request for a picture. A plaintext client has none.
    pub fn poll_blob_request(&mut self) -> Option<BlobRequest> {
        match self {
            RoomClient::Sealed(c) => c.poll_blob_request(),
            RoomClient::Plain(_) => None,
        }
    }

    /// The answer to a picture request. A plaintext client ignores it.
    pub fn on_blob_response(&mut self, now_ms: u64, response: BlobResponse) {
        if let RoomClient::Sealed(c) = self {
            c.on_blob_response(now_ms, response);
        }
    }

    /// Where the picture transfers stand. All zero for a plaintext client.
    pub fn images(&self) -> ImageProgress {
        match self {
            RoomClient::Sealed(c) => c.images(),
            RoomClient::Plain(_) => ImageProgress::default(),
        }
    }
}
