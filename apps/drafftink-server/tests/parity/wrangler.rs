//! Target `wrangler`: the Worker under `wrangler dev`, the local runtime of Cloudflare (workerd). Nothing is deployed
//! and no Cloudflare login is used. It needs Node 22 and the built wasm of the Worker, so it runs only when
//! `DRAFFTINK_PARITY_WRANGLER` is set; otherwise the test prints a `SKIPPED:` line and passes.
//!
//! The harness is built to take more than the demo: [`Wrangler`] names the environment of the config (`demo` today),
//! extra `--var`s, and an optional stand-in in front of wrangler (the relay's address is then the stand-in's).

use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use super::common::http;
use super::relay::{Host, Lifetime, Relay, Traits};

/// The switch that turns the target on.
const SWITCH: &str = "DRAFFTINK_PARITY_WRANGLER";

/// How long wrangler may take to answer its first request. The first start compiles the bundle and starts workerd.
const STARTUP: Duration = Duration::from_secs(240);

/// How the Worker is started.
#[derive(Debug, Clone)]
pub struct Wrangler {
    /// The environment the top level of the generated config mirrors: `demo` or `private`.
    pub environment: &'static str,
    /// More `--var NAME:VALUE` settings, after the ones that set the lifetime of rooms.
    pub vars: Vec<(String, String)>,
}

impl Default for Wrangler {
    fn default() -> Self {
        Wrangler { environment: "demo", vars: Vec::new() }
    }
}

/// Starts the demo Worker, or prints the `SKIPPED:` line and returns `None`.
pub fn start(life: Lifetime) -> Option<Relay> {
    if !drafftink_profile::env_flag(SWITCH) {
        drafftink_profile::report_skip(&format!(
            "the wrangler target needs Node 22 and the built wasm; set {SWITCH}=1 to run it (apps/drafftink-worker/README.md, \"Parity suite\")"
        ));
        return None;
    }
    Some(start_with(&Wrangler::default(), life))
}

#[cfg(unix)]
fn start_with(spec: &Wrangler, life: Lifetime) -> Relay {
    unix::start(spec, life)
}

#[cfg(not(unix))]
fn start_with(_: &Wrangler, _: Lifetime) -> Relay {
    panic!("the wrangler target runs on Unix only (it stops wrangler's process group)")
}

/// The Worker's folder: `apps/drafftink-worker`.
fn worker_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../drafftink-worker")
}

/// The folder cargo builds into, found from the test executable (`target/<profile>/deps/<test>`).
fn target_dir() -> PathBuf {
    let exe = std::env::current_exe().expect("test exe path");
    exe.ancestors().nth(3).expect("target/<profile>/deps/<exe>").to_path_buf()
}

/// A TCP port that is free now.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).expect("a free port").port()
}

/// The name of the running test, made fit for a file name.
fn test_name() -> String {
    let thread = std::thread::current();
    let name = thread.name().unwrap_or("parity");
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '-' }).collect()
}

/// The arguments of `wrangler dev` for this run, after the binary name.
fn dev_args(config: &Path, port: u16, inspector: u16, persist: &Path, vars: &[(String, String)]) -> Vec<String> {
    let mut args: Vec<String> = ["dev", "--config"].map(String::from).to_vec();
    args.push(config.display().to_string());
    args.extend(["--ip", "127.0.0.1", "--port"].map(String::from));
    args.push(port.to_string());
    args.extend(["--inspector-port".to_owned(), inspector.to_string()]);
    args.extend(["--persist-to".to_owned(), persist.display().to_string()]);
    for (name, value) in vars {
        args.extend(["--var".to_owned(), format!("{name}:{value}")]);
    }
    args
}

/// The settings of a run: the lifetime of rooms first, then the extras of the spec (a later `--var` wins).
fn all_vars(spec: &Wrangler, life: Lifetime) -> Vec<(String, String)> {
    let mut vars = vec![
        ("ROOM_TTL_SECS".to_owned(), life.ttl().as_secs().to_string()),
        ("ROOM_LEAVE_GRACE_SECS".to_owned(), life.grace().as_secs().to_string()),
    ];
    vars.extend(spec.vars.iter().cloned());
    vars
}

/// The last lines of a log, for a failure message.
fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

#[cfg(unix)]
mod unix {
    use std::os::unix::process::CommandExt;

    use super::*;

    /// A running `wrangler dev` in its own process group, so that wrangler, its helper processes and workerd all stop
    /// together. Its output goes to a file that is printed when the test fails.
    struct Running {
        child: Option<Child>,
        group: u32,
        log: PathBuf,
        // The folders live as long as the relay, so that the canary can read the persisted state after the stop.
        _folders: Vec<tempfile::TempDir>,
    }

    impl Running {
        fn signal_group(&self, signal: &str) {
            // `kill -<signal> -- -<group>` signals the whole group; no libc, no unsafe.
            let _ = Command::new("kill")
                .args([&format!("-{signal}"), "--", &format!("-{}", self.group)])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }

        fn print_log(&self) {
            match std::fs::read_to_string(&self.log) {
                Ok(text) => eprintln!(
                    "---- wrangler output ({}), last 300 lines ----\n{}\n----",
                    self.log.display(),
                    tail(&text, 300)
                ),
                Err(e) => eprintln!("---- no wrangler output at {}: {e} ----", self.log.display()),
            }
        }
    }

    impl Host for Running {
        fn stop(&mut self) {
            let Some(mut child) = self.child.take() else { return };
            // Asks wrangler to stop, which stops workerd with it, and gives it time to close its files.
            self.signal_group("TERM");
            let deadline = Instant::now() + Duration::from_secs(15);
            while Instant::now() < deadline && matches!(child.try_wait(), Ok(None)) {
                std::thread::sleep(Duration::from_millis(50));
            }
            // Whatever is still in the group (workerd outlives a killed node) goes now.
            self.signal_group("KILL");
            let _ = child.wait();
            if std::thread::panicking() {
                self.print_log();
            }
        }
    }

    pub fn start(spec: &Wrangler, life: Lifetime) -> Relay {
        let app = worker_dir();
        let wrangler = app.join("node_modules/.bin/wrangler");
        assert!(
            wrangler.exists(),
            "wrangler is not installed in {}. Run `cd apps/drafftink-worker && npm ci` (Node 22 or newer)",
            app.display()
        );
        assert!(
            app.join("pkg/drafftink_worker_bg.wasm").exists(),
            "the Worker's wasm is not built. Run `cd apps/drafftink-worker && npm run build:wasm`"
        );

        let state = tempfile::tempdir().expect("a state folder");
        let assets = tempfile::tempdir().expect("an assets folder");
        std::fs::write(assets.path().join("index.html"), "<!doctype html><title>parity</title>\n")
            .expect("a stub page");
        let persist = state.path().join("persist");
        let config = state.path().join("wrangler.toml");
        let render = Command::new(app.join("scripts/render-config.sh"))
            .arg(spec.environment)
            .arg(&config)
            .env("ASSETS_DIR", assets.path())
            .output()
            .expect("run render-config.sh");
        assert!(
            render.status.success(),
            "render-config.sh failed: {}{}",
            String::from_utf8_lossy(&render.stdout),
            String::from_utf8_lossy(&render.stderr)
        );

        let logs = target_dir().join("parity-wrangler");
        std::fs::create_dir_all(&logs).expect("a log folder");
        let log = logs.join(format!("{}.log", test_name()));
        let out = std::fs::File::create(&log).expect("a log file");
        let port = free_port();
        let args = dev_args(&config, port, free_port(), &persist, &all_vars(spec, life));
        let child = Command::new(&wrangler)
            .args(&args)
            .current_dir(&app)
            .env("WRANGLER_SEND_METRICS", "false")
            .env("NO_COLOR", "1")
            .env("CI", "1")
            .stdin(Stdio::null())
            .stdout(out.try_clone().expect("a second handle"))
            .stderr(out)
            .process_group(0)
            .spawn()
            .unwrap_or_else(|e| panic!("cannot start {}: {e}. Is Node 22 on PATH?", wrangler.display()));
        let group = child.id();
        let mut running = Running { child: Some(child), group, log, _folders: vec![state, assets] };
        eprintln!("wrangler dev on port {port}, output in {}", running.log.display());

        let addr: SocketAddr = format!("127.0.0.1:{port}").parse().expect("an address");
        if let Err(why) = wait_ready(&mut running, addr) {
            running.print_log();
            running.stop();
            panic!("{why}");
        }
        Relay::new(
            "wrangler",
            addr,
            Traits { deletion_slack: Duration::from_secs(8) },
            life,
            &persist,
            Box::new(running),
        )
    }

    /// Waits until `/api/status` answers `ok`, or fails when wrangler exits or takes too long.
    fn wait_ready(running: &mut Running, addr: SocketAddr) -> Result<(), String> {
        let deadline = Instant::now() + STARTUP;
        while Instant::now() < deadline {
            if let Some(child) = running.child.as_mut()
                && let Ok(Some(status)) = child.try_wait()
            {
                return Err(format!("wrangler dev exited early with {status}; its output is above"));
            }
            if let Ok(reply) = http::try_request(addr, "GET", "/api/status", &[], &[])
                && reply.status == 200
                && String::from_utf8_lossy(&reply.body).contains("\"ok\"")
            {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Err(format!("wrangler dev did not answer /api/status within {}s; its output is above", STARTUP.as_secs()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lifetime_of_rooms_comes_before_the_extras() {
        let spec = Wrangler { environment: "demo", vars: vec![("MAX_IMAGE_BYTES".into(), "1000".into())] };
        let vars = all_vars(&spec, Lifetime::Short);
        assert_eq!(vars[0], ("ROOM_TTL_SECS".to_owned(), "3".to_owned()));
        assert_eq!(vars[1], ("ROOM_LEAVE_GRACE_SECS".to_owned(), "1".to_owned()));
        assert_eq!(vars[2].0, "MAX_IMAGE_BYTES");
    }

    #[test]
    fn the_arguments_name_the_config_the_port_and_the_state() {
        let args = dev_args(Path::new("/c/w.toml"), 8791, 9300, Path::new("/s"), &[("A".into(), "1".into())]);
        let line = args.join(" ");
        assert!(line.starts_with("dev --config /c/w.toml --ip 127.0.0.1 --port 8791"), "{line}");
        assert!(line.contains("--persist-to /s") && line.ends_with("--var A:1"), "{line}");
    }

    #[test]
    fn the_tail_keeps_the_last_lines() {
        assert_eq!(tail("a\nb\nc", 2), "b\nc");
        assert_eq!(tail("a", 5), "a");
        assert_eq!(tail("", 5), "");
    }
}
