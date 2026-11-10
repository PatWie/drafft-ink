//! The relay: how an agent drives the browser tab (native only).
//!
//! A web page cannot listen on a socket. So the roles turn around: the MCP app (`drafftink-mcp --relay`)
//! listens on 127.0.0.1, and the page connects out to it. After the page has said `hello`, the relay is the
//! JSON-RPC client and the page is the JSON-RPC server: the relay sends `execute`, `batch`, `commands`,
//! `describe`, `recent`, `state` and `ping`, and the page answers each with [`crate::core::dispatch`] on its own
//! `Session`. The page's side is specified in `docs/agents.md`; [`crate::page_sim`] plays it for tests.
//!
//! Pairing: the relay makes a one-time *pairing token*. The user pastes it into the page (or opens the link
//! the relay prints). The page's first `hello` with the pairing token uses it up and receives a *resume
//! token* for its tab, so that a reload (or a dropped connection) reconnects without the user doing
//! anything. A second page cannot pair with the used token. Host and Origin are checked as in the server;
//! the default allowed origin is `https://drafft.ink`.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use futures_util::SinkExt;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use crate::core::PROTOCOL;
use crate::files::new_token;
use crate::guard;
use crate::rpc::{self, Incoming, NOT_AUTHENTICATED, RpcError, TIMED_OUT, UNAUTHORIZED};
use crate::server::BridgeError;
use crate::ws;

/// The error code of a call when no page is connected.
pub const NOT_PAIRED: i64 = -32005;

/// The origin of the hosted web app.
pub const HOSTED_ORIGIN: &str = "https://drafft.ink";

/// How the relay is set up.
#[derive(Debug, Clone)]
pub struct RelayConfig {
    /// Where to listen. Must be 127.0.0.1.
    pub addr: SocketAddr,
    /// Pages allowed besides loopback pages.
    pub allowed_origins: Vec<String>,
    /// Largest frame in bytes, both ways.
    pub max_frame_bytes: usize,
    /// Time from connecting to a correct `hello`.
    pub auth_deadline: Duration,
    /// Time to wait for the page to answer a call.
    pub request_timeout: Duration,
    /// Use this pairing token (at least 32 visible characters) instead of a random one. For tests.
    pub pairing_token: Option<String>,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            allowed_origins: vec![HOSTED_ORIGIN.to_owned()],
            max_frame_bytes: 16 << 20,
            auth_deadline: Duration::from_secs(5),
            request_timeout: Duration::from_secs(60),
            pairing_token: None,
        }
    }
}

/// What the relay knows about its page.
#[derive(Debug, Clone, PartialEq)]
pub struct RelayStatus {
    /// The pairing token was used.
    pub paired: bool,
    /// A page is connected now.
    pub connected: bool,
    /// What the page said about itself in `hello` (`version`, `registry`).
    pub page: Option<Value>,
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, RpcError>>>>>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Link {
    tx: mpsc::UnboundedSender<String>,
    pending: Pending,
    page: Value,
}

struct State {
    port: u16,
    cfg: RelayConfig,
    pairing: Mutex<Option<String>>,
    resume: Mutex<Option<String>>,
    link: Mutex<Option<Link>>,
    next_id: AtomicU64,
}

/// A running relay. Dropping it stops it.
pub struct RelayServer {
    state: Arc<State>,
    pairing_token: String,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

/// Calls in flight to the page at once.
const MAX_PENDING: usize = 64;

impl RelayServer {
    /// Starts listening.
    pub fn start(cfg: RelayConfig) -> Result<Self, BridgeError> {
        if cfg.addr.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) {
            return Err(BridgeError::NotLoopback(cfg.addr));
        }
        let pairing_token = match &cfg.pairing_token {
            Some(t) => match guard::token_problem(t) {
                Some(why) => return Err(BridgeError::Token(why)),
                None => t.clone(),
            },
            None => new_token()?,
        };
        let listener = std::net::TcpListener::bind(cfg.addr).map_err(|e| BridgeError::Bind(cfg.addr, e))?;
        listener.set_nonblocking(true).map_err(|e| BridgeError::Bind(cfg.addr, e))?;
        let port = listener.local_addr().map_err(|e| BridgeError::Bind(cfg.addr, e))?.port();
        let state = Arc::new(State {
            port,
            cfg,
            pairing: Mutex::new(Some(pairing_token.clone())),
            resume: Mutex::new(None),
            link: Mutex::new(None),
            next_id: AtomicU64::new(1),
        });
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
        let st = state.clone();
        let thread = std::thread::Builder::new()
            .name("drafftink-relay".to_owned())
            .spawn(move || {
                let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
                    log::error!("relay: no runtime");
                    return;
                };
                rt.block_on(async move {
                    let Ok(l) = TcpListener::from_std(listener) else { return };
                    loop {
                        tokio::select! {
                            _ = &mut shutdown_rx => break,
                            accepted = l.accept() => if let Ok((stream, peer)) = accepted && peer.ip().is_loopback() {
                                let st = st.clone();
                                tokio::spawn(async move { serve_page(stream, st).await });
                            }
                        }
                    }
                });
            })
            .map_err(BridgeError::Thread)?;
        Ok(Self { state, pairing_token, shutdown: Some(shutdown_tx), thread: Some(thread) })
    }

    /// The port.
    pub fn port(&self) -> u16 {
        self.state.port
    }

    /// `ws://127.0.0.1:<port>/`.
    pub fn url(&self) -> String {
        format!("ws://127.0.0.1:{}/", self.state.port)
    }

    /// The one-time pairing token. Treat it as a password.
    pub fn pairing_token(&self) -> &str {
        &self.pairing_token
    }

    /// The link to open in the browser: the page URL plus a fragment with the port and the token. A fragment
    /// is never sent to a server. The page reads it, removes it from the address bar and connects to
    /// `ws://127.0.0.1:<port>/`, so the link cannot point the page at another host.
    pub fn pairing_url(&self, page: &str) -> String {
        let base = page.split('#').next().unwrap_or(page);
        format!("{base}#drafftink-agent-port={}&drafftink-agent-token={}", self.state.port, self.pairing_token)
    }

    /// Prints how to pair to stderr.
    pub fn announce_to_stderr(&self, page: &str) {
        eprintln!("drafftink relay: listening on {}", self.url());
        eprintln!("drafftink relay: open this link in the browser tab to pair (one time only):");
        eprintln!("  {}", self.pairing_url(page));
        eprintln!("drafftink relay: or paste this pairing token into the page: {}", self.pairing_token);
    }

    /// Whether a page is paired and connected.
    pub fn status(&self) -> RelayStatus {
        let link = lock(&self.state.link);
        RelayStatus {
            paired: lock(&self.state.pairing).is_none(),
            connected: link.is_some(),
            page: link.as_ref().map(|l| l.page.clone()),
        }
    }

    /// Calls a method on the page and waits for its answer.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let (tx, pending) = {
            let link = lock(&self.state.link);
            match link.as_ref() {
                Some(l) => (l.tx.clone(), l.pending.clone()),
                None => {
                    let paired = lock(&self.state.pairing).is_none();
                    let why = if paired {
                        "the browser tab is not connected to the relay. Reload the drafft.ink tab (it reconnects by itself), or open the pairing link again after restarting the relay"
                    } else {
                        "no browser tab is paired yet. Open the pairing link the relay printed, or paste its pairing token into the drafft.ink page"
                    };
                    return Err(RpcError::new(NOT_PAIRED, why));
                }
            }
        };
        let id = self.state.next_id.fetch_add(1, Ordering::SeqCst);
        let (reply_tx, reply_rx) = oneshot::channel();
        {
            let mut p = lock(&pending);
            if p.len() >= MAX_PENDING {
                return Err(RpcError::new(rpc::BUSY, "the page has too many requests waiting. Wait for the earlier answers"));
            }
            p.insert(id, reply_tx);
        }
        if tx.send(rpc::request_text(&json!(id), method, &params)).is_err() {
            lock(&pending).remove(&id);
            return Err(RpcError::new(NOT_PAIRED, "the browser tab disconnected before the request was sent"));
        }
        let limit = self.state.cfg.request_timeout;
        match tokio::time::timeout(limit, reply_rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err(RpcError::new(NOT_PAIRED, "the browser tab disconnected before it answered. The request may or may not have run: check `recent`")),
            Err(_) => {
                lock(&pending).remove(&id);
                Err(RpcError::new(
                    TIMED_OUT,
                    format!(
                        "the browser tab did not answer `{method}` within {} s. It may be in a background tab (browsers pause those). The request was NOT cancelled and may still apply: check `recent` before sending it again",
                        limit.as_secs()
                    ),
                ))
            }
        }
    }

    /// Stops the relay and waits for its thread.
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

impl Drop for RelayServer {
    fn drop(&mut self) {
        self.shutdown_now();
    }
}

#[derive(Deserialize)]
struct HelloParams {
    #[serde(default)]
    token: String,
    #[serde(default)]
    page: Value,
}

enum Granted {
    Paired(String),
    Resumed(String),
}

fn grant(st: &State, given: &str) -> Option<Granted> {
    let mut pairing = lock(&st.pairing);
    if pairing.as_deref().is_some_and(|p| guard::tokens_match(given, p)) {
        *pairing = None;
        let resume = new_token().ok()?;
        *lock(&st.resume) = Some(resume.clone());
        return Some(Granted::Paired(resume));
    }
    let resume = lock(&st.resume);
    match resume.as_deref() {
        Some(r) if guard::tokens_match(given, r) => Some(Granted::Resumed(r.to_owned())),
        _ => None,
    }
}

async fn reject<S>(ws: &mut WebSocketStream<S>, id: Option<&Value>, err: RpcError)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let _ = ws.send(Message::text(rpc::response_err(id, &err))).await;
    let _ = ws.close(Some(CloseFrame { code: CloseCode::Policy, reason: "refused".into() })).await;
}

async fn authenticate(stream: TcpStream, st: &State) -> Option<(WebSocketStream<TcpStream>, Value, Value)> {
    let mut ws = ws::accept(stream, st.port, Arc::new(st.cfg.allowed_origins.clone()), st.cfg.max_frame_bytes).await.ok()?;
    let first = ws::recv_text(&mut ws).await.ok().flatten()?;
    if first.len() > 16 * 1024 {
        reject(&mut ws, None, RpcError::new(NOT_AUTHENTICATED, "the first frame must be a small `hello`")).await;
        return None;
    }
    match rpc::parse(&first) {
        Ok(Incoming::Request { id, method, params }) if method == "hello" => {
            let p: HelloParams = serde_json::from_value(params).unwrap_or(HelloParams { token: String::new(), page: Value::Null });
            let Some(granted) = grant(st, &p.token) else {
                let why = "the token is wrong or was already used. Ask for a new pairing link: restart the relay, or reload the tab if it was paired before";
                reject(&mut ws, Some(&id), RpcError::new(UNAUTHORIZED, why)).await;
                return None;
            };
            let (Granted::Paired(resume) | Granted::Resumed(resume)) = granted;
            let hello = json!({
                "protocol": PROTOCOL,
                "server": "drafftink-relay",
                "role": "relay",
                "version": env!("CARGO_PKG_VERSION"),
                "resume_token": resume,
                "calls": ["execute", "batch", "commands", "describe", "recent", "state", "ping"],
                "limits": { "max_frame_bytes": st.cfg.max_frame_bytes },
            });
            ws.send(Message::text(rpc::response_ok(&id, &hello))).await.ok()?;
            Some((ws, hello, p.page))
        }
        other => {
            let id = match &other {
                Ok(Incoming::Request { id, .. }) => Some(id.clone()),
                Err(m) => m.id.clone(),
                _ => None,
            };
            reject(&mut ws, id.as_ref(), RpcError::new(NOT_AUTHENTICATED, "not authenticated: the first message must be `hello` with the token")).await;
            None
        }
    }
}

async fn serve_page(stream: TcpStream, st: Arc<State>) {
    let Ok(Some((mut ws, _hello, page))) = tokio::time::timeout(st.cfg.auth_deadline, authenticate(stream, &st)).await else {
        return;
    };
    let (tx, mut out_rx) = mpsc::unbounded_channel::<String>();
    let pending: Pending = Arc::default();
    // A new page replaces the old link; the old task notices its channel closed.
    let previous = lock(&st.link).replace(Link { tx: tx.clone(), pending: pending.clone(), page });
    drop(previous);
    loop {
        tokio::select! {
            frame = ws::recv_text(&mut ws) => match frame {
                Ok(Some(text)) => match rpc::parse(&text) {
                    Ok(Incoming::Response { id, outcome }) => {
                        let key = id.as_u64();
                        if let Some(k) = key && let Some(reply) = lock(&pending).remove(&k) {
                            let _ = reply.send(outcome);
                        }
                    }
                    Ok(Incoming::Request { id, method, .. }) => {
                        let answer = if method == "ping" {
                            rpc::response_ok(&id, &json!({}))
                        } else {
                            rpc::response_err(Some(&id), &RpcError::new(rpc::METHOD_NOT_FOUND, format!("the relay answers only `ping`, not `{method}`")))
                        };
                        if ws.send(Message::text(answer)).await.is_err() {
                            break;
                        }
                    }
                    Ok(Incoming::Notification { .. }) => {}
                    Err(m) => {
                        if ws.send(Message::text(rpc::response_err(m.id.as_ref(), &m.error))).await.is_err() {
                            break;
                        }
                    }
                },
                Ok(None) | Err(_) => break,
            },
            out = out_rx.recv() => match out {
                Some(text) => if ws.send(Message::text(text)).await.is_err() { break },
                None => break,
            },
        }
    }
    let _ = ws.close(None).await;
    {
        let mut link = lock(&st.link);
        if link.as_ref().is_some_and(|l| l.tx.same_channel(&tx)) {
            *link = None;
        }
    }
    let waiting: Vec<_> = lock(&pending).drain().collect();
    for (_, reply) in waiting {
        let _ = reply.send(Err(RpcError::new(
            NOT_PAIRED,
            "the browser tab disconnected before it answered. The request may or may not have run: check `recent`",
        )));
    }
}
