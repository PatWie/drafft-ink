//! The document store of one tab, its rollback journal, and per-tab undo history.
//!
//! **This is the only module that knows how a tab's document is held.** A tab has two stores:
//!
//! * the **working store** ([`WorkStore`], an in-memory scene): what commands read and edit. Every edit
//!   is journaled as a [`Patch`], so a refused command, a failed batch and every gesture frame can be
//!   taken back without touching anything else;
//! * the **replicated store** ([`DocStore`] = `drafftink_doc::LoroStore`): the CRDT truth, which owns
//!   undo history and sync. It is written once per command, batch or gesture end, in one transaction
//!   labelled with the command title and carrying the editor state before and after
//!   ([`TabDoc::commit_since`]). Live gesture frames and refused commands never reach it.
//!
//! Undo, redo and remote updates change the replicated store; [`TabDoc`] then copies the touched
//! elements into the working store. The cost is a second in-memory copy of the scene. A follow-up in the
//! doc crate (a transaction that stays open across a command) would remove it; nothing outside this file
//! would change.
//!
//! Known limits of this arrangement: a gesture rollback restores whole elements, so a remote edit to the
//! same element during a gesture is overwritten in the working store until the gesture ends (the final
//! command re-runs against the current state).
//!
//! **Coalescing.** A [`StepMeta`] can carry a [`CoalesceReq`]: a repeat of the same key on the same elements
//! within the window is folded into the previous undo step (`LoroTx::merge_with_previous` in the doc crate).
//! The tab remembers the last coalescing step to decide; anything else that touches undo history resets it.

use std::cell::{Ref, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use drafftink_canvas::SceneIndex;
use drafftink_doc::{DocError, LoroStore, LoroTx};
use drafftink_geom::kurbo::Point;
use drafftink_scene::{
    Change, ChangeKind, ChangeSet, DerivedIndex, DocMeta, Document, Element, ElementId, Fields, FileEntry, FileId,
    MemoryStore, SceneRead, SceneStore, SceneTx, StoreError, ZIndex,
};

use crate::fingerprint::{Feed, Fingerprint, Tracker};
use crate::state::Observed;

/// The replicated store behind a tab: the CRDT truth, undo history and sync.
pub type DocStore = LoroStore;

/// The working store commands read and edit.
pub type WorkStore = MemoryStore;

/// Everything needed to put the document back as it was: for each touched element its previous value
/// (`None`: it did not exist), and the previous metadata and blobs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Patch {
    elements: BTreeMap<ElementId, Option<Element>>,
    meta: Option<DocMeta>,
    files: BTreeMap<FileId, Option<FileEntry>>,
}

impl Patch {
    /// True when the patch touches nothing.
    pub fn is_empty(&self) -> bool {
        self.elements.is_empty() && self.meta.is_none() && self.files.is_empty()
    }

    /// Folds a patch recorded *after* this one into it: for every thing both touch, this (earlier) patch's
    /// previous value is kept, because it is the older state.
    pub fn absorb_later(&mut self, later: Patch) {
        for (id, v) in later.elements {
            self.elements.entry(id).or_insert(v);
        }
        if self.meta.is_none() {
            self.meta = later.meta;
        }
        for (id, v) in later.files {
            self.files.entry(id).or_insert(v);
        }
    }

    /// Ids of the elements the patch touches.
    pub fn touched(&self) -> impl Iterator<Item = &ElementId> {
        self.elements.keys()
    }

    /// Writes the patch into a transaction.
    fn apply<T: SceneTx>(&self, tx: &mut T) -> Result<(), StoreError> {
        for (id, prev) in &self.elements {
            match prev {
                Some(e) if tx.contains(id) => tx.update(id, |cur| *cur = e.clone())?,
                Some(e) => tx.insert(e.clone())?,
                None if tx.contains(id) => tx.remove(id)?,
                None => {}
            }
        }
        if let Some(m) = &self.meta {
            if tx.meta().name != m.name {
                tx.set_name(&m.name)?;
            }
            tx.set_background(m.background);
        }
        for (id, prev) in &self.files {
            match prev {
                Some(f) => {
                    tx.put_file(f.clone())?;
                }
                None => {
                    tx.remove_file(id);
                }
            }
        }
        Ok(())
    }
}

/// The transaction handed to commands: a store transaction that notes the previous value of everything
/// it touches. Implements [`SceneTx`], so the domain functions of the `edit` crate take it unchanged.
pub struct DocTx<'a, 'p> {
    inner: <WorkStore as SceneStore>::Tx<'a>,
    patch: &'p mut Patch,
}

impl DocTx<'_, '_> {
    fn note_element(&mut self, id: &ElementId) {
        if !self.patch.elements.contains_key(id) {
            let prev = self.inner.get(id).cloned();
            self.patch.elements.insert(id.clone(), prev);
        }
    }

    fn note_meta(&mut self) {
        if self.patch.meta.is_none() {
            self.patch.meta = Some(self.inner.meta().clone());
        }
    }

    fn note_file(&mut self, id: &FileId) {
        if !self.patch.files.contains_key(id) {
            let prev = self.inner.file(id).cloned();
            self.patch.files.insert(id.clone(), prev);
        }
    }
}

impl SceneRead for DocTx<'_, '_> {
    fn get(&self, id: &ElementId) -> Option<&Element> {
        self.inner.get(id)
    }
    fn iter_all(&self) -> impl Iterator<Item = &Element> {
        self.inner.iter_all()
    }
    fn len(&self) -> usize {
        self.inner.len()
    }
    fn len_all(&self) -> usize {
        self.inner.len_all()
    }
    fn meta(&self) -> &DocMeta {
        self.inner.meta()
    }
    fn file(&self, id: &FileId) -> Option<&FileEntry> {
        self.inner.file(id)
    }
    fn files(&self) -> impl Iterator<Item = (&FileId, &FileEntry)> {
        self.inner.files()
    }
    fn top_z(&self) -> Option<ZIndex> {
        self.inner.top_z()
    }
    fn bottom_z(&self) -> Option<ZIndex> {
        self.inner.bottom_z()
    }
    fn z_neighbors(&self, id: &ElementId) -> (Option<ZIndex>, Option<ZIndex>) {
        self.inner.z_neighbors(id)
    }
    fn derived_index(&self) -> Option<&DerivedIndex> {
        self.inner.derived_index()
    }
    fn version_stamp(&self) -> Option<u64> {
        self.inner.version_stamp()
    }
}

impl SceneTx for DocTx<'_, '_> {
    fn insert(&mut self, element: Element) -> Result<(), StoreError> {
        let id = element.id.clone();
        self.note_element(&id);
        self.inner.insert(element)
    }

    fn update<F: FnOnce(&mut Element)>(&mut self, id: &ElementId, f: F) -> Result<(), StoreError> {
        self.note_element(id);
        self.inner.update(id, f)
    }

    fn remove(&mut self, id: &ElementId) -> Result<(), StoreError> {
        self.note_element(id);
        self.inner.remove(id)
    }

    fn set_name(&mut self, name: &str) -> Result<(), StoreError> {
        self.note_meta();
        self.inner.set_name(name)
    }

    fn set_background(&mut self, color: drafftink_scene::Color) {
        self.note_meta();
        self.inner.set_background(color);
    }

    fn put_file(&mut self, entry: FileEntry) -> Result<FileId, StoreError> {
        self.note_file(&FileId::of(&entry.bytes));
        self.inner.put_file(entry)
    }

    fn remove_file(&mut self, id: &FileId) -> bool {
        self.note_file(id);
        self.inner.remove_file(id)
    }

    fn commit(self) -> Result<ChangeSet, StoreError> {
        self.inner.commit()
    }
}

/// A point in a tab's journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocMark(usize);

/// Ids of the elements committed to the working store since the spatial index last looked. Every commit adds its
/// change set, so the index re-reads the touched elements instead of comparing the whole scene with what it saw.
type TouchedFeed = Rc<RefCell<BTreeSet<ElementId>>>;

/// The spatial index of the working store and the store revision it describes (`None`: not built yet).
#[derive(Default)]
struct IndexCache {
    index: SceneIndex,
    revision: Option<u64>,
}

/// One tab's document: the working store, the replicated store and the journal of working-store edits.
pub struct TabDoc {
    work: WorkStore,
    loro: DocStore,
    journal: Vec<Patch>,
    /// Working-store commits that a rollback undid, with the rollback commits themselves. The logical
    /// revision leaves them out, so a refused command does not make the document look edited.
    undone_commits: u64,
    /// The undo depth last given to the replicated store.
    depth: usize,
    /// The last step written with a coalesce request, while it is still the top of the undo stack.
    last_coalesce: Option<LastCoalesce>,
    /// Local writes to the replicated store: commits, undo steps and redo steps. Remote updates do not count.
    /// The collab glue compares it with the value it saw last to know that the peers need an update.
    local_writes: u64,
    /// The revisions before and after each undo step, oldest first. It mirrors the undo stack of the replicated
    /// store, so that undo can tell that it brought the document back to an earlier revision.
    undo_revs: Vec<StepRevs>,
    /// The same for the redo stack.
    redo_revs: Vec<StepRevs>,
    /// Revisions whose content is the content of an earlier revision, because undo or redo restored it. The
    /// value is the earlier revision. [`TabDoc::same_content`] reads it, so that undoing back to the saved
    /// state does not show the document as changed.
    same_as: BTreeMap<u64, u64>,
    /// Elements committed to the working store since the spatial index last synced.
    touched: TouchedFeed,
    /// The same ids for a cache outside the engine (the app's paint index), drained by [`TabDoc::take_touched`].
    outside_touched: TouchedFeed,
    /// The spatial index, built on first use and kept current from `touched`.
    scene_index: RefCell<IndexCache>,
    /// What the working store changed since the content fingerprint caught up.
    print_feed: Feed,
    /// The hashes behind [`TabDoc::fingerprint`].
    print: RefCell<Tracker>,
}

/// The logical revisions around one undo step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StepRevs {
    before: u64,
    after: u64,
}

/// Most revisions remembered as equal to an earlier one. Forgetting is safe: a document then counts as changed.
const MAX_SAME_AS: usize = 4096;

impl Default for TabDoc {
    fn default() -> Self {
        Self::new()
    }
}

/// A request to fold a step into the previous one when it repeats it (`CommandSpec::coalesce`).
#[derive(Debug, Clone, Copy)]
pub struct CoalesceReq<'a> {
    /// Steps with the same key may merge.
    pub key: &'a str,
    /// Longest gap since the last step with this key, in milliseconds.
    pub window_ms: u64,
    /// The time of this step (the clock port), in milliseconds.
    pub now_ms: u64,
}

/// The step the tab last wrote with a coalesce request.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LastCoalesce {
    key: String,
    targets: Vec<ElementId>,
    t_ms: u64,
    /// Undo depth right after it: if the stack changed some other way, the step is not the top any more.
    depth: usize,
}

/// What a committed step records for undo.
pub struct StepMeta<'a> {
    /// Label of the step (the command title or the batch label).
    pub label: &'a str,
    /// False: sync the change but leave it out of undo history (document replacement).
    pub undoable: bool,
    /// Editor state before the step; undo restores it.
    pub before: &'a Observed,
    /// Editor state after the step; redo restores it.
    pub after: &'a Observed,
    /// Most undo steps to keep.
    pub depth: usize,
    /// Fold into the previous step when this repeats it.
    pub coalesce: Option<CoalesceReq<'a>>,
}

impl TabDoc {
    /// An empty document.
    pub fn new() -> Self {
        Self::assemble(WorkStore::new(), DocStore::new())
    }

    /// A tab document holding `doc`, with no undo history.
    pub fn from_document(doc: &Document) -> Result<Self, StoreError> {
        Ok(Self::assemble(WorkStore::from_document(doc)?, DocStore::from_document(doc)?))
    }

    fn assemble(mut work: WorkStore, loro: DocStore) -> Self {
        let touched = TouchedFeed::default();
        let outside_touched = TouchedFeed::default();
        let print_feed = Feed::default();
        let (feed, outside_feed, print_sink) =
            (Rc::clone(&touched), Rc::clone(&outside_touched), Rc::clone(&print_feed));
        work.subscribe(Box::new(move |cs| {
            feed.borrow_mut().extend(cs.ids().cloned());
            outside_feed.borrow_mut().extend(cs.ids().cloned());
            print_sink.borrow_mut().record(cs);
        }));
        Self {
            work,
            loro,
            journal: Vec::new(),
            undone_commits: 0,
            depth: 200,
            last_coalesce: None,
            local_writes: 0,
            undo_revs: Vec::new(),
            redo_revs: Vec::new(),
            same_as: BTreeMap::new(),
            touched,
            outside_touched,
            scene_index: RefCell::default(),
            print_feed,
            print: RefCell::default(),
        }
    }

    /// The fingerprint of the content now: elements, name, background and file ids, whatever path led here. Two
    /// states with the same content have the same fingerprint, so a document that came back to what was saved
    /// (a remote update that reverts an edit, the same edit made again) reads as unchanged. The first call reads
    /// the whole scene; later calls cost what changed since the previous one.
    pub fn fingerprint(&self) -> Fingerprint {
        match self.print.try_borrow_mut() {
            Ok(mut tracker) => tracker.print(&self.work, &self.print_feed),
            // Called from inside itself, which cannot happen; a fingerprint nothing equals reads as changed.
            Err(_) => Fingerprint::default(),
        }
    }

    /// Ids of the indexed elements whose bounds are within `reach` world units of `p`, in no particular order.
    /// It is one query of [`Self::scene_index`]; the exact rules then run on these few elements.
    pub fn near(&self, p: Point, reach: f64) -> Vec<ElementId> {
        drafftink_profile::inc(drafftink_profile::Counter::SpatialQueries);
        self.scene_index().spatial().query_point(p, reach).map(|(id, _)| id.clone()).collect()
    }

    /// The spatial index of the working store (hit testing, culling, "what is near this point").
    ///
    /// It is built from the whole scene on first use. After that, each call re-reads only the elements committed
    /// since the previous call (edits, gesture frames, rollbacks, undo, redo and remote updates all count), so
    /// the cost follows what changed and not the size of the scene. Do not call it while holding the guard it
    /// returned: the nested call would find the index borrowed and return it as it is.
    pub fn scene_index(&self) -> Ref<'_, SceneIndex> {
        if let Ok(mut cache) = self.scene_index.try_borrow_mut() {
            let revision = self.work.revision();
            if cache.revision != Some(revision) {
                let touched = std::mem::take(&mut *self.touched.borrow_mut());
                if cache.revision.is_none() {
                    cache.index.rebuild(&self.work);
                } else if !touched.is_empty() {
                    let elements = touched
                        .into_iter()
                        .map(|id| Change { id, kind: ChangeKind::Updated, fields: Fields::all(), version: 0 })
                        .collect();
                    cache.index.apply(&self.work, &ChangeSet { revision, elements, meta: false, files: Vec::new() });
                }
                cache.revision = Some(revision);
            }
        }
        Ref::map(self.scene_index.borrow(), |cache| &cache.index)
    }

    /// The ids of the elements committed to the working store since the previous call, in id order. A cache that
    /// lives outside the engine and mirrors the scene (the paint index) re-reads these instead of comparing the
    /// whole scene with what it saw: the cost follows what changed, not the size of the document. Edits, gesture
    /// frames, rollbacks, undo, redo and remote updates all count. A new cache must build from the whole scene
    /// once and call this to start from an empty list.
    pub fn take_touched(&self) -> Vec<ElementId> {
        std::mem::take(&mut *self.outside_touched.borrow_mut()).into_iter().collect()
    }

    /// Read access to the working store (what commands and the renderer read).
    pub fn store(&self) -> &WorkStore {
        &self.work
    }

    /// The schema version of a newer app that made this document read-only, if any (docs/crdt.md §8).
    pub fn blocked_by_schema(&self) -> Option<u32> {
        self.loro.blocked_by_schema()
    }

    /// The replicated store, for export and sync glue.
    pub fn replicated(&self) -> &DocStore {
        &self.loro
    }

    /// The replicated store, mutable, for sync glue that does not change the scene itself
    /// (`take_unsent_update`, trimming).
    pub fn replicated_mut(&mut self) -> &mut DocStore {
        &mut self.loro
    }

    /// The document revision: raised by every kept change, including undo, redo and remote updates. A
    /// change that was rolled back does not count.
    pub fn revision(&self) -> u64 {
        self.work.revision().saturating_sub(self.undone_commits)
    }

    /// True when revisions `a` and `b` hold the same content: they are the same, or undo or redo brought the
    /// document from one back to the other with nothing else changed in between.
    pub fn same_content(&self, a: u64, b: u64) -> bool {
        self.canonical(a) == self.canonical(b)
    }

    fn canonical(&self, mut r: u64) -> u64 {
        // An entry always points at a revision that is not itself an alias, so one hop is enough; the bound
        // keeps a corrupt map from looping.
        for _ in 0..4 {
            match self.same_as.get(&r) {
                Some(&p) => r = p,
                None => break,
            }
        }
        r
    }

    fn remember_same(&mut self, now: u64, earlier: u64) {
        if self.same_as.len() >= MAX_SAME_AS {
            self.same_as.clear();
        }
        let target = self.canonical(earlier);
        if now != target {
            self.same_as.insert(now, target);
        }
    }

    /// Checks that the revision stacks have the sizes of the stacks of the replicated store, and forgets all
    /// of them when they do not (an undo step that was dropped at the depth limit is the usual reason, and
    /// then the oldest entries go).
    fn align_revs(&mut self) {
        let (u, r) = (self.loro.undo_depth(), self.loro.redo_depth());
        if self.undo_revs.len() > u {
            let excess = self.undo_revs.len() - u;
            self.undo_revs.drain(..excess);
        }
        if self.redo_revs.len() > r {
            let excess = self.redo_revs.len() - r;
            self.redo_revs.drain(..excess);
        }
        if self.undo_revs.len() != u || self.redo_revs.len() != r {
            self.undo_revs.clear();
            self.redo_revs.clear();
        }
    }

    /// Runs `f` in one transaction. Commits and journals it when `f` returns `Ok`; drops the transaction
    /// (nothing changes) when it returns `Err`. The change set is empty when `f` changed nothing.
    pub fn edit<R, E: From<StoreError>>(
        &mut self,
        f: impl FnOnce(&mut DocTx<'_, '_>) -> Result<R, E>,
    ) -> Result<(R, ChangeSet), E> {
        let (r, cs, patch) = Self::run_tx(&mut self.work, f)?;
        if !cs.is_empty() {
            self.journal.push(patch);
        }
        Ok((r, cs))
    }

    fn run_tx<R, E: From<StoreError>>(
        store: &mut WorkStore,
        f: impl FnOnce(&mut DocTx<'_, '_>) -> Result<R, E>,
    ) -> Result<(R, ChangeSet, Patch), E> {
        let mut patch = Patch::default();
        let tx = store.begin();
        let mut jt = DocTx { inner: tx, patch: &mut patch };
        let r = f(&mut jt)?;
        let DocTx { inner, .. } = jt;
        let cs = inner.commit()?;
        Ok((r, cs, patch))
    }

    /// Applies `patch` outside the journal and returns the patch that takes it back.
    fn apply_raw(&mut self, patch: &Patch) -> Result<(ChangeSet, Patch), StoreError> {
        let (_, cs, inverse) = Self::run_tx::<(), StoreError>(&mut self.work, |tx| patch.apply(tx))?;
        Ok((cs, inverse))
    }

    /// Replaces the whole content with `doc` in one journaled transaction (document open, new, intro).
    /// The document is validated first; one with errors is refused and nothing changes.
    pub fn load(&mut self, doc: &Document) -> Result<ChangeSet, StoreError> {
        let report = doc.validate();
        if report.has_errors() {
            return Err(StoreError::InvalidDocument(report));
        }
        let existing: Vec<ElementId> = self.work.iter_all().map(|e| e.id.clone()).collect();
        let old_files: Vec<FileId> = self.work.files().map(|(id, _)| id.clone()).collect();
        let (_, cs) = self.edit::<(), StoreError>(|tx| {
            for id in &existing {
                tx.remove(id)?;
            }
            for id in old_files.iter().filter(|id| !doc.files.contains_key(*id)) {
                tx.remove_file(id);
            }
            tx.set_name(&doc.name)?;
            tx.set_background(doc.background);
            for entry in doc.files.values() {
                tx.put_file(entry.clone())?;
            }
            for e in &doc.elements {
                tx.insert(e.clone())?;
            }
            Ok(())
        })?;
        Ok(cs)
    }

    /// A copy of the content as a [`Document`].
    pub fn snapshot(&self) -> Document {
        drafftink_scene::snapshot(&self.work)
    }

    /// The content as it is now, without a copy of any element: for a job that takes longer than a frame
    /// (`drafftink_scene::JsonJob`). Later edits do not show in it.
    pub fn share_document(&self) -> drafftink_scene::SharedDocument {
        self.work.share_document()
    }

    /// The current journal position.
    pub fn mark(&self) -> DocMark {
        DocMark(self.journal.len())
    }

    /// Takes back every commit made since `mark` and returns the patches that re-apply them, in the order
    /// to apply them (a gesture uses this to keep its last good frame).
    pub fn rollback_to(&mut self, mark: DocMark) -> Result<Vec<Patch>, StoreError> {
        let mut forward = Vec::new();
        while self.journal.len() > mark.0 {
            let Some(p) = self.journal.pop() else { break };
            let (cs, redo) = self.apply_raw(&p)?;
            if !cs.is_empty() {
                // The commit being taken back and the commit that takes it back.
                self.undone_commits += 2;
            }
            forward.push(redo);
        }
        forward.reverse();
        Ok(forward)
    }

    /// Applies patches returned by [`TabDoc::rollback_to`] again, journaling them.
    pub fn reapply(&mut self, patches: &[Patch]) -> Result<(), StoreError> {
        for p in patches {
            let (cs, inverse) = self.apply_raw(p)?;
            if !cs.is_empty() {
                self.journal.push(inverse);
            }
        }
        Ok(())
    }

    /// Removes the commits made since `mark` from the journal and returns them as one patch that takes
    /// them all back (the body of an undo step), with a summary of what they did.
    pub fn take_since(&mut self, mark: DocMark) -> (Patch, Summary) {
        let tail: Vec<Patch> = self.journal.drain(mark.0.min(self.journal.len())..).collect();
        let summary = self.summarize(&tail);
        let mut merged = Patch::default();
        for p in tail {
            merged.absorb_later(p);
        }
        (merged, summary)
    }

    /// The net change of the commits since `mark` as a change set, judged against the present state: which
    /// elements were added, removed or changed, and in which field groups. The journal stays. The invariant pass
    /// reads it to see what a command did.
    pub fn changes_since(&self, mark: DocMark) -> ChangeSet {
        let tail = self.journal.get(mark.0..).unwrap_or(&[]);
        let mut first: BTreeMap<&ElementId, &Option<Element>> = BTreeMap::new();
        let mut meta: Option<&DocMeta> = None;
        let mut files: BTreeMap<&FileId, &Option<FileEntry>> = BTreeMap::new();
        for p in tail {
            for (id, prev) in &p.elements {
                first.entry(id).or_insert(prev);
            }
            if meta.is_none() {
                meta = p.meta.as_ref();
            }
            for (id, prev) in &p.files {
                files.entry(id).or_insert(prev);
            }
        }
        let mut elements = Vec::new();
        for (id, prev) in first {
            let now = self.work.get(id);
            let (kind, fields) = match (prev, now) {
                (None, Some(_)) => (ChangeKind::Added, Fields::all()),
                (Some(_), None) => (ChangeKind::Removed, Fields::all()),
                (Some(p), Some(n)) => (ChangeKind::Updated, Fields::diff(p, n)),
                (None, None) => continue,
            };
            if kind == ChangeKind::Updated && fields.is_empty() {
                continue;
            }
            elements.push(Change { id: id.clone(), kind, fields, version: now.map_or(0, |e| e.version) });
        }
        let files = files
            .into_iter()
            .filter(|(id, prev)| prev.as_ref() != self.work.file(id))
            .map(|(id, _)| id.clone())
            .collect();
        ChangeSet { revision: self.work.revision(), elements, meta: meta.is_some_and(|m| m != self.work.meta()), files }
    }

    /// Ids of the elements the commits since `mark` touched, whether or not they differ from before.
    pub fn touched_since(&self, mark: DocMark) -> BTreeSet<ElementId> {
        self.journal.get(mark.0..).unwrap_or(&[]).iter().flat_map(Patch::touched).cloned().collect()
    }

    /// Copies `ids` from the replicated store into the working store, outside the journal: an element the replicated
    /// store lacks is removed. For after a rollback that put back an older copy of an element a collaborator changed.
    pub fn reload(&mut self, ids: &[ElementId]) -> Result<(), StoreError> {
        let elements = ids
            .iter()
            .map(|id| Change { id: id.clone(), kind: ChangeKind::Updated, fields: Fields::all(), version: 0 })
            .collect();
        self.mirror(&ChangeSet { revision: self.work.revision(), elements, meta: false, files: Vec::new() })
    }

    /// What the commits since `mark` did, without removing them.
    pub fn summary_since(&self, mark: DocMark) -> Summary {
        self.summarize(self.journal.get(mark.0..).unwrap_or(&[]))
    }

    /// Writes what the commits since `mark` did to the replicated store as one transaction, labelled
    /// `meta.label` and carrying the editor state before and after. This is the one place that touches
    /// the CRDT for a command, a batch or the end of a gesture. Returns the summary of what changed.
    pub fn commit_since(&mut self, mark: DocMark, meta: &StepMeta<'_>) -> Result<Summary, StoreError> {
        // Every journaled commit raised the logical revision by one.
        let commits = self.journal.len().saturating_sub(mark.0) as u64;
        let (patch, summary) = self.take_since(mark);
        if !summary.changed {
            return Ok(summary);
        }
        let merge = self.should_merge(meta, &summary);
        let written = self.write_to_loro(&patch, meta, merge);
        if let Err(e) = written {
            // Keep the edits reachable by `rollback_to`, so the caller can take the command back.
            self.journal.push(patch);
            return Err(e);
        }
        self.note_coalesce(meta, &summary);
        if meta.undoable {
            let after = self.revision();
            match self.undo_revs.last_mut() {
                Some(top) if merge => top.after = after,
                _ => self.undo_revs.push(StepRevs { before: after.saturating_sub(commits), after }),
            }
            self.redo_revs.clear();
        }
        self.align_revs();
        Ok(summary)
    }

    /// True when `meta` asks to coalesce and the tab's last step was the same key on the same elements within the
    /// window and is still the top of the undo stack.
    fn should_merge(&self, meta: &StepMeta<'_>, summary: &Summary) -> bool {
        let (Some(req), true) = (&meta.coalesce, meta.undoable) else { return false };
        let Some(last) = &self.last_coalesce else { return false };
        last.key == req.key
            && req.now_ms.saturating_sub(last.t_ms) <= req.window_ms
            && last.targets == sorted(&summary.touched)
            && last.depth == self.loro.undo_depth()
    }

    /// Remembers the step just written when it asked to coalesce; forgets the last one otherwise.
    fn note_coalesce(&mut self, meta: &StepMeta<'_>, summary: &Summary) {
        self.last_coalesce = match (&meta.coalesce, meta.undoable) {
            (Some(req), true) => Some(LastCoalesce {
                key: req.key.to_owned(),
                targets: sorted(&summary.touched),
                t_ms: req.now_ms,
                depth: self.loro.undo_depth(),
            }),
            _ => None,
        };
    }

    fn write_to_loro(&mut self, patch: &Patch, meta: &StepMeta<'_>, merge: bool) -> Result<(), StoreError> {
        if self.depth != meta.depth {
            self.loro.set_undo_depth(meta.depth);
            self.depth = meta.depth;
        }
        let mut tx = self.loro.begin();
        if meta.undoable {
            tx.describe(meta.label, meta.before.to_state(), meta.after.to_state());
            if merge {
                tx.merge_with_previous();
            }
        } else {
            tx.not_undoable();
            tx.set_label(meta.label);
        }
        for id in patch.touched() {
            match (self.work.get(id), tx.get(id).is_some()) {
                (Some(w), true) => {
                    let w = w.clone();
                    tx.update(id, |x| {
                        let version = x.version;
                        *x = w;
                        x.version = version;
                    })?;
                }
                (Some(w), false) => tx.insert(w.clone())?,
                (None, true) => tx.remove(id)?,
                (None, false) => {}
            }
        }
        if patch.meta.is_some() {
            let m = self.work.meta();
            if tx.meta().name != m.name {
                tx.set_name(&m.name)?;
            }
            if tx.meta().background != m.background {
                tx.set_background(m.background);
            }
        }
        for id in patch.files.keys() {
            match self.work.file(id) {
                Some(f) if tx.file(id).is_none() => {
                    tx.put_file(f.clone())?;
                }
                None if tx.file(id).is_some() => {
                    tx.remove_file(id);
                }
                _ => {}
            }
        }
        tx.commit()?;
        self.local_writes += 1;
        Ok(())
    }

    /// Copies what a change set from the replicated store touched into the working store, outside the
    /// journal.
    fn mirror(&mut self, cs: &ChangeSet) -> Result<(), StoreError> {
        let loro = &self.loro;
        let mut tx = self.work.begin();
        for c in &cs.elements {
            match (c.kind, loro.get(&c.id)) {
                (ChangeKind::Removed, _) | (_, None) => {
                    if tx.contains(&c.id) {
                        tx.remove(&c.id)?;
                    }
                }
                (_, Some(e)) if tx.contains(&c.id) => {
                    let e = e.clone();
                    tx.update(&c.id, |x| {
                        let version = x.version;
                        *x = e;
                        x.version = version;
                    })?;
                }
                (_, Some(e)) => tx.insert(e.clone())?,
            }
        }
        if cs.meta {
            let m = loro.meta();
            if tx.meta().name != m.name {
                tx.set_name(&m.name)?;
            }
            tx.set_background(m.background);
        }
        for id in &cs.files {
            match loro.file(id) {
                Some(f) => {
                    tx.put_file(f.clone())?;
                }
                None => {
                    tx.remove_file(id);
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// How many times this tab wrote to the replicated store itself (commits, undo, redo). It never counts a
    /// remote update, so a change means "the peers lack something".
    pub fn local_writes(&self) -> u64 {
        self.local_writes
    }

    /// Copies a change set that the replicated store already applied (the collab client imports remote updates
    /// into it directly) into the working store.
    pub fn mirror_remote(&mut self, cs: &ChangeSet) -> Result<(), StoreError> {
        self.last_coalesce = None;
        self.mirror(cs)
    }

    /// Applies a remote update from a peer and brings the working store up to date. Returns what changed.
    pub fn apply_remote(&mut self, bytes: &[u8]) -> Result<ChangeSet, DocError> {
        let cs = self.loro.apply_remote(bytes)?;
        self.mirror(&cs).map_err(DocError::backend)?;
        Ok(cs)
    }

    /// Repairs a merged document: runs `f` in one transaction on the replicated store that undo cannot see, then
    /// copies what it changed into the working store, outside the journal. The commit is this peer's own write (the
    /// collab glue sends it, [`TabDoc::local_writes`] counts it) but is on no undo stack, and it reads the
    /// replicated truth, so a gesture that is running keeps its journal untouched. Returns what `f` returned and
    /// what the commit changed. An error from `f` writes nothing.
    pub(crate) fn repair<R, E: From<StoreError>>(
        &mut self,
        f: impl FnOnce(&mut LoroTx<'_>) -> Result<R, E>,
    ) -> Result<(R, ChangeSet), E> {
        let mut tx = self.loro.begin();
        tx.not_undoable();
        tx.set_label("Merge repair");
        let r = f(&mut tx)?;
        let cs = tx.commit()?;
        if !cs.is_empty() {
            self.local_writes += 1;
            self.last_coalesce = None;
            self.mirror(&cs)?;
        }
        Ok((r, cs))
    }

    /// Takes back up to `n` of this peer's steps. A step that has become a no-op (a collaborator already
    /// changed the same things) is consumed and not counted.
    pub fn undo(&mut self, n: usize) -> Result<UndoOutcome, StoreError> {
        self.undo_redo(n, true)
    }

    /// Re-applies up to `n` undone steps.
    pub fn redo(&mut self, n: usize) -> Result<UndoOutcome, StoreError> {
        self.undo_redo(n, false)
    }

    fn undo_redo(&mut self, n: usize, undo: bool) -> Result<UndoOutcome, StoreError> {
        self.last_coalesce = None;
        let mut out = UndoOutcome { labels: Vec::new(), observed: None, touched: Vec::new() };
        while out.labels.len() < n {
            let before_rev = self.revision();
            let step = if undo { self.loro.undo() } else { self.loro.redo() }.map_err(StoreError::backend)?;
            let Some(step) = step else { break };
            self.local_writes += 1;
            self.mirror(&step.changes)?;
            self.move_step_revs(undo, before_rev, !step.changes.is_empty());
            if step.changes.is_empty() {
                continue;
            }
            out.touched.extend(step.changes.ids().cloned());
            out.labels.push(step.label);
            out.observed = Some(Observed::from_state(&step.restore));
        }
        self.align_revs();
        Ok(out)
    }

    /// Moves the revisions of the step that undo or redo just took to the other stack. When the document was
    /// at the revision the step started from (its end for undo, its start for redo), it is now at the other
    /// one, and the revision it has now is remembered as equal to it.
    fn move_step_revs(&mut self, undo: bool, revision_before: u64, applied: bool) {
        let (from, to) =
            if undo { (&mut self.undo_revs, &mut self.redo_revs) } else { (&mut self.redo_revs, &mut self.undo_revs) };
        let Some(step) = from.pop() else { return };
        to.push(step);
        let (was_at, now_at) = if undo { (step.after, step.before) } else { (step.before, step.after) };
        if applied && self.same_content(revision_before, was_at) {
            let now = self.revision();
            self.remember_same(now, now_at);
        }
    }

    /// True when there is a step to undo.
    pub fn can_undo(&self) -> bool {
        self.loro.can_undo()
    }

    /// True when there is a step to redo.
    pub fn can_redo(&self) -> bool {
        self.loro.can_redo()
    }

    /// Label of the step undo would take back.
    pub fn undo_label(&self) -> Option<String> {
        self.loro.undo_label()
    }

    /// Label of the step redo would re-apply.
    pub fn redo_label(&self) -> Option<String> {
        self.loro.redo_label()
    }

    /// Steps on the undo stack.
    pub fn undo_depth(&self) -> usize {
        self.loro.undo_depth()
    }

    /// Steps on the redo stack.
    pub fn redo_depth(&self) -> usize {
        self.loro.redo_depth()
    }

    /// Drops both stacks.
    pub fn clear_history(&mut self) {
        self.loro.clear_history();
        self.undo_revs.clear();
        self.redo_revs.clear();
        self.last_coalesce = None;
    }

    /// Writes every journaled edit to the replicated store as one step that undo cannot see, then drops the
    /// journal. For setting up a document that has no history (a test fixture, a document built by code): the CRDT
    /// then holds what the working store holds, so a later undo restores to it and not to an empty document.
    pub fn commit_untracked(&mut self) -> Result<(), StoreError> {
        let o = Observed::default();
        let meta =
            StepMeta { label: "setup", undoable: false, before: &o, after: &o, depth: self.depth, coalesce: None };
        self.commit_since(DocMark(0), &meta).map(|_| ())
    }

    /// Drops the journal. Called when no mark is open any more.
    pub fn forget_journal(&mut self) {
        self.journal.clear();
    }

    /// True when commits since `mark` exist.
    pub fn changed_since(&self, mark: DocMark) -> bool {
        self.journal.len() > mark.0
    }

    /// What the inverse patches in `tail` (oldest first) say happened, judged against the present state.
    fn summarize(&self, tail: &[Patch]) -> Summary {
        let mut first: BTreeMap<&ElementId, &Option<Element>> = BTreeMap::new();
        let mut meta: Option<&DocMeta> = None;
        let mut files: BTreeMap<&FileId, &Option<FileEntry>> = BTreeMap::new();
        for p in tail {
            for (id, prev) in &p.elements {
                first.entry(id).or_insert(prev);
            }
            if meta.is_none() {
                meta = p.meta.as_ref();
            }
            for (id, prev) in &p.files {
                files.entry(id).or_insert(prev);
            }
        }
        let mut s = Summary::default();
        for (id, prev) in first {
            let now = self.work.get(id);
            match (prev, now) {
                (None, Some(n)) if !n.deleted => s.created.push(id.clone()),
                (Some(p), Some(n)) if !p.deleted && n.deleted => s.deleted.push(id.clone()),
                (Some(p), None) if !p.deleted => s.deleted.push(id.clone()),
                (Some(p), Some(n)) if p.deleted && !n.deleted => s.created.push(id.clone()),
                _ => {}
            }
            if prev.as_ref() != now {
                s.touched.push(id.clone());
            }
        }
        let meta_changed = meta.is_some_and(|m| m != self.work.meta());
        let files_changed = files.iter().any(|(id, prev)| prev.as_ref() != self.work.file(id));
        s.changed = !s.touched.is_empty() || meta_changed || files_changed;
        s
    }
}

fn sorted(ids: &[ElementId]) -> Vec<ElementId> {
    let mut v = ids.to_vec();
    v.sort();
    v.dedup();
    v
}

/// What a set of commits did to the elements.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Summary {
    /// Elements that now exist and did not.
    pub created: Vec<ElementId>,
    /// Elements that existed and are now gone (deleted or removed).
    pub deleted: Vec<ElementId>,
    /// Every element whose value changed.
    pub touched: Vec<ElementId>,
    /// Anything differs from before: an element, the metadata or a blob.
    pub changed: bool,
}

/// What undo or redo did.
#[derive(Debug, Clone, PartialEq)]
pub struct UndoOutcome {
    /// Labels of the steps applied, newest first for undo.
    pub labels: Vec<String>,
    /// The editor state to restore, if any step was applied.
    pub observed: Option<Observed>,
    /// Elements whose value changed.
    pub touched: Vec<ElementId>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use drafftink_scene::{ElementKind, IdSource, SeqIds, Transform};

    fn rect(ids: &mut SeqIds, z: &ZIndex) -> Element {
        let mut e = Element::new(ids.element_id(), ElementKind::Rectangle, z.clone());
        e.transform = Transform::new(0.0, 0.0, 10.0, 10.0);
        e
    }

    fn add(doc: &mut TabDoc, e: Element) {
        doc.edit::<(), StoreError>(|tx| tx.insert(e)).unwrap();
    }

    #[test]
    fn rollback_returns_to_the_mark_and_reapply_restores() {
        let mut ids = SeqIds::new();
        let mut doc = TabDoc::new();
        let a = rect(&mut ids, &ZIndex::first());
        let a_id = a.id.clone();
        add(&mut doc, a);
        let mark = doc.mark();
        doc.edit::<(), StoreError>(|tx| tx.update(&a_id, |e| e.transform.x = 50.0)).unwrap();
        doc.edit::<(), StoreError>(|tx| tx.set_name("Renamed")).unwrap();
        assert_eq!(doc.store().get(&a_id).unwrap().transform.x, 50.0);
        let forward = doc.rollback_to(mark).unwrap();
        assert_eq!(doc.store().get(&a_id).unwrap().transform.x, 0.0);
        assert_eq!(doc.store().meta().name, "Untitled");
        doc.reapply(&forward).unwrap();
        assert_eq!(doc.store().get(&a_id).unwrap().transform.x, 50.0);
        assert_eq!(doc.store().meta().name, "Renamed");
        // The reapplied frame can be rolled back again.
        doc.rollback_to(mark).unwrap();
        assert_eq!(doc.store().get(&a_id).unwrap().transform.x, 0.0);
    }

    #[test]
    fn failed_edit_changes_nothing_and_journals_nothing() {
        let mut doc = TabDoc::new();
        let mark = doc.mark();
        let r = doc.edit::<(), StoreError>(|tx| {
            tx.set_name("Half")?;
            tx.set_name("") // BadName
        });
        assert!(r.is_err());
        assert_eq!(doc.store().meta().name, "Untitled");
        assert!(!doc.changed_since(mark));
    }

    fn step<'a>(label: &'a str, before: &'a Observed, after: &'a Observed) -> StepMeta<'a> {
        StepMeta { label, undoable: true, before, after, depth: 50, coalesce: None }
    }

    #[test]
    fn commit_since_makes_one_undo_step_that_restores_the_editor_state() {
        let mut ids = SeqIds::new();
        let mut doc = TabDoc::new();
        let mark = doc.mark();
        let e = rect(&mut ids, &ZIndex::first());
        let id = e.id.clone();
        add(&mut doc, e);
        doc.edit::<(), StoreError>(|tx| tx.update(&id, |e| e.transform.x = 4.0)).unwrap();
        let before = Observed::default();
        let mut after = Observed::default();
        after.selection.insert(id.clone());
        let summary = doc.commit_since(mark, &step("Create", &before, &after)).unwrap();
        assert_eq!(summary.created, vec![id.clone()]);
        assert_eq!(doc.undo_depth(), 1, "two edits, one transaction, one step");
        assert_eq!(doc.undo_label().as_deref(), Some("Create"));
        assert_eq!(doc.replicated().get(&id).unwrap().transform.x, 4.0);

        let out = doc.undo(1).unwrap();
        assert_eq!(out.labels, ["Create"]);
        assert_eq!(out.observed, Some(before));
        assert!(doc.store().get(&id).is_none() && doc.replicated().get(&id).is_none());
        let out = doc.redo(1).unwrap();
        assert_eq!(out.observed, Some(after));
        assert_eq!(doc.store().get(&id).unwrap().transform.x, 4.0);
    }

    #[test]
    fn a_rolled_back_edit_never_reaches_the_replicated_store() {
        let mut ids = SeqIds::new();
        let mut doc = TabDoc::new();
        let e = rect(&mut ids, &ZIndex::first());
        let id = e.id.clone();
        add(&mut doc, e);
        let o = Observed::default();
        doc.commit_since(DocMark(0), &step("Add", &o, &o)).unwrap();
        let rev = doc.replicated().revision();
        let mark = doc.mark();
        doc.edit::<(), StoreError>(|tx| tx.update(&id, |e| e.transform.x = 77.0)).unwrap();
        doc.rollback_to(mark).unwrap();
        doc.commit_since(mark, &step("Nothing", &o, &o)).unwrap();
        assert_eq!(doc.replicated().revision(), rev);
        assert_eq!(doc.undo_depth(), 1);
    }

    #[test]
    fn not_undoable_commits_sync_but_leave_no_step() {
        let mut ids = SeqIds::new();
        let mut doc = TabDoc::new();
        add(&mut doc, rect(&mut ids, &ZIndex::first()));
        let o = Observed::default();
        doc.commit_since(
            DocMark(0),
            &StepMeta { label: "Open", undoable: false, before: &o, after: &o, depth: 50, coalesce: None },
        )
        .unwrap();
        assert_eq!(doc.undo_depth(), 0);
        assert_eq!(doc.replicated().len(), 1);
    }

    #[test]
    fn remote_updates_reach_the_working_store() {
        let mut ids = SeqIds::new();
        let mut a = TabDoc::new();
        let o = Observed::default();
        add(&mut a, rect(&mut ids, &ZIndex::first()));
        a.commit_since(DocMark(0), &step("Add", &o, &o)).unwrap();
        let snapshot = a.replicated().export_snapshot().unwrap();
        let mut b = TabDoc::new();
        let cs = b.apply_remote(&snapshot).unwrap();
        assert_eq!(cs.elements.len(), 1);
        assert_eq!(b.store().len(), 1, "the working store follows the replicated one");
        assert_eq!(b.undo_depth(), 0, "a remote edit is not on this peer's undo stack");
    }

    #[test]
    fn depth_is_capped_by_the_step_meta() {
        let mut ids = SeqIds::new();
        let mut doc = TabDoc::new();
        let o = Observed::default();
        for _ in 0..6 {
            let mark = doc.mark();
            let z = doc.store().next_top_index().unwrap();
            add(&mut doc, rect(&mut ids, &z));
            doc.commit_since(
                mark,
                &StepMeta { label: "Add", undoable: true, before: &o, after: &o, depth: 3, coalesce: None },
            )
            .unwrap();
        }
        assert_eq!(doc.undo_depth(), 3);
    }

    #[test]
    fn load_is_journaled_and_rolls_back() {
        let mut ids = SeqIds::new();
        let mut doc = TabDoc::new();
        let e = rect(&mut ids, &ZIndex::first());
        let keep = e.id.clone();
        add(&mut doc, e);
        let other = Document { name: "Other".into(), ..Document::default() };
        let mark = doc.mark();
        doc.load(&other).unwrap();
        assert!(doc.store().get(&keep).is_none());
        assert_eq!(doc.store().meta().name, "Other");
        doc.rollback_to(mark).unwrap();
        assert!(doc.store().get(&keep).is_some());
        assert_eq!(doc.store().meta().name, "Untitled");
    }
}
