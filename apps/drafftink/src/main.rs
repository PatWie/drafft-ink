//! Drafft.ink for the desktop.
//!
//! ```text
//! drafftink [FILE] [--room ROOM] [--server URL] [--bridge [PORT]] [--mcp-http [PORT]] [--perf-overlay] [--perf-dump FILE]
//!           [--perf-scenario KEY --perf-out FILE [--perf-secs N]]
//! ```
//!
//! The bridge (an agent's door into the running app, `docs/agents.md`) is off unless `--bridge` is given.
//! It listens on 127.0.0.1 only and writes a token file that only this user can read. `--mcp-http` serves MCP over
//! streamable HTTP on 127.0.0.1 for clients that speak HTTP, with the same rules and a bearer token. The
//! preferences have a switch for it as well. Both are off by default.
//!
//! `--perf-scenario` runs a performance scenario (`docs/performance.md` section 5.3): it shows a generated scene,
//! prints `PERF-READY` when the app is quiet, measures until a line arrives on stdin (or `--perf-secs` have passed),
//! writes the report to `--perf-out` and exits.

mod mcp_http;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use drafftink_app::native::{self, NativeOptions, ScenarioRun};
use drafftink_app::{App, AppConfig, StartupLink};
use drafftink_bridge::host::channel;
use drafftink_bridge::{BridgeConfig, BridgeError, BridgeServer};
use drafftink_engine::cmd::collab::RoomLink;
use drafftink_engine::ports::FileRef;
use drafftink_gpu::{Options, RedrawReason, ShellError};
use drafftink_profile::ScenarioKey;

use crate::mcp_http::McpHttp;

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
    /// Let an AI agent drive this window. Listens on 127.0.0.1 (a free port, or PORT) and writes a token file.
    /// Off by default.
    #[arg(long, value_name = "PORT", num_args = 0..=1, default_missing_value = "0")]
    bridge: Option<u16>,
    /// Serve MCP over streamable HTTP on 127.0.0.1 (PORT, default 3737) for this launch, so that MCP clients that
    /// speak HTTP connect without a helper. Needs the token that is written to a private file. Off by default.
    #[arg(long, value_name = "PORT", num_args = 0..=1, default_missing_value = "3737")]
    mcp_http: Option<u16>,
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
    // The servers must outlive the window: dropping one closes its socket and removes its token file.
    // Every transport puts its requests on the one queue that the app polls at the top of a frame.
    let (bridge, jobs, host) = match args.bridge {
        Some(port) => {
            let config = BridgeConfig {
                addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
                ..BridgeConfig::default()
            };
            let (server, host) = BridgeServer::start(config, platform.wake.hook(RedrawReason::Load))?;
            match server.token_file() {
                Some(path) => eprintln!("bridge: {} (token file: {})", server.url(), path.display()),
                None => eprintln!("bridge: {} (no token file)", server.url()),
            }
            let jobs = server.jobs();
            (Some(server), jobs, host)
        }
        None => {
            let (jobs, host) = channel(platform.wake.hook(RedrawReason::Load), None);
            (None, jobs, host)
        }
    };
    platform.bridge = Some(host);
    let mut mcp = McpHttp::new(jobs, args.mcp_http);
    if args.mcp_http.is_some() {
        mcp.start_announced()?;
    }
    platform.mcp_http = Some(Box::new(mcp));
    let config = AppConfig {
        join: StartupLink { link: RoomLink { room: args.room, server: args.server } },
        default_server: None,
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
