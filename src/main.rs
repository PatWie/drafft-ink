//! Drafft.ink for the desktop.
//!
//! ```text
//! drafftink [FILE] [--room ROOM] [--server URL] [--token-file FILE] [--bridge [PORT]] [--mcp-http [PORT]] [--perf-overlay] [--perf-dump FILE]
//!           [--perf-scenario KEY --perf-out FILE [--perf-secs N]]   (only in a build with `--features scenarios`)
//! drafftink convert|run|outline|inspect|commands|schemas ...      (the headless tools of `drafftink-cli`)
//! drafftink mcp [--attach | --relay | ...]                        (the MCP server of `drafftink-mcp`)
//! ```
//!
//! A subcommand never creates a window, a GPU device or a display connection (`tool.rs`, `docs/cli.md`). The first
//! word decides: a file named like a subcommand is given as `./run` or after `--`.
//!
//! A collaboration server that asks for an access token gets it from `--token-file` or, without that flag, from the
//! environment variable `DRAFFTINK_COLLAB_TOKEN`. There is no flag that takes the token itself: arguments are
//! visible to every user of the machine. The token is never printed or logged.
//!
//! The bridge (an agent's door into the running app, `docs/agents.md`) is off unless `--bridge` is given.
//! It listens on 127.0.0.1 only and writes a token file that only this user can read. `--mcp-http` serves MCP over
//! streamable HTTP on 127.0.0.1 for clients that speak HTTP, with the same rules and a bearer token. The
//! preferences have a switch for it as well. Both are off by default.
//!
//! `--perf-scenario` exists only in a build with `--features scenarios` (the perf gate builds one; releases do not).
//! It runs a performance scenario (`docs/performance.md` section 5.3): it shows a generated scene,
//! prints `PERF-READY` when the app is quiet, measures until a line arrives on stdin (or `--perf-secs` have passed),
//! writes the report to `--perf-out` and exits.

mod mcp_http;
mod tool;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
#[cfg(feature = "scenarios")]
use drafftink_app::native::ScenarioRun;
use drafftink_app::native::{self, NativeOptions};
use drafftink_app::{App, AppConfig, StartupLink, StartupToken};
use drafftink_bridge::host::channel;
use drafftink_bridge::{BridgeConfig, BridgeError, BridgeServer};
use drafftink_engine::cmd::collab::RoomLink;
use drafftink_engine::ports::FileRef;
use drafftink_gpu::{Options, RedrawReason, ShellError};
#[cfg(feature = "scenarios")]
use drafftink_profile::ScenarioKey;

use crate::mcp_http::McpHttp;
use crate::tool::Tool;

/// Drafft.ink: an infinite canvas whiteboard.
///
/// Without a subcommand it opens the window. A subcommand runs a headless tool and never opens a window; a file
/// named like a subcommand is opened as `drafftink ./run` or `drafftink -- run`.
#[derive(Debug, Parser)]
#[command(name = "drafftink", version, args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    tool: Option<Tool>,
    #[command(flatten)]
    window: WindowArgs,
}

/// What `drafftink` does for the command line it was given.
#[derive(Debug)]
enum Launch {
    /// Open the window.
    Window(Box<WindowArgs>),
    /// Run a headless tool, which needs no window, no GPU and no display.
    Tool(Tool),
}

impl From<Cli> for Launch {
    fn from(cli: Cli) -> Self {
        match cli.tool {
            Some(tool) => Self::Tool(tool),
            None => Self::Window(Box::new(cli.window)),
        }
    }
}

/// The arguments that open the window.
#[derive(Debug, clap::Args)]
struct WindowArgs {
    /// A document to open (.drafftink, .json, .excalidraw, or a PNG with an embedded scene). A file named like a
    /// subcommand (`run`, `mcp`, ...) is given as `./run` or after `--`.
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
    #[cfg(feature = "scenarios")]
    #[arg(long, value_name = "KEY", requires = "perf_out", value_parser = parse_key)]
    perf_scenario: Option<ScenarioKey>,
    /// Where `--perf-scenario` writes its report (JSON).
    #[cfg(feature = "scenarios")]
    #[arg(long, value_name = "FILE")]
    perf_out: Option<PathBuf>,
    /// Length of the measured window of `--perf-scenario` in seconds. Default: until a line arrives on stdin.
    #[cfg(feature = "scenarios")]
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

#[cfg(feature = "scenarios")]
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
    match Launch::from(Cli::parse()) {
        Launch::Tool(tool) => tool.run(),
        Launch::Window(args) => {
            env_logger::init();
            match run(*args) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("drafftink: {e}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

fn run(args: WindowArgs) -> Result<(), Failure> {
    #[cfg(feature = "scenarios")]
    let scenario = args.perf_scenario.is_some();
    #[cfg(not(feature = "scenarios"))]
    let scenario = false;
    if args.perf_dump.is_some() || args.perf_overlay || scenario || drafftink_profile::enable_from_env("DRAFFTINK_PERF")
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
        collab_token: collab_token(args.token_file.as_deref(), std::env::var(TOKEN_ENV).ok())?,
        open: args.file.map(|p| FileRef::Path { path: p.display().to_string() }),
        perf_overlay: args.perf_overlay,
        stored_default_target: false,
    };
    #[cfg(feature = "scenarios")]
    let wake = platform.wake.clone();
    #[cfg_attr(not(feature = "scenarios"), allow(unused_mut))]
    let mut app = drafftink_profile::scope("startup_app", || App::new(platform, config));
    #[cfg(feature = "scenarios")]
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
    use clap::CommandFactory;

    use super::*;

    fn launch(args: &[&str]) -> Result<Launch, clap::Error> {
        Cli::try_parse_from(std::iter::once("drafftink").chain(args.iter().copied())).map(Launch::from)
    }

    /// The file a window launch opens, or `None` when the launch is not a window or opens no file.
    fn opened(args: &[&str]) -> Option<PathBuf> {
        match launch(args) {
            Ok(Launch::Window(window)) => window.file,
            _ => None,
        }
    }

    #[test]
    fn the_command_line_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn no_arguments_and_the_window_flags_open_the_window() {
        for args in [&[][..], &["scene.drafftink"], &["--room", "r1"], &["scene.drafftink", "--bridge", "--mcp-http"]] {
            assert!(matches!(launch(args), Ok(Launch::Window(_))), "{args:?}");
        }
        assert_eq!(opened(&["scene.drafftink"]), Some(PathBuf::from("scene.drafftink")));
    }

    #[test]
    fn every_tool_is_dispatched_without_a_window() {
        for args in [
            &["convert", "a.drafftink", "b.excalidraw"][..],
            &["run", "script.jsonl"],
            &["outline", "a.drafftink"],
            &["inspect", "a.drafftink"],
            &["commands"],
            &["schemas"],
            &["mcp"],
            &["mcp", "--attach", "--port", "5"],
        ] {
            assert!(matches!(launch(args), Ok(Launch::Tool(_))), "{args:?}");
        }
    }

    #[test]
    fn a_file_named_like_a_subcommand_is_opened_as_a_path_or_after_two_dashes() {
        for name in ["run", "convert", "outline", "inspect", "commands", "schemas", "mcp", "help"] {
            assert!(!matches!(launch(&[name]), Ok(Launch::Window(_))), "a bare `{name}` is not a file");
            let dotted = format!("./{name}");
            assert_eq!(opened(&[&dotted]), Some(PathBuf::from(&dotted)), "{dotted}");
            assert_eq!(opened(&["--", name]), Some(PathBuf::from(name)), "-- {name}");
        }
    }

    #[test]
    fn window_flags_and_subcommands_do_not_mix() {
        for args in [&["run", "s.jsonl", "--room", "r"][..], &["--room", "r", "run", "s.jsonl"], &["mcp", "--bridge"]] {
            assert!(launch(args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn a_tool_that_is_given_a_wrong_argument_is_a_usage_error_not_a_window() {
        let err = launch(&["outline"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn help_lists_every_subcommand_on_one_line() {
        let help = Cli::command().render_help().to_string();
        for name in ["run", "convert", "outline", "inspect", "commands", "schemas", "mcp"] {
            let line = help.lines().find(|l| l.trim_start().starts_with(&format!("{name} "))).unwrap_or("");
            assert!(!line.is_empty() && line.len() < 120, "{name}: {line:?}");
        }
    }

    #[test]
    fn help_works_for_a_subcommand_of_either_crate() {
        for args in [&["help", "mcp"][..], &["help", "outline"], &["mcp", "--help"], &["outline", "--help"]] {
            let kind = launch(args).unwrap_err().kind();
            assert_eq!(kind, clap::error::ErrorKind::DisplayHelp, "{args:?}");
        }
    }

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
