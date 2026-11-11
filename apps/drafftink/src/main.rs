//! Drafft.ink for the desktop.
//!
//! ```text
//! drafftink [FILE] [--room ROOM] [--server URL] [--token-file FILE] [--bridge [PORT]] [--perf-overlay] [--perf-dump FILE]
//!           [--perf-scenario KEY --perf-out FILE [--perf-secs N]]
//! ```
//!
//! A collaboration server that asks for an access token gets it from `--token-file` or, without that flag, from the
//! environment variable `DRAFFTINK_COLLAB_TOKEN`. There is no flag that takes the token itself: arguments are
//! visible to every user of the machine. The token is never printed or logged.
//!
//! The bridge (an agent's door into the running app, `docs/agents.md`) is off unless `--bridge` is given.
//! It listens on 127.0.0.1 only and writes a token file that only this user can read.
//!
//! `--perf-scenario` runs a performance scenario (`docs/performance.md` section 5.3): it shows a generated scene,
//! prints `PERF-READY` when the app is quiet, measures until a line arrives on stdin (or `--perf-secs` have passed),
//! writes the report to `--perf-out` and exits.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use drafftink_app::native::{self, NativeOptions, ScenarioRun};
use drafftink_app::{App, AppConfig, StartupLink, StartupToken};
use drafftink_bridge::{BridgeConfig, BridgeError, BridgeServer};
use drafftink_engine::cmd::collab::RoomLink;
use drafftink_engine::ports::FileRef;
use drafftink_gpu::{Options, RedrawReason, ShellError};
use drafftink_profile::ScenarioKey;

/// Drafft.ink: an infinite canvas whiteboard.
#[derive(Debug, Parser)]
#[command(name = "drafftink", version)]
struct Args {
    /// A document to open (.drafftink, .json, .excalidraw, or a PNG with an embedded scene).
    file: Option<PathBuf>,
    /// Join this room at start (1 to 64 letters, digits, `-` and `_`).
    #[arg(long)]
    room: Option<String>,
    /// The collaboration server, such as `ws://localhost:3030/ws`. Default: the `collab.server_url` setting.
    #[arg(long, requires = "room")]
    server: Option<String>,
    /// A file that holds the access token of the collaboration server. Without it the environment variable
    /// `DRAFFTINK_COLLAB_TOKEN` is used, if set. Only needed when the server asks for a token.
    #[arg(long, value_name = "FILE", requires = "room")]
    token_file: Option<PathBuf>,
    /// Let an AI agent drive this window. Listens on 127.0.0.1 (a free port, or PORT) and writes a token file.
    /// Off by default.
    #[arg(long, value_name = "PORT", num_args = 0..=1, default_missing_value = "0")]
    bridge: Option<u16>,
    /// Show frame counts and cache numbers over the canvas.
    #[arg(long)]
    perf_overlay: bool,
    /// At exit, write the performance counters (PerfStats JSON) to this file.
    #[arg(long, value_name = "FILE")]
    perf_dump: Option<PathBuf>,
    /// Run a performance scenario, such as `idle`, `pan@10k` or `hover@100k`, and exit (needs `--perf-out`).
    #[arg(long, value_name = "KEY", requires = "perf_out", value_parser = parse_key)]
    perf_scenario: Option<ScenarioKey>,
    /// Where `--perf-scenario` writes its report (JSON).
    #[arg(long, value_name = "FILE")]
    perf_out: Option<PathBuf>,
    /// Length of the measured window of `--perf-scenario` in seconds. Default: until a line arrives on stdin.
    #[arg(long, value_name = "SECONDS", requires = "perf_scenario")]
    perf_secs: Option<f64>,
    /// Keep settings and documents in this folder instead of the user's data folder.
    #[arg(long, value_name = "DIR")]
    data_dir: Option<PathBuf>,
}

/// The environment variable that holds the access token when no `--token-file` is given.
const TOKEN_ENV: &str = "DRAFFTINK_COLLAB_TOKEN";

/// The access token: the file if one is named, else the environment value. White space around it is ignored and
/// an empty token counts as none.
fn collab_token(file: Option<&Path>, env: Option<String>) -> Result<Option<StartupToken>, Failure> {
    match file {
        Some(path) => {
            let text =
                std::fs::read_to_string(path).map_err(|source| Failure::TokenFile { path: path.to_owned(), source })?;
            Ok(StartupToken::new(&text))
        }
        None => Ok(env.as_deref().and_then(StartupToken::new)),
    }
}

fn parse_key(text: &str) -> Result<ScenarioKey, String> {
    text.parse::<ScenarioKey>().map_err(|e| e.to_string())
}

/// Why the app could not run.
#[derive(Debug, thiserror::Error)]
enum Failure {
    #[error("{0}")]
    Shell(#[from] ShellError),
    #[error("the agent bridge could not start: {0}")]
    Bridge(#[from] BridgeError),
    #[error("could not read the access token from {path}: {source}")]
    TokenFile { path: PathBuf, source: std::io::Error },
    #[error("could not write the performance counters to {path}: {source}")]
    PerfDump { path: PathBuf, source: std::io::Error },
}

fn main() -> ExitCode {
    env_logger::init();
    match run(Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("drafftink: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<(), Failure> {
    if args.perf_dump.is_some()
        || args.perf_overlay
        || args.perf_scenario.is_some()
        || drafftink_profile::enable_from_env("DRAFFTINK_PERF")
    {
        drafftink_profile::set_enabled(true);
    }
    let mut platform = drafftink_profile::scope("startup_platform", || {
        native::platform(&NativeOptions { data_dir: args.data_dir.clone() })
    });
    // The server must outlive the window: dropping it closes the socket and removes the token file.
    let bridge = match args.bridge {
        Some(port) => {
            let config = BridgeConfig {
                addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
                ..BridgeConfig::default()
            };
            let (server, host) = BridgeServer::start(config, platform.wake.hook(RedrawReason::Load))?;
            platform.bridge = Some(host);
            match server.token_file() {
                Some(path) => eprintln!("bridge: {} (token file: {})", server.url(), path.display()),
                None => eprintln!("bridge: {} (no token file)", server.url()),
            }
            Some(server)
        }
        None => None,
    };
    let config = AppConfig {
        join: StartupLink { link: RoomLink { room: args.room, server: args.server } },
        default_server: None,
        collab_token: collab_token(args.token_file.as_deref(), std::env::var(TOKEN_ENV).ok())?,
        open: args.file.map(|p| FileRef::Path { path: p.display().to_string() }),
        perf_overlay: args.perf_overlay,
        stored_default_target: false,
    };
    let wake = platform.wake.clone();
    let mut app = drafftink_profile::scope("startup_app", || App::new(platform, config));
    if let (Some(key), Some(out)) = (args.perf_scenario, args.perf_out) {
        // The thread ends the app itself when the window is over.
        native::spawn_scenario(ScenarioRun { key, out, secs: args.perf_secs }, app.scene_loader(), wake);
    }
    let result = drafftink_gpu::run(app, Options::default());
    // The idle checks read this line.
    eprintln!("frames={} vello_runs={}", drafftink_gpu::frames_drawn(), drafftink_gpu::vello_runs());
    drop(bridge);
    if let Some(path) = args.perf_dump {
        let json = serde_json::to_string_pretty(&drafftink_profile::snapshot()).unwrap_or_default();
        std::fs::write(&path, json).map_err(|source| Failure::PerfDump { path, source })?;
    }
    Ok(result?)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn the_token_comes_from_the_file_first_then_the_environment() {
        let dir = std::env::temp_dir().join(format!("drafftink-token-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("token");
        std::fs::write(&file, "from-file\n").unwrap();
        let expected = StartupToken::new("from-file");
        assert_eq!(collab_token(Some(&file), Some("from-env".into())).unwrap(), expected);
        assert_eq!(collab_token(None, Some(" from-env ".into())).unwrap(), StartupToken::new("from-env"));
        assert_eq!(collab_token(None, None).unwrap(), None);
        assert_eq!(collab_token(None, Some("  ".into())).unwrap(), None, "an empty value is no token");
        std::fs::write(&file, "\n").unwrap();
        assert_eq!(collab_token(Some(&file), None).unwrap(), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_token_file_is_an_error_that_names_the_file_but_not_a_token() {
        let err = collab_token(Some(Path::new("/nonexistent/drafftink-token")), Some("env".into())).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("/nonexistent/drafftink-token") && !text.contains("env"), "{text}");
    }
}
