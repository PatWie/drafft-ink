#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// Real sockets, real threads and the real clock; natively only.
#![allow(clippy::disallowed_methods)]
//! Two engine sessions against the real `drafftink-server` binary, through the native WebSocket transport.
//!
//! The server is a child process on an ephemeral port (built by the test with `cargo build -p drafftink-server`).
//! This test is the only place where the engine's `Net` port meets a real socket.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use drafftink_collab::{NativeTransport, Transport, WakeFn};
use drafftink_engine::ports::{Clock, Net, PortError, Ports};
use drafftink_engine::services::Services;
use drafftink_engine::{Origin, Session};
use drafftink_scene::{
    Element, ElementId, ElementKind, Rng64, SceneRead, SceneTx, SeqIds, SplitMix64, StoreError, Transform,
};
use serde_json::json;

fn server_binary() -> &'static PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let exe = std::env::current_exe().expect("test exe path");
        let profile_dir = exe.parent().and_then(|p| p.parent()).expect("target/<profile>").to_path_buf();
        let profile = profile_dir.file_name().and_then(|n| n.to_str()).unwrap_or("debug").to_owned();
        let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
        cmd.args(["build", "--quiet", "-p", "drafftink-server"]).current_dir(env!("CARGO_MANIFEST_DIR"));
        match profile.as_str() {
            "debug" => {}
            "release" => {
                cmd.arg("--release");
            }
            other => {
                cmd.args(["--profile", other]);
            }
        }
        assert!(cmd.status().expect("run cargo build").success(), "cargo build -p drafftink-server failed");
        let bin = profile_dir.join("drafftink-server");
        assert!(bin.exists(), "server binary not found at {}", bin.display());
        bin
    })
}

struct Server {
    child: Child,
    url: String,
}

impl Server {
    fn start() -> Self {
        let mut child = Command::new(server_binary())
            .args(["--bind", "127.0.0.1:0", "--storage", "memory", "--flush-interval-ms", "0"])
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
        let addr = rx.recv_timeout(Duration::from_secs(30)).expect("the server prints its address");
        Self { child, url: format!("ws://{addr}/ws") }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Wakes the test thread from any transport thread.
#[derive(Default)]
struct Waker {
    flag: Mutex<bool>,
    cv: Condvar,
}

impl Waker {
    fn wake(&self) {
        *self.flag.lock().unwrap() = true;
        self.cv.notify_all();
    }

    fn wait(&self, max: Duration) {
        let mut flag = self.flag.lock().unwrap();
        if !*flag {
            flag = self.cv.wait_timeout(flag, max).unwrap().0;
        }
        *flag = false;
    }
}

/// The shell's side of the `Net` port: a native WebSocket per connection, woken through the shared `Waker`.
struct NativeNet {
    waker: Arc<Waker>,
    rng: SplitMix64,
}

impl Net for NativeNet {
    fn open_transport(&mut self) -> Result<Box<dyn Transport>, PortError> {
        let w = Arc::clone(&self.waker);
        let wake: WakeFn = Arc::new(move || w.wake());
        Ok(Box::new(NativeTransport::new(wake)))
    }

    fn random(&mut self) -> u128 {
        (u128::from(self.rng.next_u64()) << 64) | u128::from(self.rng.next_u64())
    }
}

struct RealClock(Instant);

impl Clock for RealClock {
    fn now_ms(&self) -> u64 {
        1_730_000_000_000 + self.mono_ms()
    }

    fn mono_ms(&self) -> u64 {
        u64::try_from(self.0.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

fn session(waker: &Arc<Waker>, seed: u64) -> Session {
    let ports = Ports::new(Box::new(RealClock(Instant::now())), Box::new(SeqIds::new()))
        .with_net(Box::new(NativeNet { waker: Arc::clone(waker), rng: SplitMix64::new(seed) }));
    Session::with_services(ports, Services::fake())
}

fn add_rect(s: &mut Session, x: f64) -> ElementId {
    let id = s.ports.ids.element_id();
    let ret = id.clone();
    s.tab_mut()
        .doc
        .edit::<(), StoreError>(|tx| {
            let z = tx.next_top_index().map_err(|e| StoreError::backend(e))?;
            let mut e = Element::new(id, ElementKind::Rectangle, z);
            e.transform = Transform::new(x, 0.0, 10.0, 10.0);
            tx.insert(e)
        })
        .unwrap();
    s.tab_mut().doc.commit_untracked().unwrap();
    ret
}

/// Polls both sessions, sleeping on the wake-up hook between rounds, until `cond` holds.
fn wait_until(what: &str, waker: &Waker, sessions: &mut [&mut Session], cond: impl Fn(&[&mut Session]) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let mut wake_in = 200;
        for s in sessions.iter_mut() {
            let p = s.collab_poll();
            wake_in = wake_in.min(p.wake_in_ms.unwrap_or(200).max(1));
        }
        if cond(sessions) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        waker.wait(Duration::from_millis(wake_in.min(200)));
    }
}

fn fill(s: &Session, id: &ElementId) -> Option<String> {
    s.element(id).and_then(|e| e.style.fill_color).map(|c| c.to_hex())
}

#[test]
fn two_sessions_edit_undo_and_see_each_other_through_the_real_server() {
    let server = Server::start();
    let waker = Arc::new(Waker::default());
    let (mut a, mut b) = (session(&waker, 1), session(&waker, 2));
    for _ in 0..50 {
        b.ports.ids.element_id();
    }
    for s in [&mut a, &mut b] {
        s.execute("collab.connect", json!({"url": server.url, "room": "engine-it"})).unwrap();
    }
    a.execute("collab.set_user", json!({"name": "Ana"})).unwrap();
    wait_until("both online", &waker, &mut [&mut a, &mut b], |s| {
        s.iter().all(|x| x.collab_view().state == drafftink_engine::cmd::collab::ConnectionState::Connected)
    });

    // An edit made with a command reaches the other session.
    let r = add_rect(&mut a, 0.0);
    wait_until("the element on B", &waker, &mut [&mut a, &mut b], |s| s[1].element(&r).is_some());
    a.execute("style.set", json!({"ids": [r.as_str()], "props": {"fill_color": "#ff0000"}})).unwrap();
    wait_until("the fill on B", &waker, &mut [&mut a, &mut b], |s| fill(s[1], &r).as_deref() == Some("#ff0000"));

    // Undo from the keyboard is sent.
    a.execute_from(Origin::Shortcut, "history.undo", json!({})).unwrap();
    wait_until("the undo on B", &waker, &mut [&mut a, &mut b], |s| fill(s[1], &r).is_none());

    // Presence: B sees A by name.
    wait_until("A's name on B", &waker, &mut [&mut a, &mut b], |s| {
        s[1].collab_view().peers.iter().any(|p| p.name == "Ana")
    });

    // Leaving: A goes, B stays.
    a.execute("collab.leave_room", json!({})).unwrap();
    wait_until("B alone", &waker, &mut [&mut a, &mut b], |s| s[1].collab_view().peers.is_empty());
}
