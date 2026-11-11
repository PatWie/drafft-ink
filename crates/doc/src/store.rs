//! `LoroStore`: the Loro-backed [`SceneStore`].
//!
//! Two copies of the scene exist and agree at all times. The Loro document is the replicated truth. A
//! [`MemoryStore`] is the read view: it serves `get`, `iter`, ordering, derived indexes and revisions at memory
//! speed. A local transaction edits the view (so reads see their own writes, rollback is free, and the net
//! `ChangeSet` comes from the view's own rules), and at commit writes exactly the changed fields into Loro in one
//! commit. Remote updates and undo change Loro first; Loro's diff events name the touched elements, and only
//! those are decoded and patched into the view.

use std::cell::Cell;
use std::collections::{BTreeSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use drafftink_scene::{
    ChangeSet, DerivedIndex, DocMeta, Document, Element, ElementId, FileEntry, FileId, MemoryStore, SceneRead,
    SceneStore, StoreError, SubscriptionId, ZIndex, load_document,
};
use loro::{ExportMode, Frontiers, LoroDoc, UndoManager, VersionVector};

use crate::error::DocError;
use crate::schema::{self, META_SCHEMA, Migration, ROOT_META, SCHEMA_VERSION, SYSTEM_ORIGIN_PREFIX};
use crate::sync::{self, Direction};
use crate::text;
use crate::tx::LoroTx;
use crate::undo::{ObservedState, UndoInfo, UndoShared, UndoStep, make_manager};

type Callback = Box<dyn FnMut(&ChangeSet)>;

/// Settings of a [`LoroStore`].
#[derive(Debug, Clone)]
pub struct LoroConfig {
    /// Most undo steps kept (default 200). The engine passes its `history.depth` setting.
    pub undo_depth: usize,
    /// Loro peer id. `None` (the default) picks a random one, which is what you want: never reuse a peer id on a
    /// second copy of the same document, because two writers with one id corrupt the history. Loro reserves
    /// `u64::MAX`; [`LoroStore::with_config`] then fails with [`DocError::BadPeerId`].
    pub peer_id: Option<u64>,
    /// Largest update or snapshot `apply_remote` / `from_snapshot` look at, in bytes.
    pub max_import_bytes: usize,
    /// An update or snapshot of at least this many bytes that lands in an empty replica is imported into the CRDT at
    /// once but decoded into the view in slices (see [`LoroStore::import_step`]), so the caller never stalls on it.
    /// Default `usize::MAX`: off, because the caller must then call `import_step` until the import is done, or the
    /// view stays incomplete. The collab client of the engine turns it on for the store it drives.
    pub stage_import_bytes: usize,
}

impl Default for LoroConfig {
    fn default() -> Self {
        Self { undo_depth: 200, peer_id: None, max_import_bytes: 256 * 1024 * 1024, stage_import_bytes: usize::MAX }
    }
}

/// Counts of work done, for tests and for the perf gate. "Incremental" means these stay proportional to what
/// changed, not to the size of the scene.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkStats {
    /// Elements read out of Loro and decoded.
    pub elements_decoded: u64,
    /// Elements written into Loro by local commits.
    pub elements_written: u64,
    /// Individual Loro fields inserted or deleted by local commits.
    pub fields_written: u64,
    /// Local commits that reached Loro.
    pub commits: u64,
    /// Remote updates applied.
    pub remote_updates: u64,
    /// Times the whole scene was decoded (opening a snapshot, replacing from a snapshot).
    pub full_rebuilds: u64,
}

/// How far a staged import has come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportProgress {
    /// Items decoded into the view so far.
    pub done: usize,
    /// All items of the import (elements, files and the meta).
    pub total: usize,
}

pub(crate) struct Core {
    pub doc: LoroDoc,
    pub undo: UndoManager,
    pub shared: Arc<Mutex<UndoShared>>,
    pub config: LoroConfig,
    pub subscribers: Vec<(SubscriptionId, Callback)>,
    pub next_subscription: u64,
    pub stats: WorkStats,
    /// Elements in Loro that failed to decode or validate. They are hidden from the view.
    pub invalid: BTreeSet<ElementId>,
    /// Frontiers just before each recent undoable local commit, oldest first.
    pub marks: VecDeque<Frontiers>,
    /// Counts local commits; stamps the undo step a commit pushes (`UndoInfo::seq`).
    pub commit_seq: u64,
    /// The latest undoable commit was folded into the step below it.
    pub last_merged: bool,
    /// The editor state after the newest commit folded into the top undo step. The step's own metadata still holds
    /// the state after its first commit; undo passes this one on to the redo step.
    pub merged_after: Option<ObservedState>,
    /// Set when the document turned out to use a newer schema: writes are refused.
    pub blocked: Option<u32>,
    /// Remote operations are waiting for missing causal predecessors.
    pub pending: bool,
    /// A Loro write failed halfway; the view is rebuilt from Loro before the next transaction.
    pub needs_resync: bool,
    /// Version vector already handed out by [`LoroStore::take_unsent_update`].
    pub sent: VersionVector,
    pub migrations: Vec<Migration>,
    /// Set when the CRDT library panicked; see [`DocError::Poisoned`].
    pub poisoned: Rc<Cell<bool>>,
    /// A big import whose elements are still being decoded into the view.
    pub staged: Option<crate::staged::Staged>,
}

/// Runs a Loro call that parses bytes from outside. Loro 1.17 asserts on some corrupted inputs (found by the
/// mutation test in `tests/hostile.rs`), so a panic becomes an error here.
pub(crate) fn guarded<T>(f: impl FnOnce() -> T) -> Result<T, DocError> {
    catch_unwind(AssertUnwindSafe(f))
        .map_err(|_| DocError::import("the data is corrupt (the CRDT library rejected it)"))
}

/// Runs `f`; a panic poisons `flag`, and a poisoned flag refuses to run anything.
pub(crate) fn guard<T>(flag: &Cell<bool>, f: impl FnOnce() -> T) -> Result<T, DocError> {
    if flag.get() {
        return Err(DocError::Poisoned);
    }
    catch_unwind(AssertUnwindSafe(f)).map_err(|_| {
        flag.set(true);
        DocError::Poisoned
    })
}

/// Decodes a version vector sent by a peer.
pub(crate) fn decode_vv(bytes: &[u8]) -> Result<VersionVector, DocError> {
    guarded(|| VersionVector::decode(bytes))?.map_err(|_| DocError::BadVersion)
}

impl Core {
    /// A new `LoroDoc` with the text mark configuration and the configured peer id.
    pub(crate) fn make_doc(config: &LoroConfig) -> Result<LoroDoc, DocError> {
        let doc = LoroDoc::new();
        text::configure(&doc);
        if let Some(p) = config.peer_id {
            doc.set_peer_id(p).map_err(|_| DocError::BadPeerId(p))?;
        }
        Ok(doc)
    }

    fn with_doc(doc: LoroDoc, config: LoroConfig, migrations: Vec<Migration>) -> Core {
        let shared = Arc::new(Mutex::new(UndoShared::default()));
        let undo = make_manager(&doc, config.undo_depth, &shared);
        Self::assemble(doc, undo, shared, config, migrations)
    }

    /// Like [`Core::with_doc`], but the undo manager watches a scratch document until [`Core::attach_undo`].
    /// Loro hands every commit to an attached manager, which makes a big commit about twice as slow; a document
    /// that is loaded in one non-undoable commit has no use for it.
    fn with_doc_unwatched(doc: LoroDoc, config: LoroConfig, migrations: Vec<Migration>) -> Core {
        let shared = Arc::new(Mutex::new(UndoShared::default()));
        let undo = make_manager(&LoroDoc::new(), config.undo_depth, &shared);
        Self::assemble(doc, undo, shared, config, migrations)
    }

    /// Makes the undo manager watch the real document, with an empty history.
    fn attach_undo(&mut self) {
        self.undo = make_manager(&self.doc, self.config.undo_depth, &self.shared);
        self.marks.clear();
    }

    fn assemble(
        doc: LoroDoc,
        undo: UndoManager,
        shared: Arc<Mutex<UndoShared>>,
        config: LoroConfig,
        migrations: Vec<Migration>,
    ) -> Core {
        let sent = doc.oplog_vv();
        Core {
            doc,
            undo,
            shared,
            config,
            subscribers: Vec::new(),
            next_subscription: 0,
            stats: WorkStats::default(),
            invalid: BTreeSet::new(),
            marks: VecDeque::new(),
            commit_seq: 0,
            last_merged: false,
            merged_after: None,
            blocked: None,
            pending: false,
            needs_resync: false,
            sent,
            migrations,
            poisoned: Rc::new(Cell::new(false)),
            staged: None,
        }
    }

    /// A new, empty document with the schema version written.
    pub fn create(config: LoroConfig, migrations: Vec<Migration>) -> Result<Core, DocError> {
        Self::create_with(config, migrations, Self::with_doc)
    }

    /// Like [`Core::create`], with the undo manager not yet watching the document (see [`Core::with_doc_unwatched`]).
    pub fn create_unwatched(config: LoroConfig, migrations: Vec<Migration>) -> Result<Core, DocError> {
        Self::create_with(config, migrations, Self::with_doc_unwatched)
    }

    fn create_with(
        config: LoroConfig,
        migrations: Vec<Migration>,
        make: fn(LoroDoc, LoroConfig, Vec<Migration>) -> Core,
    ) -> Result<Core, DocError> {
        let doc = Self::make_doc(&config)?;
        doc.get_map(ROOT_META).insert(META_SCHEMA, i64::from(SCHEMA_VERSION))?;
        doc.commit_with(loro::CommitOptions::new().origin(&format!("{SYSTEM_ORIGIN_PREFIX}init")));
        let mut core = make(doc, config, migrations);
        // Nothing of a brand-new document has been sent anywhere, the first operations included.
        core.sent = VersionVector::default();
        Ok(core)
    }

    /// Opens a snapshot or update blob: imports it, checks the schema version, migrates when it is older.
    pub fn open(bytes: &[u8], config: LoroConfig, migrations: Vec<Migration>) -> Result<Core, DocError> {
        check_size(bytes.len(), config.max_import_bytes)?;
        let doc = import_fresh(Self::make_doc(&config)?, bytes)?;
        Self::adopt(doc, config, migrations)
    }

    /// Checks the schema version of a document that holds imported data, migrates it when it is older and wraps it.
    pub(crate) fn adopt(doc: LoroDoc, config: LoroConfig, migrations: Vec<Migration>) -> Result<Core, DocError> {
        match schema::read_version(&doc) {
            None => {
                if !schema::is_blank(&doc) {
                    return Err(DocError::NotADocument(
                        "it has no schema version. Documents from the old collaboration format are not \
                         compatible; open the original file instead"
                            .into(),
                    ));
                }
                doc.get_map(ROOT_META).insert(META_SCHEMA, i64::from(SCHEMA_VERSION))?;
            }
            Some(v) => {
                if v > SCHEMA_VERSION {
                    return Err(DocError::NewerSchema { found: v, max: SCHEMA_VERSION });
                }
                schema::migrate(&doc, v, &migrations)?;
            }
        }
        doc.commit_with(loro::CommitOptions::new().origin(&format!("{SYSTEM_ORIGIN_PREFIX}open")));
        Ok(Self::with_doc(doc, config, migrations))
    }

    /// Calls every subscriber with `cs`, in subscription order.
    pub fn notify(&mut self, cs: &ChangeSet) {
        if cs.is_empty() {
            return;
        }
        let mut subscribers = std::mem::take(&mut self.subscribers);
        for (_, cb) in &mut subscribers {
            cb(cs);
        }
        // Callbacks cannot reach the store, so nothing was added meanwhile.
        self.subscribers = subscribers;
    }

    /// Replaces the Loro document, resetting everything that belongs to it.
    pub fn reset_doc(&mut self, doc: LoroDoc) {
        self.shared = Arc::new(Mutex::new(UndoShared::default()));
        let undo = make_manager(&doc, self.config.undo_depth, &self.shared);
        self.sent = doc.oplog_vv();
        let old_undo = std::mem::replace(&mut self.undo, undo);
        let old_doc = std::mem::replace(&mut self.doc, doc);
        if self.poisoned.get() {
            // Dropping a poisoned document panics; leak it.
            std::mem::forget(old_undo);
            std::mem::forget(old_doc);
        }
        self.marks.clear();
        self.last_merged = false;
        self.merged_after = None;
        self.pending = false;
        self.blocked = None;
        self.needs_resync = false;
        self.staged = None;
        self.poisoned.set(false);
    }

    /// Runs a call into the CRDT library. A panic inside it poisons the document: the library's locks and caches are
    /// in an unknown state afterwards.
    pub fn guard<T>(&self, f: impl FnOnce() -> T) -> Result<T, DocError> {
        guard(&self.poisoned, f)
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        if self.poisoned.get() {
            // Dropping a document or undo manager whose lock a panic poisoned panics again. Leak them instead.
            let fresh = LoroDoc::new();
            let undo = std::mem::replace(&mut self.undo, UndoManager::new(&fresh));
            let doc = std::mem::replace(&mut self.doc, fresh);
            std::mem::forget(undo);
            std::mem::forget(doc);
        }
    }
}

pub(crate) fn check_size(size: usize, limit: usize) -> Result<(), DocError> {
    if size > limit { Err(DocError::TooLarge { size, limit }) } else { Ok(()) }
}

/// Imports bytes into a document that is not in use yet (a fresh one), without ever panicking. Returns the document.
/// A document whose import panicked is leaked on purpose: dropping a `LoroDoc` with a poisoned lock panics.
pub(crate) fn import_fresh(doc: LoroDoc, bytes: &[u8]) -> Result<LoroDoc, DocError> {
    match catch_unwind(AssertUnwindSafe(|| doc.import_with(bytes, "remote"))) {
        Ok(Ok(_)) => Ok(doc),
        Ok(Err(e)) => Err(DocError::import(e)),
        Err(_) => {
            std::mem::forget(doc);
            Err(DocError::import("the data is corrupt (the CRDT library rejected it)"))
        }
    }
}

/// Like [`import_fresh`], and also says whether some imported operations wait for causal predecessors.
pub(crate) fn import_fresh_pending(doc: LoroDoc, bytes: &[u8]) -> Result<(LoroDoc, bool), DocError> {
    match catch_unwind(AssertUnwindSafe(|| doc.import_with(bytes, "remote"))) {
        Ok(Ok(status)) => Ok((doc, status.pending.is_some())),
        Ok(Err(e)) => Err(DocError::import(e)),
        Err(_) => {
            std::mem::forget(doc);
            Err(DocError::import("the data is corrupt (the CRDT library rejected it)"))
        }
    }
}

/// The scene store backed by a Loro document. See the module docs and `docs/crdt.md`.
pub struct LoroStore {
    pub(crate) view: MemoryStore,
    pub(crate) core: Core,
}

impl std::fmt::Debug for LoroStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoroStore")
            .field("elements", &self.view.len_all())
            .field("revision", &self.view.revision())
            .field("poisoned", &self.core.poisoned.get())
            .finish()
    }
}

impl Default for LoroStore {
    fn default() -> Self {
        Self::new()
    }
}

impl LoroStore {
    /// An empty scene named "Untitled" with a white background.
    pub fn new() -> Self {
        // A random peer id is never refused, so `with_config` fails here only when Loro cannot write the schema
        // stamp. The store then starts from a bare document instead of failing a constructor with no error.
        Self::with_config(LoroConfig::default()).unwrap_or_else(|_| Self {
            view: MemoryStore::new(),
            core: Core::with_doc(LoroDoc::new(), LoroConfig::default(), Vec::new()),
        })
    }

    /// An empty scene with explicit settings.
    ///
    /// Fails with [`DocError::BadPeerId`] when `config.peer_id` is an id Loro refuses (the reserved `u64::MAX`).
    pub fn with_config(config: LoroConfig) -> Result<Self, DocError> {
        let core = Core::create(config, Vec::new())?;
        Ok(Self { view: MemoryStore::new(), core })
    }

    /// A store holding `doc`, loaded as one non-undoable commit. The document is validated first.
    pub fn from_document(doc: &Document) -> Result<Self, StoreError> {
        let config = LoroConfig::default();
        let core = Core::create_unwatched(config.clone(), Vec::new())
            .or_else(|_| Core::create_unwatched(LoroConfig { peer_id: None, ..config.clone() }, Vec::new()))
            .unwrap_or_else(|_| Core::with_doc(LoroDoc::new(), config, Vec::new()));
        let mut store = Self { view: MemoryStore::new(), core };
        load_document(&mut store, doc)?;
        // The load is not an undo step, and Loro would not have seen it as one: start watching from here.
        store.core.attach_undo();
        store.clear_history();
        Ok(store)
    }

    /// Opens a Loro snapshot (or a full update blob). Migrates older schema versions, refuses newer ones, and
    /// refuses data that is not a drafft.ink document. Decodes every element once.
    pub fn from_snapshot(bytes: &[u8]) -> Result<Self, DocError> {
        Self::from_snapshot_with(bytes, LoroConfig::default())
    }

    /// Like [`LoroStore::from_snapshot`] with explicit settings.
    pub fn from_snapshot_with(bytes: &[u8], config: LoroConfig) -> Result<Self, DocError> {
        Self::open_with_migrations(bytes, config, schema::MIGRATIONS)
    }

    /// Like [`LoroStore::from_snapshot_with`] with an explicit migration table (the built-in table is
    /// [`schema::MIGRATIONS`]).
    pub fn open_with_migrations(bytes: &[u8], config: LoroConfig, table: &[Migration]) -> Result<Self, DocError> {
        let core = Core::open(bytes, config, table.to_vec())?;
        let mut store = Self { view: MemoryStore::new(), core };
        sync::rebuild_all(&mut store.view, &mut store.core)?;
        Ok(store)
    }

    /// Replaces the whole content with the one in `bytes` (a snapshot, for example a shallow one from the
    /// server after the history this peer needs was trimmed). Returns the net change of the visible scene,
    /// element by element. The undo history is cleared and local changes that were never sent are lost.
    pub fn replace_from_snapshot(&mut self, bytes: &[u8]) -> Result<ChangeSet, DocError> {
        let migrations = std::mem::take(&mut self.core.migrations);
        let opened = Core::open(bytes, self.core.config.clone(), migrations.clone());
        self.core.migrations = migrations;
        let fresh = opened?;
        self.core.reset_doc(fresh.doc.clone());
        self.core.invalid.clear();
        let cs = sync::rebuild_all(&mut self.view, &mut self.core)?;
        self.core.notify(&cs);
        Ok(cs)
    }

    /// Turns the staging of big imports on (`bytes` is the smallest message that is staged) or off (`usize::MAX`).
    /// While it is on, the owner must call [`LoroStore::import_step`] until [`LoroStore::import_progress`] is `None`.
    pub fn set_stage_import_bytes(&mut self, bytes: usize) {
        self.core.config.stage_import_bytes = bytes;
    }

    /// True when a message of `len` bytes would be staged now: staging is on, `len` reaches the threshold and the
    /// replica is empty. A shell uses it to decide whether a big message needs a "loading" frame first.
    pub fn would_stage(&self, len: usize) -> bool {
        sync::wants_staging(self, len)
    }

    /// How far a staged import has come, or `None` when no import is waiting. The CRDT already holds everything; the
    /// view and everything built on it catch up with [`LoroStore::import_step`].
    pub fn import_progress(&self) -> Option<ImportProgress> {
        self.core.staged.as_ref().map(crate::staged::Staged::progress)
    }

    /// Decodes up to `max_items` more elements (or files, or the document meta) of a staged import into the view and
    /// returns what changed. Does nothing when no import is staged. Call it until [`LoroStore::import_progress`]
    /// returns `None`; a slice of a few hundred elements takes a few milliseconds.
    pub fn import_step(&mut self, max_items: usize) -> Result<ChangeSet, DocError> {
        sync::import_step(self, max_items)
    }

    /// Applies an update or snapshot from a peer. Only the elements the update touched are decoded and patched
    /// into the view; nothing is rebuilt. Returns the net change. Hostile or malformed bytes return an error and
    /// change nothing visible.
    pub fn apply_remote(&mut self, bytes: &[u8]) -> Result<ChangeSet, DocError> {
        sync::apply_remote(self, bytes)
    }

    /// True while some received operations wait for causal predecessors that have not arrived. The sync client
    /// should then ask the server for the missing range (`export_updates_since(version_vector())`).
    pub fn has_pending_remote(&self) -> bool {
        self.core.pending
    }

    /// Elements that exist in the CRDT but are hidden because they failed to decode or validate.
    pub fn invalid_elements(&self) -> impl Iterator<Item = &ElementId> {
        self.core.invalid.iter()
    }

    /// The raw content of the Loro document as JSON (containers resolved). For tests, tools and bug reports; not
    /// a stable format.
    pub fn debug_json(&self) -> serde_json::Value {
        use loro::ToJson;
        self.core.guard(|| self.core.doc.get_deep_value().to_json_value()).unwrap_or(serde_json::Value::Null)
    }

    /// Work counters. See [`WorkStats`].
    pub fn stats(&self) -> WorkStats {
        self.core.stats
    }

    /// Sets the work counters to zero.
    pub fn reset_stats(&mut self) {
        self.core.stats = WorkStats::default();
    }

    /// This replica's Loro peer id. Fails with [`DocError::Poisoned`] on a store whose CRDT state is lost.
    pub fn peer_id(&self) -> Result<u64, DocError> {
        self.core.guard(|| self.core.doc.peer_id())
    }

    /// The newer schema version that made this store read-only, if any. A peer running a newer app raised
    /// `meta.schema`; the scene shown is the last one this build understood. Edits, undo and further updates return
    /// [`DocError::NewerSchema`] ("Update the app") until `replace_from_snapshot` succeeds.
    pub fn blocked_by_schema(&self) -> Option<u32> {
        self.core.blocked
    }

    /// True after the CRDT library panicked on some input. The scene can still be read, but edits, sync and undo
    /// return [`DocError::Poisoned`] until [`LoroStore::replace_from_snapshot`] succeeds.
    pub fn is_poisoned(&self) -> bool {
        self.core.poisoned.get()
    }

    /// The schema version of the open document.
    pub fn schema_version(&self) -> u32 {
        self.core.guard(|| schema::read_version(&self.core.doc)).ok().flatten().unwrap_or(SCHEMA_VERSION)
    }

    // ---- export -----------------------------------------------------------------------------------------------

    /// This replica's version vector, encoded. Opaque to callers; send it in `Join`. Fails with
    /// [`DocError::Poisoned`] on a store whose CRDT state is lost: an empty vector would make a peer send the
    /// whole room to a store that cannot take it.
    pub fn version_vector(&self) -> Result<Vec<u8>, DocError> {
        self.core.guard(|| self.core.doc.oplog_vv().encode())
    }

    /// Every operation the peer with version vector `vv` lacks. An empty `vv` means "nothing" and yields all
    /// history this replica still has. Fails with [`DocError::HistoryTrimmed`] when the peer is older than the
    /// kept history; send a snapshot then.
    pub fn export_updates_since(&self, vv: &[u8]) -> Result<Vec<u8>, DocError> {
        let from = if vv.is_empty() { VersionVector::default() } else { decode_vv(vv)? };
        self.core.guard(|| {
            if self.core.doc.is_shallow() && !from.includes_vv(&self.shallow_start_vv()) {
                return Err(DocError::HistoryTrimmed);
            }
            self.core.doc.export(ExportMode::updates(&from)).map_err(DocError::backend)
        })?
    }

    fn shallow_start_vv(&self) -> VersionVector {
        let im = self.core.doc.shallow_since_vv();
        let mut vv = VersionVector::default();
        for (peer, counter) in im.iter() {
            vv.insert(*peer, *counter);
        }
        vv
    }

    /// The operations committed since the last call, or `None` when there are none. A store made by `new` starts
    /// with nothing sent; a store opened from a snapshot starts with everything in the snapshot counted as sent.
    /// This is what the sync client sends after each commit.
    pub fn take_unsent_update(&mut self) -> Result<Option<Vec<u8>>, DocError> {
        let (now, bytes) = self.core.guard(|| {
            let now = self.core.doc.oplog_vv();
            if now == self.core.sent {
                return Ok::<_, DocError>((now, None));
            }
            let bytes = self.core.doc.export(ExportMode::updates(&self.core.sent)).map_err(DocError::backend)?;
            Ok((now, Some(bytes)))
        })??;
        if bytes.is_some() {
            self.core.sent = now;
        }
        Ok(bytes)
    }

    /// The whole document with all its history. Largest, and the only form that lets peers merge any past
    /// version.
    pub fn export_snapshot(&self) -> Result<Vec<u8>, DocError> {
        self.core.guard(|| self.core.doc.export(ExportMode::Snapshot).map_err(DocError::backend))?
    }

    /// The current state with no history at all (the smallest snapshot). Whoever imports it can read and edit
    /// but cannot merge versions older than this one.
    pub fn export_state_only(&self) -> Result<Vec<u8>, DocError> {
        self.core.guard(|| self.core.doc.export(ExportMode::state_only(None)).map_err(DocError::backend))?
    }

    /// The current state with the history of the last `keep_commits` local commits (all of it when fewer exist,
    /// none when 0). Late joiners get this; see `docs/crdt.md` for the trade-offs.
    pub fn export_shallow_snapshot(&self, keep_commits: usize) -> Result<Vec<u8>, DocError> {
        self.core.guard(|| {
            let frontiers = self.keep_frontiers(keep_commits);
            self.core.doc.export(ExportMode::shallow_snapshot(&frontiers)).map_err(DocError::backend)
        })?
    }

    pub(crate) fn keep_frontiers(&self, keep_commits: usize) -> Frontiers {
        if keep_commits > 0 && !self.core.marks.is_empty() {
            let idx = self.core.marks.len().saturating_sub(keep_commits);
            if let Some(f) = self.core.marks.get(idx) {
                return f.clone();
            }
        }
        self.core.guard(|| self.core.doc.oplog_frontiers()).unwrap_or_default()
    }

    // ---- undo -------------------------------------------------------------------------------------------------

    /// Number of undo steps available.
    pub fn undo_depth(&self) -> usize {
        self.core.guard(|| self.core.undo.undo_count()).unwrap_or(0)
    }

    /// Number of redo steps available.
    pub fn redo_depth(&self) -> usize {
        self.core.guard(|| self.core.undo.redo_count()).unwrap_or(0)
    }

    /// True when there is something to undo.
    pub fn can_undo(&self) -> bool {
        self.core.guard(|| self.core.undo.can_undo()).unwrap_or(false)
    }

    /// True when there is something to redo.
    pub fn can_redo(&self) -> bool {
        self.core.guard(|| self.core.undo.can_redo()).unwrap_or(false)
    }

    /// The label of the step undo would revert.
    pub fn undo_label(&self) -> Option<String> {
        self.core.guard(|| self.core.undo.top_undo_meta()).ok().flatten().map(|m| UndoInfo::from_loro(&m.value).label)
    }

    /// The label of the step redo would re-apply.
    pub fn redo_label(&self) -> Option<String> {
        self.core.guard(|| self.core.undo.top_redo_meta()).ok().flatten().map(|m| UndoInfo::from_loro(&m.value).label)
    }

    /// Drops the undo and redo stacks (opening a document, joining a room with `discard`).
    pub fn clear_history(&mut self) {
        let _ = self.core.guard(|| self.core.undo.clear());
        self.core.marks.clear();
        self.core.last_merged = false;
        self.core.merged_after = None;
    }

    /// True when the latest undoable commit was folded into the undo step below it (see
    /// [`LoroTx::merge_with_previous`](crate::LoroTx::merge_with_previous)). False after any other commit, undo,
    /// redo or `clear_history`.
    pub fn last_commit_merged(&self) -> bool {
        self.core.last_merged
    }

    /// Changes the most undo steps kept. Takes effect for the steps pushed from now on.
    pub fn set_undo_depth(&mut self, depth: usize) {
        self.core.config.undo_depth = depth;
        let flag = Rc::clone(&self.core.poisoned);
        let undo = &mut self.core.undo;
        let _ = guard(&flag, || undo.set_max_undo_steps(depth));
    }

    /// Reverts this peer's most recent step that has not been undone. Never reverts a collaborator's edit.
    /// `Ok(None)` when there is nothing to undo.
    pub fn undo(&mut self) -> Result<Option<UndoStep>, DocError> {
        sync::undo_redo(self, Direction::Undo)
    }

    /// Re-applies the step most recently undone. `Ok(None)` when there is nothing to redo.
    pub fn redo(&mut self) -> Result<Option<UndoStep>, DocError> {
        sync::undo_redo(self, Direction::Redo)
    }

    /// The observed editor state recorded with the step undo would revert (`before`), if any.
    pub fn undo_restore_state(&self) -> Option<ObservedState> {
        self.core.guard(|| self.core.undo.top_undo_meta()).ok().flatten().map(|m| UndoInfo::from_loro(&m.value).before)
    }
}

impl SceneRead for LoroStore {
    fn get(&self, id: &ElementId) -> Option<&Element> {
        self.view.get(id)
    }
    fn iter_all(&self) -> impl Iterator<Item = &Element> {
        self.view.iter_all()
    }
    fn len(&self) -> usize {
        self.view.len()
    }
    fn len_all(&self) -> usize {
        self.view.len_all()
    }
    fn meta(&self) -> &DocMeta {
        self.view.meta()
    }
    fn file(&self, id: &FileId) -> Option<&FileEntry> {
        self.view.file(id)
    }
    fn files(&self) -> impl Iterator<Item = (&FileId, &FileEntry)> {
        self.view.files()
    }
    fn top_z(&self) -> Option<ZIndex> {
        self.view.top_z()
    }
    fn bottom_z(&self) -> Option<ZIndex> {
        self.view.bottom_z()
    }
    fn z_neighbors(&self, id: &ElementId) -> (Option<ZIndex>, Option<ZIndex>) {
        self.view.z_neighbors(id)
    }
    fn derived_index(&self) -> Option<&DerivedIndex> {
        self.view.derived_index()
    }
    fn version_stamp(&self) -> Option<u64> {
        self.view.version_stamp()
    }
}

impl SceneStore for LoroStore {
    type Tx<'a> = LoroTx<'a>;

    fn begin(&mut self) -> LoroTx<'_> {
        let LoroStore { view, core } = self;
        if core.needs_resync {
            // A previous commit failed in Loro after the view was updated. Make the view equal to Loro again.
            if sync::rebuild_all(view, core).is_ok() {
                core.needs_resync = false;
            }
        }
        LoroTx::new(view.begin(), core)
    }

    fn revision(&self) -> u64 {
        self.view.revision()
    }

    fn derived(&self) -> &DerivedIndex {
        self.view.derived()
    }

    fn subscribe(&mut self, callback: Box<dyn FnMut(&ChangeSet)>) -> SubscriptionId {
        self.core.next_subscription += 1;
        let id = SubscriptionId(self.core.next_subscription);
        self.core.subscribers.push((id, callback));
        id
    }

    fn unsubscribe(&mut self, id: SubscriptionId) -> bool {
        let before = self.core.subscribers.len();
        self.core.subscribers.retain(|(s, _)| *s != id);
        self.core.subscribers.len() != before
    }
}

#[cfg(test)]
mod tests {
    use drafftink_scene::{Element, ElementKind, SceneTx};

    use super::*;

    fn rect(id: &str) -> Element {
        Element::new(ElementId::new(id).unwrap(), ElementKind::Rectangle, ZIndex::first())
            .with_bounds(0.0, 0.0, 10.0, 10.0)
    }

    /// A panic inside a guarded call poisons the store: reads work, everything that needs the CRDT is refused with
    /// `Poisoned`, replacing the document recovers, and dropping a poisoned store does not panic.
    #[test]
    fn a_panic_in_the_crdt_layer_poisons_the_store_and_replacing_recovers() {
        let mut s = LoroStore::new();
        let mut tx = s.begin();
        tx.insert(rect("a")).unwrap();
        tx.commit().unwrap();
        let good = s.export_snapshot().unwrap();
        assert_eq!(
            s.core.guard(|| -> u8 { panic!("simulated failure inside the CRDT library") }),
            Err(DocError::Poisoned)
        );
        assert!(s.is_poisoned());
        assert_eq!(s.len(), 1, "reads still work");

        let mut tx = s.begin();
        tx.update(&ElementId::new("a").unwrap(), |e| e.transform.x = 5.0).unwrap();
        assert!(tx.commit().is_err());
        assert_eq!(s.apply_remote(&good).unwrap_err(), DocError::Poisoned);
        assert_eq!(s.undo().unwrap_err(), DocError::Poisoned);
        assert!(s.export_snapshot().is_err() && s.export_updates_since(&[]).is_err());
        assert!(s.version_vector() == Err(DocError::Poisoned) && !s.can_undo() && s.take_unsent_update().is_err());

        s.replace_from_snapshot(&good).unwrap();
        assert!(!s.is_poisoned());
        let mut tx = s.begin();
        tx.update(&ElementId::new("a").unwrap(), |e| e.transform.x = 6.0).unwrap();
        tx.commit().unwrap();
        s.export_snapshot().unwrap();

        // And a store that stays poisoned can be dropped.
        let _ = s.core.guard(|| -> u8 { panic!("again") });
        drop(s);
    }
}
