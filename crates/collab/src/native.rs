//! The native WebSocket transport.
//!
//! A background thread runs a small single-threaded tokio runtime. Each `open` starts one connection task on it. The
//! task pushes events into a queue and calls the wake hook; the owner drains the queue with [`Transport::poll`]. The
//! owner's thread never blocks and never polls on a timer.
//!
//! The queue is bounded in bytes ([`NativeOptions::max_queued_bytes`]). When the owner stops draining it (a stalled UI
//! thread), the task stops reading the socket, TCP pushes back on the server, and the server's own slow-consumer
//! rule applies: it drops its queued relays and sends `ResyncRequired`. Memory use stays bounded on both sides.
//!
//! A service token for a private server (`set_access`) goes into the handshake as two headers, under the rules of
//! [`AccessGrant::headers_for`]. The client never follows a redirect (tungstenite does not), so a `302` to the Access
//! login is reported as [`TransportEvent::Denied`] and the token goes nowhere else.
//!
//! `wss://` uses rustls with the ring provider and the operating system's root certificates (the bundled Mozilla
//! list when the system has none). A private CA can be added with [`TlsOptions`].

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use drafftink_protocol::MAX_FRAME_BYTES;
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpSocket, TcpStream};
use tokio::sync::Notify;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue, Request};
use tokio_tungstenite::tungstenite::protocol::{Message, WebSocketConfig};
use tokio_tungstenite::{Connector, client_async_tls_with_config};

use crate::access::{AccessDenial, AccessGrant, AccessOrigin};
use crate::client::CONNECT_TIMEOUT_MS;
use crate::transport::{Transport, TransportError, TransportEvent, WakeFn, lock};
use crate::url::ServerUrl;

/// Extra trust for `wss://`.
#[derive(Debug, Clone, Default)]
pub struct TlsOptions {
    /// DER-encoded root certificates to trust besides the system's. For a server with a private CA.
    pub extra_roots_der: Vec<Vec<u8>>,
}

/// Tuning of the native transport.
#[derive(Debug, Clone)]
pub struct NativeOptions {
    /// Trust for `wss://`.
    pub tls: TlsOptions,
    /// Most bytes of received frames held for the owner before the socket is no longer read.
    pub max_queued_bytes: usize,
    /// Size of the socket receive buffer; `None` keeps the operating system's choice.
    pub recv_buffer_bytes: Option<u32>,
}

impl Default for NativeOptions {
    fn default() -> Self {
        NativeOptions { tls: TlsOptions::default(), max_queued_bytes: 8 * 1024 * 1024, recv_buffer_bytes: None }
    }
}

enum Out {
    Frame(Vec<u8>),
    Text(String),
    Close,
}

struct Shared {
    events: Mutex<VecDeque<(u64, TransportEvent)>>,
    /// Bytes of `Message` events in `events`.
    queued: AtomicUsize,
    /// Signalled when `poll` frees space.
    space: Notify,
    max_queued: usize,
    recv_buffer: Option<u32>,
}

struct Runtime {
    handle: tokio::runtime::Handle,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

/// A WebSocket client on a background thread. See the module docs.
pub struct NativeTransport {
    wake: WakeFn,
    tls: Arc<TlsCell>,
    runtime: Option<Runtime>,
    shared: Arc<Shared>,
    generation: u64,
    conn: Option<UnboundedSender<Out>>,
    access: Option<Arc<AccessGrant>>,
}

struct TlsCell {
    options: TlsOptions,
    config: OnceLock<Result<Arc<rustls::ClientConfig>, ConnectError>>,
}

impl NativeTransport {
    /// A transport that calls `wake` (from its own thread) whenever it queues an event.
    pub fn new(wake: WakeFn) -> Self {
        Self::with_options(wake, NativeOptions::default())
    }

    /// Like [`new`](Self::new), with explicit options.
    pub fn with_options(wake: WakeFn, options: NativeOptions) -> Self {
        NativeTransport {
            wake,
            tls: Arc::new(TlsCell { options: options.tls, config: OnceLock::new() }),
            runtime: None,
            shared: Arc::new(Shared {
                events: Mutex::new(VecDeque::new()),
                queued: AtomicUsize::new(0),
                space: Notify::new(),
                max_queued: options.max_queued_bytes.max(1),
                recv_buffer: options.recv_buffer_bytes,
            }),
            generation: 0,
            conn: None,
            access: None,
        }
    }

    fn runtime_handle(&mut self) -> Result<tokio::runtime::Handle, TransportError> {
        if let Some(rt) = &self.runtime {
            return Ok(rt.handle.clone());
        }
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| TransportError::Failed(format!("cannot start the network thread: {e}")))?;
        let handle = rt.handle().clone();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        // Exception to the no-threads rule below L6 (xtask/clippy-below-l6): the native transport owns its network
        // thread. The browser transport (`web.rs`) has none.
        #[allow(clippy::disallowed_methods)]
        let spawned = std::thread::Builder::new().name("drafftink-collab".to_owned()).spawn(move || {
            rt.block_on(async {
                let _ = stopped.await;
            });
            // Connection tasks are dropped with the runtime.
            rt.shutdown_background();
        });
        spawned.map_err(|e| TransportError::Failed(format!("cannot start the network thread: {e}")))?;
        self.runtime = Some(Runtime { handle: handle.clone(), stop: Some(stop) });
        Ok(handle)
    }
}

impl Transport for NativeTransport {
    fn set_access(&mut self, access: Option<AccessGrant>) {
        self.access = access.map(Arc::new);
    }

    fn open(&mut self, url: &ServerUrl) -> Result<(), TransportError> {
        let handle = self.runtime_handle()?;
        self.generation += 1;
        let (tx, rx) = unbounded_channel();
        self.conn = Some(tx);
        let task = Connection {
            generation: self.generation,
            url: url.clone(),
            tls: Arc::clone(&self.tls),
            shared: Arc::clone(&self.shared),
            wake: Arc::clone(&self.wake),
            access: self.access.clone(),
        };
        handle.spawn(task.run(rx));
        Ok(())
    }

    fn send(&mut self, frame: Vec<u8>) -> Result<(), TransportError> {
        match &self.conn {
            Some(tx) => tx.send(Out::Frame(frame)).map_err(|_| TransportError::NotOpen),
            None => Err(TransportError::NotOpen),
        }
    }

    fn send_text(&mut self, text: &str) -> Result<(), TransportError> {
        match &self.conn {
            Some(tx) => tx.send(Out::Text(text.to_owned())).map_err(|_| TransportError::NotOpen),
            None => Err(TransportError::NotOpen),
        }
    }

    fn close(&mut self) {
        if let Some(tx) = self.conn.take() {
            let _ = tx.send(Out::Close);
        }
        // Events of the closed connection are stale from now on.
        self.generation += 1;
    }

    fn poll(&mut self) -> Option<TransportEvent> {
        let mut found = None;
        let mut freed = 0usize;
        {
            let mut q = lock(&self.shared.events);
            while let Some((generation, event)) = q.pop_front() {
                if let TransportEvent::Message(b) = &event {
                    freed += b.len();
                }
                if generation == self.generation {
                    found = Some(event);
                    break;
                }
            }
        }
        if freed > 0 {
            self.shared.queued.fetch_sub(freed, Ordering::AcqRel);
            self.shared.space.notify_one();
        }
        found
    }
}

struct Connection {
    generation: u64,
    url: ServerUrl,
    tls: Arc<TlsCell>,
    shared: Arc<Shared>,
    wake: WakeFn,
    access: Option<Arc<AccessGrant>>,
}

/// Why a connection attempt or a running connection failed. Its text goes to the status line and the log.
#[derive(Debug, Clone, thiserror::Error)]
enum ConnectError {
    #[error("the server name has no address")]
    NoAddress,
    #[error("{0}")]
    Io(String),
    #[error("{0}")]
    Handshake(String),
    /// The server's gate turned the handshake away. Reported as [`TransportEvent::Denied`].
    #[error("the server's gate refused the connection")]
    Denied(AccessDenial),
    #[error("a trusted certificate is not valid: {0}")]
    BadCertificate(String),
    #[error("{0}")]
    Tls(String),
    #[error("timed out")]
    TimedOut,
    #[error("{0}")]
    Socket(String),
}

enum Ended {
    /// We closed it; report nothing.
    ByUs,
    /// The peer closed it.
    ByPeer,
}

impl Connection {
    fn push(&self, event: TransportEvent) {
        if let TransportEvent::Message(b) = &event {
            self.shared.queued.fetch_add(b.len(), Ordering::AcqRel);
        }
        lock(&self.shared.events).push_back((self.generation, event));
        (self.wake)();
    }

    async fn tcp(&self) -> Result<TcpStream, ConnectError> {
        let addrs = tokio::net::lookup_host((self.url.host(), self.url.port()))
            .await
            .map_err(|e| ConnectError::Io(e.to_string()))?;
        let mut last = ConnectError::NoAddress;
        for addr in addrs {
            let socket = if addr.is_ipv4() { TcpSocket::new_v4() } else { TcpSocket::new_v6() }
                .map_err(|e| ConnectError::Io(e.to_string()))?;
            if let Some(n) = self.shared.recv_buffer {
                // A hint: the operating system may refuse it, and the connection works without it.
                let _ = socket.set_recv_buffer_size(n);
            }
            match socket.connect(addr).await {
                Ok(stream) => {
                    // Also a hint: it lowers latency but nothing depends on it.
                    let _ = stream.set_nodelay(true);
                    return Ok(stream);
                }
                Err(e) => last = ConnectError::Io(e.to_string()),
            }
        }
        Err(last)
    }

    /// Adds the service token to the handshake when the rules allow it. True when it was added.
    fn add_access(&self, request: &mut Request<()>) -> Result<bool, ConnectError> {
        let Some(headers) = self.access.as_deref().and_then(|g| g.headers_for(&AccessOrigin::of(&self.url))) else {
            return Ok(false);
        };
        for (name, value) in headers.pairs() {
            // The text of these errors holds neither the name nor the value.
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| ConnectError::Handshake("the access token cannot be sent".to_owned()))?;
            let mut value = HeaderValue::from_str(value)
                .map_err(|_| ConnectError::Handshake("the access token cannot be sent".to_owned()))?;
            value.set_sensitive(true);
            request.headers_mut().insert(name, value);
        }
        Ok(true)
    }

    async fn run(self, mut rx: UnboundedReceiver<Out>) {
        match self.session(&mut rx).await {
            Ok(Ended::ByUs) => {}
            Ok(Ended::ByPeer) => self.push(TransportEvent::Closed { error: None }),
            Err(ConnectError::Denied(denial)) => self.push(TransportEvent::Denied(denial)),
            Err(e) => self.push(TransportEvent::Closed { error: Some(e.to_string()) }),
        }
    }

    async fn session(&self, rx: &mut UnboundedReceiver<Out>) -> Result<Ended, ConnectError> {
        let mut request =
            self.url.to_string().into_client_request().map_err(|e| ConnectError::Handshake(e.to_string()))?;
        let sent_access = self.add_access(&mut request)?;
        let connector = if self.url.is_secure() {
            let tls = Arc::clone(&self.tls);
            let config =
                tokio::task::spawn_blocking(move || tls.config.get_or_init(|| build_tls(&tls.options)).clone())
                    .await
                    .map_err(|e| ConnectError::Tls(e.to_string()))??;
            Connector::Rustls(config)
        } else {
            Connector::Plain
        };
        let limits =
            WebSocketConfig::default().max_message_size(Some(MAX_FRAME_BYTES)).max_frame_size(Some(MAX_FRAME_BYTES));
        let connect = async {
            let stream = self.tcp().await?;
            client_async_tls_with_config(request, stream, Some(limits), Some(connector))
                .await
                .map_err(|e| handshake_error(e, sent_access))
        };
        let timeout = Duration::from_millis(CONNECT_TIMEOUT_MS);
        let mut ws = tokio::select! {
            r = tokio::time::timeout(timeout, connect) => match r {
                Ok(Ok((ws, _))) => ws,
                Ok(Err(e)) => return Err(e),
                Err(_) => return Err(ConnectError::TimedOut),
            },
            // The owner closed or dropped the transport while connecting.
            _ = rx.recv() => return Ok(Ended::ByUs),
        };
        self.push(TransportEvent::Opened);
        loop {
            let reading = self.shared.queued.load(Ordering::Acquire) < self.shared.max_queued;
            tokio::select! {
                // The owner is behind: stop reading until it drains the queue.
                () = self.shared.space.notified(), if !reading => {}
                incoming = ws.next(), if reading => match incoming {
                    Some(Ok(Message::Binary(b))) => self.push(TransportEvent::Message(b.to_vec())),
                    Some(Ok(Message::Text(t))) => self.push(TransportEvent::Text(t.as_str().to_owned())),
                    Some(Ok(Message::Close(_))) | None => return Ok(Ended::ByPeer),
                    // Pings are answered by the library.
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(ConnectError::Socket(e.to_string())),
                },
                outgoing = rx.recv() => match outgoing {
                    Some(Out::Frame(f)) => {
                        if let Err(e) = ws.send(Message::Binary(f.into())).await {
                            return Err(ConnectError::Socket(e.to_string()));
                        }
                    }
                    Some(Out::Text(t)) => {
                        if let Err(e) = ws.send(Message::Text(t.into())).await {
                            return Err(ConnectError::Socket(e.to_string()));
                        }
                    }
                    Some(Out::Close) | None => {
                        let _ = tokio::time::timeout(Duration::from_secs(2), ws.close(None)).await;
                        return Ok(Ended::ByUs);
                    }
                },
            }
        }
    }
}

/// A failed handshake. An answer that is the server's gate (a `401`, a redirect to the Access login) is a denial.
fn handshake_error(error: tokio_tungstenite::tungstenite::Error, sent_access: bool) -> ConnectError {
    if let tokio_tungstenite::tungstenite::Error::Http(response) = &error {
        let body = response.body().as_deref().unwrap_or_default();
        if let Some(denial) = AccessDenial::of_reply(response.status().as_u16(), body, sent_access) {
            return ConnectError::Denied(denial);
        }
    }
    ConnectError::Handshake(error.to_string())
}

fn build_tls(options: &TlsOptions) -> Result<Arc<rustls::ClientConfig>, ConnectError> {
    use rustls::pki_types::CertificateDer;
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_native_certs::load_native_certs().certs {
        // A malformed system certificate is skipped, like browsers do.
        let _ = roots.add(cert);
    }
    if roots.is_empty() {
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    }
    for der in &options.extra_roots_der {
        roots.add(CertificateDer::from(der.clone())).map_err(|e| ConnectError::BadCertificate(e.to_string()))?;
    }
    // The provider is named here instead of installed process-wide, so another crate that enables a different one
    // cannot make `builder()` panic.
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| ConnectError::Tls(e.to_string()))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}
