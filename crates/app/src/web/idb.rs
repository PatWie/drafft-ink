//! IndexedDB, as futures over `web-sys` requests. One database, one object store.

use js_sys::{Array, Object, Promise, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{IdbDatabase, IdbOpenDbRequest, IdbRequest, IdbTransactionMode};

use super::cache::{Entry, Pending};
use super::dom::js_error;

const DB_NAME: &str = "drafftink";
const DB_VERSION: u32 = 1;
const STORE: &str = "kv";

/// Why IndexedDB could not be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum IdbError {
    /// The browser has no IndexedDB, or it is switched off (some private modes).
    #[error("this browser does not offer IndexedDB")]
    Missing,
    /// A request failed.
    #[error("{0}")]
    Failed(String),
}

fn failed(value: &JsValue) -> IdbError {
    IdbError::Failed(js_error(value))
}

/// Waits for a request to finish and returns its result.
async fn finished(request: &IdbRequest) -> Result<JsValue, IdbError> {
    let promise = Promise::new(&mut |resolve, reject| {
        let source = request.clone();
        let on_success = Closure::once_into_js(move || {
            let _ = resolve.call1(&JsValue::NULL, &source.result().unwrap_or(JsValue::UNDEFINED));
        });
        let source = request.clone();
        let on_error = Closure::once_into_js(move || {
            let why = source.error().ok().flatten().map_or(JsValue::from_str("the request failed"), JsValue::from);
            let _ = reject.call1(&JsValue::NULL, &why);
        });
        request.set_onsuccess(Some(on_success.unchecked_ref()));
        request.set_onerror(Some(on_error.unchecked_ref()));
    });
    JsFuture::from(promise).await.map_err(|e| failed(&e))
}

/// Opens (and on first use creates) the database.
pub(crate) async fn open() -> Result<IdbDatabase, IdbError> {
    let window = web_sys::window().ok_or(IdbError::Missing)?;
    let factory = window.indexed_db().map_err(|_| IdbError::Missing)?.ok_or(IdbError::Missing)?;
    let request: IdbOpenDbRequest = factory.open_with_u32(DB_NAME, DB_VERSION).map_err(|e| failed(&e))?;
    let upgrade = {
        let request = request.clone();
        Closure::once_into_js(move || {
            if let Ok(db) = request.result().and_then(|r| r.dyn_into::<IdbDatabase>()) {
                // The store exists after the first run; a second create would throw, so ignore the result.
                let _ = db.create_object_store(STORE);
            }
        })
    };
    request.set_onupgradeneeded(Some(upgrade.unchecked_ref()));
    let db = finished(&request).await?;
    db.dyn_into::<IdbDatabase>().map_err(|e| failed(&e))
}

fn record(entry: &Entry) -> Result<JsValue, IdbError> {
    let object = Object::new();
    Reflect::set(&object, &"b".into(), &Uint8Array::from(entry.bytes.as_slice())).map_err(|e| failed(&e))?;
    Reflect::set(&object, &"t".into(), &JsValue::from_f64(entry.modified_ms as f64)).map_err(|e| failed(&e))?;
    Ok(object.into())
}

fn entry_of(value: &JsValue) -> Option<Entry> {
    let bytes = Reflect::get(value, &"b".into()).ok()?.dyn_into::<Uint8Array>().ok()?.to_vec();
    let modified_ms = Reflect::get(value, &"t".into()).ok()?.as_f64().map_or(0, |t| t as u64);
    Some(Entry { bytes: std::sync::Arc::new(bytes), modified_ms })
}

/// Every record as `(key, entry)`. A record that does not look like ours is skipped.
pub(crate) async fn load_all(db: &IdbDatabase) -> Result<Vec<(String, Entry)>, IdbError> {
    let tx = db.transaction_with_str(STORE).map_err(|e| failed(&e))?;
    let store = tx.object_store(STORE).map_err(|e| failed(&e))?;
    let keys_request = store.get_all_keys().map_err(|e| failed(&e))?;
    let values_request = store.get_all().map_err(|e| failed(&e))?;
    let keys = Array::from(&finished(&keys_request).await?);
    let values = Array::from(&finished(&values_request).await?);
    Ok(keys.iter().zip(values.iter()).filter_map(|(k, v)| Some((k.as_string()?, entry_of(&v)?))).collect())
}

/// Starts one transaction that applies `batch`. `done` is called with the outcome when the browser has
/// committed (or refused) it. Returns an error only when the transaction could not even start.
pub(crate) fn write(
    db: &IdbDatabase,
    batch: &Pending,
    key_of: impl Fn(&(drafftink_engine::ports::Ns, String)) -> String,
    done: impl FnOnce(Result<(), IdbError>) + 'static,
) -> Result<(), IdbError> {
    let tx = db.transaction_with_str_and_mode(STORE, IdbTransactionMode::Readwrite).map_err(|e| failed(&e))?;
    let store = tx.object_store(STORE).map_err(|e| failed(&e))?;
    for (slot, write) in batch {
        let key = JsValue::from_str(&key_of(slot));
        let request = match write {
            Some(entry) => store.put_with_key(&record(entry)?, &key),
            None => store.delete(&key),
        };
        request.map_err(|e| failed(&e))?;
    }
    let done = std::rc::Rc::new(std::cell::RefCell::new(Some(done)));
    let on_complete = {
        let done = std::rc::Rc::clone(&done);
        Closure::once_into_js(move || {
            if let Some(done) = done.borrow_mut().take() {
                done(Ok(()));
            }
        })
    };
    // A failed request aborts the transaction, so `abort` is the one failure event to listen to.
    let on_abort = {
        let tx = tx.clone();
        Closure::once_into_js(move || {
            let why = tx.error().map_or_else(|| "the browser aborted the write".to_owned(), |e| js_error(&e));
            if let Some(done) = done.borrow_mut().take() {
                done(Err(IdbError::Failed(why)));
            }
        })
    };
    tx.set_oncomplete(Some(on_complete.unchecked_ref()));
    tx.set_onabort(Some(on_abort.unchecked_ref()));
    Ok(())
}
