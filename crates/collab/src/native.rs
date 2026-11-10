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
//! `wss://` uses rustls with the ring provider and the operating system's root certificates (the bundled Mozilla
//! list when the system has none). A private CA can be added with [`TlsOptions`].

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use drafftink_protocol::MAX_FRAME_BYTES;
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpSocket, TcpStream};
use tokio::sync::Notify;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::{Message, WebSocketConfig};
use tokio_tungstenite::{Connector, client_async_tls_with_config};

use crate::client::CONNECT_TIMEOUT_MS;
use crate::transport::{Transport, TransportError, TransportEvent, WakeFn};
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

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
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
}

struct TlsCell {
    options: TlsOptions,
    config: OnceLock<Result<Arc<rustls::ClientConfig>, String>>,
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
        std::thread::Builder::new()
            .name("drafftink-collab".to_owned())
            .spawn(move || {
                rt.block_on(async {
                    let _ = stopped.await;
                });
                // Connection tasks are dropped with the runtime.
                rt.shutdown_background();
            })
            .map_err(|e| TransportError::Failed(format!("cannot start the network thread: {e}")))?;
        self.runtime = Some(Runtime { handle: handle.clone(), stop: Some(stop) });
        Ok(handle)
    }
}

impl Transport for NativeTransport {
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

    async fn tcp(&self) -> Result<TcpStream, String> {
        let addrs = tokio::net::lookup_host((self.url.host(), self.url.port())).await.map_err(|e| e.to_string())?;
        let mut last = "the server name has no address".to_owned();
        for addr in addrs {
            let socket =
                if addr.is_ipv4() { TcpSocket::new_v4() } else { TcpSocket::new_v6() }.map_err(|e| e.to_string())?;
            if let Some(n) = self.shared.recv_buffer {
                let _ = socket.set_recv_buffer_size(n);
            }
            match socket.connect(addr).await {
                Ok(stream) => {
                    let _ = stream.set_nodelay(true);
                    return Ok(stream);
                }
                Err(e) => last = e.to_string(),
            }
        }
        Err(last)
    }

    async fn run(self, mut rx: UnboundedReceiver<Out>) {
        match self.session(&mut rx).await {
            Ok(Ended::ByUs) => {}
            Ok(Ended::ByPeer) => self.push(TransportEvent::Closed { error: None }),
            Err(e) => self.push(TransportEvent::Closed { error: Some(e) }),
        }
    }

    async fn session(&self, rx: &mut UnboundedReceiver<Out>) -> Result<Ended, String> {
        let request = self.url.to_string().into_client_request().map_err(|e| e.to_string())?;
        let connector = if self.url.is_secure() {
            let tls = Arc::clone(&self.tls);
            let config =
                tokio::task::spawn_blocking(move || tls.config.get_or_init(|| build_tls(&tls.options)).clone())
                    .await
                    .map_err(|e| e.to_string())??;
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
                .map_err(|e| e.to_string())
        };
        let timeout = Duration::from_millis(CONNECT_TIMEOUT_MS);
        let mut ws = tokio::select! {
            r = tokio::time::timeout(timeout, connect) => match r {
                Ok(Ok((ws, _))) => ws,
                Ok(Err(e)) => return Err(e),
                Err(_) => return Err("timed out".to_owned()),
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
                    Some(Ok(Message::Close(_))) | None => return Ok(Ended::ByPeer),
                    // Pings are answered by the library; the server sends no text.
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(e.to_string()),
                },
                outgoing = rx.recv() => match outgoing {
                    Some(Out::Frame(f)) => {
                        if let Err(e) = ws.send(Message::Binary(f.into())).await {
                            return Err(e.to_string());
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

fn build_tls(options: &TlsOptions) -> Result<Arc<rustls::ClientConfig>, String> {
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
        roots.add(CertificateDer::from(der.clone())).map_err(|e| format!("a trusted certificate is not valid: {e}"))?;
    }
    // The provider is named here instead of installed process-wide, so another crate that enables a different one
    // cannot make `builder()` panic.
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}
