//! Sealed rooms (protocol v2) on this server: the host around [`drafftink_relay`]. It owns the sockets, the clock
//! and the storage; the rules are the relay core's. See `docs/design/collab-e2e.md`.
//!
//! Routes: `/r/<id>/ws` (WebSocket), `/r/<id>/f` (list the images) and `/r/<id>/f/<name>` (`PUT` and `GET` one
//! sealed image). The room id is in the path so that the membership check can happen before any room is loaded.

mod actor;
mod codec;
mod conn;
mod fs;
mod images;
#[cfg(feature = "sqlite")]
mod sqlite;
mod storage;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use drafftink_protocol::v2::RoomId;
use tokio::sync::{Mutex, oneshot};
use tracing::{info, warn};

use self::actor::{Cmd, RoomHandle};
use self::storage::SealedStorage;
use crate::config::Config;

pub(crate) use self::conn::serve as serve_socket;
pub use self::fs::{FsBlobStore, FsRelayStore};
pub(crate) use self::images::routes as image_routes;
#[cfg(feature = "sqlite")]
pub use self::sqlite::{SqliteBlobStore, SqliteRelayDb, SqliteRelayStore};

/// Why a room could not be served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unavailable {
    /// The storage provider holds no sealed rooms.
    Storage(&'static str),
    /// This server has as many rooms loaded as it may.
    TooManyRooms,
}

/// The sealed rooms of one server.
pub(crate) struct SealedHost {
    pub(crate) config: Arc<Config>,
    storage: SealedStorage,
    rooms: Mutex<HashMap<RoomId, RoomHandle>>,
    generation: AtomicU64,
    next_conn: AtomicU64,
}

impl SealedHost {
    pub(crate) fn new(config: Arc<Config>) -> Result<Arc<Self>, crate::StartError> {
        let storage = SealedStorage::from_config(&config.storage)
            .map_err(|e| crate::StartError::Store(crate::store::StoreError::Backend(e.to_string())))?;
        if let Some(why) = storage.unsupported() {
            warn!("{why}");
        }
        Ok(Arc::new(SealedHost {
            config,
            storage,
            rooms: Mutex::new(HashMap::new()),
            generation: AtomicU64::new(1),
            next_conn: AtomicU64::new(1),
        }))
    }

    /// Loads the rooms that have a deadline, so that they are deleted on time even if nobody visits them.
    pub(crate) async fn resume(self: &Arc<Self>) {
        if matches!(self.config.relay.retention, drafftink_relay::Retention::Never) {
            return;
        }
        let stored = self.storage.rooms();
        info!("resuming {} sealed rooms with a retention deadline", stored.len());
        for room in stored {
            let _ = self.room(room).await;
        }
    }

    pub(crate) fn next_conn_id(&self) -> drafftink_relay::ConnId {
        drafftink_relay::ConnId(self.next_conn.fetch_add(1, Ordering::Relaxed))
    }

    /// The task of `room`, started when it is not running.
    pub(in crate::sealed) async fn room(self: &Arc<Self>, room: RoomId) -> Result<RoomHandle, Unavailable> {
        let mut rooms = self.rooms.lock().await;
        if let Some(handle) = rooms.get(&room).filter(|h| !h.tx.is_closed()) {
            return Ok(handle.clone());
        }
        if rooms.len() >= self.config.limits.max_rooms {
            return Err(Unavailable::TooManyRooms);
        }
        let stores = self.storage.open(&room).ok_or(Unavailable::Storage(
            self.storage.unsupported().unwrap_or("this server cannot store sealed rooms"),
        ))?;
        let generation = self.generation.fetch_add(1, Ordering::Relaxed);
        let handle = actor::spawn(self.clone(), room, stores, self.config.relay.clone(), generation);
        rooms.insert(room, handle.clone());
        Ok(handle)
    }

    /// A room task ended. It is forgotten unless a newer task took its place.
    pub(in crate::sealed) async fn forget(&self, room: &RoomId, generation: u64) {
        let mut rooms = self.rooms.lock().await;
        if rooms.get(room).is_some_and(|h| h.generation == generation) {
            rooms.remove(room);
        }
    }

    pub(crate) async fn loaded_rooms(&self) -> usize {
        self.rooms.lock().await.len()
    }

    /// Writes every room and tells its people to come back later.
    pub(crate) async fn shutdown(&self) {
        let handles: Vec<RoomHandle> = self.rooms.lock().await.values().cloned().collect();
        for handle in handles {
            let (done, wait) = oneshot::channel();
            if handle.tx.send(Cmd::Shutdown { done }).await.is_ok() {
                let _ = tokio::time::timeout(std::time::Duration::from_secs(10), wait).await;
            }
        }
    }
}
