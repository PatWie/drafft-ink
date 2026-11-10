//! The `drafftink-server` binary: parse the command line, start, wait for a signal, stop.

use std::process::ExitCode;

use clap::Parser;
use drafftink_server::{Cli, Config};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    let config = match Cli::parse().into_config() {
        Ok(c) => c,
        Err(e) => {
            error!("{e}");
            return ExitCode::from(2);
        }
    };
    warn_about_exposure(&config);
    let handle = match drafftink_server::start(config).await {
        Ok(h) => h,
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
