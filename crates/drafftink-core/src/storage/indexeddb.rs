//! IndexedDB storage implementation for WebAssembly.
//!
//! Uses the browser's IndexedDB for persistent document storage.

use super::{BoxFuture, Storage, StorageError, StorageResult};
use crate::canvas::CanvasDocument;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::{IdbDatabase, IdbObjectStore, IdbRequest, IdbTransactionMode};

const DB_NAME: &str = "drafftink";
const DB_VERSION: u32 = 1;
const STORE_NAME: &str = "documents";

/// IndexedDB-based storage for WebAssembly.
///
/// Note: This is intentionally not Send/Sync since WASM is single-threaded
/// and IndexedDB handles are not thread-safe.
pub struct IndexedDbStorage {
    /// Cached database connection.
    db: Rc<RefCell<Option<IdbDatabase>>>,
}

impl IndexedDbStorage {
    /// Create a new IndexedDB storage.
    ///
    /// Note: The actual database connection is established lazily on first use.
    pub fn new() -> Self {
        Self {
            db: Rc::new(RefCell::new(None)),
        }
    }

    /// Open or create the database, returning a handle.
    async fn get_db(&self) -> StorageResult<IdbDatabase> {
        // Return cached connection if available
        if let Some(db) = self.db.borrow().as_ref() {
            return Ok(db.clone());
        }

        // Open database
        let window =
            web_sys::window().ok_or_else(|| StorageError::Other("No window object".to_string()))?;

        let idb_factory = window
            .indexed_db()
            .map_err(|e| StorageError::Other(format!("IndexedDB error: {:?}", e)))?
            .ok_or_else(|| StorageError::Other("IndexedDB not available".to_string()))?;

        let open_request = idb_factory
            .open_with_u32(DB_NAME, DB_VERSION)
            .map_err(|e| StorageError::Other(format!("Failed to open DB: {:?}", e)))?;

        // Set up upgrade handler to create object store
        let onupgrade = Closure::once(Box::new(move |event: web_sys::IdbVersionChangeEvent| {
            let target = event.target().unwrap();
            let request: IdbRequest = target.unchecked_into();
            let db: IdbDatabase = request.result().unwrap().unchecked_into();

            // Create object store if it doesn't exist
            if !db.object_store_names().contains(STORE_NAME) {
                db.create_object_store(STORE_NAME)
                    .expect("Failed to create object store");
            }
        }) as Box<dyn FnOnce(_)>);

        open_request.set_onupgradeneeded(Some(onupgrade.as_ref().unchecked_ref()));
        onupgrade.forget(); // Let JS handle cleanup

        // Wait for success or error
        let db = await_idb_request::<IdbDatabase>(&open_request).await?;

        // Cache the connection
        *self.db.borrow_mut() = Some(db.clone());

        Ok(db)
    }

    /// Get the object store for a transaction.
    fn get_store(
        &self,
        db: &IdbDatabase,
        mode: IdbTransactionMode,
    ) -> StorageResult<IdbObjectStore> {
        let transaction = db
            .transaction_with_str_and_mode(STORE_NAME, mode)
            .map_err(|e| StorageError::Other(format!("Transaction error: {:?}", e)))?;

        transaction
            .object_store(STORE_NAME)
            .map_err(|e| StorageError::Other(format!("Store error: {:?}", e)))
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
        let json = match document.to_json() {
            Ok(j) => j,
            Err(e) => {
                return Box::pin(async move { Err(StorageError::Serialization(e.to_string())) });
            }
        };

        Box::pin(async move {
            let db = self.get_db().await?;
            let store = self.get_store(&db, IdbTransactionMode::Readwrite)?;

            let js_value = JsValue::from_str(&json);
            let request = store
                .put_with_key(&js_value, &JsValue::from_str(&id))
                .map_err(|e| StorageError::Other(format!("Put error: {:?}", e)))?;

            await_idb_request::<JsValue>(&request).await?;
            Ok(())
        })
    }

    fn load(&self, id: &str) -> BoxFuture<'_, StorageResult<CanvasDocument>> {
        let id = id.to_string();

        Box::pin(async move {
            let db = self.get_db().await?;
            let store = self.get_store(&db, IdbTransactionMode::Readonly)?;

            let request = store
                .get(&JsValue::from_str(&id))
                .map_err(|e| StorageError::Other(format!("Get error: {:?}", e)))?;

            let result = await_idb_request::<JsValue>(&request).await?;

            if result.is_undefined() || result.is_null() {
                return Err(StorageError::NotFound(id));
            }

            let json = result
                .as_string()
                .ok_or_else(|| StorageError::Serialization("Invalid stored data".to_string()))?;

            CanvasDocument::from_json(&json).map_err(|e| StorageError::Serialization(e.to_string()))
        })
    }

    fn delete(&self, id: &str) -> BoxFuture<'_, StorageResult<()>> {
        let id = id.to_string();

        Box::pin(async move {
            let db = self.get_db().await?;
            let store = self.get_store(&db, IdbTransactionMode::Readwrite)?;

            let request = store
                .delete(&JsValue::from_str(&id))
                .map_err(|e| StorageError::Other(format!("Delete error: {:?}", e)))?;

            await_idb_request::<JsValue>(&request).await?;
            Ok(())
        })
    }

    fn list(&self) -> BoxFuture<'_, StorageResult<Vec<String>>> {
        Box::pin(async move {
            let db = self.get_db().await?;
            let store = self.get_store(&db, IdbTransactionMode::Readonly)?;

            let request = store
                .get_all_keys()
                .map_err(|e| StorageError::Other(format!("GetAllKeys error: {:?}", e)))?;

            let result = await_idb_request::<js_sys::Array>(&request).await?;

            let mut ids = Vec::new();
            for i in 0..result.length() {
                if let Some(key) = result.get(i).as_string() {
                    ids.push(key);
                }
            }
            Ok(ids)
        })
    }

    fn exists(&self, id: &str) -> BoxFuture<'_, StorageResult<bool>> {
        let id = id.to_string();

        Box::pin(async move {
            let db = self.get_db().await?;
            let store = self.get_store(&db, IdbTransactionMode::Readonly)?;

            let request = store
                .count_with_key(&JsValue::from_str(&id))
                .map_err(|e| StorageError::Other(format!("Count error: {:?}", e)))?;

            let result = await_idb_request::<JsValue>(&request).await?;
            let count = result.as_f64().unwrap_or(0.0) as u32;
            Ok(count > 0)
        })
    }
}

/// Helper to await an IndexedDB request using a Promise.
async fn await_idb_request<T: JsCast>(request: &IdbRequest) -> StorageResult<T> {
    use wasm_bindgen_futures::JsFuture;

    let promise = js_sys::Promise::new(&mut |resolve, reject| {
        let onsuccess = Closure::once(Box::new(move |event: web_sys::Event| {
            let target = event.target().unwrap();
            let request: IdbRequest = target.unchecked_into();
            let result = request.result().unwrap();
            resolve.call1(&JsValue::NULL, &result).unwrap();
        }) as Box<dyn FnOnce(_)>);

        let onerror = Closure::once(Box::new(move |event: web_sys::Event| {
            let _target = event.target();
            // Just report a generic error since error() requires additional web-sys features
            reject
                .call1(
                    &JsValue::NULL,
                    &JsValue::from_str("IndexedDB request failed"),
                )
                .unwrap();
        }) as Box<dyn FnOnce(_)>);

        request.set_onsuccess(Some(onsuccess.as_ref().unchecked_ref()));
        request.set_onerror(Some(onerror.as_ref().unchecked_ref()));

        onsuccess.forget();
        onerror.forget();
    });

    JsFuture::from(promise)
        .await
        .map_err(|e| StorageError::Other(format!("IndexedDB request failed: {:?}", e)))?
        .dyn_into::<T>()
        .map_err(|_| StorageError::Other("Type conversion failed".to_string()))
}
