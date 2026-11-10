//! The native bridge server: loopback only, one token per launch, `Host` and `Origin` checks, bounded
//! frames, a 5 second deadline to say `hello`.
//!
//! The server runs on its own thread with a small tokio runtime. It never touches the `Session`. Each
//! request becomes a [`Job`] for the shell's [`BridgeHost`], and the connection waits for the answer.
//!
//! Wire protocol: see `docs/agents.md`. In short, the first frame must be
//! `{"jsonrpc":"2.0","id":1,"method":"hello","params":{"token":"..."}}`; after the answer, the methods of
//! [`crate::core`] plus `subscribe` and `unsubscribe` work.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use drafftink_engine::Registry;
use futures_util::SinkExt;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, oneshot};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use crate::core::{SESSION_METHODS, hello_result};
use crate::files::{self, TokenFile};
use crate::guard;
use crate::host::{BridgeHost, EventSink, Job, JobSender, Waker, channel};
use crate::rpc::{self, Incoming, NOT_AUTHENTICATED, RpcError, TIMED_OUT, UNAUTHORIZED};
use crate::ws::{self, RecvError};

/// Why the bridge could not start.
#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    /// The address is not 127.0.0.1. The bridge never listens anywhere else.
    #[error("the bridge only listens on 127.0.0.1, and {0} is not that address")]
    NotLoopback(SocketAddr),
    /// The port could not be bound.
    #[error("could not listen on {0}: {1}")]
    Bind(SocketAddr, std::io::Error),
    /// The supplied token is not acceptable.
    #[error("{0}")]
    Token(String),
    /// The operating system gave no random bytes.
    #[error("no random numbers for the token: {0}")]
    Random(String),
    /// The token file could not be written.
    #[error("could not write the token file: {0}")]
    Files(std::io::Error),
    /// The server thread did not start.
    #[error("could not start the bridge thread: {0}")]
    Thread(std::io::Error),
    /// No directory for the token file can be derived from the environment.
    #[error("no directory for the token file: set DRAFFTINK_BRIDGE_DIR")]
    NoDir,
}

/// How the server is set up. [`Default`] is the safe setup of the desktop app.
#[derive(Debug, Clone)]
pub struct BridgeConfig {
    /// Where to listen. Must be 127.0.0.1; port 0 picks a free port.
    pub addr: SocketAddr,
    /// Use this token instead of a random one (at least 32 visible characters). For sandboxes and tests.
    pub token: Option<String>,
    /// Where to write the token file. `None`: [`files::default_dir`].
    pub dir: Option<PathBuf>,
    /// Write the token file. Off only for tests that read the token from [`BridgeServer::token`].
    pub write_token_file: bool,
    /// Pages that may connect besides loopback pages, for example the app's own origin.
    pub allowed_origins: Vec<String>,
    /// Largest frame in bytes, both ways.
    pub max_frame_bytes: usize,
    /// Time from connecting to a correct `hello`.
    pub auth_deadline: Duration,
    /// Time to wait for the shell to run a request.
    pub request_timeout: Duration,
    /// Open connections at once.
    pub max_connections: usize,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            token: None,
            dir: None,
            write_token_file: true,
            allowed_origins: Vec::new(),
            max_frame_bytes: 4 << 20,
            auth_deadline: Duration::from_secs(5),
            request_timeout: Duration::from_secs(30),
            max_connections: 8,
        }
    }
}

/// Bad frames after which a connection is closed.
const MAX_BAD_FRAMES: u32 = 8;
/// The `hello` frame is tiny. Anything larger before authentication is refused unread.
const MAX_HELLO_BYTES: usize = 4096;

struct Events {
    tx: broadcast::Sender<String>,
    subscribers: AtomicUsize,
}

impl EventSink for Events {
    fn has_subscribers(&self) -> bool {
        self.subscribers.load(Ordering::Relaxed) > 0
    }
    fn publish(&self, method: &str, params: Value) {
        let _ = self.tx.send(rpc::notification_text(method, &params));
    }
}

struct Shared {
    token: String,
    port: u16,
    cfg: BridgeConfig,
    jobs: JobSender,
    events: Arc<Events>,
    hello: Value,
    open: AtomicUsize,
}

/// A running server. Dropping it stops listening and removes the token file.
pub struct BridgeServer {
    port: u16,
    token: String,
    token_file: Option<TokenFile>,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl BridgeServer {
    /// Starts listening and returns the server with the host the shell must poll. `waker` is called from
    /// the server thread whenever a request waits (for example `ctx.request_repaint()`).
    pub fn start(cfg: BridgeConfig, waker: Waker) -> Result<(Self, BridgeHost), BridgeError> {
        if cfg.addr.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) {
            return Err(BridgeError::NotLoopback(cfg.addr));
        }
        let token = match &cfg.token {
            Some(t) => match guard::token_problem(t) {
                Some(why) => return Err(BridgeError::Token(why)),
                None => t.clone(),
            },
            None => files::new_token()?,
        };
        let std_listener = std::net::TcpListener::bind(cfg.addr).map_err(|e| BridgeError::Bind(cfg.addr, e))?;
        std_listener.set_nonblocking(true).map_err(|e| BridgeError::Bind(cfg.addr, e))?;
        let port = std_listener.local_addr().map_err(|e| BridgeError::Bind(cfg.addr, e))?.port();
        let token_file = if cfg.write_token_file {
            let dir = cfg.dir.clone().or_else(files::default_dir).ok_or(BridgeError::NoDir)?;
            Some(TokenFile::write(&dir, port, &token).map_err(BridgeError::Files)?)
        } else {
            None
        };
        let (events_tx, _) = broadcast::channel(16);
        let events = Arc::new(Events { tx: events_tx, subscribers: AtomicUsize::new(0) });
        let sink: Arc<dyn EventSink> = events.clone();
        let (jobs, host) = channel(waker, Some(sink));
        let hello = hello_result(&Registry::global(), cfg.max_frame_bytes);
        let shared =
            Arc::new(Shared { token: token.clone(), port, cfg, jobs, events, hello, open: AtomicUsize::new(0) });
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let thread = std::thread::Builder::new()
            .name("drafftink-bridge".to_owned())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                    Ok(rt) => rt,
                    Err(e) => {
                        log::error!("bridge: no runtime: {e}");
                        return;
                    }
                };
                rt.block_on(async move {
                    match TcpListener::from_std(std_listener) {
                        Ok(l) => accept_loop(l, shared, shutdown_rx).await,
                        Err(e) => log::error!("bridge: {e}"),
                    }
                });
            })
            .map_err(BridgeError::Thread)?;
        let server = Self { port, token, token_file, shutdown: Some(shutdown_tx), thread: Some(thread) };
        Ok((server, host))
    }

    /// The port.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// `ws://127.0.0.1:<port>/`.
    pub fn url(&self) -> String {
        format!("ws://127.0.0.1:{}/", self.port)
    }

    /// The token. Treat it as a password.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// The token file, if one was written.
    pub fn token_file(&self) -> Option<&std::path::Path> {
        self.token_file.as_ref().map(TokenFile::path)
    }

    /// Prints the address and the token to stderr. Call it once at launch: this is the only place the token
    /// is shown.
    pub fn announce_to_stderr(&self) {
        eprintln!("drafftink bridge: listening on {}", self.url());
        if let Some(p) = self.token_file() {
            eprintln!("drafftink bridge: token file {} (mode 0600)", p.display());
        }
        eprintln!("drafftink bridge: token {}", self.token);
    }

    /// Stops the server and waits for its thread.
    pub fn stop(mut self) {
        self.shutdown_now();
    }

    fn shutdown_now(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for BridgeServer {
    fn drop(&mut self) {
        self.shutdown_now();
    }
}

struct ConnGuard<'a>(&'a AtomicUsize);

impl<'a> ConnGuard<'a> {
    fn enter(counter: &'a AtomicUsize, max: usize) -> Option<Self> {
        if counter.fetch_add(1, Ordering::SeqCst) >= max {
            counter.fetch_sub(1, Ordering::SeqCst);
            None
        } else {
            Some(Self(counter))
        }
    }
}

impl Drop for ConnGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn accept_loop(listener: TcpListener, shared: Arc<Shared>, mut shutdown: oneshot::Receiver<()>) {
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    if !peer.ip().is_loopback() {
                        continue;
                    }
                    let sh = shared.clone();
                    tokio::spawn(async move {
                        let Some(_guard) = ConnGuard::enter(&sh.open, sh.cfg.max_connections) else {
                            log::warn!("bridge: too many connections, dropped one");
                            return;
                        };
                        serve(stream, &sh).await;
                    });
                }
                Err(e) => {
                    log::warn!("bridge: accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }
}

#[derive(Deserialize)]
struct HelloParams {
    #[serde(default)]
    token: String,
}

async fn close<S>(ws: &mut WebSocketStream<S>, code: CloseCode, why: &str)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let _ = ws.close(Some(CloseFrame { code, reason: why.chars().take(100).collect::<String>().into() })).await;
}

async fn send<S>(ws: &mut WebSocketStream<S>, text: String) -> bool
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    ws.send(Message::text(text)).await.is_ok()
}

/// The handshake and `hello`, all inside the auth deadline.
async fn authenticate(stream: TcpStream, sh: &Shared) -> Option<WebSocketStream<TcpStream>> {
    let mut ws =
        match ws::accept(stream, sh.port, Arc::new(sh.cfg.allowed_origins.clone()), sh.cfg.max_frame_bytes).await {
            Ok(ws) => ws,
            Err(e) => {
                log::debug!("bridge: handshake failed: {e}");
                return None;
            }
        };
    let first = match ws::recv_text(&mut ws).await {
        Ok(Some(t)) => t,
        Ok(None) => return None,
        Err(e) => {
            close(&mut ws, e.close_code(), "bad first frame").await;
            return None;
        }
    };
    if first.len() > MAX_HELLO_BYTES {
        let err = RpcError::new(NOT_AUTHENTICATED, "the first frame must be a small `hello` with the token");
        let _ = send(&mut ws, rpc::response_err(None, &err)).await;
        close(&mut ws, CloseCode::Policy, "hello too large").await;
        return None;
    }
    match rpc::parse(&first) {
        Ok(Incoming::Request { id, method, params }) if method == "hello" => {
            let given: HelloParams = serde_json::from_value(params).unwrap_or(HelloParams { token: String::new() });
            if guard::tokens_match(&given.token, &sh.token) {
                if send(&mut ws, rpc::response_ok(&id, &sh.hello)).await { Some(ws) } else { None }
            } else {
                let err = RpcError::new(
                    UNAUTHORIZED,
                    "the token is wrong. Read the current one from the token file or the app's start-up message",
                );
                let _ = send(&mut ws, rpc::response_err(Some(&id), &err)).await;
                close(&mut ws, CloseCode::Policy, "unauthorized").await;
                None
            }
        }
        other => {
            let id = match &other {
                Ok(Incoming::Request { id, .. }) => Some(id.clone()),
                Err(m) => m.id.clone(),
                _ => None,
            };
            let err =
                RpcError::new(NOT_AUTHENTICATED, "not authenticated: the first message must be `hello` with the token");
            let _ = send(&mut ws, rpc::response_err(id.as_ref(), &err)).await;
            close(&mut ws, CloseCode::Policy, "hello first").await;
            None
        }
    }
}

async fn serve(stream: TcpStream, sh: &Shared) {
    let Ok(Some(mut ws)) = tokio::time::timeout(sh.cfg.auth_deadline, authenticate(stream, sh)).await else {
        return;
    };
    let mut events = sh.events.tx.subscribe();
    let mut subscribed = false;
    let mut bad = 0u32;
    loop {
        tokio::select! {
            frame = ws::recv_text(&mut ws) => match frame {
                Ok(Some(text)) => {
                    let (answer, was_bad) = handle_frame(sh, &mut subscribed, &text).await;
                    bad = if was_bad { bad + 1 } else { 0 };
                    if let Some(a) = answer && !send(&mut ws, a).await {
                        break;
                    }
                    if bad >= MAX_BAD_FRAMES {
                        close(&mut ws, CloseCode::Policy, "too many bad frames").await;
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    log::debug!("bridge: connection ended: {}", describe(&e));
                    close(&mut ws, e.close_code(), "frame refused").await;
                    break;
                }
            },
            ev = events.recv(), if subscribed => {
                if let Ok(frame) = ev && !send(&mut ws, frame).await {
                    break;
                }
            }
        }
    }
    if subscribed {
        sh.events.subscribers.fetch_sub(1, Ordering::Relaxed);
    }
}

fn describe(e: &RecvError) -> String {
    match e {
        RecvError::Binary => "binary frame".to_owned(),
        RecvError::Ws(e) => e.to_string(),
    }
}

/// Answers one frame. The flag says whether the frame was malformed.
async fn handle_frame(sh: &Shared, subscribed: &mut bool, text: &str) -> (Option<String>, bool) {
    let (id, method, params) = match rpc::parse(text) {
        Err(m) => return (Some(rpc::response_err(m.id.as_ref(), &m.error)), true),
        Ok(Incoming::Request { id, method, params }) => (id, method, params),
        Ok(_) => return (None, false),
    };
    let reply = |r: Result<Value, RpcError>| match r {
        Ok(v) => rpc::response_ok(&id, &v),
        Err(e) => rpc::response_err(Some(&id), &e),
    };
    match method.as_str() {
        "hello" => (
            Some(reply(Err(RpcError::new(NOT_AUTHENTICATED, "already authenticated: `hello` is only sent once")))),
            true,
        ),
        "subscribe" => {
            if !*subscribed {
                *subscribed = true;
                sh.events.subscribers.fetch_add(1, Ordering::Relaxed);
            }
            (Some(reply(Ok(json!({ "subscribed": true })))), false)
        }
        "unsubscribe" => {
            if *subscribed {
                *subscribed = false;
                sh.events.subscribers.fetch_sub(1, Ordering::Relaxed);
            }
            (Some(reply(Ok(json!({ "subscribed": false })))), false)
        }
        m if SESSION_METHODS.contains(&m) => {
            let (tx, rx) = oneshot::channel();
            let job = Job {
                method: method.clone(),
                params,
                reply: Box::new(move |r| {
                    let _ = tx.send(r);
                }),
            };
            if let Err((err, _)) = sh.jobs.submit(job) {
                return (Some(reply(Err(err))), false);
            }
            let out = match tokio::time::timeout(sh.cfg.request_timeout, rx).await {
                Ok(Ok(r)) => r,
                Ok(Err(_)) => Err(RpcError::new(rpc::INTERNAL_ERROR, "the app closed before it ran the request")),
                Err(_) => Err(RpcError::new(
                    TIMED_OUT,
                    format!(
                        "the app did not run `{method}` within {} s. It may be hidden, busy or showing a dialog. The request was NOT cancelled and may still apply: check `recent` before sending it again",
                        sh.cfg.request_timeout.as_secs()
                    ),
                )),
            };
            (Some(reply(out)), false)
        }
        other => {
            let e = RpcError::new(
                rpc::METHOD_NOT_FOUND,
                format!(
                    "unknown method `{other}`. Methods: hello, execute, batch, commands, describe, recent, state, ping, subscribe, unsubscribe"
                ),
            );
            (Some(reply(Err(e))), true)
        }
    }
}
