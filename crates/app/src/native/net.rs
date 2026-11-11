//! The network port on the desktop: a WebSocket client thread per connection.

use std::sync::Arc;

use drafftink_collab::{NativeTransport, Transport};
use drafftink_engine::PortError;
use drafftink_engine::ports::Net;

/// Opens `NativeTransport`s that wake the event loop when a frame arrives.
pub(crate) struct NativeNet {
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl NativeNet {
    pub(crate) fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self { wake }
    }
}

impl Net for NativeNet {
    fn open_transport(&mut self) -> Result<Box<dyn Transport>, PortError> {
        Ok(Box::new(NativeTransport::new(Arc::clone(&self.wake))))
    }

    fn random(&mut self) -> u128 {
        super::rng::random_u128()
    }
}
