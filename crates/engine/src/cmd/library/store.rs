//! The persisted shape library: items in memory (a per-session extension) and in the `Storage` library
//! namespace.
//!
//! Layout: key `index` holds `{format: "drafftink-library", schema: 2, order: [item ids], sections: [{id, name,
//! created_ms}]}`; key `item.<id>` holds `{id, name, created_ms, section?, document}` with the item as a native
//! document. One item per key, so adding an item writes that item and the index only. The items are read on
//! first use. A record from a newer build is read as far as it can be and never overwritten.
//!
//! Sections: an item belongs to the section it was imported with ([`Home::Section`]), or to the person's own
//! items ([`Home::Mine`], which has no record: an item without `section` is theirs). Schema 1 had no sections,
//! so a library written by an earlier build reads as one "My items" section. A section with no items is dropped
//! when the library is written.

use std::collections::HashSet;
use std::sync::Arc;

use drafftink_io::limits::MAX_LIBRARY_ITEMS;
use drafftink_scene::Document;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ports::{Ns, PortWrite};
use crate::refusal::Refusal;
use crate::session::Session;

/// The schema this build reads and writes.
pub const SCHEMA: u32 = 2;
/// The key of the index.
pub const INDEX_KEY: &str = "index";
/// The prefix of the item keys.
pub const ITEM_PREFIX: &str = "item.";

/// The id of the section of the person's own items, and the name it is shown under.
pub const MINE_ID: &str = "mine";
/// The name of the section of the person's own items.
pub const MINE_NAME: &str = "My items";

/// Which section an item belongs to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Home {
    /// The person's own items: added from the canvas, or from before sections existed.
    #[default]
    Mine,
    /// An imported library; the id is that of a [`Section`].
    Section(String),
}

impl Home {
    /// The id the commands show: [`MINE_ID`] or the section's.
    pub fn id(&self) -> &str {
        match self {
            Self::Mine => MINE_ID,
            Self::Section(id) => id,
        }
    }

    fn stored(&self) -> Option<&str> {
        match self {
            Self::Mine => None,
            Self::Section(id) => Some(id),
        }
    }

    fn from_stored(id: Option<String>) -> Self {
        id.map_or(Self::Mine, Self::Section)
    }
}

/// An imported library, as a group of items. The person's own items are not a record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    /// Stable id, `sec_...`.
    pub id: String,
    /// Display name: the file the library came from, unless renamed.
    pub name: String,
    /// When it was imported, wall clock milliseconds.
    pub created_ms: u64,
}

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
    /// The section it belongs to.
    pub home: Home,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    id: String,
    name: Option<String>,
    created_ms: u64,
    /// The section id; absent for the person's own items (and in every item of schema 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    section: Option<String>,
    document: Document,
}

#[derive(Default)]
pub(super) struct LibState {
    loaded: bool,
    items: Vec<Arc<Item>>,
    sections: Vec<Section>,
    /// Counts the writes, so that a reader can tell that the library changed.
    revision: u64,
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
                        st.sections = read_sections(&index, &mut st.notes);
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
                                            home: Home::from_stored(it.section),
                                        }));
                                    }
                                    Err(e) => st.notes.push(format!("library item `{id}` could not be read and was skipped: {e}")),
                                },
                                Ok(None) => st.notes.push(format!("library item `{id}` is missing from storage and was skipped")),
                                Err(e) => st.notes.push(format!("library item `{id}` could not be read: {e}")),
                            }
                        }
                        adopt_orphans(&mut st);
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

/// The section records of the index. A schema 1 index has none.
fn read_sections(index: &Value, notes: &mut Vec<String>) -> Vec<Section> {
    let Some(list) = index.get("sections").and_then(Value::as_array) else { return Vec::new() };
    let mut sections: Vec<Section> = Vec::new();
    for record in list {
        match serde_json::from_value::<Section>(record.clone()) {
            Ok(sec) if sec.id != MINE_ID && !sections.iter().any(|s| s.id == sec.id) => sections.push(sec),
            Ok(sec) => notes.push(format!("the library section `{}` is listed twice or is reserved and was skipped", sec.id)),
            Err(e) => notes.push(format!("a library section could not be read and was skipped: {e}")),
        }
    }
    sections
}

/// Items whose section is not in the index are moved to the person's own items, so that none is lost.
fn adopt_orphans(st: &mut LibState) {
    let known: HashSet<&str> = st.sections.iter().map(|s| s.id.as_str()).collect();
    let lost = |it: &Arc<Item>| matches!(&it.home, Home::Section(id) if !known.contains(id.as_str()));
    if !st.items.iter().any(lost) {
        return;
    }
    let moved: Vec<Arc<Item>> = st
        .items
        .iter()
        .map(|it| if lost(it) { Arc::new(Item { home: Home::Mine, ..(**it).clone() }) } else { Arc::clone(it) })
        .collect();
    st.notes.push("some library items belonged to a section that is missing and were moved to My items".to_owned());
    st.items = moved;
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

/// The library as the commands change it: the items in order and the imported sections.
pub(super) struct Library {
    pub items: Vec<Arc<Item>>,
    pub sections: Vec<Section>,
}

/// The items and the section records, in order.
pub(super) fn library(s: &mut Session) -> Library {
    ensure_loaded(s);
    s.ext.with::<LibState, _>(|st| Library { items: st.items.clone(), sections: st.sections.clone() })
}

/// How many times the library was written. A reader that keeps a copy of the list compares it.
pub(crate) fn revision(s: &Session) -> u64 {
    s.ext.get::<LibState>().map_or(0, |st| st.revision)
}

/// Replaces the in-memory library and queues the storage writes that make storage match: the changed items,
/// the index, and the deletes. `touched` are the ids whose record is new or changed; `removed` are gone.
/// Sections without items are dropped here, so that the last item to leave a section takes it along.
pub(super) fn commit(s: &mut Session, mut lib: Library, touched: &[String], removed: &[String]) -> Result<(), Refusal> {
    let homes: HashSet<&str> = lib.items.iter().map(|i| i.home.id()).collect();
    lib.sections.retain(|sec| homes.contains(sec.id.as_str()));
    let mut writes: Vec<PortWrite> = Vec::new();
    for it in &lib.items {
        if touched.contains(&it.id) {
            let stored = Stored {
                id: it.id.clone(),
                name: it.name.clone(),
                created_ms: it.created_ms,
                section: it.home.stored().map(str::to_owned),
                document: it.document.clone(),
            };
            let bytes = serde_json::to_vec(&stored).map_err(|e| Refusal::internal(s.cmd(), format!("an item cannot be written: {e}")))?;
            writes.push(PortWrite::StoragePut { ns: Ns::Library, key: item_key(&it.id), bytes });
        }
    }
    let index = json!({
        "format": "drafftink-library",
        "schema": SCHEMA,
        "order": lib.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
        "sections": lib.sections,
    });
    let bytes = serde_json::to_vec(&index).map_err(|e| Refusal::internal(s.cmd(), format!("the index cannot be written: {e}")))?;
    writes.push(PortWrite::StoragePut { ns: Ns::Library, key: INDEX_KEY.to_owned(), bytes });
    for id in removed {
        writes.push(PortWrite::StorageDelete { ns: Ns::Library, key: item_key(id) });
    }
    for w in writes {
        s.queue_write(w);
    }
    s.ext.with::<LibState, _>(|st| {
        st.items = lib.items;
        st.sections = lib.sections;
        st.revision += 1;
    });
    Ok(())
}

/// A fresh section id.
pub(super) fn fresh_section_id(s: &mut Session, existing: &[Section]) -> String {
    let free = |id: &str| id != MINE_ID && !existing.iter().any(|sec| sec.id == id);
    for _ in 0..64 {
        let id = format!("sec_{}", s.ports.ids.element_id().as_str());
        if free(&id) {
            return id;
        }
    }
    // The id source keeps repeating itself: fall back to a counter that cannot repeat.
    (existing.len()..).map(|n| format!("sec_n{n}")).find(|id| free(id)).unwrap_or_default()
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
