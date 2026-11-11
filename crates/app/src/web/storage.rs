//! [`WebStorage`]: the engine's synchronous `Storage` port over IndexedDB.
//!
//! IndexedDB cannot be read synchronously, so the whole store is loaded into memory before the app starts
//! ([`WebStorage::open`]). Reads are answered from memory; a write updates memory and is queued for IndexedDB,
//! which happens right after the current task, and again when the page is hidden or closed. Without IndexedDB
//! (some private modes) the storage works in memory only and says so ([`WebStorage::notice`]).
//!
//! Two tabs of the same site see the database as it was when each opened; the autosave slot belongs to
//! whichever tab wrote last.

use std::cell::RefCell;
use std::rc::Rc;

use drafftink_engine::PortError;
use drafftink_engine::ports::{Ns, Storage, StoredInfo};
use wasm_bindgen::prelude::*;
use web_sys::IdbDatabase;

use super::cache::{Cache, Pending};
use super::idb;
use super::pure::{ns_from_name, record_key};

struct Inner {
    cache: Cache,
    db: Option<IdbDatabase>,
    in_flight: bool,
    notice: Option<String>,
}

/// The shared handle onto the storage. Clones see the same data.
#[derive(Clone)]
pub struct WebStorage {
    inner: Rc<RefCell<Inner>>,
}

impl WebStorage {
    /// Opens IndexedDB and loads every entry. Never fails: if the database cannot be used the storage keeps
    /// everything in memory for this tab and [`notice`](Self::notice) says why.
    pub async fn open() -> Self {
        let mut cache = Cache::default();
        let (db, notice) = match Self::load(&mut cache).await {
            Ok(db) => (Some(db), None),
            Err(e) => {
                log::warn!("browser storage is not available, working in memory only: {e}");
                (None, Some(Self::memory_only_notice()))
            }
        };
        Self { inner: Rc::new(RefCell::new(Inner { cache, db, in_flight: false, notice })) }
    }

    fn memory_only_notice() -> String {
        "This browser blocks local storage (private mode?). Your drawing is kept only while this tab stays open. \
         Download it to keep it."
            .to_owned()
    }

    async fn load(cache: &mut Cache) -> Result<IdbDatabase, idb::IdbError> {
        let db = idb::open().await?;
        for (key, entry) in idb::load_all(&db).await? {
            let Some((ns, name)) = key.split_once('/') else { continue };
            if let Some(ns) = ns_from_name(ns) {
                cache.load(ns, name.to_owned(), entry);
            }
        }
        Ok(db)
    }

    /// A plain-words note for the user when nothing is persisted, `None` when IndexedDB works.
    pub fn notice(&self) -> Option<String> {
        self.inner.borrow().notice.clone()
    }

    /// True when writes reach IndexedDB.
    pub fn is_persistent(&self) -> bool {
        self.inner.borrow().db.is_some()
    }

    /// Writes what is waiting to IndexedDB now. Called after every change and when the page is hidden, so the
    /// browser starts the transaction while the page still exists.
    pub fn flush(&self) {
        let (db, batch) = {
            let mut inner = self.inner.borrow_mut();
            if inner.in_flight || !inner.cache.has_pending() {
                return;
            }
            let Some(db) = inner.db.clone() else {
                // Memory only: nothing to write, nothing to keep.
                inner.cache.take_pending();
                return;
            };
            inner.in_flight = true;
            (db, inner.cache.take_pending())
        };
        let this = self.clone();
        let kept = batch.clone();
        let started =
            idb::write(&db, &batch, |(ns, key)| record_key(*ns, key), move |outcome| this.finished(kept, outcome));
        if let Err(e) = started {
            self.finished(batch, Err(e));
        }
    }

    fn finished(&self, batch: Pending, outcome: Result<(), idb::IdbError>) {
        {
            let mut inner = self.inner.borrow_mut();
            inner.in_flight = false;
            if let Err(e) = &outcome {
                log::error!("could not save to browser storage: {e}");
                inner.cache.requeue(batch);
            }
        }
        // After a failure the retry waits for the next change or the next flush trigger, so a full disk
        // does not make a busy loop.
        if outcome.is_ok() {
            self.flush();
        }
    }

    /// Writes pending changes when the page goes to the background or closes.
    pub fn flush_on_page_hide(&self) {
        let Some(window) = web_sys::window() else { return };
        let hide = {
            let this = self.clone();
            Closure::<dyn FnMut()>::new(move || this.flush())
        };
        let _ = window.add_event_listener_with_callback("pagehide", hide.as_ref().unchecked_ref());
        let visibility = {
            let this = self.clone();
            Closure::<dyn FnMut()>::new(move || {
                let hidden = web_sys::window()
                    .and_then(|w| w.document())
                    .is_some_and(|d| d.visibility_state() == web_sys::VisibilityState::Hidden);
                if hidden {
                    this.flush();
                }
            })
        };
        if let Some(document) = window.document() {
            let _ = document.add_event_listener_with_callback("visibilitychange", visibility.as_ref().unchecked_ref());
        }
        // Both live as long as the page.
        hide.forget();
        visibility.forget();
    }

    fn schedule_flush(&self) {
        let this = self.clone();
        wasm_bindgen_futures::spawn_local(async move { this.flush() });
    }
}

impl Storage for WebStorage {
    fn get(&self, ns: Ns, key: &str) -> Result<Option<Vec<u8>>, PortError> {
        Ok(self.inner.borrow().cache.get(ns, key).map(|e| e.bytes.to_vec()))
    }

    fn put(&mut self, ns: Ns, key: &str, bytes: &[u8]) -> Result<(), PortError> {
        let now_ms = js_sys::Date::now() as u64;
        self.inner.borrow_mut().cache.put(ns, key, bytes, now_ms);
        self.schedule_flush();
        Ok(())
    }

    fn put_later(&mut self, ns: Ns, key: &str, bytes: Vec<u8>) -> Result<(), PortError> {
        let now_ms = js_sys::Date::now() as u64;
        self.inner.borrow_mut().cache.put_owned(ns, key, bytes, now_ms);
        self.schedule_flush();
        Ok(())
    }

    fn delete(&mut self, ns: Ns, key: &str) -> Result<(), PortError> {
        self.inner.borrow_mut().cache.delete(ns, key);
        self.schedule_flush();
        Ok(())
    }

    fn list(&self, ns: Ns) -> Result<Vec<StoredInfo>, PortError> {
        Ok(self.inner.borrow().cache.list(ns))
    }
}

impl std::fmt::Debug for WebStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebStorage").field("persistent", &self.is_persistent()).finish()
    }
}
