//! The command line: choose a mode, build its backend, serve MCP on stdio.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use drafftink_bridge::files;
use drafftink_bridge::mcp::handler::DrafftHandler;
use drafftink_bridge::relay::{HOSTED_ORIGIN, RelayConfig, RelayServer};
use drafftink_bridge::server::BridgeError;
use drafftink_bridge::tools::service::Service;
use rmcp::ServiceExt;

use crate::backend::{Backend, Headless, Target, TokenSource};
use crate::ports::{FileScope, headless_session};

/// The environment variable that carries the bridge token. Preferred over a flag: arguments are visible to
/// other users of the machine.
pub const TOKEN_ENV: &str = "DRAFFTINK_BRIDGE_TOKEN";

/// The options of the two modes that talk to a window: the running desktop app (`--attach`) and a browser tab
/// (`--relay`). They need no editor of their own, so `drafftink mcp` takes exactly these, and `drafftink-headless
/// mcp` takes them besides its own session.
#[derive(Debug, clap::Args)]
pub struct BridgeArgs {
    /// Attach to the running desktop app, so the user watches the agent draw. This is the default of `drafftink
    /// mcp`.
    #[arg(long, conflicts_with = "relay")]
    attach: bool,
    /// Listen on 127.0.0.1 for the drafft.ink browser tab, which connects out to this server.
    #[arg(long)]
    relay: bool,

    /// Attach: the app's bridge port (default: the one in `current.port`).
    #[arg(long, conflicts_with = "relay")]
    port: Option<u16>,
    /// Attach: the folder with the app's token files (default: the app's runtime folder).
    #[arg(long, conflicts_with = "relay")]
    bridge_dir: Option<PathBuf>,
    /// Attach: a ws://127.0.0.1:<port>/ URL instead of discovery. Needs --token-file or the token variable.
    #[arg(long, conflicts_with = "relay")]
    url: Option<String>,
    /// Attach: read the token from this file (read again on every reconnect).
    #[arg(long, conflicts_with = "relay")]
    token_file: Option<PathBuf>,

    /// Relay: the page that the pairing link opens.
    #[arg(long, default_value = HOSTED_ORIGIN, requires = "relay")]
    page_url: String,
    /// Relay: the port to listen on (default: a free one).
    #[arg(long, requires = "relay")]
    relay_port: Option<u16>,
    /// Relay: another page origin that may connect, for a self-hosted drafft.ink (repeatable).
    #[arg(long = "allow-origin", requires = "relay")]
    allow_origin: Vec<String>,
}

/// What `drafftink-headless mcp` accepts: the bridge modes, and without a mode flag a session of its own.
#[derive(Debug, clap::Args)]
// Without `--attach` there is no app to attach to, so an attach option alone is a mistake here. (`drafftink mcp`
// attaches by default and takes the same options with no flag.)
#[command(
    mut_arg("port", |a| a.requires("attach")),
    mut_arg("bridge_dir", |a| a.requires("attach")),
    mut_arg("url", |a| a.requires("attach")),
    mut_arg("token_file", |a| a.requires("attach"))
)]
pub struct Args {
    /// `--attach`, `--relay` and their options.
    #[command(flatten)]
    bridge: BridgeArgs,
    /// Headless: a folder that files may be read from and written to (repeatable). Default: the current folder.
    #[arg(long = "allow-dir")]
    allow_dir: Vec<PathBuf>,
    /// Headless: let files be read and written anywhere the user can.
    #[arg(long, conflicts_with = "allow_dir")]
    allow_any_path: bool,
}

/// Why the server could not start.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// The bridge or relay could not start.
    #[error(transparent)]
    Bridge(#[from] BridgeError),
    /// The arguments do not make sense together.
    #[error("{0}")]
    Usage(String),
    /// The session thread did not start.
    #[error("could not start the editor session: {0}")]
    Session(std::io::Error),
    /// The MCP transport failed.
    #[error("MCP transport: {0}")]
    Transport(String),
}

fn origin_of(page_url: &str) -> Option<String> {
    let (scheme, rest) = page_url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    Some(format!("{scheme}://{host}"))
}

/// Builds the backend of `drafftink-headless mcp`: the bridge when a mode flag is given, else a session of its own.
fn backend(args: &Args) -> Result<Backend, CliError> {
    if args.bridge.attach || args.bridge.relay {
        return bridge_backend(&args.bridge);
    }
    let scope = if args.allow_any_path {
        FileScope::Anywhere
    } else if args.allow_dir.is_empty() {
        let cwd = std::env::current_dir().map_err(|e| CliError::Usage(format!("no current folder: {e}")))?;
        FileScope::folders(&[cwd])
    } else {
        FileScope::folders(&args.allow_dir)
    };
    let shown = scope.describe();
    let headless = Headless::start(shown, move || headless_session(scope)).map_err(CliError::Session)?;
    Ok(Backend::Headless(headless))
}

/// Builds the backend of `drafftink mcp`: a browser tab with `--relay`, else the running desktop app.
fn bridge_backend(args: &BridgeArgs) -> Result<Backend, CliError> {
    if args.relay {
        let port = args.relay_port.unwrap_or(0);
        let mut allowed_origins = vec![HOSTED_ORIGIN.to_owned()];
        allowed_origins.extend(origin_of(&args.page_url));
        allowed_origins.extend(args.allow_origin.iter().cloned());
        let cfg = RelayConfig {
            addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
            allowed_origins,
            ..RelayConfig::default()
        };
        let relay = RelayServer::start(cfg)?;
        relay.announce_to_stderr(&args.page_url);
        return Ok(Backend::Relay { relay, page_url: args.page_url.clone() });
    }
    attach_backend(args)
}

fn attach_backend(args: &BridgeArgs) -> Result<Backend, CliError> {
    let env_token = std::env::var(TOKEN_ENV).ok().filter(|t| !t.is_empty());
    let token = match (&args.token_file, env_token) {
        (Some(f), _) => Some(TokenSource::File(f.clone())),
        (None, Some(t)) => Some(TokenSource::Value(t)),
        (None, None) => None,
    };
    let target = match (&args.url, token, args.port) {
        (Some(url), Some(token), _) => Target::Explicit { url: url.clone(), token },
        (None, Some(token), Some(port)) => Target::Explicit { url: format!("ws://127.0.0.1:{port}/"), token },
        (Some(_), None, _) => {
            return Err(CliError::Usage(format!("--url needs the token: pass --token-file, or set {TOKEN_ENV}")));
        }
        _ => {
            let dir =
                args.bridge_dir.clone().or_else(files::default_dir).ok_or_else(|| {
                    CliError::Usage("no folder for the app's token files: pass --bridge-dir".to_owned())
                })?;
            Target::Discover { dir, port: args.port }
        }
    };
    Ok(Backend::attached(target))
}

async fn serve(backend: Backend) -> Result<(), CliError> {
    let service = Arc::new(Service::new(backend));
    let handler = DrafftHandler::new(service);
    let running = handler.serve(rmcp::transport::stdio()).await.map_err(|e| CliError::Transport(e.to_string()))?;
    running.waiting().await.map_err(|e| CliError::Transport(e.to_string()))?;
    Ok(())
}

fn run(backend: Result<Backend, CliError>) -> Result<(), CliError> {
    let backend = backend?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| CliError::Transport(e.to_string()))?;
    rt.block_on(serve(backend))
}

/// Serves MCP on standard input and output until the client closes it, as the process `program` (the name the
/// user typed). Diagnostics go to standard error and start with `program`; standard output carries only the
/// protocol. The exit code is 0 after a clean end and 1 when the server could not start or the transport failed.
fn finish(backend: Result<Backend, CliError>, program: &str) -> ExitCode {
    // Another logger may be installed already (the desktop app's); then that one stays.
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).try_init();
    match run(backend) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{program}: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `drafftink-headless mcp`: serves MCP on standard input and output, on a session of its own or on a bridge mode.
pub fn run_server(args: &Args, program: &str) -> ExitCode {
    finish(backend(args), program)
}

/// `drafftink mcp`: serves MCP on standard input and output for the running desktop app, or for a browser tab with
/// `--relay`. It owns no document.
pub fn run_bridge(args: &BridgeArgs, program: &str) -> ExitCode {
    finish(bridge_backend(args), program)
}
