//! The desktop app under test.

use std::fs::{self, File};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use drafftink_bridge::client::{BridgeClient, ClientError};
use drafftink_bridge::files;
use drafftink_bridge::rpc::REFUSED;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::runtime::Runtime;

use crate::error::E2eError;
use crate::image::Screenshot;

/// How long the app gets to open its window and its bridge.
const START_LIMIT: Duration = Duration::from_secs(90);
/// How long one call may take.
const CALL_LIMIT: Duration = Duration::from_secs(60);

/// True when `DRAFFTINK_REQUIRE_GPU` is set: a missing display or GPU fails instead of skipping.
pub fn require_gpu() -> bool {
    std::env::var("DRAFFTINK_REQUIRE_GPU").is_ok_and(|v| !v.is_empty() && v != "0")
}

/// The result of trying to start the app.
#[derive(Debug)]
pub enum Launch {
    /// The app runs.
    Running(Box<NativeApp>),
    /// This machine cannot run it; the text says why. Tests print it and pass, unless a GPU is required.
    Skipped(String),
}

/// A refusal as the bridge sends it (`data` of a `-32010` error): the whole refusal.
#[derive(Debug, Clone)]
pub struct Refusal {
    /// The stable code, such as `locked`.
    pub code: String,
    /// The plain-words message.
    pub message: String,
    /// The whole refusal object.
    pub data: Value,
}

/// A running `drafftink --mcp-http`: the app with MCP over streamable HTTP as its only door.
///
/// A test connects an MCP client to [`McpHttpApp::url`] with [`McpHttpApp::token`] as the bearer token.
pub struct McpHttpApp {
    child: Child,
    url: String,
    token: String,
    bridge_dir: TempDir,
    _data_dir: TempDir,
    log: PathBuf,
}

impl std::fmt::Debug for McpHttpApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpHttpApp").field("pid", &self.child.id()).field("url", &self.url).finish_non_exhaustive()
    }
}

impl McpHttpApp {
    /// Starts the app with `--mcp-http 0` (a free port) and no bridge, or says why not (see [`start_or_skip`]).
    pub fn launch() -> Result<Result<Self, String>, E2eError> {
        Ok(spawn(&["--mcp-http", "0"], Some("mcp-http"))?.map(|s| Self {
            child: s.child,
            url: format!("http://127.0.0.1:{}/mcp", s.found.port),
            token: s.found.token,
            bridge_dir: s.bridge_dir,
            _data_dir: s.data_dir,
            log: s.log,
        }))
    }

    /// `http://127.0.0.1:<port>/mcp`.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The bearer token, read from the private token file as a user would.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// The runtime folder of this app. The bridge's token files would be in it; there are none.
    pub fn bridge_dir(&self) -> &Path {
        self.bridge_dir.path()
    }

    /// The app's stderr so far (the tail), for failure messages.
    pub fn log(&self) -> String {
        tail(&self.log)
    }
}

impl Drop for McpHttpApp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Starts the app for the test `test` with MCP over HTTP, or skips like [`start_or_skip`].
pub fn start_mcp_http_or_skip(test: &str) -> Result<Option<McpHttpApp>, E2eError> {
    if std::env::var_os("DRAFFTINK_E2E").is_none_or(|v| v.is_empty()) {
        println!(
            "E2E-SKIP {test}: not run here; `cargo xtask e2e` runs the native suite (it needs a display and a GPU)"
        );
        return Ok(None);
    }
    match McpHttpApp::launch()? {
        Ok(app) => Ok(Some(app)),
        Err(reason) if require_gpu() => Err(E2eError::Required(reason)),
        Err(reason) => {
            println!("E2E-SKIP {test}: {reason}");
            Ok(None)
        }
    }
}

/// A running `drafftink --bridge` and a connected client.
pub struct NativeApp {
    child: Child,
    client: Option<BridgeClient>,
    runtime: Runtime,
    // Kept for the lifetime of the app: the token directory and the data directory.
    _bridge_dir: TempDir,
    _data_dir: TempDir,
    log: PathBuf,
}

impl std::fmt::Debug for NativeApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeApp").field("pid", &self.child.id()).finish_non_exhaustive()
    }
}

/// Where the app binary is: `DRAFFTINK_BIN`, else the release build, else the debug build.
fn binary() -> Result<PathBuf, E2eError> {
    if let Some(path) = std::env::var_os("DRAFFTINK_BIN").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from);
    let candidates = [target.join("release/drafftink"), target.join("debug/drafftink")];
    candidates.iter().find(|p| p.exists()).cloned().ok_or_else(|| E2eError::NoBinary(candidates[0].clone()))
}

fn tail(path: &Path) -> String {
    let mut text = String::new();
    if let Ok(mut file) = File::open(path) {
        let _ = file.read_to_string(&mut text);
    }
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(40)..].join("\n")
}

/// Words in the app's log that mean "this machine has no usable GPU or display".
fn looks_like_no_gpu(log: &str) -> bool {
    let lower = log.to_lowercase();
    ["no suitable", "no adapter", "no gpu", "cannot open display", "no display", "failed to open"]
        .iter()
        .any(|needle| lower.contains(needle))
}

/// Starts the app for the test `test`, or says why not. `Ok(None)` is a skip: the reason is printed as an
/// `E2E-SKIP` line (the `cargo xtask e2e` summary counts those) and the test returns. With
/// `DRAFFTINK_REQUIRE_GPU=1` a skip is an error.
pub fn start_or_skip(test: &str) -> Result<Option<NativeApp>, E2eError> {
    // `cargo xtask test` runs every test of the workspace on a machine that may have a display. The native
    // suite opens windows, so it runs only under `cargo xtask e2e`, which sets this variable.
    if std::env::var_os("DRAFFTINK_E2E").is_none_or(|v| v.is_empty()) {
        println!(
            "E2E-SKIP {test}: not run here; `cargo xtask e2e` runs the native suite (it needs a display and a GPU)"
        );
        return Ok(None);
    }
    match NativeApp::launch()? {
        Launch::Running(app) => Ok(Some(*app)),
        Launch::Skipped(reason) if require_gpu() => Err(E2eError::Required(reason)),
        Launch::Skipped(reason) => {
            println!("E2E-SKIP {test}: {reason}");
            Ok(None)
        }
    }
}

/// A started app process, its private directories and where it listens.
struct Spawned {
    child: Child,
    bridge_dir: TempDir,
    data_dir: TempDir,
    log: PathBuf,
    found: files::Found,
}

/// True when a display is named in the environment.
fn has_display() -> bool {
    std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty())
        || std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
}

const NO_DISPLAY: &str = "no display: DISPLAY and WAYLAND_DISPLAY are unset. Run under Xvfb, or use `cargo xtask e2e`";

/// Starts the app with `flags` and waits for the token file of the server they switch on, which is in
/// `token_subdir` of the runtime folder (`None`: the folder itself, the bridge's). `Err(reason)` is a skip.
fn spawn(flags: &[&str], token_subdir: Option<&str>) -> Result<Result<Spawned, String>, E2eError> {
    if !has_display() {
        return Ok(Err(NO_DISPLAY.into()));
    }
    let bin = binary()?;
    if !bin.exists() {
        return Err(E2eError::NoBinary(bin));
    }
    let bridge_dir = tempfile::tempdir()?;
    let data_dir = tempfile::tempdir()?;
    let log = data_dir.path().join("app.log");
    let stderr = File::create(&log)?;
    let mut child = Command::new(&bin)
        .args(flags)
        .arg("--data-dir")
        .arg(data_dir.path())
        .env("DRAFFTINK_BRIDGE_DIR", bridge_dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr)
        .spawn()
        .map_err(|source| E2eError::Spawn { path: bin.clone(), source })?;

    let token_dir = token_subdir.map_or_else(|| bridge_dir.path().to_owned(), |d| bridge_dir.path().join(d));
    let started = Instant::now();
    let found = loop {
        if let Some(status) = child.try_wait()? {
            let text = tail(&log);
            if looks_like_no_gpu(&text) {
                return Ok(Err(format!(
                    "the app could not open a window or a GPU adapter on this machine: {}",
                    text.lines().last().unwrap_or("")
                )));
            }
            return Err(E2eError::Exited { status: status.to_string(), log: text });
        }
        match files::find(&token_dir, None) {
            Ok(found) => break found,
            Err(files::FindError::Missing(_)) if started.elapsed() < START_LIMIT => {
                std::thread::sleep(Duration::from_millis(100))
            }
            Err(files::FindError::Missing(_)) => {
                let _ = child.kill();
                return Err(E2eError::NoBridge { seconds: START_LIMIT.as_secs(), log: tail(&log) });
            }
            Err(other) => {
                let _ = child.kill();
                return Err(other.into());
            }
        }
    };
    Ok(Ok(Spawned { child, bridge_dir, data_dir, log, found }))
}

impl NativeApp {
    /// Starts the app on the current display. A machine without a display or without a GPU gives
    /// [`Launch::Skipped`] with the reason.
    pub fn launch() -> Result<Launch, E2eError> {
        let spawned = match spawn(&["--bridge"], None)? {
            Ok(spawned) => spawned,
            Err(reason) => return Ok(Launch::Skipped(reason)),
        };
        let Spawned { child, bridge_dir, data_dir, log, found } = spawned;
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(E2eError::Runtime)?;
        let url = format!("ws://127.0.0.1:{}/", found.port);
        let client = runtime.block_on(BridgeClient::connect(&url, &found.token))?;
        let mut app = Self { child, client: Some(client), runtime, _bridge_dir: bridge_dir, _data_dir: data_dir, log };
        app.wait_for_document()?;
        Ok(Launch::Running(Box::new(app)))
    }

    /// Waits until the shell has answered a request: the first frame is up and the session is live.
    fn wait_for_document(&mut self) -> Result<(), E2eError> {
        let started = Instant::now();
        loop {
            match self.call("state", json!({})) {
                Ok(_) => return Ok(()),
                Err(E2eError::Client(ClientError::Timeout(_))) if started.elapsed() < START_LIMIT => {}
                Err(e) => return Err(e),
            }
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, E2eError> {
        let client = self.client.as_mut().ok_or_else(|| E2eError::Shape("the client is closed".into()))?;
        Ok(self.runtime.block_on(client.call_within(method, params, CALL_LIMIT))?)
    }

    /// Runs one command. A refusal is `Ok(Err(refusal))`; a broken bridge is `Err`.
    pub fn try_exec(&mut self, id: &str, args: Value) -> Result<Result<Value, Refusal>, E2eError> {
        match self.call("execute", json!({ "id": id, "args": args })) {
            Ok(v) => Ok(Ok(v)),
            Err(E2eError::Client(ClientError::Rpc(e))) if e.code == REFUSED => {
                let data = e.data.unwrap_or(Value::Null);
                let code = data.get("code").and_then(Value::as_str).unwrap_or_default().to_owned();
                Ok(Err(Refusal { code, message: e.message, data }))
            }
            Err(other) => Err(other),
        }
    }

    /// Runs one command that must succeed.
    pub fn exec(&mut self, id: &str, args: Value) -> Result<Value, E2eError> {
        match self.try_exec(id, args)? {
            Ok(v) => Ok(v),
            Err(r) => Err(E2eError::Shape(format!("`{id}` was refused ({}): {}", r.code, r.message))),
        }
    }

    /// Runs one command that must be refused, and returns the refusal.
    pub fn refusal(&mut self, id: &str, args: Value) -> Result<Refusal, E2eError> {
        match self.try_exec(id, args)? {
            Ok(v) => Err(E2eError::Shape(format!("`{id}` should have been refused but it ran: {v}"))),
            Err(r) => Ok(r),
        }
    }

    /// Runs commands as one undo step.
    pub fn batch(&mut self, steps: Value, opts: Value) -> Result<Value, E2eError> {
        self.call("batch", json!({ "steps": steps, "opts": opts }))
    }

    /// The `recent` ring, newest last.
    pub fn recent(&mut self, limit: u32) -> Result<Vec<Value>, E2eError> {
        let v = self.call("recent", json!({ "limit": limit }))?;
        Ok(v.get("entries").and_then(Value::as_array).cloned().unwrap_or_default())
    }

    /// The command ids of the registry, as the app lists them.
    pub fn command_ids(&mut self) -> Result<Vec<String>, E2eError> {
        let v = self.call("commands", json!({}))?;
        let list = v
            .get("commands")
            .and_then(Value::as_array)
            .ok_or_else(|| E2eError::Shape("commands has no list".into()))?;
        Ok(list.iter().filter_map(|c| c.get("id").and_then(Value::as_str).map(str::to_owned)).collect())
    }

    /// A screenshot of the viewport, rendered by the app's renderer (`view.screenshot`).
    pub fn screenshot(&mut self) -> Result<Screenshot, E2eError> {
        let out = self.exec("view.screenshot", json!({ "region": "viewport" }))?;
        let base64 = out
            .pointer("/bytes/base64")
            .and_then(Value::as_str)
            .ok_or_else(|| E2eError::Shape("view.screenshot has no bytes".into()))?;
        Screenshot::from_base64(base64)
    }

    /// Decodes the screenshot in the result of a `view.screenshot` call.
    pub fn screenshot_of(&self, result: &Value) -> Result<Screenshot, E2eError> {
        let base64 = result
            .pointer("/bytes/base64")
            .and_then(Value::as_str)
            .ok_or_else(|| E2eError::Shape("view.screenshot has no bytes".into()))?;
        Screenshot::from_base64(base64)
    }

    /// The app's stderr so far (the tail), for failure messages.
    pub fn log(&self) -> String {
        tail(&self.log)
    }

    /// A bridge client that says `hello` with this token, for the security checks.
    pub fn bridge_url(&self) -> Result<String, E2eError> {
        let found = files::find(self._bridge_dir.path(), None)?;
        Ok(format!("ws://127.0.0.1:{}/", found.port))
    }

    /// Tries to connect with a wrong token and returns what the bridge says.
    pub fn connect_with_token(&self, token: &str) -> Result<ClientError, E2eError> {
        let url = self.bridge_url()?;
        match self.runtime.block_on(BridgeClient::connect(&url, token)) {
            Ok(_) => Err(E2eError::Shape("the bridge accepted a wrong token".into())),
            Err(e) => Ok(e),
        }
    }

    /// The directory of the token files (private to this app).
    pub fn bridge_dir(&self) -> &Path {
        self._bridge_dir.path()
    }

    /// Stops the app and removes its directories.
    pub fn close(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.client = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// A fixture file of the repository, by its path under `tests/fixtures`.
    pub fn fixture(relative: &str) -> PathBuf {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures").join(relative);
        fs::canonicalize(&path).unwrap_or(path)
    }
}

impl Drop for NativeApp {
    fn drop(&mut self) {
        self.shutdown();
    }
}
