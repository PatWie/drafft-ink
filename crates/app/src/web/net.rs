//! [`WebNet`]: the engine's `Net` port in a browser. It opens `WebSocket`s and knows the address of the
//! server that delivered the page.

use drafftink_collab::{Transport, WakeFn, WebTransport};
use drafftink_engine::PortError;
use drafftink_engine::ports::Net;

use super::pure::{is_loopback_host, same_origin_ws_url};
use super::rng::random_u64;

/// The browser network.
pub(crate) struct WebNet {
    wake: WakeFn,
    protocol: String,
    host: String,
    loopback: bool,
}

impl WebNet {
    /// `wake` is called whenever a socket event arrives. `location` is where the page came from.
    pub(crate) fn new(wake: WakeFn, location: &web_sys::Location) -> Self {
        Self {
            wake,
            protocol: location.protocol().unwrap_or_default(),
            host: location.host().unwrap_or_default(),
            loopback: is_loopback_host(&location.hostname().unwrap_or_default()),
        }
    }
}

impl Net for WebNet {
    fn open_transport(&mut self) -> Result<Box<dyn Transport>, PortError> {
        Ok(Box::new(WebTransport::new(std::sync::Arc::clone(&self.wake))))
    }

    fn random(&mut self) -> u128 {
        (u128::from(random_u64()) << 64) | u128::from(random_u64())
    }

    fn same_origin_url(&self, path: &str) -> Option<String> {
        same_origin_ws_url(&self.protocol, &self.host, path)
    }

    fn allow_insecure(&self) -> bool {
        // A page served from this machine may use plain ws:// to its own server. Everything else needs wss://.
        self.loopback
    }
}
