//! The byte pipe under the client. A [`Transport`] moves whole protocol frames and reports what happened; it knows
//! nothing about messages. Implementations: a native WebSocket (`native`), a browser WebSocket (`web`) and an
//! in-memory pair for tests ([`loopback`]).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use drafftink_protocol::{ClientMessage, ServerMessage};

use crate::access::{AccessDenial, AccessGrant, AccessOrigin};
use crate::url::ServerUrl;

/// Called by a transport, possibly from another thread, when it has queued an event for [`Transport::poll`]. The
/// owner uses it to wake its loop (for example `egui::Context::request_repaint`). It must be cheap and must not call
/// back into the transport.
pub type WakeFn = Arc<dyn Fn() + Send + Sync>;

/// What a transport reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportEvent {
    /// The connection is open; frames can be sent.
    Opened,
    /// One binary frame arrived.
    Message(Vec<u8>),
    /// One text frame arrived. Only the keepalive answer of a sealed room (`pong`) is one.
    Text(String),
    /// The server turned the connection away at its door (an access token missing or refused, a sign-in that
    /// ended). It ends the connection like `Closed`, with a reason the client can act on: retrying with the same
    /// token would only be refused again. After this event the transport emits nothing for that connection.
    Denied(AccessDenial),
    /// The connection is gone, or never opened. `error` says why when it was not a clean close. After this event
    /// the transport emits nothing for that connection.
    Closed {
        /// A short reason, for the log and the status text.
        error: Option<String>,
    },
}

/// A transport call that could not be carried out.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    /// `send` without an open connection.
    #[error("the connection is not open")]
    NotOpen,
    /// The transport could not start the connection.
    #[error("{0}")]
    Failed(String),
}

/// A bidirectional frame pipe to the server.
pub trait Transport {
    /// Gives the transport the service token of a private server (`None`: no token). The transport sends it in the
    /// handshake of a connection, under the rules of [`AccessGrant::headers_for`] and nowhere else, and never follows
    /// a redirect while it does. A transport that has no way to send headers (the browser's `WebSocket`, which
    /// carries the sign-in cookie instead) ignores it, which is why this has a default.
    fn set_access(&mut self, _access: Option<AccessGrant>) {}

    /// Starts connecting. The result arrives as [`TransportEvent::Opened`] or [`TransportEvent::Closed`]. Opening
    /// while a connection exists closes that one first; its late events are dropped.
    fn open(&mut self, url: &ServerUrl) -> Result<(), TransportError>;
    /// Queues one binary frame. Frames sent before a `close` are delivered before the close.
    fn send(&mut self, frame: Vec<u8>) -> Result<(), TransportError>;
    /// Queues one text frame: the keepalive `ping` of a sealed room. A transport that cannot say so refuses.
    fn send_text(&mut self, _text: &str) -> Result<(), TransportError> {
        Err(TransportError::Failed("this transport sends no text frames".to_owned()))
    }
    /// Closes the connection after the queued frames are written. Emits no event.
    fn close(&mut self);
    /// The next event, without blocking.
    fn poll(&mut self) -> Option<TransportEvent>;
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// An in-memory transport and the server end that scripts it. For tests and demos.
pub fn loopback() -> (LoopbackTransport, LoopbackServer) {
    let link = Arc::new(Mutex::new(Link::default()));
    (LoopbackTransport { link: Arc::clone(&link) }, LoopbackServer { link })
}

#[derive(Default)]
struct Link {
    to_client: VecDeque<TransportEvent>,
    from_client: VecDeque<Vec<u8>>,
    texts_from_client: VecDeque<String>,
    opens: Vec<String>,
    /// For each open: the client id of the token the transport would have sent, if any.
    access_at_open: Vec<Option<String>>,
    access: Option<AccessGrant>,
    open: bool,
    auto_accept: bool,
    closes: usize,
    wake: Option<WakeFn>,
}

/// The client end of [`loopback`].
pub struct LoopbackTransport {
    link: Arc<Mutex<Link>>,
}

impl LoopbackTransport {
    /// Sets the wake-up hook, called when the server end queues an event.
    pub fn with_wake(self, wake: WakeFn) -> Self {
        lock(&self.link).wake = Some(wake);
        self
    }
}

impl Transport for LoopbackTransport {
    fn set_access(&mut self, access: Option<AccessGrant>) {
        lock(&self.link).access = access;
    }

    fn open(&mut self, url: &ServerUrl) -> Result<(), TransportError> {
        let mut l = lock(&self.link);
        l.to_client.clear();
        l.from_client.clear();
        l.texts_from_client.clear();
        l.opens.push(url.to_string());
        let sent =
            l.access.as_ref().and_then(|g| g.headers_for(&AccessOrigin::of(url)).map(|h| h.pairs()[0].1.to_owned()));
        l.access_at_open.push(sent);
        l.open = false;
        if l.auto_accept {
            l.open = true;
            l.to_client.push_back(TransportEvent::Opened);
        }
        Ok(())
    }

    fn send(&mut self, frame: Vec<u8>) -> Result<(), TransportError> {
        let mut l = lock(&self.link);
        if !l.open {
            return Err(TransportError::NotOpen);
        }
        l.from_client.push_back(frame);
        Ok(())
    }

    fn send_text(&mut self, text: &str) -> Result<(), TransportError> {
        let mut l = lock(&self.link);
        if !l.open {
            return Err(TransportError::NotOpen);
        }
        l.texts_from_client.push_back(text.to_owned());
        Ok(())
    }

    fn close(&mut self) {
        let mut l = lock(&self.link);
        l.open = false;
        l.closes += 1;
    }

    fn poll(&mut self) -> Option<TransportEvent> {
        lock(&self.link).to_client.pop_front()
    }
}

/// The server end of [`loopback`]: accepts connections, reads what the client sent and says what the "server" says.
pub struct LoopbackServer {
    link: Arc<Mutex<Link>>,
}

impl LoopbackServer {
    fn queue(&self, event: TransportEvent) {
        let wake = {
            let mut l = lock(&self.link);
            l.to_client.push_back(event);
            l.wake.clone()
        };
        if let Some(w) = wake {
            w();
        }
    }

    /// Accept every connection at once, without a call to [`accept`](Self::accept).
    pub fn set_auto_accept(&self, on: bool) {
        lock(&self.link).auto_accept = on;
    }

    /// How many times the client opened a connection.
    pub fn opens(&self) -> usize {
        lock(&self.link).opens.len()
    }

    /// The address of the latest open.
    pub fn last_url(&self) -> Option<String> {
        lock(&self.link).opens.last().cloned()
    }

    /// For each open, the client id of the service token the transport sent with it, or `None` when it sent none.
    /// Only the client id is kept: the secret never leaves the grant.
    pub fn access_client_ids(&self) -> Vec<Option<String>> {
        lock(&self.link).access_at_open.clone()
    }

    /// Turns the connection away at the door, as a private server does: it ends like a close, with the reason.
    pub fn deny(&self, denial: AccessDenial) {
        lock(&self.link).open = false;
        self.queue(TransportEvent::Denied(denial));
    }

    /// How many times the client closed the connection.
    pub fn closes(&self) -> usize {
        lock(&self.link).closes
    }

    /// True while the client has an open connection.
    pub fn is_open(&self) -> bool {
        lock(&self.link).open
    }

    /// Completes the pending open.
    pub fn accept(&self) {
        lock(&self.link).open = true;
        self.queue(TransportEvent::Opened);
    }

    /// Fails the pending open or drops the connection.
    pub fn drop_connection(&self, error: &str) {
        lock(&self.link).open = false;
        self.queue(TransportEvent::Closed { error: Some(error.to_owned()) });
    }

    /// Closes the connection cleanly from the server side.
    pub fn close_cleanly(&self) {
        lock(&self.link).open = false;
        self.queue(TransportEvent::Closed { error: None });
    }

    /// Sends a message to the client.
    pub fn push(&self, msg: &ServerMessage) {
        if let Ok(bytes) = msg.encode() {
            self.push_raw(bytes);
        }
    }

    /// Sends raw bytes to the client.
    pub fn push_raw(&self, bytes: Vec<u8>) {
        self.queue(TransportEvent::Message(bytes));
    }

    /// Sends a text frame to the client.
    pub fn push_text(&self, text: &str) {
        self.queue(TransportEvent::Text(text.to_owned()));
    }

    /// The text frames the client sent so far, oldest first.
    pub fn take_texts(&self) -> Vec<String> {
        lock(&self.link).texts_from_client.drain(..).collect()
    }

    /// The next frame the client sent, as bytes.
    pub fn recv_raw(&self) -> Option<Vec<u8>> {
        lock(&self.link).from_client.pop_front()
    }

    /// The next frame the client sent, decoded.
    pub fn recv(&self) -> Option<ClientMessage> {
        let frame = lock(&self.link).from_client.pop_front()?;
        ClientMessage::decode(&frame).ok()
    }

    /// Everything the client sent so far, decoded, oldest first.
    pub fn recv_all(&self) -> Vec<ClientMessage> {
        std::iter::from_fn(|| self.recv()).collect()
    }
}
