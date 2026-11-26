//! [`WebNet`]: the engine's `Net` port in a browser. It opens `WebSocket`s and knows the address of the
//! server that delivered the page.

use drafftink_collab::{BlobTransport, Transport, WakeFn, WebBlobTransport, WebTransport};
use drafftink_engine::PortError;
use drafftink_engine::ports::Net;

use super::pure::{CollabMode, is_loopback_host, same_origin_ws_url};
use super::rng::random_u64;

/// The browser network.
pub(crate) struct WebNet {
    wake: WakeFn,
    protocol: String,
    host: String,
    loopback: bool,
    rooms: bool,
}

impl WebNet {
    /// `wake` is called whenever a socket event arrives. `location` is where the page came from. `mode` says whether
    /// this deployment has a relay.
    pub(crate) fn new(wake: WakeFn, location: &web_sys::Location, mode: CollabMode) -> Self {
        Self {
            wake,
            protocol: location.protocol().unwrap_or_default(),
            host: location.host().unwrap_or_default(),
            loopback: is_loopback_host(&location.hostname().unwrap_or_default()),
            rooms: mode.rooms(),
        }
    }
}

impl Net for WebNet {
    fn open_transport(&mut self) -> Result<Box<dyn Transport>, PortError> {
        Ok(Box::new(WebTransport::new(std::sync::Arc::clone(&self.wake))))
    }

    fn open_blob_transport(&mut self) -> Result<Box<dyn BlobTransport>, PortError> {
        Ok(Box::new(WebBlobTransport::new(std::sync::Arc::clone(&self.wake))))
    }

    fn random(&mut self) -> u128 {
        (u128::from(random_u64()) << 64) | u128::from(random_u64())
    }

    fn same_origin_url(&self, path: &str) -> Option<String> {
        same_origin_ws_url(&self.protocol, &self.host, path)
    }

    fn default_server(&self) -> Option<String> {
        // The page's own server, wherever the page is (a `file:` page has none).
        self.same_origin_url("/ws").map(|_| "/ws".to_owned())
    }

    fn allow_insecure(&self) -> bool {
        // A page served from this machine may use plain ws:// to its own server. Everything else needs wss://.
        self.loopback
    }

    fn rooms_available(&self) -> bool {
        self.rooms
    }
}
