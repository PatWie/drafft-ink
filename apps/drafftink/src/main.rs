//! Drafft.ink for the desktop.
//!
//! ```text
//! drafftink [FILE] [--room ROOM] [--server URL] [--bridge [PORT]] [--perf-overlay] [--perf-dump FILE]
//! ```
//!
//! The bridge (an agent's door into the running app, `docs/agents.md`) is off unless `--bridge` is given.
//! It listens on 127.0.0.1 only and writes a token file that only this user can read.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use drafftink_app::native::{self, NativeOptions};
use drafftink_app::{App, AppConfig, StartupLink};
use drafftink_bridge::{BridgeConfig, BridgeError, BridgeServer};
use drafftink_engine::cmd::collab::RoomLink;
use drafftink_engine::ports::FileRef;
use drafftink_gpu::{Options, RedrawReason, ShellError};

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
    /// Show frame counts and cache numbers over the canvas.
    #[arg(long)]
    perf_overlay: bool,
    /// At exit, write the performance counters (PerfStats JSON) to this file.
    #[arg(long, value_name = "FILE")]
    perf_dump: Option<PathBuf>,
    /// Keep settings and documents in this folder instead of the user's data folder.
    #[arg(long, value_name = "DIR")]
    data_dir: Option<PathBuf>,
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
    if args.perf_dump.is_some() || args.perf_overlay || drafftink_profile::enable_from_env("DRAFFTINK_PERF") {
        drafftink_profile::set_enabled(true);
    }
    let mut platform = native::platform(&NativeOptions { data_dir: args.data_dir.clone() });
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
        open: args.file.map(|p| FileRef::Path { path: p.display().to_string() }),
        perf_overlay: args.perf_overlay,
        stored_default_target: false,
    };
    let result = drafftink_gpu::run(App::new(platform, config), Options::default());
    // The idle checks read this line.
    eprintln!("frames={} vello_runs={}", drafftink_gpu::frames_drawn(), drafftink_gpu::vello_runs());
    drop(bridge);
    if let Some(path) = args.perf_dump {
        let json = serde_json::to_string_pretty(&drafftink_profile::snapshot()).unwrap_or_default();
        std::fs::write(&path, json).map_err(|source| Failure::PerfDump { path, source })?;
    }
    Ok(result?)
}
