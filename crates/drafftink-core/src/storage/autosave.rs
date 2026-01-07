//! Auto-save functionality for document persistence.
//!
//! Provides automatic periodic saving of documents to prevent data loss.

use crate::canvas::CanvasDocument;
use crate::storage::{Storage, StorageResult};
use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};

#[cfg(target_arch = "wasm32")]
use web_time::{Duration, Instant};

/// Default auto-save interval in seconds.
pub const DEFAULT_AUTOSAVE_INTERVAL_SECS: u64 = 30;

/// Key for the "last opened" document.
pub const LAST_DOCUMENT_KEY: &str = "__last_document__";

/// Manages automatic document persistence.
pub struct AutoSaveManager<S: Storage> {
    /// Storage backend.
    storage: Arc<S>,
    /// Auto-save interval.
    interval: Duration,
    /// Last save timestamp.
    last_save: Option<Instant>,
    /// Whether the document has unsaved changes.
    dirty: bool,
    /// Current document ID being edited.
    current_doc_id: Option<String>,
}

impl<S: Storage> AutoSaveManager<S> {
    /// Create a new auto-save manager with the given storage backend.
    pub fn new(storage: Arc<S>) -> Self {
        Self {
            storage,
            interval: Duration::from_secs(DEFAULT_AUTOSAVE_INTERVAL_SECS),
            last_save: None,
            dirty: false,
            current_doc_id: None,
        }
    }

    /// Set the auto-save interval.
    pub fn set_interval(&mut self, interval: Duration) {
        self.interval = interval;
    }

    /// Get the auto-save interval.
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Mark the document as having unsaved changes.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Check if the document has unsaved changes.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Set the current document ID.
    pub fn set_document_id(&mut self, id: Option<String>) {
        self.current_doc_id = id;
    }

    /// Get the current document ID.
    pub fn document_id(&self) -> Option<&str> {
        self.current_doc_id.as_deref()
    }

    /// Check if enough time has passed for an auto-save.
    pub fn should_save(&self) -> bool {
        if !self.dirty {
            return false;
        }

        match self.last_save {
            Some(last) => last.elapsed() >= self.interval,
            None => true, // Never saved, should save
        }
    }

    /// Save the document if needed (dirty + interval elapsed).
    /// Returns true if save was performed.
    pub async fn maybe_save(&mut self, document: &CanvasDocument) -> StorageResult<bool> {
        if !self.should_save() {
            return Ok(false);
        }

        self.save(document).await?;
        Ok(true)
    }

    /// Force save the document immediately.
    pub async fn save(&mut self, document: &CanvasDocument) -> StorageResult<()> {
        let doc_id = self
            .current_doc_id
            .clone()
            .unwrap_or_else(|| document.id.clone());

        self.storage.save(&doc_id, document).await?;

        // Also save as the "last document" for auto-restore
        self.storage.save(LAST_DOCUMENT_KEY, document).await?;

        self.last_save = Some(Instant::now());
        self.dirty = false;

        Ok(())
    }

    /// Load a document by ID.
    pub async fn load(&mut self, id: &str) -> StorageResult<CanvasDocument> {
        let doc = self.storage.load(id).await?;
        self.current_doc_id = Some(id.to_string());
        self.dirty = false;
        self.last_save = Some(Instant::now());
        Ok(doc)
    }

    /// Try to load the last opened document.
    /// Returns None if no last document exists.
    pub async fn load_last(&mut self) -> Option<CanvasDocument> {
        match self.storage.load(LAST_DOCUMENT_KEY).await {
            Ok(doc) => {
                self.current_doc_id = Some(doc.id.clone());
                self.dirty = false;
                self.last_save = Some(Instant::now());
                Some(doc)
            }
            Err(_) => None,
        }
    }

    /// Delete a document by ID.
    pub async fn delete(&self, id: &str) -> StorageResult<()> {
        self.storage.delete(id).await
    }

    /// List all saved document IDs.
    pub async fn list_documents(&self) -> StorageResult<Vec<String>> {
        let mut docs = self.storage.list().await?;
        // Filter out the special "last document" key
        docs.retain(|id| id != LAST_DOCUMENT_KEY);
        Ok(docs)
    }

    /// Check if a document exists.
    pub async fn exists(&self, id: &str) -> StorageResult<bool> {
        self.storage.exists(id).await
    }

    /// Get a reference to the storage backend.
    pub fn storage(&self) -> &Arc<S> {
        &self.storage
    }
}

/// Create a platform-appropriate storage backend.
#[cfg(not(target_arch = "wasm32"))]
pub fn create_default_storage() -> StorageResult<Arc<crate::storage::FileStorage>> {
    Ok(Arc::new(crate::storage::FileStorage::default_location()?))
}

#[cfg(target_arch = "wasm32")]
pub fn create_default_storage() -> StorageResult<Arc<crate::storage::IndexedDbStorage>> {
    Ok(Arc::new(crate::storage::IndexedDbStorage::new()))
}

/// Convenience type alias for platform-specific storage.
#[cfg(not(target_arch = "wasm32"))]
pub type PlatformStorage = crate::storage::FileStorage;

#[cfg(target_arch = "wasm32")]
pub type PlatformStorage = crate::storage::IndexedDbStorage;

/// Type alias for the auto-save manager with platform-specific storage.
pub type PlatformAutoSaveManager = AutoSaveManager<PlatformStorage>;

/// Convenience function to create an auto-save manager with default storage.
pub fn create_autosave_manager() -> StorageResult<PlatformAutoSaveManager> {
    let storage = create_default_storage()?;
    Ok(AutoSaveManager::new(storage))
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::storage::MemoryStorage;

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

        fn dummy_raw_waker() -> RawWaker {
            fn no_op(_: *const ()) {}
            fn clone(_: *const ()) -> RawWaker {
                dummy_raw_waker()
            }
            static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, no_op, no_op, no_op);
            RawWaker::new(std::ptr::null(), &VTABLE)
        }

        let waker = unsafe { Waker::from_raw(dummy_raw_waker()) };
        let mut cx = Context::from_waker(&waker);
        let mut f = std::pin::pin!(f);

        loop {
            match f.as_mut().poll(&mut cx) {
                Poll::Ready(result) => return result,
                Poll::Pending => {}
            }
        }
    }

    #[test]
    fn test_autosave_manager_creation() {
        let storage = Arc::new(MemoryStorage::new());
        let manager = AutoSaveManager::new(storage);

        assert!(!manager.is_dirty());
        assert!(!manager.should_save());
    }

    #[test]
    fn test_autosave_dirty_flag() {
        let storage = Arc::new(MemoryStorage::new());
        let mut manager = AutoSaveManager::new(storage);

        assert!(!manager.is_dirty());
        manager.mark_dirty();
        assert!(manager.is_dirty());

        // Should save when dirty and no previous save
        assert!(manager.should_save());
    }

    #[test]
    fn test_autosave_save_clears_dirty() {
        let storage = Arc::new(MemoryStorage::new());
        let mut manager = AutoSaveManager::new(storage);

        manager.mark_dirty();
        assert!(manager.is_dirty());

        let doc = CanvasDocument::new();
        block_on(manager.save(&doc)).unwrap();

        assert!(!manager.is_dirty());
    }

    #[test]
    fn test_autosave_load_last() {
        let storage = Arc::new(MemoryStorage::new());
        let mut manager = AutoSaveManager::new(storage);

        // Save a document
        let mut doc = CanvasDocument::new();
        doc.name = "Test Document".to_string();
        manager.mark_dirty();
        block_on(manager.save(&doc)).unwrap();

        // Create new manager and load last
        let storage2 = manager.storage().clone();
        let mut manager2 = AutoSaveManager::new(storage2);

        let loaded = block_on(manager2.load_last()).expect("Should load last document");
        assert_eq!(loaded.name, "Test Document");
    }

    #[test]
    fn test_autosave_list_excludes_special_key() {
        let storage = Arc::new(MemoryStorage::new());
        let mut manager = AutoSaveManager::new(storage);

        let doc = CanvasDocument::new();
        manager.mark_dirty();
        block_on(manager.save(&doc)).unwrap();

        let list = block_on(manager.list_documents()).unwrap();

        // Should not include LAST_DOCUMENT_KEY
        assert!(!list.contains(&LAST_DOCUMENT_KEY.to_string()));
    }
}
