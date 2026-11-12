//! The server as a process: build the configuration, start, wait for a signal, stop.

use std::process::ExitCode;

use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use crate::{Args, Config};

/// The exit code for arguments that do not make a configuration.
const USAGE: u8 = 2;

/// Runs the server until Ctrl-C or SIGTERM, then saves every room and returns.
///
/// Diagnostics go through `tracing` (the `RUST_LOG` filter, `info` by default). The exit code is 0 after a clean
/// stop, 2 when the arguments are inconsistent and 1 when the server could not start.
pub fn run(args: Args) -> ExitCode {
    // Another subscriber may be installed already; then that one stays.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .try_init();
    let config = match args.into_config() {
        Ok(config) => config,
        Err(e) => {
            error!("{e}");
            return ExitCode::from(USAGE);
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(e) => {
            error!("cannot start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(serve(config))
}

async fn serve(config: Config) -> ExitCode {
    warn_about_exposure(&config);
    let handle = match crate::start(config).await {
        Ok(handle) => handle,
        Err(e) => {
            error!("{e}");
            return ExitCode::FAILURE;
        }
    };
    wait_for_signal().await;
    info!("shutting down: saving rooms");
    handle.shutdown().await;
    ExitCode::SUCCESS
}

fn warn_about_exposure(config: &Config) {
    if !config.web_dir.is_dir() {
        warn!("web client directory {} does not exist; only /ws and /health will work", config.web_dir.display());
    }
    if !config.bind.ip().is_loopback() {
        if config.auth_tokens.is_empty() {
            warn!(
                "listening on {} without auth: anyone who can reach this port and knows a room id can read and edit it",
                config.bind
            );
        }
        warn!("frames are not encrypted by the server; put a TLS-terminating proxy in front (see docs/server.md)");
    }
    if config.allowed_origins.iter().any(|o| o.trim() == "*") {
        warn!("--allowed-origin '*' lets any web page open a connection through a visitor's browser");
    }
    if config.debug_json {
        warn!("--debug-json is on: text frames are accepted on /ws");
    }
}

async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
