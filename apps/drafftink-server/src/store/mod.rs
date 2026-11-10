//! Durable room storage behind the [`RoomStore`] trait (`docs/protocol.md` §6).
//!
//! A room is a compacted snapshot plus an append-only log of Loro updates written after it.
//! Every provider keeps the same contract, and `tests/stores.rs` runs one conformance suite
//! against all of them.
//!
//! The server is the only writer of a store: run one server per store. Providers keep the next
//! log index of a room in memory.

mod fs;
mod memory;
#[cfg(feature = "s3")]
mod s3;
#[cfg(feature = "sqlite")]
mod sqlite;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use drafftink_protocol::RoomId;

pub use fs::FsStore;
pub use memory::MemoryStore;
#[cfg(feature = "s3")]
pub use s3::{S3Config, S3Store};
#[cfg(feature = "sqlite")]
pub use sqlite::SqliteStore;

/// A storage failure. Callers treat all of them as "the room cannot be served or saved now",
/// except [`StoreError::InvalidRoom`], which is a refusal of the room id itself.
#[derive(Debug, Clone, thiserror::Error)]
pub enum StoreError {
    /// This store cannot hold a room with this id.
    #[error("this store cannot hold the room id: {0}")]
    InvalidRoom(String),
    /// A read or write failed.
    #[error("storage i/o failed: {0}")]
    Io(String),
    /// Stored data does not have the expected shape.
    #[error("stored data is damaged: {0}")]
    Corrupt(String),
    /// The backend (database, object store) reported an error.
    #[error("storage backend failed: {0}")]
    Backend(String),
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        StoreError::Io(e.to_string())
    }
}

/// What a store holds for one room.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredRoom {
    /// The latest compacted snapshot, if the room was ever compacted.
    pub snapshot: Option<Vec<u8>>,
    /// The log entries written after the snapshot, oldest first.
    pub updates: Vec<Vec<u8>>,
}

/// A room as listed by [`RoomStore::list`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomMeta {
    /// The room.
    pub room: RoomId,
    /// Last write, in milliseconds since the Unix epoch.
    pub modified_ms: u64,
}

/// Durable storage for rooms. Async, object safe, errors are values.
#[async_trait]
pub trait RoomStore: Send + Sync {
    /// Short provider name for logs.
    fn name(&self) -> &'static str;

    /// Latest compacted snapshot and the update log written after it. `None` for a new room.
    async fn load(&self, room: &RoomId) -> Result<Option<StoredRoom>, StoreError>;

    /// Appends one opaque update to the room's log. Durable when it returns.
    async fn append_update(&self, room: &RoomId, update: &[u8]) -> Result<(), StoreError>;

    /// Replaces the snapshot with `snapshot` (which includes the oldest `covered` log entries)
    /// and drops those entries. A crash leaves the old or the new state, never a mix. Entries
    /// beyond `covered` stay.
    async fn compact(&self, room: &RoomId, snapshot: &[u8], covered: u64) -> Result<(), StoreError>;

    /// Deletes a room. Deleting a room that does not exist succeeds.
    async fn delete(&self, room: &RoomId) -> Result<(), StoreError>;

    /// Every stored room with its last write time.
    async fn list(&self) -> Result<Vec<RoomMeta>, StoreError>;
}

/// Which provider to open, with its parameters.
#[derive(Debug, Clone)]
pub enum StorageConfig {
    /// Rooms live in process memory and vanish on restart.
    Memory,
    /// One directory per room under `dir`.
    Filesystem {
        /// Root directory; created when missing.
        dir: PathBuf,
    },
    /// One SQLite database file.
    #[cfg(feature = "sqlite")]
    Sqlite {
        /// Database file; created when missing.
        path: PathBuf,
    },
    /// Objects in an S3-compatible bucket.
    #[cfg(feature = "s3")]
    S3(S3Config),
}

impl StorageConfig {
    /// The default provider: the filesystem under `./data`.
    pub fn default_filesystem() -> Self {
        StorageConfig::Filesystem { dir: PathBuf::from("data") }
    }
}

/// Opens the configured provider.
pub async fn open(config: &StorageConfig) -> Result<Arc<dyn RoomStore>, StoreError> {
    Ok(match config {
        StorageConfig::Memory => Arc::new(MemoryStore::new()),
        StorageConfig::Filesystem { dir } => Arc::new(FsStore::open(dir.clone()).await?),
        #[cfg(feature = "sqlite")]
        StorageConfig::Sqlite { path } => Arc::new(SqliteStore::open(path.clone()).await?),
        #[cfg(feature = "s3")]
        StorageConfig::S3(config) => Arc::new(S3Store::new(config)?),
    })
}

/// Locks a mutex and keeps going if another thread panicked while holding it: the data behind
/// these locks is always left in a valid state between statements.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Rooms whose directory or key would be unsafe or ambiguous are refused by every provider, so
/// they behave the same everywhere: ids that start with a dot (`.` and `..` are path
/// segments).
pub(crate) fn check_room(room: &RoomId) -> Result<(), StoreError> {
    if room.as_str().starts_with('.') {
        return Err(StoreError::InvalidRoom("room ids must not start with '.'".into()));
    }
    Ok(())
}

/// Tracks the next log index of each room for providers that name log entries by number.
#[derive(Default)]
pub(crate) struct NextIndex {
    map: Mutex<std::collections::HashMap<String, u64>>,
}

impl NextIndex {
    pub fn get(&self, room: &RoomId) -> Option<u64> {
        lock(&self.map).get(room.as_str()).copied()
    }
    pub fn set(&self, room: &RoomId, next: u64) {
        lock(&self.map).insert(room.as_str().to_owned(), next);
    }
    pub fn forget(&self, room: &RoomId) {
        lock(&self.map).remove(room.as_str());
    }
}
