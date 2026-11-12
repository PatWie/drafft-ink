//! drafftink-server: the collaboration server for drafft.ink, as a library.
//!
//! The server serves the web client, the `/ws` WebSocket endpoint (the protocol in
//! `docs/protocol.md`) and `/health`. Rooms live in memory as Loro documents and are saved
//! through a [`store::RoomStore`]. See `docs/server.md` for running and configuring it.
//!
//! This crate has no binary. `drafftink-headless serve` is the program: it flattens [`Args`] into its own command
//! line and calls [`run`]. Tests and other programs embed the server with [`start`].

#![warn(missing_docs, unreachable_pub)]

mod auth;
pub mod clock;
pub mod config;
mod conn;
mod http;
mod limits;
pub mod outbox;
pub mod room;
mod run;
mod schema;
pub mod store;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::{Semaphore, oneshot};
use tokio::task::JoinHandle;
use tracing::{info, warn};

pub use config::{Args, Config};
pub use http::OriginPolicy;
use room::RoomManager;
pub use run::run;
use store::{RoomStore, StoreError};

/// Why the server could not start.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// The configuration is inconsistent.
    #[error("invalid configuration: {0}")]
    Config(#[from] config::ConfigError),
    /// The room store could not be opened.
    #[error("cannot open the room store: {0}")]
    Store(#[from] StoreError),
    /// The listening socket could not be bound.
    #[error("cannot listen on {addr}: {source}")]
    Bind {
        /// The address that was requested.
        addr: SocketAddr,
        /// The operating system's answer.
        source: std::io::Error,
    },
}

/// A running server.
pub struct ServerHandle {
    addr: SocketAddr,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
    sweeper: Option<JoinHandle<()>>,
    manager: Arc<RoomManager>,
}

impl ServerHandle {
    /// The address the server listens on (useful with port 0).
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// The number of rooms currently loaded.
    pub async fn loaded_rooms(&self) -> usize {
        self.manager.loaded_rooms().await
    }

    /// Stops accepting connections, tells the peers, saves every room and returns.
    pub async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(sweeper) = self.sweeper.take() {
            sweeper.abort();
        }
        if tokio::time::timeout(Duration::from_secs(15), &mut self.task).await.is_err() {
            warn!("shutdown took too long; closing the remaining connections");
            self.task.abort();
        }
    }

    /// Waits until the server stops by itself (it does not, unless it fails).
    pub async fn wait(&mut self) {
        let _ = (&mut self.task).await;
    }
}

/// Starts the server with the store named in the configuration.
pub async fn start(config: Config) -> Result<ServerHandle, StartError> {
    config.validate()?;
    let store = store::open(&config.storage).await?;
    start_with_store(config, store).await
}

/// Starts the server with a store the caller opened (tests share one across restarts).
pub async fn start_with_store(config: Config, store: Arc<dyn RoomStore>) -> Result<ServerHandle, StartError> {
    config.validate()?;
    let listener =
        TcpListener::bind(config.bind).await.map_err(|source| StartError::Bind { addr: config.bind, source })?;
    let addr = listener.local_addr().map_err(|source| StartError::Bind { addr: config.bind, source })?;
    let config = Arc::new(config);
    let manager = RoomManager::new(config.clone(), store.clone());
    let state = Arc::new(http::AppState {
        auth: auth::Auth::new(&config.auth_tokens),
        origin: OriginPolicy::new(&config),
        permits: Arc::new(Semaphore::new(config.limits.max_connections)),
        next_conn: AtomicU64::new(1),
        manager: manager.clone(),
        config: config.clone(),
    });
    let app = http::router(state);
    let (stop, stopped) = oneshot::channel::<()>();
    let shutting = manager.clone();
    let task = tokio::spawn(async move {
        let graceful = async move {
            let _ = stopped.await;
            shutting.shutdown().await;
        };
        if let Err(e) = axum::serve(listener, app).with_graceful_shutdown(graceful).await {
            warn!("server stopped with an error: {e}");
        }
    });
    let sweeper = config.timing.expire_after.map(|expire_after| {
        let manager = manager.clone();
        let every = (expire_after / 10).clamp(Duration::from_secs(60), Duration::from_secs(3600));
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            loop {
                tick.tick().await;
                manager.expire_rooms(expire_after, clock::now_ms()).await;
            }
        })
    });
    info!("listening on {addr}, storing rooms in {}", store.name());
    Ok(ServerHandle { addr, stop: Some(stop), task, sweeper, manager })
}
