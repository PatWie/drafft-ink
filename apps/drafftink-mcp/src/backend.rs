//! Where the calls go. All three backends answer the bridge's methods (`execute`, `batch`, `commands`,
//! `describe`, `recent`, `state`, `ping`), so a tool does not know which mode it runs in:
//!
//! * [`Headless`]: a session owned by this process, on its own thread.
//! * [`Attached`]: a running desktop app, reached through its loopback bridge.
//! * the relay: a browser tab that connected out to this process.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use drafftink_bridge::client::{BridgeClient, ClientError};
use drafftink_bridge::core::dispatch;
use drafftink_bridge::files;
use drafftink_bridge::relay::{NOT_PAIRED, RelayServer};
use drafftink_bridge::rpc::{INTERNAL_ERROR, RpcError, TIMED_OUT};
use drafftink_engine::Session;
use serde_json::{Value, json};
use tokio::sync::{Mutex, oneshot};

/// The mode a server runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Own session, no window.
    Headless,
    /// Connected to the desktop app.
    Attached,
    /// Listening for the browser tab.
    Relay,
}

impl Mode {
    /// The name used in `status` and in the docs.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Headless => "headless",
            Self::Attached => "attached",
            Self::Relay => "relay",
        }
    }
}

struct Job {
    method: String,
    params: Value,
    reply: oneshot::Sender<Result<Value, RpcError>>,
}

/// A session on its own thread. Calls queue up and run one at a time, in order.
pub struct Headless {
    jobs: mpsc::Sender<Job>,
    scope: String,
}

impl Headless {
    /// Starts the thread. `make_session` runs on it, because a session is not `Send`. `scope` is the text
    /// that `status` shows for the folders the files port serves.
    pub fn start(scope: String, make_session: impl FnOnce() -> Session + Send + 'static) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new().name("drafftink-session".to_owned()).spawn(move || {
            let mut session = make_session();
            for job in rx {
                let out = dispatch(&mut session, &job.method, &job.params);
                let _ = job.reply.send(out);
            }
        })?;
        Ok(Self { jobs: tx, scope })
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let (reply, answer) = oneshot::channel();
        let job = Job { method: method.to_owned(), params, reply };
        self.jobs.send(job).map_err(|_| RpcError::new(INTERNAL_ERROR, "the editor session has stopped"))?;
        answer
            .await
            .map_err(|_| RpcError::new(INTERNAL_ERROR, "the editor session stopped while it ran the request"))?
    }
}

/// How to find the desktop app's token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenSource {
    /// Given directly (the environment variable, for sandboxes).
    Value(String),
    /// Read from this file on every connect, so an app restart is picked up.
    File(PathBuf),
}

/// Where the desktop app listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Look in the runtime directory: `current.port`, or the given port, and its token file.
    Discover {
        /// The directory with the token files.
        dir: PathBuf,
        /// A specific port instead of `current.port`.
        port: Option<u16>,
    },
    /// A URL and a token source given by the user.
    Explicit {
        /// `ws://127.0.0.1:<port>/`.
        url: String,
        /// The token.
        token: TokenSource,
    },
}

impl Target {
    fn resolve(&self) -> Result<(String, String), RpcError> {
        let unreachable = |why: String| RpcError::new(NOT_PAIRED, why);
        match self {
            Self::Discover { dir, port } => {
                let found = files::find(dir, *port).map_err(|e| unreachable(e.to_string()))?;
                Ok((format!("ws://127.0.0.1:{}/", found.port), found.token))
            }
            Self::Explicit { url, token } => {
                let token = match token {
                    TokenSource::Value(t) => t.clone(),
                    TokenSource::File(p) => std::fs::read_to_string(p)
                        .map(|t| t.trim().to_owned())
                        .map_err(|e| unreachable(format!("could not read the token file {}: {e}", p.display())))?,
                };
                Ok((url.clone(), token))
            }
        }
    }
}

/// How long a connection may sit unused before a `ping` checks it again.
const STALE_AFTER: Duration = Duration::from_secs(5);
/// How long a call to the app may take. A timeout does not cancel it.
const CALL_LIMIT: Duration = Duration::from_secs(120);

struct Connection {
    client: BridgeClient,
    last_used: Instant,
}

/// A connection to a running desktop app. It connects on the first call and reconnects when the app
/// restarted, reading the token file again.
pub struct Attached {
    target: Target,
    connection: Mutex<Option<Connection>>,
}

fn client_error(e: ClientError) -> RpcError {
    match e {
        ClientError::Rpc(r) => r,
        ClientError::Timeout(s) => RpcError::new(
            TIMED_OUT,
            format!(
                "the app did not answer within {s} s. The request was NOT cancelled and may still apply: check `recent` before sending it again"
            ),
        ),
        ClientError::Closed => RpcError::new(
            NOT_PAIRED,
            "the connection to the drafft.ink app was lost. If the request changed the document it may or may not have run: check `recent`, then try again",
        ),
        other => RpcError::new(NOT_PAIRED, other.to_string()),
    }
}

impl Attached {
    /// A client for `target`. Nothing connects yet.
    pub fn new(target: Target) -> Self {
        Self { target, connection: Mutex::new(None) }
    }

    async fn connect(&self) -> Result<Connection, RpcError> {
        let (url, token) = self.target.resolve()?;
        let client = BridgeClient::connect(&url, &token).await.map_err(client_error)?;
        Ok(Connection { client, last_used: Instant::now() })
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let mut slot = self.connection.lock().await;
        if let Some(c) = slot.as_mut()
            && c.last_used.elapsed() > STALE_AFTER
            && c.client.call_within("ping", json!({}), Duration::from_secs(3)).await.is_err()
        {
            *slot = None;
        }
        if slot.is_none() {
            *slot = Some(self.connect().await?);
        }
        let Some(conn) = slot.as_mut() else {
            return Err(RpcError::new(INTERNAL_ERROR, "no connection"));
        };
        let out = conn.client.call_within(method, params, CALL_LIMIT).await;
        conn.last_used = Instant::now();
        if matches!(out, Err(ClientError::Closed | ClientError::Protocol(_))) {
            *slot = None;
        }
        out.map_err(client_error)
    }

    async fn status(&self) -> Value {
        let mut slot = self.connection.lock().await;
        if slot.is_none() {
            match self.connect().await {
                Ok(c) => *slot = Some(c),
                Err(e) => return json!({ "mode": "attached", "connected": false, "problem": e.message }),
            }
        }
        let hello = slot.as_ref().map(|c| c.client.hello().clone());
        json!({ "mode": "attached", "connected": true, "app": hello })
    }
}

/// The three backends.
pub enum Backend {
    /// Own session.
    Headless(Headless),
    /// Desktop app.
    Attached(Box<Attached>),
    /// Browser tab.
    Relay {
        /// The listening relay.
        relay: RelayServer,
        /// The page that the user opens to pair.
        page_url: String,
    },
}

impl Backend {
    /// A backend attached to the desktop app at `target`.
    pub fn attached(target: Target) -> Self {
        Self::Attached(Box::new(Attached::new(target)))
    }

    /// The mode.
    pub fn mode(&self) -> Mode {
        match self {
            Self::Headless(_) => Mode::Headless,
            Self::Attached(_) => Mode::Attached,
            Self::Relay { .. } => Mode::Relay,
        }
    }

    /// Runs one bridge method.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        match self {
            Self::Headless(h) => h.call(method, params).await,
            Self::Attached(a) => a.call(method, params).await,
            Self::Relay { relay, .. } => relay.call(method, params).await,
        }
    }

    /// The answer of the `status` tool.
    pub async fn status(&self) -> Value {
        match self {
            Self::Headless(h) => json!({
                "mode": "headless",
                "connected": true,
                "renderer": false,
                "files": h.scope,
                "note": "This server owns its own editor session, with no window. PNG export and screenshots need a renderer: use SVG here, or attach to the desktop app.",
            }),
            Self::Attached(a) => a.status().await,
            Self::Relay { relay, page_url } => {
                let st = relay.status();
                let mut v = json!({ "mode": "relay", "connected": st.connected, "paired": st.paired, "listening": relay.url(), "page": st.page });
                if !st.paired
                    && let Some(m) = v.as_object_mut()
                {
                    m.insert("pairing_link".to_owned(), json!(relay.pairing_url(page_url)));
                    m.insert("next_step".to_owned(), json!("Ask the user to open the pairing link in the browser tab with drafft.ink, then call `status` again."));
                }
                v
            }
        }
    }
}
