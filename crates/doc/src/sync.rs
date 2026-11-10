//! Moving changes from Loro into the view: remote updates, undo and redo, and full rebuilds.
//!
//! Loro reports what an import or an undo touched as diff events. [`Touched`] collects the element ids, the meta
//! flag and the file ids from them; [`sync_view`] then re-reads exactly those elements from Loro and patches the
//! view with them in one view transaction, which yields the net `ChangeSet`, updates the derived indexes and raises
//! versions. The work is proportional to the number of touched elements.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use drafftink_scene::{
    ChangeSet, Color, DocMeta, ElementId, FileEntry, FileId, MemoryStore, SceneRead, SceneStore, SceneTx,
};
use loro::event::{Diff, DiffEvent};
use loro::{Container, Index, LoroValue, ValueOrContainer};

use crate::codec::read_element;
use crate::error::DocError;
use crate::schema::{META_BACKGROUND, META_NAME, ROOT_ELEMENTS, ROOT_FILES, ROOT_META, SCHEMA_VERSION};
use crate::store::{Core, LoroStore, check_size, guard, guarded};
use crate::undo::{UndoInfo, UndoStep, lock};

/// What a batch of Loro changes touched.
#[derive(Debug, Default)]
pub(crate) struct Touched {
    pub elements: BTreeSet<String>,
    pub files: BTreeSet<String>,
    pub meta: bool,
    /// Events could not be attributed; re-read everything.
    pub all: bool,
}

fn collect(t: &mut Touched, e: &DiffEvent) {
    for d in &e.events {
        let Some((_, Index::Key(root))) = d.path.first() else {
            t.all = true;
            continue;
        };
        let at_root = d.path.len() == 1;
        match &**root {
            ROOT_ELEMENTS => {
                if at_root {
                    if let Diff::Map(m) = &d.diff {
                        t.elements.extend(m.updated.keys().map(|k| k.to_string()));
                    }
                } else if let Some((_, Index::Key(id))) = d.path.get(1) {
                    t.elements.insert(id.to_string());
                }
            }
            ROOT_FILES => {
                if at_root && let Diff::Map(m) = &d.diff {
                    t.files.extend(m.updated.keys().map(|k| k.to_string()));
                }
            }
            ROOT_META => t.meta = true,
            _ => {}
        }
    }
}

/// Runs `f` while collecting what Loro reports as touched.
fn collecting<T>(core: &Core, f: impl FnOnce() -> T) -> (T, Touched) {
    let touched = Arc::new(Mutex::new(Touched::default()));
    let sink = Arc::clone(&touched);
    let sub = core.doc.subscribe_root(Arc::new(move |e| collect(&mut lock(&sink), &e)));
    let out = f();
    if core.poisoned.get() {
        // Dropping a subscription of a poisoned document would lock it again.
        std::mem::forget(sub);
    } else {
        drop(sub);
    }
    let t = std::mem::take(&mut *lock(&touched));
    (out, t)
}

fn file_from_loro(v: &LoroValue) -> Option<FileEntry> {
    let LoroValue::Map(m) = v else { return None };
    let mime = match m.get("mime")? {
        LoroValue::String(s) => s.to_string(),
        _ => return None,
    };
    let bytes = match m.get("bytes")? {
        LoroValue::Binary(b) => b.to_vec(),
        _ => return None,
    };
    Some(FileEntry { mime, bytes: Arc::from(bytes) })
}

fn read_meta(core: &Core) -> DocMeta {
    let m = core.doc.get_map(ROOT_META);
    let default = DocMeta::default();
    let get = |key: &str| match m.get(key) {
        Some(ValueOrContainer::Value(LoroValue::String(s))) => Some(s.to_string()),
        _ => None,
    };
    DocMeta {
        name: get(META_NAME).unwrap_or(default.name),
        background: get(META_BACKGROUND).and_then(|s| Color::parse(&s).ok()).unwrap_or(default.background),
    }
}

/// Makes the view equal to what Loro holds for the touched parts. Returns the net change. Reading from Loro is
/// guarded: a panic in the library poisons the document and leaves the view as it was.
pub(crate) fn sync_view(view: &mut MemoryStore, core: &mut Core, t: Touched) -> Result<ChangeSet, DocError> {
    let flag = std::rc::Rc::clone(&core.poisoned);
    guard(&flag, || sync_view_inner(view, core, t))?
}

fn sync_view_inner(view: &mut MemoryStore, core: &mut Core, mut t: Touched) -> Result<ChangeSet, DocError> {
    let elements = core.doc.get_map(ROOT_ELEMENTS);
    let files_map = core.doc.get_map(ROOT_FILES);
    if t.all {
        core.stats.full_rebuilds += 1;
        elements.for_each(|k, _| {
            t.elements.insert(k.to_owned());
        });
        t.elements.extend(view.iter_all().map(|e| e.id.to_string()));
        files_map.for_each(|k, _| {
            t.files.insert(k.to_owned());
        });
        t.files.extend(view.files().map(|(id, _)| id.to_string()));
        t.meta = true;
    }
    let mut tx = view.begin();
    for sid in &t.elements {
        let Ok(id) = ElementId::new(sid) else { continue };
        core.stats.elements_decoded += 1;
        let raw = elements.get(sid);
        let exists_in_loro = raw.is_some();
        let decoded = match raw {
            Some(ValueOrContainer::Container(Container::Map(m))) => read_element(&id, &m).ok(),
            _ => None,
        };
        let present = tx.get(&id).is_some();
        match (present, decoded) {
            (true, Some(new)) => {
                if tx.update(&id, move |e| *e = new).is_err() {
                    core.invalid.insert(id.clone());
                    let _ = tx.remove(&id);
                } else {
                    core.invalid.remove(&id);
                }
            }
            (false, Some(new)) => {
                if tx.insert(new).is_err() {
                    core.invalid.insert(id.clone());
                } else {
                    core.invalid.remove(&id);
                }
            }
            (had, None) => {
                if had {
                    let _ = tx.remove(&id);
                }
                if exists_in_loro {
                    core.invalid.insert(id.clone());
                } else {
                    core.invalid.remove(&id);
                }
            }
        }
    }
    if t.meta {
        let meta = read_meta(core);
        // A name that fails the store's own rule keeps the old name.
        let _ = tx.set_name(&meta.name);
        tx.set_background(meta.background);
    }
    for sid in &t.files {
        let Ok(fid) = FileId::new(sid) else { continue };
        let entry = match files_map.get(sid) {
            Some(ValueOrContainer::Value(v)) => file_from_loro(&v),
            _ => None,
        };
        match entry {
            // A blob whose bytes do not hash to its key is ignored: keys are content hashes.
            Some(f) if FileId::of(&f.bytes) == fid => {
                let _ = tx.put_file(f);
            }
            Some(_) => {}
            None => {
                tx.remove_file(&fid);
            }
        }
    }
    let cs = tx.commit().map_err(|e| DocError::Backend(e.to_string()))?;
    #[cfg(feature = "profile")]
    drafftink_profile::count(drafftink_profile::Counter::SyncOpsApplied, u64::try_from(cs.elements.len()).unwrap_or(0));
    Ok(cs)
}

/// Re-reads the whole document from Loro into the view (opening a snapshot).
pub(crate) fn rebuild_all(view: &mut MemoryStore, core: &mut Core) -> Result<ChangeSet, DocError> {
    let t = Touched { all: true, ..Touched::default() };
    sync_view(view, core, t)
}

/// `LoroStore::apply_remote`.
pub(crate) fn apply_remote(store: &mut LoroStore, bytes: &[u8]) -> Result<ChangeSet, DocError> {
    check_size(bytes.len(), store.core.config.max_import_bytes)?;
    if let Some(found) = store.core.blocked {
        return Err(DocError::NewerSchema { found, max: SCHEMA_VERSION });
    }
    if store.core.poisoned.get() {
        return Err(DocError::Poisoned);
    }
    if starts_later(&store.core, bytes)? {
        // A trimmed snapshot cannot be merged into a replica that lacks the history it starts from. A replica that
        // holds nothing of its own simply takes it; otherwise the caller must decide, because replacing loses
        // edits that were never sent.
        if is_pristine(store) {
            return store.replace_from_snapshot(bytes);
        }
        return Err(DocError::SnapshotStartsLater);
    }
    let LoroStore { view, core } = store;
    let (imported, touched) = collecting(core, || {
        core.guard(|| core.doc.import_with(bytes, "remote"))?.map_err(|e| DocError::Import(e.to_string()))
    });
    core.pending = imported?.pending.is_some();
    core.stats.remote_updates += 1;
    mark_remote_as_sent(core)?;
    if let Some(found) = core.guard(|| crate::schema::read_version(&core.doc))?
        && found > SCHEMA_VERSION
    {
        core.blocked = Some(found);
        return Err(DocError::NewerSchema { found, max: SCHEMA_VERSION });
    }
    let cs = sync_view(view, core, touched)?;
    core.notify(&cs);
    Ok(cs)
}

/// Operations that came from other peers are not this replica's to send: without this, the next
/// `take_unsent_update` would hand them back, and the sync client would echo every remote edit to the server.
fn mark_remote_as_sent(core: &mut Core) -> Result<(), DocError> {
    let me = core.guard(|| core.doc.peer_id())?;
    let now = core.guard(|| core.doc.oplog_vv())?;
    for (peer, end) in now.iter() {
        if *peer != me && core.sent.get(peer).is_none_or(|have| have < end) {
            core.sent.insert(*peer, *end);
        }
    }
    Ok(())
}

/// True when `bytes` is a trimmed (shallow or state-only) snapshot whose starting point this replica does not have.
fn starts_later(core: &Core, bytes: &[u8]) -> Result<bool, DocError> {
    // Not a readable blob: the import that follows reports the error.
    let Ok(meta) = guarded(|| loro::LoroDoc::decode_import_blob_meta(bytes, false))? else { return Ok(false) };
    if meta.mode != loro::EncodedBlobMode::ShallowSnapshot {
        return Ok(false);
    }
    let have = core.guard(|| core.doc.oplog_vv())?;
    Ok(!meta.start_frontiers.iter().all(|id| have.includes_id(id)))
}

/// Nothing the user made is in this replica: no elements (not even tombstones), default meta, no files, no undo.
fn is_pristine(store: &LoroStore) -> bool {
    store.view.len_all() == 0
        && store.view.files().next().is_none()
        && *store.view.meta() == DocMeta::default()
        && !store.can_undo()
        && !store.can_redo()
}

/// `LoroStore::undo` and `redo`.
pub(crate) fn undo_redo(store: &mut LoroStore, undo: bool) -> Result<Option<UndoStep>, DocError> {
    let LoroStore { view, core } = store;
    if let Some(found) = core.blocked {
        return Err(DocError::NewerSchema { found, max: SCHEMA_VERSION });
    }
    if core.poisoned.get() {
        return Err(DocError::Poisoned);
    }
    let can = core.guard(|| if undo { core.undo.can_undo() } else { core.undo.can_redo() })?;
    if !can {
        return Ok(None);
    }
    let top = core.guard(|| if undo { core.undo.top_undo_meta() } else { core.undo.top_redo_meta() })?;
    let value = top.map(|m| m.value).unwrap_or(LoroValue::Null);
    let info = UndoInfo::from_loro(&value);
    lock(&core.shared).carry = Some(value);
    let touched = Arc::new(Mutex::new(Touched::default()));
    let sink = Arc::clone(&touched);
    let flag = std::rc::Rc::clone(&core.poisoned);
    let undo_manager = &mut core.undo;
    let doc = &core.doc;
    let sub = doc.subscribe_root(Arc::new(move |e| collect(&mut lock(&sink), &e)));
    let result = guard(&flag, || if undo { undo_manager.undo() } else { undo_manager.redo() });
    if flag.get() {
        std::mem::forget(sub);
    } else {
        drop(sub);
    }
    lock(&core.shared).carry = None;
    result??;
    let t = std::mem::take(&mut *lock(&touched));
    let cs = sync_view(view, core, t)?;
    core.notify(&cs);
    let restore = if undo { info.before } else { info.after };
    Ok(Some(UndoStep { label: info.label, restore, changes: cs }))
}
