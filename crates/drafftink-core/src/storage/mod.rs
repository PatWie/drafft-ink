//! Storage abstraction for persistence.

mod autosave;
mod memory;

#[cfg(not(target_arch = "wasm32"))]
mod file;

#[cfg(target_arch = "wasm32")]
mod indexeddb;

pub use autosave::{
    AutoSaveManager, DEFAULT_AUTOSAVE_INTERVAL_SECS, LAST_DOCUMENT_KEY, PlatformAutoSaveManager,
    PlatformStorage, create_autosave_manager, create_default_storage,
};
pub use memory::MemoryStorage;

#[cfg(not(target_arch = "wasm32"))]
pub use file::FileStorage;

#[cfg(target_arch = "wasm32")]
pub use indexeddb::IndexedDbStorage;

use crate::canvas::CanvasDocument;
use std::future::Future;
use std::pin::Pin;
use thiserror::Error;

/// Storage errors.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("Document not found: {0}")]
    NotFound(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("IO error: {0}")]
    Io(String),
    #[error("Storage error: {0}")]
    Other(String),
}

/// Result type for storage operations.
pub type StorageResult<T> = Result<T, StorageError>;

/// Boxed future for async operations (compatible with WASM).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// Trait for document storage backends.
///
/// Implementations can store documents in memory, local storage,
/// IndexedDB (WASM), filesystem, or S3.
///
/// Note: On native platforms, implementations must be Send + Sync.
/// On WASM, these bounds are relaxed since it's single-threaded.
#[cfg(not(target_arch = "wasm32"))]
pub trait Storage: Send + Sync {
    /// Save a document.
    fn save(&self, id: &str, document: &CanvasDocument) -> BoxFuture<'_, StorageResult<()>>;

    /// Load a document.
    fn load(&self, id: &str) -> BoxFuture<'_, StorageResult<CanvasDocument>>;

    /// Delete a document.
    fn delete(&self, id: &str) -> BoxFuture<'_, StorageResult<()>>;

    /// List all document IDs.
    fn list(&self) -> BoxFuture<'_, StorageResult<Vec<String>>>;

    /// Check if a document exists.
    fn exists(&self, id: &str) -> BoxFuture<'_, StorageResult<bool>>;
}

/// Trait for document storage backends (WASM version without Send + Sync).
#[cfg(target_arch = "wasm32")]
pub trait Storage {
    /// Save a document.
    fn save(&self, id: &str, document: &CanvasDocument) -> BoxFuture<'_, StorageResult<()>>;

    /// Load a document.
    fn load(&self, id: &str) -> BoxFuture<'_, StorageResult<CanvasDocument>>;

    /// Delete a document.
    fn delete(&self, id: &str) -> BoxFuture<'_, StorageResult<()>>;

    /// List all document IDs.
    fn list(&self) -> BoxFuture<'_, StorageResult<Vec<String>>>;

    /// Check if a document exists.
    fn exists(&self, id: &str) -> BoxFuture<'_, StorageResult<bool>>;
}
