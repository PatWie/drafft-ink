//! MCP over streamable HTTP, served by the desktop app on loopback.
//!
//! The server runs on its own thread. It never touches the `Session`: a tool call becomes a bridge request on
//! the shell's queue ([`LocalCaller`]) and runs between frames, like a request of the bridge. Off unless the
//! user asks for it.
//!
//! Every request passes the same checks as the bridge, in this order, before any MCP code sees it:
//! `Host` is `127.0.0.1:<port>` or `localhost:<port>` ([`guard::check_host`]); `Origin` is absent or a loopback
//! page ([`guard::check_origin`]); `Authorization` is `Bearer <token>` with this launch's token
//! ([`guard::check_bearer`]); the path is `/mcp`. No CORS header is ever sent, so a page cannot read an answer.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::JoinHandle;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::handler::{DrafftHandler, Program};
use super::local::LocalCaller;
use crate::files::{self, TokenFile};
use crate::guard::{self, Refused};
use crate::host::JobSender;
use crate::server::BridgeError;
use crate::tools::service::Service;

/// The path of the MCP endpoint.
pub const PATH: &str = "/mcp";
/// The port that `--mcp-http` and the setting use when no port is given. A fixed port keeps the URL in a
/// client's configuration valid from one launch to the next (the token still changes).
pub const DEFAULT_PORT: u16 = 3737;
/// Requests handled at once. More get `503`.
const MAX_IN_FLIGHT: usize = 32;
/// Largest request body in bytes.
const MAX_BODY_BYTES: usize = 4 << 20;
/// The folder, inside the bridge's runtime folder, with the token file of this server.
const TOKEN_SUBDIR: &str = "mcp-http";

/// How the server is set up. [`Default`] is the safe setup of the desktop app.
#[derive(Debug, Clone)]
pub struct McpHttpConfig {
    /// Where to listen. Must be 127.0.0.1; port 0 picks a free port.
    pub addr: SocketAddr,
    /// Use this token instead of a random one (at least 32 visible characters). For sandboxes and tests.
    pub token: Option<String>,
    /// Where to write the token file. `None`: a folder `mcp-http` inside [`files::default_dir`].
    pub dir: Option<PathBuf>,
    /// Write the token file. Off only for tests that read the token from [`McpHttpServer::token`].
    pub write_token_file: bool,
}

impl McpHttpConfig {
    /// The safe setup on `port`.
    pub fn on_port(port: u16) -> Self {
        Self {
            addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
            token: None,
            dir: None,
            write_token_file: true,
        }
    }
}

impl Default for McpHttpConfig {
    fn default() -> Self {
        Self::on_port(DEFAULT_PORT)
    }
}

struct Gate {
    token: String,
    port: u16,
    open: AtomicUsize,
}

/// A running server. Dropping it stops listening and removes the token file.
pub struct McpHttpServer {
    port: u16,
    token: String,
    token_file: Option<TokenFile>,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl McpHttpServer {
    /// Starts listening. Requests are run by the shell that polls the host of `jobs`.
    pub fn start(cfg: McpHttpConfig, jobs: JobSender) -> Result<Self, BridgeError> {
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
            let dir = match cfg.dir.clone() {
                Some(d) => d,
                None => {
                    // The runtime folder is private to the user, like the bridge's own.
                    let base = files::default_dir().ok_or(BridgeError::NoDir)?;
                    files::prepare_dir(&base).map_err(BridgeError::Files)?;
                    base.join(TOKEN_SUBDIR)
                }
            };
            Some(TokenFile::write(&dir, port, &token).map_err(BridgeError::Files)?)
        } else {
            None
        };

        let cancel = CancellationToken::new();
        let caller = LocalCaller::new(jobs, format!("http://127.0.0.1:{port}{PATH}"));
        let handler = DrafftHandler::new(Arc::new(Service::new(caller)), Program::Desktop);
        let mcp = StreamableHttpService::new(
            move || Ok(handler.clone()),
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig::default()
                .with_allowed_hosts([format!("127.0.0.1:{port}"), format!("localhost:{port}")])
                .with_max_request_body_bytes(MAX_BODY_BYTES)
                .with_cancellation_token(cancel.child_token()),
        );
        let gate = Arc::new(Gate { token: token.clone(), port, open: AtomicUsize::new(0) });
        let app = Router::new()
            .route(PATH, axum::routing::any(move |req: Request| async move { mcp.handle(req).await }))
            .layer(middleware::from_fn_with_state(gate, check));

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("drafftink-mcp-http".to_owned())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                    Ok(rt) => rt,
                    Err(e) => {
                        log::error!("mcp http: no runtime: {e}");
                        return;
                    }
                };
                rt.block_on(async move {
                    let listener = match TcpListener::from_std(std_listener) {
                        Ok(l) => l,
                        Err(e) => {
                            log::error!("mcp http: {e}");
                            return;
                        }
                    };
                    // Open streams would hold a graceful shutdown for ever, so the signal ends the select.
                    tokio::select! {
                        served = axum::serve(listener, app) => {
                            if let Err(e) = served {
                                log::error!("mcp http: {e}");
                            }
                        }
                        _ = shutdown_rx => {}
                    }
                    cancel.cancel();
                });
            })
            .map_err(BridgeError::Thread)?;
        Ok(Self { port, token, token_file, shutdown: Some(shutdown_tx), thread: Some(thread) })
    }

    /// The port.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// `http://127.0.0.1:<port>/mcp`.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}{PATH}", self.port)
    }

    /// The token. Treat it as a password.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// The token file, if one was written.
    pub fn token_file(&self) -> Option<&std::path::Path> {
        self.token_file.as_ref().map(TokenFile::path)
    }

    /// Prints the address and how to connect to stderr. Call it once at start: the token file is the other
    /// place the token is kept.
    pub fn announce_to_stderr(&self) {
        eprintln!("drafftink mcp http: listening on {}", self.url());
        if let Some(p) = self.token_file() {
            eprintln!("drafftink mcp http: token file {} (mode 0600)", p.display());
        }
        eprintln!(
            "drafftink mcp http: send the header `Authorization: Bearer <token>`; docs/agents.md has client setups"
        );
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

impl Drop for McpHttpServer {
    fn drop(&mut self) {
        self.shutdown_now();
    }
}

fn header_text(headers: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// Counts a request being handled and frees the slot when it is dropped.
struct Slot(Arc<Gate>);

impl Slot {
    fn take(gate: &Arc<Gate>) -> Option<Self> {
        if gate.open.fetch_add(1, Ordering::SeqCst) >= MAX_IN_FLIGHT {
            gate.open.fetch_sub(1, Ordering::SeqCst);
            None
        } else {
            Some(Self(gate.clone()))
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.open.fetch_sub(1, Ordering::SeqCst);
    }
}

fn refuse(status: StatusCode, why: &str) -> Response {
    let mut response = (status, format!("drafftink mcp: {why}\n")).into_response();
    if status == StatusCode::UNAUTHORIZED {
        response.headers_mut().insert(header::WWW_AUTHENTICATE, header::HeaderValue::from_static("Bearer"));
    }
    response
}

/// The checks of the module documentation, then the MCP service.
async fn check(State(gate): State<Arc<Gate>>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    let verdict = guard::check_host(header_text(headers, header::HOST), gate.port)
        .and_then(|()| guard::check_origin(header_text(headers, header::ORIGIN), &[]))
        .and_then(|()| guard::check_bearer(header_text(headers, header::AUTHORIZATION), &gate.token));
    if let Err(why) = verdict {
        log::warn!("mcp http: refused a request: {why}");
        let status = match why {
            Refused::Bearer => StatusCode::UNAUTHORIZED,
            Refused::Host(..) | Refused::Origin(_) => StatusCode::FORBIDDEN,
        };
        return refuse(status, &why.to_string());
    }
    if request.uri().path() != PATH {
        return refuse(StatusCode::NOT_FOUND, &format!("the endpoint is {PATH}"));
    }
    let Some(slot) = Slot::take(&gate) else {
        return refuse(StatusCode::SERVICE_UNAVAILABLE, "too many requests at once");
    };
    // The slot is held while the service answers. An open event stream is not counted after its headers.
    let response = next.run(request).await;
    drop(slot);
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_setup_is_loopback_on_the_default_port_with_a_token_file() {
        let cfg = McpHttpConfig::default();
        assert_eq!(cfg.addr, SocketAddr::from(([127, 0, 0, 1], DEFAULT_PORT)));
        assert!(cfg.write_token_file && cfg.token.is_none() && cfg.dir.is_none());
        assert_eq!(McpHttpConfig::on_port(0).addr.port(), 0);
    }
}
