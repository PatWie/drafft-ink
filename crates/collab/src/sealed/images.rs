//! Pictures of an encrypted room as files (`docs/design/collab-e2e.md` section 9). The document holds only
//! `{mime, size}` of a picture; its bytes are sealed with the room's image key and live on the relay under a keyed
//! name. This module is the sans-IO side: it says which HTTP requests to make ([`BlobRequest`](crate::BlobRequest))
//! and what their answers mean, and the owner moves them through a [`BlobTransport`](crate::BlobTransport).
//!
//! * **Offers.** `offer_image(id, bytes)`: "I hold these bytes". After the list of the connection, an offer the relay
//!   lacks is uploaded (`PUT`). The same picture offered twice is one name. Bytes are let go once the relay has them,
//!   so after a relay forgot a room (expiry, reseed) the owner offers its pictures again.
//! * **Wants.** `want_image(id)`: "show me this picture". It is downloaded (`GET`), opened and checked
//!   (`SHA-256(bytes) == id`), and arrives as [`ImageEvent::Arrived`]. The manifest may reach a peer before the
//!   upload does, so a `404` is tried again with a backoff of 1, 2, 4 ... seconds, at most 2 minutes.
//! * **One list per connection.** When the client is online it asks for the room's list, so that an upload that
//!   arrived while this client was away (or whose answer was lost) is not sent again.
//! * **A reconnect** forgets requests in flight and keeps offers and wants.
//! * **Limits.** At most [`MAX_DOWNLOADS`] downloads and [`MAX_UPLOADS`] uploads at once. An offer above the relay's
//!   `max_image_bytes` (from `Joined`) is refused without a request.
//!
//! The retry rules are in `replies.rs`, the timers in `backoff.rs`.

mod backoff;
mod replies;
mod state;
mod wire;

use std::sync::Arc;

use drafftink_scene::FileId;

pub use backoff::{IMAGE_RETRY_BASE_MS, IMAGE_RETRY_MAX_MS};
use state::Env;
pub(super) use state::Images;
pub use state::{BLOB_TIMEOUT_MS, MAX_DOWNLOADS, MAX_UPLOADS};

use super::{Phase, SealedClient};
use crate::blobs::{BlobRequest, BlobResponse};
use crate::client::CollabEvent;

/// Why an upload was refused for good.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageRefusal {
    /// The picture is larger than the room accepts (`413`, or the client's own check against `max_image_bytes`).
    TooLarge,
    /// The room already holds as many pictures or bytes as it may (`507`).
    RoomFull,
}

/// What happened to a picture the owner offered or wanted.
#[derive(Clone, PartialEq, Eq)]
pub enum ImageEvent {
    /// A wanted picture arrived: opened, and its SHA-256 is the id.
    Arrived {
        /// The picture.
        id: FileId,
        /// Its bytes.
        bytes: Arc<[u8]>,
    },
    /// A wanted picture cannot be used: it did not open, or it is not the picture its id names. The bytes are
    /// discarded and it is not asked for again.
    Damaged {
        /// The picture.
        id: FileId,
    },
    /// An offered picture was refused for good.
    Rejected {
        /// The picture.
        id: FileId,
        /// Why.
        why: ImageRefusal,
    },
}

impl std::fmt::Debug for ImageEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImageEvent::Arrived { id, bytes } => write!(f, "Arrived({id}, {} bytes)", bytes.len()),
            ImageEvent::Damaged { id } => write!(f, "Damaged({id})"),
            ImageEvent::Rejected { id, why } => write!(f, "Rejected({id}, {why:?})"),
        }
    }
}

/// Where the transfers stand, for a status line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImageProgress {
    /// Uploads in flight (a check of a lost upload included).
    pub uploading: u32,
    /// Downloads in flight.
    pub downloading: u32,
    /// Pictures waiting for a free place, a retry time, or the list of the connection.
    pub waiting: u32,
    /// Pictures that will not be transferred: refused uploads, damaged or forbidden downloads.
    pub failed: u32,
    /// Pictures the relay holds, as last reported by it.
    pub room_images: u32,
    /// Sealed bytes the relay holds for the room's pictures, as last reported by it.
    pub room_image_bytes: u64,
}

impl SealedClient {
    /// "I hold these bytes": uploads them unless the relay has the name. `id` must be the SHA-256 of `bytes` (the
    /// document layer guarantees it); peers check it and call a picture that does not match damaged. Offering a
    /// picture again does nothing, except that a refused one is tried again.
    pub fn offer_image(&mut self, now_ms: u64, id: &FileId, bytes: Arc<[u8]>) {
        let env = Env { keys: &self.keys, limits: &self.limits };
        self.images.offer(env, id, bytes);
        self.images_changed(now_ms);
    }

    /// "Show me this picture": downloads it, and tries again until it arrives (`ImageEvent::Arrived`) or turns out
    /// to be damaged. Asking again for a picture that is already on its way does nothing.
    pub fn want_image(&mut self, now_ms: u64, id: &FileId) {
        let env = Env { keys: &self.keys, limits: &self.limits };
        self.images.want(now_ms, env, id);
        self.images_changed(now_ms);
    }

    /// The next HTTP request for a picture, for a [`BlobTransport`](crate::BlobTransport).
    pub fn poll_blob_request(&mut self) -> Option<BlobRequest> {
        self.images.poll_request()
    }

    /// The answer to a request from [`poll_blob_request`](Self::poll_blob_request). A failure of the network is an
    /// `Err` in `response.result`.
    pub fn on_blob_response(&mut self, now_ms: u64, response: BlobResponse) {
        let env = Env { keys: &self.keys, limits: &self.limits };
        self.images.on_response(now_ms, env, response);
        self.images_changed(now_ms);
    }

    /// Where the transfers stand.
    pub fn images(&self) -> ImageProgress {
        self.images.progress()
    }

    /// Sends what is due and publishes what happened. Requests only go out while online.
    pub(super) fn images_changed(&mut self, now_ms: u64) {
        if matches!(self.phase, Phase::Online) {
            let env = Env { keys: &self.keys, limits: &self.limits };
            self.images.drive(now_ms, env);
        }
        self.events.extend(self.images.take_events().into_iter().map(CollabEvent::Image));
        self.refresh_status();
    }
}
