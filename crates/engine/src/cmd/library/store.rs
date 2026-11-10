//! The persisted shape library: items in memory (a per-session extension) and in the `Storage` library
//! namespace.
//!
//! Layout: key `index` holds `{format: "drafftink-library", schema: 1, order: [item ids]}`; key `item.<id>` holds
//! `{id, name, created_ms, document}` with the item as a native document. One item per key, so adding an item
//! writes that item and the index only. The items are read on first use. A record from a newer build is
//! read as far as it can be and never overwritten.

use std::sync::Arc;

use drafftink_io::limits::MAX_LIBRARY_ITEMS;
use drafftink_scene::Document;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ports::{Ns, PortWrite};
use crate::refusal::Refusal;
use crate::session::Session;

/// The schema this build reads and writes.
pub const SCHEMA: u32 = 1;
/// The key of the index.
pub const INDEX_KEY: &str = "index";
/// The prefix of the item keys.
pub const ITEM_PREFIX: &str = "item.";

/// One library item.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Stable id, `lib_...`.
    pub id: String,
    /// Display name.
    pub name: Option<String>,
    /// When it was added, wall clock milliseconds.
    pub created_ms: u64,
    /// The content, moved to the origin.
    pub document: Document,
    /// `drafftink_edit::library::content_hash` of the content.
    pub hash: u64,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    id: String,
    name: Option<String>,
    created_ms: u64,
    document: Document,
}

#[derive(Default)]
pub(super) struct LibState {
    loaded: bool,
    items: Vec<Arc<Item>>,
    /// The stored index has a newer schema: do not write.
    newer: Option<u32>,
    /// Notes from loading, shown once with the first output.
    notes: Vec<String>,
}

pub(super) fn item_key(id: &str) -> String {
    format!("{ITEM_PREFIX}{id}")
}

/// Reads the library from storage on first use.
pub(super) fn ensure_loaded(s: &mut Session) {
    if s.ext.with::<LibState, _>(|st| st.loaded) {
        return;
    }
    let mut st = LibState { loaded: true, ..LibState::default() };
    if let Some(storage) = s.ports.storage.as_ref() {
        match storage.get(Ns::Library, INDEX_KEY) {
            Ok(Some(bytes)) => match serde_json::from_slice::<Value>(&bytes) {
                Ok(index) => {
                    let schema = index.get("schema").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).unwrap_or(0);
                    if index.get("format").and_then(Value::as_str) != Some("drafftink-library") || schema == 0 {
                        st.notes.push("the stored library index is not a library index and was ignored".to_owned());
                    } else {
                        if schema > SCHEMA {
                            st.newer = Some(schema);
                        }
                        let order: Vec<String> = index
                            .get("order")
                            .and_then(Value::as_array)
                            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
                            .unwrap_or_default();
                        for id in order.into_iter().take(MAX_LIBRARY_ITEMS) {
                            match storage.get(Ns::Library, &item_key(&id)) {
                                Ok(Some(b)) => match serde_json::from_slice::<Stored>(&b) {
                                    Ok(it) => {
                                        let hash = drafftink_edit::library::content_hash(&it.document);
                                        st.items.push(Arc::new(Item {
                                            id: it.id,
                                            name: it.name,
                                            created_ms: it.created_ms,
                                            document: it.document,
                                            hash,
                                        }));
                                    }
                                    Err(e) => st.notes.push(format!("library item `{id}` could not be read and was skipped: {e}")),
                                },
                                Ok(None) => st.notes.push(format!("library item `{id}` is missing from storage and was skipped")),
                                Err(e) => st.notes.push(format!("library item `{id}` could not be read: {e}")),
                            }
                        }
                    }
                }
                Err(e) => st.notes.push(format!("the stored library index is damaged and was ignored: {e}")),
            },
            Ok(None) => {}
            Err(e) => st.notes.push(format!("the library could not be read from storage: {e}")),
        }
    }
    s.ext.set(st);
}

/// Hands the loading notes to the running command as warnings, once.
pub(super) fn flush_notes(s: &mut Session) {
    let notes = s.ext.with::<LibState, _>(|st| std::mem::take(&mut st.notes));
    for n in notes {
        s.warn(n);
    }
}

/// The items, in order. Cheap to call: the items are shared, not copied.
pub(super) fn items(s: &mut Session) -> Vec<Arc<Item>> {
    ensure_loaded(s);
    s.ext.with::<LibState, _>(|st| st.items.clone())
}

/// Refuses writes when the stored library is from a newer build.
pub(super) fn writable(s: &mut Session, id: &str) -> Result<(), Refusal> {
    ensure_loaded(s);
    match s.ext.with::<LibState, _>(|st| st.newer) {
        Some(n) => Err(Refusal::custom(
            "library_newer",
            id,
            format!("The stored library was written by a newer drafft.ink (format {n}), so it is left alone. Update the app to change it."),
        )),
        None => Ok(()),
    }
}

/// Replaces the in-memory items and queues the storage writes that make storage match: the changed items,
/// the index, and the deletes. `touched` are the ids whose content is new or changed; `removed` are gone.
pub(super) fn commit(s: &mut Session, items: Vec<Arc<Item>>, touched: &[String], removed: &[String]) -> Result<(), Refusal> {
    let mut writes: Vec<PortWrite> = Vec::new();
    for it in &items {
        if touched.contains(&it.id) {
            let stored = Stored { id: it.id.clone(), name: it.name.clone(), created_ms: it.created_ms, document: it.document.clone() };
            let bytes = serde_json::to_vec(&stored).map_err(|e| Refusal::internal(s.cmd(), format!("an item cannot be written: {e}")))?;
            writes.push(PortWrite::StoragePut { ns: Ns::Library, key: item_key(&it.id), bytes });
        }
    }
    let index = json!({ "format": "drafftink-library", "schema": SCHEMA, "order": items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>() });
    let bytes = serde_json::to_vec(&index).map_err(|e| Refusal::internal(s.cmd(), format!("the index cannot be written: {e}")))?;
    writes.push(PortWrite::StoragePut { ns: Ns::Library, key: INDEX_KEY.to_owned(), bytes });
    for id in removed {
        writes.push(PortWrite::StorageDelete { ns: Ns::Library, key: item_key(id) });
    }
    for w in writes {
        s.queue_write(w);
    }
    s.ext.with::<LibState, _>(|st| st.items = items);
    Ok(())
}

/// A fresh item id.
pub(super) fn fresh_id(s: &mut Session, existing: &[Arc<Item>]) -> String {
    for _ in 0..64 {
        let id = format!("lib_{}", s.ports.ids.element_id().as_str());
        if !existing.iter().any(|i| i.id == id) {
            return id;
        }
    }
    // The id source keeps repeating itself: fall back to a counter that cannot repeat.
    let mut n = existing.len();
    loop {
        let id = format!("lib_n{n}");
        if !existing.iter().any(|i| i.id == id) {
            return id;
        }
        n += 1;
    }
}
