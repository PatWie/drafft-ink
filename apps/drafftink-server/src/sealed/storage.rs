//! Where the sealed rooms of a server live: the same kind of storage as the plaintext rooms, in its own tables and
//! directories.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use drafftink_protocol::v2::RoomId;
use drafftink_relay::{BlobStore, MemoryBlobStore, MemoryRelayStore, RelayStore, StoreError};

use super::fs::{self, FsBlobStore, FsRelayStore};
use crate::store::StorageConfig;

/// A relay store and a blob store that can move to the task of a room.
pub(crate) type RoomStores = (Box<dyn RelayStore + Send>, Box<dyn BlobStore + Send>);

/// The storage of every sealed room of a server.
pub(crate) enum SealedStorage {
    /// Lost on restart. The stores stay in the map when a room is unloaded.
    Memory(Mutex<HashMap<RoomId, (MemoryRelayStore, MemoryBlobStore)>>),
    /// Files under the data directory.
    Filesystem(PathBuf),
    /// Tables in the SQLite database.
    #[cfg(feature = "sqlite")]
    Sqlite(super::sqlite::SqliteRelayDb),
    /// This storage provider cannot hold sealed rooms.
    #[cfg(feature = "s3")]
    Unsupported(&'static str),
}

impl SealedStorage {
    pub(crate) fn from_config(config: &StorageConfig) -> Result<Self, StoreError> {
        Ok(match config {
            StorageConfig::Memory => SealedStorage::Memory(Mutex::default()),
            StorageConfig::Filesystem { dir } => SealedStorage::Filesystem(dir.clone()),
            #[cfg(feature = "sqlite")]
            StorageConfig::Sqlite { path } => SealedStorage::Sqlite(super::sqlite::SqliteRelayDb::open(path)?),
            #[cfg(feature = "s3")]
            StorageConfig::S3(_) => {
                SealedStorage::Unsupported("sealed rooms are not stored in S3 yet; use filesystem or sqlite")
            }
        })
    }

    /// Why sealed rooms are not available, if they are not.
    pub(crate) fn unsupported(&self) -> Option<&'static str> {
        match self {
            #[cfg(feature = "s3")]
            SealedStorage::Unsupported(why) => Some(why),
            _ => None,
        }
    }

    /// The stores of one room. Nothing is created until the room is written.
    pub(crate) fn open(&self, room: &RoomId) -> Option<RoomStores> {
        match self {
            SealedStorage::Memory(map) => {
                let mut map = map.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let (relay, blobs) = map.entry(*room).or_default().clone();
                Some((Box::new(relay), Box::new(blobs)))
            }
            SealedStorage::Filesystem(root) => {
                Some((Box::new(FsRelayStore::new(root, room)), Box::new(FsBlobStore::new(root, room))))
            }
            #[cfg(feature = "sqlite")]
            SealedStorage::Sqlite(db) => Some((Box::new(db.relay_store(room)), Box::new(db.blob_store(room)))),
            #[cfg(feature = "s3")]
            SealedStorage::Unsupported(_) => None,
        }
    }

    /// Every stored room.
    pub(crate) fn rooms(&self) -> Vec<RoomId> {
        match self {
            SealedStorage::Memory(map) => {
                map.lock().unwrap_or_else(std::sync::PoisonError::into_inner).keys().copied().collect()
            }
            SealedStorage::Filesystem(root) => fs::list_rooms(root),
            #[cfg(feature = "sqlite")]
            SealedStorage::Sqlite(db) => db.list_rooms(),
            #[cfg(feature = "s3")]
            SealedStorage::Unsupported(_) => Vec::new(),
        }
    }
}
