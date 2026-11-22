//! The glue between a [`CollabClient`] and a [`Transport`]: one call that moves bytes both ways and says when to come
//! back. This is what the engine owns.

use drafftink_doc::LoroStore;

use crate::blobs::{BlobResponse, BlobTransport};
use crate::client::{CollabEvent, Output};
use crate::room_client::RoomClient;
use crate::transport::{Transport, TransportError, TransportEvent};

/// What one [`CollabSession::pump`] produced.
#[derive(Debug, Default)]
pub struct Pump {
    /// What happened, in order. Remote changes are already applied to the document.
    pub events: Vec<CollabEvent>,
    /// When to call `pump` again if nothing else wakes the owner earlier: the earliest timer of the client. `None`
    /// when nothing is scheduled.
    pub next_wake_ms: Option<u64>,
    /// True when something the user can see changed (any event). The owner requests one redraw with the `collab`
    /// reason; an idle pump with no events must not cause a frame.
    pub redraw: bool,
}

/// A client with its transport.
pub struct CollabSession<T: Transport> {
    client: RoomClient,
    transport: T,
    blobs: Option<Box<dyn BlobTransport>>,
}

/// The most rounds one pump runs. Each round delivers transport events, ticks and executes outputs; it stops earlier
/// when a round does nothing.
const MAX_ROUNDS: usize = 16;

impl<T: Transport> CollabSession<T> {
    /// Joins a client (plaintext or sealed) and a transport.
    pub fn new(client: impl Into<RoomClient>, transport: T) -> Self {
        CollabSession { client: client.into(), transport, blobs: None }
    }

    /// Gives the session a pipe for the pictures of an encrypted room. Without one, a picture request fails at once
    /// ("pictures are not available here") and is tried again later by the client's backoff.
    pub fn set_blobs(&mut self, blobs: Box<dyn BlobTransport>) {
        self.blobs = Some(blobs);
    }

    /// The client, for reading state.
    pub fn client(&self) -> &RoomClient {
        &self.client
    }

    /// The client, for the local inputs (`local_changed`, presence, follow, `leave`). Call
    /// [`pump`](Self::pump) afterwards.
    pub fn client_mut(&mut self) -> &mut RoomClient {
        &mut self.client
    }

    /// The transport.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Delivers transport events, advances time, executes the client's outputs, and returns what happened and when
    /// to come back. Call it when the transport wakes the owner, when the time in `next_wake_ms` arrives, and after
    /// local inputs. It never blocks.
    pub fn pump(&mut self, now_ms: u64, store: &mut LoroStore) -> Pump {
        let _span = drafftink_profile::span("collab_pump");
        let mut events = Vec::new();
        for _ in 0..MAX_ROUNDS {
            let mut busy = false;
            while let Some(event) = self.transport.poll() {
                busy = true;
                self.client.on_event(now_ms, event, store);
            }
            while let Some(response) = self.blobs.as_mut().and_then(|b| b.poll()) {
                busy = true;
                self.client.on_blob_response(now_ms, response);
            }
            self.client.tick(now_ms, store);
            while let Some(request) = self.client.poll_blob_request() {
                busy = true;
                self.send_blob_request(now_ms, request);
            }
            while let Some(output) = self.client.poll_output() {
                busy = true;
                self.execute(now_ms, output, store);
            }
            events.extend(self.client.take_events());
            if !busy {
                break;
            }
        }
        let redraw = !events.is_empty();
        Pump { events, next_wake_ms: self.client.next_wake_ms(), redraw }
    }

    /// Hands a picture request to the pipe. A request that cannot be sent is a failure of the network.
    fn send_blob_request(&mut self, now_ms: u64, request: crate::blobs::BlobRequest) {
        let id = request.id;
        let failed = match self.blobs.as_mut() {
            Some(blobs) => blobs.send(request).err().map(|e| e.to_string()),
            None => Some("pictures are not available here".to_owned()),
        };
        if let Some(why) = failed {
            log::debug!("collab: a picture request was not sent: {why}");
            self.client.on_blob_response(now_ms, BlobResponse { id, result: Err(why) });
        }
    }

    fn execute(&mut self, now_ms: u64, output: Output, store: &mut LoroStore) {
        let result = match output {
            Output::Connect(url) => self.transport.open(&url),
            Output::Send(frame) => self.transport.send(frame),
            Output::SendText(text) => self.transport.send_text(text),
            Output::Close => {
                self.transport.close();
                Ok(())
            }
        };
        match result {
            Ok(()) => {}
            // The connection is already gone; its `Closed` event reaches the client by itself.
            Err(TransportError::NotOpen) => log::debug!("collab: a frame was dropped because the connection closed"),
            Err(TransportError::Failed(e)) => {
                self.client.on_event(now_ms, TransportEvent::Closed { error: Some(e) }, store);
            }
        }
    }
}
