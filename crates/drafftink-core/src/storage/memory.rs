//! In-memory storage implementation.

use super::{BoxFuture, Storage, StorageError, StorageResult};
use crate::canvas::CanvasDocument;
use std::collections::HashMap;
use std::sync::RwLock;

/// In-memory storage for testing and ephemeral use.
#[derive(Default)]
pub struct MemoryStorage {
    documents: RwLock<HashMap<String, CanvasDocument>>,
}

impl MemoryStorage {
    /// Create a new empty memory storage.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Storage for MemoryStorage {
    fn save(&self, id: &str, document: &CanvasDocument) -> BoxFuture<'_, StorageResult<()>> {
        let id = id.to_string();
        let document = document.clone();
        Box::pin(async move {
            let mut docs = self
                .documents
                .write()
                .map_err(|e| StorageError::Other(format!("Lock error: {}", e)))?;
            docs.insert(id, document);
            Ok(())
        })
    }

    fn load(&self, id: &str) -> BoxFuture<'_, StorageResult<CanvasDocument>> {
        let id = id.to_string();
        Box::pin(async move {
            let docs = self
                .documents
                .read()
                .map_err(|e| StorageError::Other(format!("Lock error: {}", e)))?;
            docs.get(&id).cloned().ok_or(StorageError::NotFound(id))
        })
    }

    fn delete(&self, id: &str) -> BoxFuture<'_, StorageResult<()>> {
        let id = id.to_string();
        Box::pin(async move {
            let mut docs = self
                .documents
                .write()
                .map_err(|e| StorageError::Other(format!("Lock error: {}", e)))?;
            docs.remove(&id);
            Ok(())
        })
    }

    fn list(&self) -> BoxFuture<'_, StorageResult<Vec<String>>> {
        Box::pin(async move {
            let docs = self
                .documents
                .read()
                .map_err(|e| StorageError::Other(format!("Lock error: {}", e)))?;
            Ok(docs.keys().cloned().collect())
        })
    }

    fn exists(&self, id: &str) -> BoxFuture<'_, StorageResult<bool>> {
        let id = id.to_string();
        Box::pin(async move {
            let docs = self
                .documents
                .read()
                .map_err(|e| StorageError::Other(format!("Lock error: {}", e)))?;
            Ok(docs.contains_key(&id))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        // Simple blocking executor for tests
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
    fn test_save_and_load() {
        let storage = MemoryStorage::new();
        let doc = CanvasDocument::new();

        block_on(storage.save("test", &doc)).unwrap();
        let loaded = block_on(storage.load("test")).unwrap();

        assert_eq!(doc.id, loaded.id);
    }

    #[test]
    fn test_not_found() {
        let storage = MemoryStorage::new();
        let result = block_on(storage.load("nonexistent"));

        assert!(matches!(result, Err(StorageError::NotFound(_))));
    }

    #[test]
    fn test_exists() {
        let storage = MemoryStorage::new();
        let doc = CanvasDocument::new();

        assert!(!block_on(storage.exists("test")).unwrap());
        block_on(storage.save("test", &doc)).unwrap();
        assert!(block_on(storage.exists("test")).unwrap());
    }

    #[test]
    fn test_delete() {
        let storage = MemoryStorage::new();
        let doc = CanvasDocument::new();

        block_on(storage.save("test", &doc)).unwrap();
        block_on(storage.delete("test")).unwrap();
        assert!(!block_on(storage.exists("test")).unwrap());
    }

    #[test]
    fn test_list() {
        let storage = MemoryStorage::new();
        let doc = CanvasDocument::new();

        block_on(storage.save("doc1", &doc)).unwrap();
        block_on(storage.save("doc2", &doc)).unwrap();

        let list = block_on(storage.list()).unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.contains(&"doc1".to_string()));
        assert!(list.contains(&"doc2".to_string()));
    }
}
