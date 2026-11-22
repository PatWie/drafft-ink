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
    /// The environment of the generated config: `demo` or `private`.
    pub environment: &'static str,
    /// Run `wrangler dev --env <environment>` (the section of that environment) and not the top level of the config.
    pub named_environment: bool,
    /// Set `ROOM_TTL_SECS` and `ROOM_LEAVE_GRACE_SECS` from the lifetime of the test. Off where rooms never expire.
    pub lifetime_vars: bool,
    /// More `--var NAME:VALUE` settings, after the ones that set the lifetime of rooms.
    pub vars: Vec<(String, String)>,
    /// Put a stand-in for Cloudflare Access in front of wrangler, and require Access in the Worker.
    pub gate: bool,
}

impl Wrangler {
    /// The demo environment: the top level of the config, rooms with a lifetime, no Access.
    pub fn demo() -> Self {
        Wrangler { environment: "demo", named_environment: false, lifetime_vars: true, vars: Vec::new(), gate: false }
    }

    /// The private environment: `--env private`, R2, rooms that never expire, Access required and a stand-in for it.
    pub fn private() -> Self {
        Wrangler { environment: "private", named_environment: true, lifetime_vars: false, vars: Vec::new(), gate: true }
    }
}

/// Starts the demo Worker, or prints the `SKIPPED:` line and returns `None`.
pub fn start(life: Lifetime) -> Option<Relay> {
    enabled().then(|| start_with(&Wrangler::demo(), life))
}

/// Starts the private Worker behind the stand-in for Access, or prints the `SKIPPED:` line and returns `None`.
pub fn start_private() -> Option<Relay> {
    enabled().then(|| start_with(&Wrangler::private(), Lifetime::Normal))
}

/// Whether the switch is on. When it is off the skip is printed here.
fn enabled() -> bool {
    let on = drafftink_profile::env_flag(SWITCH);
    if !on {
        drafftink_profile::report_skip(&format!(
            "the wrangler target needs Node 22 and the built wasm; set {SWITCH}=1 to run it (apps/drafftink-worker/README.md, \"Parity suite\")"
        ));
    }
    on
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
fn dev_args(
    config: &Path,
    environment: Option<&str>,
    port: u16,
    inspector: u16,
    persist: &Path,
    vars: &[(String, String)],
) -> Vec<String> {
    let mut args: Vec<String> = ["dev", "--config"].map(String::from).to_vec();
    args.push(config.display().to_string());
    if let Some(name) = environment {
        args.extend(["--env".to_owned(), name.to_owned()]);
    }
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
    let mut vars = Vec::new();
    if spec.lifetime_vars {
        vars.push(("ROOM_TTL_SECS".to_owned(), life.ttl().as_secs().to_string()));
        vars.push(("ROOM_LEAVE_GRACE_SECS".to_owned(), life.grace().as_secs().to_string()));
    }
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

    use drafftink_collab::AccessCredentials;

    use super::super::stand_in::{self, StandIn};
    use super::*;

    /// How to ask a running wrangler whether it is ready: `/api/status` on this address with these headers (the token of
    /// the gate, when there is one).
    struct Probe {
        addr: SocketAddr,
        headers: Vec<(&'static str, String)>,
    }

    /// What it takes to start wrangler again exactly as it was started.
    struct Launch {
        program: PathBuf,
        args: Vec<String>,
        dir: PathBuf,
        log: PathBuf,
    }

    impl Launch {
        /// Starts wrangler in a process group of its own, so that wrangler, its helper processes and workerd all stop
        /// together. Its output is appended to the log file.
        fn spawn(&self) -> Child {
            let out = std::fs::OpenOptions::new().create(true).append(true).open(&self.log).expect("a log file");
            Command::new(&self.program)
                .args(&self.args)
                .current_dir(&self.dir)
                .env("WRANGLER_SEND_METRICS", "false")
                .env("NO_COLOR", "1")
                .env("CI", "1")
                .stdin(Stdio::null())
                .stdout(out.try_clone().expect("a second handle"))
                .stderr(out)
                .process_group(0)
                .spawn()
                .unwrap_or_else(|e| panic!("cannot start {}: {e}. Is Node 22 on PATH?", self.program.display()))
        }
    }

    /// A running `wrangler dev`, and the stand-in for Access in front of it when there is one. The output goes to a file
    /// that is printed when the test fails.
    struct Running {
        child: Option<Child>,
        launch: Launch,
        probe: Probe,
        stand_in: Option<StandIn>,
        // The folders live as long as the relay, so that the canary can read the persisted state after the stop.
        _folders: Vec<tempfile::TempDir>,
    }

    impl Running {
        fn signal_group(&self, signal: &str) {
            let Some(child) = &self.child else { return };
            // `kill -<signal> -- -<group>` signals the whole group; no libc, no unsafe.
            let _ = Command::new("kill")
                .args([&format!("-{signal}"), "--", &format!("-{}", child.id())])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }

        fn print_log(&self) {
            let log = &self.launch.log;
            match std::fs::read_to_string(log) {
                Ok(text) => eprintln!(
                    "---- wrangler output ({}), last 300 lines ----\n{}\n----",
                    log.display(),
                    tail(&text, 300)
                ),
                Err(e) => eprintln!("---- no wrangler output at {}: {e} ----", log.display()),
            }
        }

        /// Stops wrangler and everything in its process group. The stand-in keeps running.
        fn stop_wrangler(&mut self) {
            // Asks wrangler to stop, which stops workerd with it, and gives it time to close its files.
            self.signal_group("TERM");
            if let Some(mut child) = self.child.take() {
                let deadline = Instant::now() + Duration::from_secs(15);
                while Instant::now() < deadline && matches!(child.try_wait(), Ok(None)) {
                    std::thread::sleep(Duration::from_millis(50));
                }
                // Whatever is still in the group (workerd outlives a killed node) goes now.
                let _ = Command::new("kill")
                    .args(["-KILL", "--", &format!("-{}", child.id())])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                let _ = child.wait();
            }
        }

        /// Waits until `/api/status` answers `ok` (through the gate, when there is one), or fails when wrangler exits
        /// or takes too long.
        fn wait_ready(&mut self) -> Result<(), String> {
            let deadline = Instant::now() + STARTUP;
            while Instant::now() < deadline {
                if let Some(child) = self.child.as_mut()
                    && let Ok(Some(status)) = child.try_wait()
                {
                    return Err(format!("wrangler dev exited early with {status}; its output is above"));
                }
                let reply = http::try_request(self.probe.addr, "GET", "/api/status", &self.probe.headers, &[]);
                if let Ok(reply) = reply
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

    impl Host for Running {
        fn stop(&mut self) {
            if self.child.is_none() && self.stand_in.is_none() {
                return;
            }
            self.stop_wrangler();
            if let Some(stand_in) = self.stand_in.as_mut() {
                stand_in.stop();
            }
            self.stand_in = None;
            if std::thread::panicking() {
                self.print_log();
            }
        }

        fn restart(&mut self) -> Result<(), String> {
            self.stop_wrangler();
            self.child = Some(self.launch.spawn());
            self.wait_ready().inspect_err(|_| self.print_log())
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
        let port = free_port();
        let upstream: SocketAddr = format!("127.0.0.1:{port}").parse().expect("an address");

        // The gate comes first: the Worker is told where to fetch its key set.
        let stand_in = spec.gate.then(|| StandIn::start(upstream));
        let mut settings = Command::new(app.join("scripts/render-config.sh"));
        settings.arg(spec.environment).arg(&config).env("ASSETS_DIR", assets.path());
        if let Some(gate) = &stand_in {
            let prefix = spec.environment.to_ascii_uppercase();
            settings
                .env(format!("{prefix}_ACCESS_REQUIRED"), "true")
                .env(format!("{prefix}_ACCESS_TEAM_DOMAIN"), stand_in::TEAM)
                .env(format!("{prefix}_ACCESS_AUD"), stand_in::AUD)
                .env(format!("{prefix}_DOMAIN"), stand_in::APP_HOSTNAME)
                .env(format!("{prefix}_ACCESS_APP_HOSTNAME"), stand_in::APP_HOSTNAME)
                .env(format!("{prefix}_ACCESS_CERTS_URL"), &gate.certs_url);
        }
        let render = settings.output().expect("run render-config.sh");
        assert!(
            render.status.success(),
            "render-config.sh failed: {}{}",
            String::from_utf8_lossy(&render.stdout),
            String::from_utf8_lossy(&render.stderr)
        );

        let logs = target_dir().join("parity-wrangler");
        std::fs::create_dir_all(&logs).expect("a log folder");
        let log = logs.join(format!("{}.log", test_name()));
        let _ = std::fs::remove_file(&log);
        let environment = spec.named_environment.then_some(spec.environment);
        let args = dev_args(&config, environment, port, free_port(), &persist, &all_vars(spec, life));
        let launch = Launch { program: wrangler, args, dir: app, log };

        let token = stand_in
            .as_ref()
            .map(|_| AccessCredentials::new(stand_in::CLIENT_ID, stand_in::CLIENT_SECRET).expect("a valid token"));
        let front = stand_in.as_ref().map_or(upstream, |gate| gate.front);
        let gate_log = stand_in.as_ref().map(|gate| gate.log.clone());
        let headers = token
            .as_ref()
            .map(|t| {
                vec![
                    (drafftink_collab::CLIENT_ID_HEADER, t.client_id().to_owned()),
                    (drafftink_collab::CLIENT_SECRET_HEADER, t.expose_secret().to_owned()),
                ]
            })
            .unwrap_or_default();
        let mut running = Running {
            child: None,
            launch,
            probe: Probe { addr: front, headers },
            stand_in,
            _folders: vec![state, assets],
        };
        running.child = Some(running.launch.spawn());
        eprintln!("wrangler dev on port {port} (clients on {front}), output in {}", running.launch.log.display());

        if let Err(why) = running.wait_ready() {
            running.print_log();
            running.stop();
            panic!("{why}");
        }
        let traits = Traits { deletion_slack: Duration::from_secs(8), room_deadline: spec.lifetime_vars };
        let name = if spec.gate { "private" } else { "wrangler" };
        let relay = Relay::new(name, front, traits, life, &persist, Box::new(running));
        match (token, gate_log) {
            (Some(token), Some(log)) => relay.behind(token, log),
            _ => relay,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lifetime_of_rooms_comes_before_the_extras() {
        let spec = Wrangler { vars: vec![("MAX_IMAGE_BYTES".into(), "1000".into())], ..Wrangler::demo() };
        let vars = all_vars(&spec, Lifetime::Short);
        assert_eq!(vars[0], ("ROOM_TTL_SECS".to_owned(), "3".to_owned()));
        assert_eq!(vars[1], ("ROOM_LEAVE_GRACE_SECS".to_owned(), "1".to_owned()));
        assert_eq!(vars[2].0, "MAX_IMAGE_BYTES");
    }

    #[test]
    fn the_arguments_name_the_config_the_port_and_the_state() {
        let args = dev_args(Path::new("/c/w.toml"), None, 8791, 9300, Path::new("/s"), &[("A".into(), "1".into())]);
        let line = args.join(" ");
        assert!(line.starts_with("dev --config /c/w.toml --ip 127.0.0.1 --port 8791"), "{line}");
        assert!(line.contains("--persist-to /s") && line.ends_with("--var A:1"), "{line}");
        let private = dev_args(Path::new("/c/w.toml"), Some("private"), 1, 2, Path::new("/s"), &[]).join(" ");
        assert!(private.starts_with("dev --config /c/w.toml --env private --ip"), "{private}");
    }

    #[test]
    fn rooms_that_never_expire_get_no_lifetime_settings() {
        assert!(all_vars(&Wrangler::private(), Lifetime::Short).is_empty());
    }

    #[test]
    fn the_tail_keeps_the_last_lines() {
        assert_eq!(tail("a\nb\nc", 2), "b\nc");
        assert_eq!(tail("a", 5), "a");
        assert_eq!(tail("", 5), "");
    }
}
