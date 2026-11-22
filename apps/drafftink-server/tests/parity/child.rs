//! Target `child`: the real `drafftink-headless serve` process, with the filesystem store.

use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

use super::relay::{Host, Lifetime, Relay, Traits};

/// How long the server may take to print the address it listens on.
const STARTUP: Duration = Duration::from_secs(120);

/// The `drafftink-headless` binary, built once per test process. It is a separate package at the app layer, so the
/// test runs its `serve` as a child process instead of linking it. The build keeps `hand-fonts`, as the tests of
/// `drafftink-collab` do: the program is the same file in `target/<profile>/` for every test that builds it.
fn server_binary() -> &'static PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let exe = std::env::current_exe().expect("test exe path");
        let profile_dir = exe.parent().and_then(|p| p.parent()).expect("target/<profile>").to_path_buf();
        let profile = profile_dir.file_name().and_then(|n| n.to_str()).unwrap_or("debug").to_owned();
        let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
        cmd.args(["build", "--quiet", "-p", "drafftink-headless", "--features", "hand-fonts"])
            .current_dir(env!("CARGO_MANIFEST_DIR"));
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

struct ChildServer {
    child: Option<Child>,
    _dir: tempfile::TempDir,
}

impl Host for ChildServer {
    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // Stopped like a crash, not like a goodbye: the store writes before every Ack, so nothing is lost.
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub fn start(life: Lifetime) -> Relay {
    let dir = tempfile::tempdir().expect("a temp dir");
    let mut child = Command::new(server_binary())
        .args(["serve", "--bind", "127.0.0.1:0", "--storage", "filesystem", "--flush-interval-ms", "0"])
        .arg("--data-dir")
        .arg(dir.path())
        .args(["--room-ttl", &format!("{}s", life.ttl().as_secs())])
        .args(["--room-grace", &format!("{}s", life.grace().as_secs())])
        .env("RUST_LOG", "info")
        .env_remove("DRAFFTINK_AUTH_TOKENS")
        .stdin(Stdio::null())
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
                let addr: String = rest.chars().take_while(|c| c.is_ascii_digit() || matches!(c, '.' | ':')).collect();
                let _ = tx.send(addr);
            }
        }
    });
    let stored = dir.path().to_path_buf();
    let mut host = ChildServer { child: Some(child), _dir: dir };
    let Ok(text) = rx.recv_timeout(STARTUP) else {
        host.stop();
        panic!("drafftink-headless did not print its address within {}s", STARTUP.as_secs());
    };
    let addr: SocketAddr = text.parse().expect("a socket address");
    Relay::new(
        "child",
        addr,
        Traits { deletion_slack: Duration::from_secs(1), room_deadline: true },
        life,
        &stored,
        Box::new(host),
    )
}
