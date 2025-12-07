//! File-based storage implementation for native platforms.

use super::{BoxFuture, Storage, StorageError, StorageResult};
use crate::canvas::CanvasDocument;
use std::fs;
use std::path::PathBuf;

/// File-based storage for native platforms.
///
/// Stores documents as JSON files in a specified directory.
pub struct FileStorage {
    /// Base directory for document storage.
    base_path: PathBuf,
}

impl FileStorage {
    /// Create a new file storage with the given base directory.
    ///
    /// Creates the directory if it doesn't exist.
    pub fn new(base_path: PathBuf) -> StorageResult<Self> {
        if !base_path.exists() {
            fs::create_dir_all(&base_path).map_err(|e| {
                StorageError::Io(format!("Failed to create storage directory: {}", e))
            })?;
        }
        Ok(Self { base_path })
    }

    /// Create file storage in the default location.
    ///
    /// On Unix: `~/.drafftink/documents/`
    /// On Windows: `%APPDATA%\drafftink\documents\`
    pub fn default_location() -> StorageResult<Self> {
        let base = dirs::data_local_dir()
            .or_else(dirs::home_dir)
            .ok_or_else(|| StorageError::Io("Could not determine home directory".to_string()))?;

        let path = base.join("drafftink").join("documents");
        Self::new(path)
    }

    /// Get the file path for a document ID.
    fn document_path(&self, id: &str) -> PathBuf {
        // Sanitize ID to be safe for filenames
        let safe_id: String = id
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.base_path.join(format!("{}.json", safe_id))
    }

    /// Get the base path.
    pub fn base_path(&self) -> &PathBuf {
        &self.base_path
    }
}

impl Storage for FileStorage {
    fn save(&self, id: &str, document: &CanvasDocument) -> BoxFuture<'_, StorageResult<()>> {
        let path = self.document_path(id);
        let json = match document.to_json() {
            Ok(j) => j,
            Err(e) => {
                return Box::pin(async move { Err(StorageError::Serialization(e.to_string())) });
            }
        };

        Box::pin(async move {
            fs::write(&path, json)
                .map_err(|e| StorageError::Io(format!("Failed to write {}: {}", path.display(), e)))
        })
    }

    fn load(&self, id: &str) -> BoxFuture<'_, StorageResult<CanvasDocument>> {
        let path = self.document_path(id);
        let id_owned = id.to_string();

        Box::pin(async move {
            if !path.exists() {
                return Err(StorageError::NotFound(id_owned));
            }

            let json = fs::read_to_string(&path).map_err(|e| {
                StorageError::Io(format!("Failed to read {}: {}", path.display(), e))
            })?;

            CanvasDocument::from_json(&json).map_err(|e| {
                StorageError::Serialization(format!("Failed to parse {}: {}", path.display(), e))
            })
        })
    }

    fn delete(&self, id: &str) -> BoxFuture<'_, StorageResult<()>> {
        let path = self.document_path(id);

        Box::pin(async move {
            if path.exists() {
                fs::remove_file(&path).map_err(|e| {
                    StorageError::Io(format!("Failed to delete {}: {}", path.display(), e))
                })?;
            }
            Ok(())
        })
    }

    fn list(&self) -> BoxFuture<'_, StorageResult<Vec<String>>> {
        let base = self.base_path.clone();

        Box::pin(async move {
            if !base.exists() {
                return Ok(vec![]);
            }

            let entries = fs::read_dir(&base)
                .map_err(|e| StorageError::Io(format!("Failed to read directory: {}", e)))?;

            let mut ids = Vec::new();
            for entry in entries.flatten() {
                if let Some(name) = entry.path().file_stem() {
                    if let Some(name_str) = name.to_str() {
                        // Only include .json files
                        if entry
                            .path()
                            .extension()
                            .map(|e| e == "json")
                            .unwrap_or(false)
                        {
                            ids.push(name_str.to_string());
                        }
                    }
                }
            }
            Ok(ids)
        })
    }

    fn exists(&self, id: &str) -> BoxFuture<'_, StorageResult<bool>> {
        let path = self.document_path(id);
        Box::pin(async move { Ok(path.exists()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

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
    fn test_file_storage_save_load() {
        let dir = tempdir().unwrap();
        let storage = FileStorage::new(dir.path().to_path_buf()).unwrap();

        let mut doc = CanvasDocument::new();
        doc.name = "Test Document".to_string();

        block_on(storage.save("test-doc", &doc)).unwrap();
        let loaded = block_on(storage.load("test-doc")).unwrap();

        assert_eq!(loaded.name, "Test Document");
    }

    #[test]
    fn test_file_storage_not_found() {
        let dir = tempdir().unwrap();
        let storage = FileStorage::new(dir.path().to_path_buf()).unwrap();

        let result = block_on(storage.load("nonexistent"));
        assert!(matches!(result, Err(StorageError::NotFound(_))));
    }

    #[test]
    fn test_file_storage_list() {
        let dir = tempdir().unwrap();
        let storage = FileStorage::new(dir.path().to_path_buf()).unwrap();

        let doc = CanvasDocument::new();
        block_on(storage.save("doc1", &doc)).unwrap();
        block_on(storage.save("doc2", &doc)).unwrap();

        let list = block_on(storage.list()).unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.contains(&"doc1".to_string()));
        assert!(list.contains(&"doc2".to_string()));
    }

    #[test]
    fn test_file_storage_delete() {
        let dir = tempdir().unwrap();
        let storage = FileStorage::new(dir.path().to_path_buf()).unwrap();

        let doc = CanvasDocument::new();
        block_on(storage.save("test", &doc)).unwrap();
        assert!(block_on(storage.exists("test")).unwrap());

        block_on(storage.delete("test")).unwrap();
        assert!(!block_on(storage.exists("test")).unwrap());
    }

    #[test]
    fn test_file_storage_sanitizes_id() {
        let dir = tempdir().unwrap();
        let storage = FileStorage::new(dir.path().to_path_buf()).unwrap();

        let doc = CanvasDocument::new();
        // ID with special characters should be sanitized
        block_on(storage.save("test/doc:with*special", &doc)).unwrap();

        // Should still be loadable with the same ID
        let loaded = block_on(storage.load("test/doc:with*special")).unwrap();
        assert_eq!(loaded.id, doc.id);
    }
}
