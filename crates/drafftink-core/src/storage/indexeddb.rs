//! IndexedDB storage implementation using rexie.

use super::{BoxFuture, Storage, StorageError, StorageResult};
use crate::canvas::CanvasDocument;
use rexie::{ObjectStore, Rexie, TransactionMode};
use wasm_bindgen::JsValue;

const DB_NAME: &str = "drafftink";
const STORE_NAME: &str = "documents";

/// IndexedDB-based storage using rexie.
pub struct IndexedDbStorage;

impl IndexedDbStorage {
    /// Create a new IndexedDB storage.
    pub fn new() -> Self {
        Self
    }

    /// Get or create the database connection.
    async fn get_db(&self) -> StorageResult<Rexie> {
        // Build database with rexie (no caching since rexie is lightweight)
        let rexie = Rexie::builder(DB_NAME)
            .version(1)
            .add_object_store(ObjectStore::new(STORE_NAME))
            .build()
            .await
            .map_err(|e| StorageError::Other(e.to_string()))?;

        Ok(rexie)
    }
}

impl Default for IndexedDbStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl Storage for IndexedDbStorage {
    fn save(&self, id: &str, document: &CanvasDocument) -> BoxFuture<'_, StorageResult<()>> {
        let id = id.to_string();
        let doc_clone = document.clone();

        Box::pin(async move {
            let db = self.get_db().await?;
            let transaction = db
                .transaction(&[STORE_NAME], TransactionMode::ReadWrite)
                .map_err(|e| StorageError::Other(e.to_string()))?;

            let store = transaction
                .store(STORE_NAME)
                .map_err(|e| StorageError::Other(e.to_string()))?;

            // Use serde-wasm-bindgen for better performance
            let js_val = serde_wasm_bindgen::to_value(&doc_clone)
                .map_err(|e| StorageError::Serialization(e.to_string()))?;

            store
                .put(&js_val, Some(&JsValue::from_str(&id)))
                .await
                .map_err(|e| StorageError::Other(e.to_string()))?;

            transaction
                .done()
                .await
                .map_err(|e| StorageError::Other(e.to_string()))?;

            Ok(())
        })
    }

    fn load(&self, id: &str) -> BoxFuture<'_, StorageResult<CanvasDocument>> {
        let id = id.to_string();

        Box::pin(async move {
            let db = self.get_db().await?;
            let transaction = db
                .transaction(&[STORE_NAME], TransactionMode::ReadOnly)
                .map_err(|e| StorageError::Other(e.to_string()))?;

            let store = transaction
                .store(STORE_NAME)
                .map_err(|e| StorageError::Other(e.to_string()))?;

            let js_val = store
                .get(JsValue::from_str(&id))
                .await
                .map_err(|e| StorageError::Other(e.to_string()))?;

            match js_val {
                Some(val) => serde_wasm_bindgen::from_value(val)
                    .map_err(|e| StorageError::Serialization(e.to_string())),
                None => Err(StorageError::NotFound(id)),
            }
        })
    }

    fn delete(&self, id: &str) -> BoxFuture<'_, StorageResult<()>> {
        let id = id.to_string();

        Box::pin(async move {
            let db = self.get_db().await?;
            let transaction = db
                .transaction(&[STORE_NAME], TransactionMode::ReadWrite)
                .map_err(|e| StorageError::Other(e.to_string()))?;

            let store = transaction
                .store(STORE_NAME)
                .map_err(|e| StorageError::Other(e.to_string()))?;

            store
                .delete(JsValue::from_str(&id))
                .await
                .map_err(|e| StorageError::Other(e.to_string()))?;

            transaction
                .done()
                .await
                .map_err(|e| StorageError::Other(e.to_string()))?;

            Ok(())
        })
    }

    fn list(&self) -> BoxFuture<'_, StorageResult<Vec<String>>> {
        Box::pin(async move {
            let db = self.get_db().await?;
            let transaction = db
                .transaction(&[STORE_NAME], TransactionMode::ReadOnly)
                .map_err(|e| StorageError::Other(e.to_string()))?;

            let store = transaction
                .store(STORE_NAME)
                .map_err(|e| StorageError::Other(e.to_string()))?;

            let js_keys = store
                .get_all_keys(None, None)
                .await
                .map_err(|e| StorageError::Other(e.to_string()))?;

            let mut ids = Vec::new();
            for key in js_keys {
                if let Some(key_str) = key.as_string() {
                    ids.push(key_str);
                }
            }
            Ok(ids)
        })
    }

    fn exists(&self, id: &str) -> BoxFuture<'_, StorageResult<bool>> {
        let id = id.to_string();

        Box::pin(async move {
            let db = self.get_db().await?;
            let transaction = db
                .transaction(&[STORE_NAME], TransactionMode::ReadOnly)
                .map_err(|e| StorageError::Other(e.to_string()))?;

            let store = transaction
                .store(STORE_NAME)
                .map_err(|e| StorageError::Other(e.to_string()))?;

            let js_val = store
                .get(JsValue::from_str(&id))
                .await
                .map_err(|e| StorageError::Other(e.to_string()))?;

            Ok(js_val.is_some())
        })
    }
}
