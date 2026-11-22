//! Moving changes from Loro into the view: remote updates, undo and redo, and full rebuilds.
//!
//! Loro reports what an import or an undo touched as diff events. [`Touched`] collects the element ids, the meta
//! flag and the file ids from them; [`sync_view`] then re-reads exactly those elements from Loro and patches the
//! view with them in one view transaction, which yields the net `ChangeSet`, updates the derived indexes and raises
//! versions. The work is proportional to the number of touched elements.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use drafftink_scene::{
    ChangeSet, Color, DocMeta, ElementId, FileId, FileManifest, MemoryStore, SceneRead, SceneStore, SceneTx, VersionId,
};
use loro::event::{Diff, DiffEvent};
use loro::{Container, ExportMode, Index, LoroValue, ValueOrContainer};

use crate::codec::read_element;
use crate::error::DocError;
use crate::files::{self, Stored};
use crate::schema::{META_BACKGROUND, META_NAME, ROOT_ELEMENTS, ROOT_FILES, ROOT_META, ROOT_VERSIONS, SCHEMA_VERSION};
use crate::staged::{Item, Staged};
use crate::store::{Core, LoroStore, check_size, guard, guarded, import_fresh_pending};
use crate::undo::{UndoInfo, UndoStep, lock};

/// What a batch of Loro changes touched.
#[derive(Debug, Default)]
pub(crate) struct Touched {
    pub elements: BTreeSet<String>,
    pub files: BTreeSet<String>,
    pub versions: BTreeSet<String>,
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
            ROOT_VERSIONS => {
                if at_root {
                    if let Diff::Map(m) = &d.diff {
                        t.versions.extend(m.updated.keys().map(|k| k.to_string()));
                    }
                } else if let Some((_, Index::Key(id))) = d.path.get(1) {
                    t.versions.insert(id.to_string());
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
    let versions_map = core.doc.get_map(ROOT_VERSIONS);
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
        t.files.extend(core.manifests.keys().map(ToString::to_string));
        versions_map.for_each(|k, _| {
            t.versions.insert(k.to_owned());
        });
        t.versions.extend(view.versions().map(|v| v.id.to_string()));
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
        if decoded.as_ref().is_some_and(|d| d.refitted) {
            core.refitted.insert(id.clone());
        } else {
            core.refitted.remove(&id);
        }
        match decoded.as_ref().map(|d| d.unknown.as_slice()).filter(|u| !u.is_empty()) {
            Some(keys) => {
                core.newer.insert(id.clone(), keys.to_vec());
            }
            None => {
                core.newer.remove(&id);
            }
        }
        let decoded = decoded.map(|d| d.element);
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
            Some(ValueOrContainer::Value(v)) => files::from_loro(&v),
            _ => None,
        };
        match entry {
            // A blob whose bytes do not hash to its key is ignored: keys are content hashes.
            Some(Stored::Full(f)) if FileId::of(&f.bytes) == fid => {
                core.set_manifest(&fid, Some(FileManifest::of(&f)));
                let _ = tx.put_file(f);
            }
            Some(Stored::Full(_)) => {}
            // A description without bytes never removes bytes the view has: they hash to the key.
            Some(Stored::Manifest(m)) => core.set_manifest(&fid, Some(m)),
            None => {
                core.set_manifest(&fid, None);
                tx.remove_file(&fid);
            }
        }
    }
    for sid in &t.versions {
        let Ok(id) = VersionId::new(sid) else { continue };
        let raw = versions_map.get(sid);
        let exists_in_loro = raw.is_some();
        let decoded = match raw {
            Some(ValueOrContainer::Container(Container::Map(m))) => crate::versions::read(&id, &m),
            _ => None,
        };
        match decoded {
            // Past the limit a version stays in Loro and out of the view, like an invalid one.
            Some(v) => {
                if tx.put_version(v).is_ok() {
                    core.invalid_versions.remove(&id);
                } else {
                    core.invalid_versions.insert(id);
                }
            }
            None => {
                tx.remove_version(&id);
                if exists_in_loro {
                    core.invalid_versions.insert(id);
                } else {
                    core.invalid_versions.remove(&id);
                }
            }
        }
    }
    let cs = tx.commit().map_err(DocError::backend)?;
    #[cfg(feature = "profile")]
    drafftink_profile::count(drafftink_profile::Counter::SyncOpsApplied, u64::try_from(cs.elements.len()).unwrap_or(0));
    Ok(cs)
}

/// Re-reads the whole document from Loro into the view (opening a snapshot).
pub(crate) fn rebuild_all(view: &mut MemoryStore, core: &mut Core) -> Result<ChangeSet, DocError> {
    let t = Touched { all: true, ..Touched::default() };
    sync_view(view, core, t)
}

/// True when `bytes` should be imported in slices: it is big and nothing of the user's is in this replica.
pub(crate) fn wants_staging(store: &LoroStore, len: usize) -> bool {
    len >= store.core.config.stage_import_bytes && store.core.staged.is_none() && is_pristine(store)
}

/// The first part of a staged import. The bytes go into a fresh CRDT document (about 8 ms for 100,000 elements),
/// and the store switches to it. Importing into the document in use would be about 400 times slower, because its undo
/// manager asks Loro for the diff of everything the import touches. The old document held nothing of the user's, but
/// it can hold history: the schema operations of this replica and whatever earlier snapshots and updates of the room
/// brought. A later update of the room builds on that history, so the fresh document takes it over first, under
/// the same peer id. What was sent stays as it was: this replica's own operations are still unsent. A trimmed
/// snapshot (`replaces`) starts a new history, so nothing is carried over for it.
/// The view is left empty for [`import_step`] to fill.
fn begin_staged(store: &mut LoroStore, bytes: &[u8], replaces: bool) -> Result<ChangeSet, DocError> {
    let config = store.core.config.clone();
    let doc = Core::make_doc(&config)?;
    let doc = if replaces {
        doc
    } else {
        let (held, own) = store.core.guard(|| {
            let held = store.core.doc.export(ExportMode::all_updates()).map_err(DocError::backend);
            (held, store.core.doc.peer_id())
        })?;
        doc.set_peer_id(own).map_err(|_| DocError::BadPeerId(own))?;
        import_fresh_pending(doc, &held?)?.0
    };
    let (doc, pending) = import_fresh_pending(doc, bytes)?;
    let migrations = std::mem::take(&mut store.core.migrations);
    let opened = Core::adopt(doc, config, migrations.clone());
    store.core.migrations = migrations;
    let fresh = opened?;
    let sent = store.core.sent.clone();
    store.core.reset_doc(fresh.doc.clone());
    store.core.sent = sent;
    mark_remote_as_sent(&mut store.core)?;
    store.core.invalid.clear();
    store.core.invalid_versions.clear();
    store.core.newer.clear();
    store.core.pending = pending;
    store.core.stats.remote_updates += 1;
    store.core.staged = Some(Staged::scan(&store.view, &store.core)?);
    Ok(ChangeSet::default())
}

/// `LoroStore::import_step`.
pub(crate) fn import_step(store: &mut LoroStore, max_items: usize) -> Result<ChangeSet, DocError> {
    let LoroStore { view, core } = store;
    let Some(mut staged) = core.staged.take() else { return Ok(ChangeSet::default()) };
    let mut touched = Touched::default();
    for item in staged.take(max_items) {
        match item {
            Item::Meta => touched.meta = true,
            Item::Element(id) => {
                touched.elements.insert(id.clone());
            }
            Item::File(id) => {
                touched.files.insert(id.clone());
            }
            Item::Version(id) => {
                touched.versions.insert(id.clone());
            }
        }
    }
    // An error leaves nothing staged: the caller reports it and the view keeps what it has.
    let cs = sync_view(view, core, touched)?;
    if !staged.is_done() {
        core.staged = Some(staged);
    }
    core.notify(&cs);
    Ok(cs)
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
            if wants_staging(store, bytes.len()) {
                return begin_staged(store, bytes, true);
            }
            return store.replace_from_snapshot(bytes);
        }
        return Err(DocError::SnapshotStartsLater);
    }
    if wants_staging(store, bytes.len()) {
        return begin_staged(store, bytes, false);
    }
    let LoroStore { view, core } = store;
    let (imported, touched) =
        collecting(core, || core.guard(|| core.doc.import_with(bytes, "remote"))?.map_err(DocError::import));
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
        && store.core.manifests.is_empty()
        && *store.view.meta() == DocMeta::default()
        && !store.can_undo()
        && !store.can_redo()
}

/// Which way [`undo_redo`] steps through the history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Undo,
    Redo,
}

/// `LoroStore::undo` and `redo`.
pub(crate) fn undo_redo(store: &mut LoroStore, direction: Direction) -> Result<Option<UndoStep>, DocError> {
    let undo = direction == Direction::Undo;
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
    let mut value = top.map(|m| m.value).unwrap_or(LoroValue::Null);
    let mut info = UndoInfo::from_loro(&value);
    core.last_merged = false;
    if let Some(after) = core.merged_after.take()
        && undo
    {
        // The top step absorbed later commits: the redo step must restore the state after the last of them.
        info.after = after;
        if let Some(v) = info.to_loro() {
            value = v;
        }
    }
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
