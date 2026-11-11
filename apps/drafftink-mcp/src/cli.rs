//! The command line: choose a mode, build its backend, serve MCP on stdio.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
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

/// `drafftink-mcp`: an MCP server for drafft.ink.
///
/// With no mode flag it runs headless: it owns its own editor session, for scripts and CI.
#[derive(Debug, Parser)]
#[command(name = "drafftink-mcp", version)]
pub struct Args {
    /// Attach to the running desktop app, so the user watches the agent draw.
    #[arg(long, conflicts_with = "relay")]
    attach: bool,
    /// Listen on 127.0.0.1 for the drafft.ink browser tab, which connects out to this server.
    #[arg(long)]
    relay: bool,

    /// Attach: the app's bridge port (default: the one in `current.port`).
    #[arg(long, requires = "attach")]
    port: Option<u16>,
    /// Attach: the folder with the app's token files (default: the app's runtime folder).
    #[arg(long, requires = "attach")]
    bridge_dir: Option<PathBuf>,
    /// Attach: a ws://127.0.0.1:<port>/ URL instead of discovery. Needs --token-file or the token variable.
    #[arg(long, requires = "attach")]
    url: Option<String>,
    /// Attach: read the token from this file (read again on every reconnect).
    #[arg(long, requires = "attach")]
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

/// Builds the backend that the arguments ask for.
pub fn backend(args: &Args) -> Result<Backend, CliError> {
    if args.attach {
        return attach_backend(args);
    }
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

fn attach_backend(args: &Args) -> Result<Backend, CliError> {
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

fn run(args: &Args) -> Result<(), CliError> {
    let backend = backend(args)?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| CliError::Transport(e.to_string()))?;
    rt.block_on(serve(backend))
}

/// The entry point of the binary. Diagnostics go to stderr; stdout carries only the protocol.
pub fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let args = Args::parse();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("drafftink-mcp: {e}");
            ExitCode::FAILURE
        }
    }
}
