#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// These tests run on real sockets with the real clock, natively only.
#![allow(clippy::disallowed_methods)]
//! Helpers for tests against the real `drafftink-headless serve` process: start and stop it, run clients on real threads and
//! sockets, and wait on their wake-ups instead of polling on a timer.

use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use drafftink_collab::*;
use drafftink_doc::LoroStore;
use drafftink_protocol::{ClientId, RoomId, UserInfo};
use drafftink_scene::*;

/// The `drafftink-headless` binary, built once per test process. It is a separate crate at the app layer, so the tests
/// run its `serve` as a child process instead of linking it.
fn server_binary() -> &'static PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let exe = std::env::current_exe().expect("test exe path");
        let profile_dir = exe.parent().and_then(|p| p.parent()).expect("target/<profile>").to_path_buf();
        let profile = profile_dir.file_name().and_then(|n| n.to_str()).unwrap_or("debug").to_owned();
        let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
        cmd.args(["build", "--quiet", "-p", "drafftink-headless"]).current_dir(env!("CARGO_MANIFEST_DIR"));
        match profile.as_str() {
            "debug" => {}
            "release" => {
                cmd.arg("--release");
            }
            other => {
                cmd.args(["--profile", other]);
            }
        }
        let status = cmd.status().expect("run cargo build");
        assert!(status.success(), "cargo build -p drafftink-headless failed");
        let bin = profile_dir.join(if cfg!(windows) { "drafftink-headless.exe" } else { "drafftink-headless" });
        assert!(bin.exists(), "server binary not found at {}", bin.display());
        bin
    })
}

pub struct Server {
    child: Option<Child>,
    pub addr: SocketAddr,
    pub data_dir: Option<tempfile::TempDir>,
    args: Vec<String>,
}

impl Server {
    /// A server on an ephemeral loopback port with in-memory rooms and a store that writes before every Ack.
    pub fn start(extra: &[&str]) -> Server {
        let mut args: Vec<String> =
            ["--storage", "memory", "--flush-interval-ms", "0", "--max-updates-per-sec", "10000"]
                .map(String::from)
                .to_vec();
        args.extend(extra.iter().map(|s| (*s).to_owned()));
        Self::spawn(args, None, None)
    }

    /// A server that keeps its rooms in a directory, so a restart on the same port finds them.
    pub fn start_persistent(extra: &[&str]) -> Server {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut args: Vec<String> =
            ["--storage", "filesystem", "--flush-interval-ms", "0", "--max-updates-per-sec", "10000"]
                .map(String::from)
                .to_vec();
        args.push("--data-dir".into());
        args.push(dir.path().display().to_string());
        args.extend(extra.iter().map(|s| (*s).to_owned()));
        Self::spawn(args, None, Some(dir))
    }

    fn spawn(args: Vec<String>, port: Option<u16>, data_dir: Option<tempfile::TempDir>) -> Server {
        let bind = format!("127.0.0.1:{}", port.unwrap_or(0));
        let mut child = Command::new(server_binary())
            .args(["serve", "--bind", &bind])
            .args(&args)
            .env("RUST_LOG", "info")
            .env_remove("DRAFFTINK_AUTH_TOKENS")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start the server");
        let stdout = child.stdout.take().expect("stdout");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(i) = line.find("listening on ") {
                    let rest = &line[i + "listening on ".len()..];
                    let addr: String =
                        rest.chars().take_while(|c| c.is_ascii_digit() || matches!(c, '.' | ':')).collect();
                    let _ = tx.send(addr);
                }
            }
        });
        let addr = rx.recv_timeout(PATIENCE).expect("the server prints its address");
        Server { child: Some(child), addr: addr.parse().expect("socket address"), data_dir, args }
    }

    pub fn url(&self) -> String {
        format!("ws://{}/ws", self.addr)
    }

    /// Kills the server without a goodbye, like a crash or a power cut.
    pub fn kill(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    /// Starts it again on the same port with the same arguments and data.
    pub fn restart(&mut self) {
        self.kill();
        let args = self.args.clone();
        let again = Self::spawn(args, Some(self.addr.port()), self.data_dir.take());
        *self = again;
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.kill();
    }
}

/// The longest any test waits for an event. Waits return as soon as the event happens; this bound only ends a test
/// that hangs, and it is generous so that a machine with a load average of 200 does not fail a correct test.
pub const PATIENCE: Duration = Duration::from_secs(300);

/// Wakes the test thread from any transport thread.
#[derive(Default)]
pub struct Waker {
    flag: Mutex<bool>,
    cv: Condvar,
}

impl Waker {
    pub fn wake(&self) {
        *self.flag.lock().unwrap() = true;
        self.cv.notify_all();
    }

    /// Sleeps until woken or until `max` passes.
    pub fn wait(&self, max: Duration) {
        let mut flag = self.flag.lock().unwrap();
        if !*flag {
            flag = self.cv.wait_timeout(flag, max).unwrap().0;
        }
        *flag = false;
    }
}

pub struct Peer {
    pub name: String,
    pub session: CollabSession<NativeTransport>,
    pub store: LoroStore,
    pub events: Vec<CollabEvent>,
    pub next_wake: Option<u64>,
    pub started: Instant,
    pub redraws: usize,
    /// Added to the real clock, to let a test run a client's time faster than the wall clock.
    pub skew_ms: u64,
}

pub struct PeerSpec<'a> {
    pub n: u8,
    pub url: &'a str,
    pub room: &'a str,
    pub token: Option<&'a str>,
    pub options: NativeOptions,
    pub replace_when_trimmed: bool,
}

impl<'a> PeerSpec<'a> {
    pub fn new(n: u8, url: &'a str, room: &'a str) -> Self {
        PeerSpec { n, url, room, token: None, options: NativeOptions::default(), replace_when_trimmed: false }
    }
}

pub fn make_store(n: u8) -> LoroStore {
    LoroStore::with_config(drafftink_doc::LoroConfig { peer_id: Some(u64::from(n) + 1000), ..Default::default() })
        .unwrap()
}

impl Peer {
    pub fn new(spec: PeerSpec<'_>, waker: &Arc<Waker>) -> Peer {
        let w = Arc::clone(waker);
        let wake: WakeFn = Arc::new(move || w.wake());
        let transport = NativeTransport::with_options(wake, spec.options);
        let cfg = CollabConfig {
            url: ServerUrl::parse(spec.url, UrlPolicy::default()).expect("url"),
            room: RoomId::new(spec.room).expect("room"),
            client_id: ClientId::from_bytes([spec.n; 16]),
            user: Some(UserInfo { name: format!("user{}", spec.n), color: 0x336699 }),
            auth_token: spec.token.map(|t| AuthToken::new(t).expect("token")),
            jitter_seed: u64::from(spec.n) * 7919,
            replace_when_trimmed: spec.replace_when_trimmed,
        };
        let mut client = CollabClient::new(cfg).expect("client");
        let started = Instant::now();
        client.start(0);
        Peer {
            name: format!("peer{}", spec.n),
            session: CollabSession::new(client, transport),
            store: make_store(spec.n),
            events: Vec::new(),
            next_wake: Some(0),
            started,
            redraws: 0,
            skew_ms: 0,
        }
    }

    pub fn now(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX).saturating_add(self.skew_ms)
    }

    pub fn pump(&mut self) {
        let now = self.now();
        let p = self.session.pump(now, &mut self.store);
        self.redraws += usize::from(p.redraw);
        self.events.extend(p.events);
        self.next_wake = p.next_wake_ms;
    }

    pub fn client(&self) -> &CollabClient {
        self.session.client()
    }

    pub fn state(&self) -> ConnState {
        self.client().status().state
    }

    /// One local commit, then tells the client.
    pub fn edit(&mut self, f: impl FnOnce(&mut drafftink_doc::LoroTx<'_>)) {
        let mut tx = self.store.begin();
        f(&mut tx);
        tx.commit().expect("commit");
        let now = self.now();
        self.session.client_mut().local_changed(now);
    }

    pub fn add(&mut self, id: &str) {
        self.edit(|tx| {
            tx.insert(super::rect(id)).expect("insert");
        });
    }

    pub fn ids(&self) -> Vec<String> {
        self.store.iter_all().map(|e| e.id.to_string()).collect()
    }

    pub fn remote_batches(&self) -> Vec<RemoteBatch> {
        super::Harness::remote_batches(&self.events)
    }
}

/// The scene without local-only version counters.
pub fn content(s: &LoroStore) -> Document {
    let mut d = snapshot(s);
    for e in &mut d.elements {
        e.version = 0;
    }
    d
}

pub struct World {
    pub peers: Vec<Peer>,
    pub waker: Arc<Waker>,
}

impl World {
    pub fn new() -> World {
        World { peers: Vec::new(), waker: Arc::new(Waker::default()) }
    }

    pub fn spawn(&mut self, spec: PeerSpec<'_>) -> usize {
        self.peers.push(Peer::new(spec, &self.waker));
        self.peers.len() - 1
    }

    pub fn pump_all(&mut self) {
        for p in &mut self.peers {
            p.pump();
        }
    }

    /// Pumps every peer, and sleeps on the wake-up hook between rounds, until `cond` holds. There is no tuned
    /// timeout: the wait ends when the event happens, and [`PATIENCE`] is only the bound that keeps a hung test from
    /// running forever (a test machine under heavy load needs many times the time of an idle one).
    pub fn wait_until(&mut self, what: &str, cond: impl Fn(&World) -> bool) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            self.pump_all();
            if cond(self) {
                return;
            }
            if Instant::now() > deadline {
                let report: Vec<String> = self
                    .peers
                    .iter()
                    .map(|p| {
                        format!(
                            "{}: {:?} {:?} ids={:?} stats={:?}",
                            p.name,
                            p.client().status().state,
                            p.client().status().text,
                            p.ids().len(),
                            p.client().stats()
                        )
                    })
                    .collect();
                panic!("timed out after {}s waiting for {what}:\n{}", PATIENCE.as_secs(), report.join("\n"));
            }
            // Wake for the next timer of any peer, but never sleep long: new events call the hook.
            let wait = self
                .peers
                .iter()
                .filter_map(|p| p.next_wake.map(|w| w.saturating_sub(p.now())))
                .min()
                .map_or(200, |ms| ms.clamp(1, 200));
            self.waker.wait(Duration::from_millis(wait));
        }
    }

    pub fn all_equal(&self) -> bool {
        let first = content(&self.peers[0].store);
        self.peers.iter().all(|p| content(&p.store) == first)
    }

    pub fn converged(&mut self) {
        self.wait_until("all documents to be equal and acknowledged", |w| {
            w.all_equal() && w.peers.iter().all(|p| p.state() == ConnState::Online && !p.client().status().unsynced)
        });
    }
}
