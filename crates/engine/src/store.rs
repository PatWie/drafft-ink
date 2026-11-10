//! The document store of one tab, its rollback journal, and per-tab undo history.
//!
//! **This is the only module that knows which store backs a tab.** Today it is the in-memory store; when
//! the Loro store lands, `DocStore`, [`Patch`], [`TabDoc`] and [`UndoHistory`] change and nothing else
//! does. The contract the rest of the engine relies on:
//!
//! * [`TabDoc::edit`] is the only way to change a document. It commits one transaction and records how to
//!   take it back.
//! * [`TabDoc::mark`] / [`TabDoc::rollback_to`] return the document to an earlier point (command
//!   failure, batch failure, gesture frames, gesture cancel). The Loro version remembers frontiers.
//! * [`UndoHistory`] holds the user-visible steps. A step carries the observed editor state from before
//!   and after it (selection, editing group), and restores it on undo and redo.
//!
//! With the in-memory store a [`Patch`] holds the previous value of every touched element, so rollback,
//! undo and redo cost what the command touched, not the size of the document.

use std::collections::BTreeMap;

use drafftink_scene::{
    Change, ChangeKind, ChangeSet, DocMeta, Document, Element, ElementId, FileEntry, FileId, MemoryStore, SceneRead,
    SceneStore, SceneTx, StoreError, ZIndex,
};

use crate::state::Observed;

/// The store behind a tab. The coordinator switches this to the Loro store together with the bodies of
/// [`Patch`], [`TabDoc`] and [`UndoHistory`].
pub type DocStore = MemoryStore;

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
    inner: <DocStore as SceneStore>::Tx<'a>,
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

/// One tab's document: the store plus the journal of patches committed since the oldest open mark.
pub struct TabDoc {
    store: DocStore,
    journal: Vec<Patch>,
    /// Store commits that a rollback undid, with the rollback commits themselves. The logical revision
    /// leaves them out, so a refused command does not make the document look edited.
    undone_commits: u64,
}

impl Default for TabDoc {
    fn default() -> Self {
        Self::new()
    }
}

impl TabDoc {
    /// An empty document.
    pub fn new() -> Self {
        Self { store: DocStore::new(), journal: Vec::new(), undone_commits: 0 }
    }

    /// A tab document holding `doc`.
    pub fn from_document(doc: &Document) -> Result<Self, StoreError> {
        Ok(Self { store: DocStore::from_document(doc)?, journal: Vec::new(), undone_commits: 0 })
    }

    /// Read access to the store.
    pub fn store(&self) -> &DocStore {
        &self.store
    }

    /// The document revision: raised by every kept change, including undo and redo. A change that was
    /// rolled back does not count.
    pub fn revision(&self) -> u64 {
        self.store.revision().saturating_sub(self.undone_commits)
    }

    /// Runs `f` in one transaction. Commits and journals it when `f` returns `Ok`; drops the transaction
    /// (nothing changes) when it returns `Err`. The change set is empty when `f` changed nothing.
    pub fn edit<R, E: From<StoreError>>(
        &mut self,
        f: impl FnOnce(&mut DocTx<'_, '_>) -> Result<R, E>,
    ) -> Result<(R, ChangeSet), E> {
        let (r, cs, patch) = Self::run_tx(&mut self.store, f)?;
        if !cs.is_empty() {
            self.journal.push(patch);
        }
        Ok((r, cs))
    }

    fn run_tx<R, E: From<StoreError>>(
        store: &mut DocStore,
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
        let (_, cs, inverse) = Self::run_tx::<(), StoreError>(&mut self.store, |tx| patch.apply(tx))?;
        Ok((cs, inverse))
    }

    /// Replaces the whole content with `doc` in one journaled transaction (document open, new, intro).
    /// The document is validated first; one with errors is refused and nothing changes.
    pub fn load(&mut self, doc: &Document) -> Result<ChangeSet, StoreError> {
        let report = doc.validate();
        if report.has_errors() {
            return Err(StoreError::InvalidDocument(report));
        }
        let existing: Vec<ElementId> = self.store.iter_all().map(|e| e.id.clone()).collect();
        let old_files: Vec<FileId> = self.store.files().map(|(id, _)| id.clone()).collect();
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
        drafftink_scene::snapshot(&self.store)
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

    /// What the commits since `mark` did, without removing them.
    pub fn summary_since(&self, mark: DocMark) -> Summary {
        self.summarize(self.journal.get(mark.0..).unwrap_or(&[]))
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
            let now = self.store.get(id);
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
        let meta_changed = meta.is_some_and(|m| m != self.store.meta());
        let files_changed = files.iter().any(|(id, prev)| prev.as_ref() != self.store.file(id));
        s.changed = !s.touched.is_empty() || meta_changed || files_changed;
        s
    }
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

/// Which repeats merge into one undo step: same `key` (the command id) on the same targets, within `ms`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoalesceKey {
    /// The command id.
    pub key: &'static str,
    /// Time window in milliseconds.
    pub ms: u64,
}

/// A step ready to be pushed.
pub struct NewStep {
    /// Label shown by undo and redo (the command title or the batch label).
    pub label: String,
    /// Takes the step back.
    pub patch: Patch,
    /// Editor state before the step.
    pub obs_before: Observed,
    /// Editor state after the step.
    pub obs_after: Observed,
    /// Merge rule, if the command has one.
    pub coalesce: Option<CoalesceKey>,
    /// Elements the step changed.
    pub targets: Vec<ElementId>,
    /// Wall time in milliseconds.
    pub t_ms: u64,
}

struct Step {
    label: String,
    patch: Patch,
    obs_before: Observed,
    obs_after: Observed,
    coalesce: Option<CoalesceKey>,
    targets: Vec<ElementId>,
    t_ms: u64,
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

/// Per-tab undo and redo stacks.
#[derive(Default)]
pub struct UndoHistory {
    undo: Vec<Step>,
    redo: Vec<Step>,
}

impl UndoHistory {
    /// Empty history.
    pub fn new() -> Self {
        Self::default()
    }

    /// Pushes a step, clears redo, merges with the previous step when `coalesce` allows it, and keeps at
    /// most `depth` steps.
    pub fn push(&mut self, step: NewStep, depth: usize) {
        self.redo.clear();
        if let (Some(key), Some(prev)) = (&step.coalesce, self.undo.last_mut()) {
            let same = prev.coalesce.as_ref() == Some(key)
                && prev.targets == step.targets
                && step.t_ms.saturating_sub(prev.t_ms) <= key.ms;
            if same {
                prev.patch.absorb_later(step.patch);
                prev.obs_after = step.obs_after;
                prev.t_ms = step.t_ms;
                return;
            }
        }
        self.undo.push(Step {
            label: step.label,
            patch: step.patch,
            obs_before: step.obs_before,
            obs_after: step.obs_after,
            coalesce: step.coalesce,
            targets: step.targets,
            t_ms: step.t_ms,
        });
        let depth = depth.max(1);
        if self.undo.len() > depth {
            let drop = self.undo.len() - depth;
            self.undo.drain(..drop);
        }
    }

    /// Takes back up to `n` steps. A step that would change nothing (a remote edit already did it) is
    /// dropped without counting.
    pub fn undo(&mut self, doc: &mut TabDoc, n: usize) -> Result<UndoOutcome, StoreError> {
        let mut out = UndoOutcome { labels: Vec::new(), observed: None, touched: Vec::new() };
        while out.labels.len() < n {
            let Some(step) = self.undo.pop() else { break };
            let (cs, inverse) = doc.apply_raw(&step.patch)?;
            out.touched.extend(cs.ids().cloned());
            let skipped = cs.is_empty();
            self.redo.push(Step { patch: inverse, ..step });
            if skipped {
                // Nothing visible changed: the step stays reachable by redo but does not count.
                continue;
            }
            if let Some(s) = self.redo.last() {
                out.labels.push(s.label.clone());
                out.observed = Some(s.obs_before.clone());
            }
        }
        Ok(out)
    }

    /// Re-applies up to `n` undone steps.
    pub fn redo(&mut self, doc: &mut TabDoc, n: usize) -> Result<UndoOutcome, StoreError> {
        let mut out = UndoOutcome { labels: Vec::new(), observed: None, touched: Vec::new() };
        while out.labels.len() < n {
            let Some(step) = self.redo.pop() else { break };
            let (cs, inverse) = doc.apply_raw(&step.patch)?;
            out.touched.extend(cs.ids().cloned());
            let skipped = cs.is_empty();
            self.undo.push(Step { patch: inverse, ..step });
            if skipped {
                continue;
            }
            if let Some(s) = self.undo.last() {
                out.labels.push(s.label.clone());
                out.observed = Some(s.obs_after.clone());
            }
        }
        Ok(out)
    }

    /// True when there is a step to undo.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// True when there is a step to redo.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Label of the step undo would take back.
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|s| s.label.as_str())
    }

    /// Label of the step redo would re-apply.
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|s| s.label.as_str())
    }

    /// Steps on the undo stack.
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    /// Steps on the redo stack.
    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }

    /// Drops both stacks.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

/// Number of live elements added, removed or changed by a change set. Used for output summaries.
pub fn count_changes(cs: &ChangeSet) -> (usize, usize, usize) {
    let mut a = 0;
    let mut r = 0;
    let mut u = 0;
    for Change { kind, .. } in &cs.elements {
        match kind {
            ChangeKind::Added => a += 1,
            ChangeKind::Removed => r += 1,
            ChangeKind::Updated => u += 1,
        }
    }
    (a, r, u)
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

    #[test]
    fn undo_and_redo_walk_the_stacks_with_observed_state() {
        let mut ids = SeqIds::new();
        let mut doc = TabDoc::new();
        let mut hist = UndoHistory::new();
        let mark = doc.mark();
        let e = rect(&mut ids, &ZIndex::first());
        let id = e.id.clone();
        add(&mut doc, e);
        let (patch, summary) = doc.take_since(mark);
        assert_eq!(summary.created, vec![id.clone()]);
        let before = Observed::default();
        let mut after = Observed::default();
        after.selection.insert(id.clone());
        hist.push(
            NewStep {
                label: "Create".into(),
                patch,
                obs_before: before.clone(),
                obs_after: after.clone(),
                coalesce: None,
                targets: vec![id.clone()],
                t_ms: 0,
            },
            50,
        );
        assert!(hist.can_undo() && !hist.can_redo());
        let out = hist.undo(&mut doc, 1).unwrap();
        assert_eq!(out.labels, ["Create"]);
        assert_eq!(out.observed, Some(before));
        assert!(doc.store().get(&id).is_none());
        let out = hist.redo(&mut doc, 1).unwrap();
        assert_eq!(out.observed, Some(after));
        assert!(doc.store().get(&id).is_some());
    }

    #[test]
    fn coalesced_repeats_make_one_step_and_depth_is_capped() {
        let mut ids = SeqIds::new();
        let mut doc = TabDoc::new();
        let e = rect(&mut ids, &ZIndex::first());
        let id = e.id.clone();
        add(&mut doc, e);
        doc.forget_journal();
        let mut hist = UndoHistory::new();
        for (i, x) in [10.0, 20.0, 30.0].into_iter().enumerate() {
            let mark = doc.mark();
            doc.edit::<(), StoreError>(|tx| tx.update(&id, |e| e.transform.x = x)).unwrap();
            let (patch, _) = doc.take_since(mark);
            hist.push(
                NewStep {
                    label: "Nudge".into(),
                    patch,
                    obs_before: Observed::default(),
                    obs_after: Observed::default(),
                    coalesce: Some(CoalesceKey { key: "element.nudge", ms: 500 }),
                    targets: vec![id.clone()],
                    t_ms: 100 * i as u64,
                },
                50,
            );
        }
        assert_eq!(hist.undo_depth(), 1);
        hist.undo(&mut doc, 1).unwrap();
        assert_eq!(doc.store().get(&id).unwrap().transform.x, 0.0);

        let mut small = UndoHistory::new();
        for i in 0..5 {
            small.push(
                NewStep {
                    label: format!("s{i}"),
                    patch: Patch::default(),
                    obs_before: Observed::default(),
                    obs_after: Observed::default(),
                    coalesce: None,
                    targets: vec![],
                    t_ms: 0,
                },
                3,
            );
        }
        assert_eq!(small.undo_depth(), 3);
        assert_eq!(small.undo_label(), Some("s4"));
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
