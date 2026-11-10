//! The glue between a [`CollabClient`] and a [`Transport`]: one call that moves bytes both ways and says when to come
//! back. This is what the engine owns.

use drafftink_doc::LoroStore;

use crate::client::{CollabClient, CollabEvent, Output};
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
    client: CollabClient,
    transport: T,
}

/// The most rounds one pump runs. Each round delivers transport events, ticks and executes outputs; it stops earlier
/// when a round does nothing.
const MAX_ROUNDS: usize = 16;

impl<T: Transport> CollabSession<T> {
    /// Joins a client and a transport.
    pub fn new(client: CollabClient, transport: T) -> Self {
        CollabSession { client, transport }
    }

    /// The client, for reading state.
    pub fn client(&self) -> &CollabClient {
        &self.client
    }

    /// The client, for the local inputs (`local_changed`, presence, follow, `leave`). Call
    /// [`pump`](Self::pump) afterwards.
    pub fn client_mut(&mut self) -> &mut CollabClient {
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
            self.client.tick(now_ms, store);
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

    fn execute(&mut self, now_ms: u64, output: Output, store: &mut LoroStore) {
        let result = match output {
            Output::Connect(url) => self.transport.open(&url),
            Output::Send(frame) => self.transport.send(frame),
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
