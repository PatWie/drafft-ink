//! Transactions: edit the view, then write only the changed fields into Loro in one commit.

use std::collections::{BTreeSet, HashMap};

use drafftink_scene::MemoryTx;
use drafftink_scene::{
    Change, ChangeKind, ChangeSet, Color, DerivedIndex, DocMeta, Element, ElementId, ElementKind, FileEntry, FileId,
    SceneRead, SceneTx, StoreError, ZIndex,
};
use loro::{CommitOptions, LoroMap, LoroValue};

use crate::codec::{self, Flat};
use crate::error::DocError;
use crate::schema::{META_BACKGROUND, META_NAME, ROOT_ELEMENTS, ROOT_FILES, ROOT_META, SYSTEM_ORIGIN_PREFIX};
use crate::store::Core;
use crate::text;
use crate::undo::{ObservedState, UndoInfo, lock};

/// A transaction on a [`crate::LoroStore`].
///
/// Reads see the transaction's own writes. Nothing reaches Loro until [`SceneTx::commit`], which writes one Loro
/// commit containing only the fields that differ from what is stored. Dropping the transaction discards it.
pub struct LoroTx<'a> {
    inner: MemoryTx<'a>,
    core: &'a mut Core,
    touched: BTreeSet<ElementId>,
    touched_files: BTreeSet<FileId>,
    label: Option<String>,
    info: Option<UndoInfo>,
    history: History,
}

/// How a commit appears in the undo history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum History {
    /// Its own undo step.
    Step,
    /// Folded into the step below it.
    MergeIntoPrevious,
    /// Not in the history at all: syncs, but undo does not see it.
    Hidden,
}

impl<'a> LoroTx<'a> {
    pub(crate) fn new(inner: MemoryTx<'a>, core: &'a mut Core) -> Self {
        Self {
            inner,
            core,
            touched: BTreeSet::new(),
            touched_files: BTreeSet::new(),
            label: None,
            info: None,
            history: History::Step,
        }
    }

    /// Labels the commit (stored as the Loro commit message, so peers and history tools can show it). The label
    /// of an undo step is the one from [`LoroTx::describe`] or, failing that, this one.
    pub fn set_label(&mut self, label: &str) {
        self.label = Some(label.to_owned());
    }

    /// Labels the commit and records the observed editor state before and after it. Undo returns `before`,
    /// redo returns `after`.
    pub fn describe(&mut self, label: &str, before: ObservedState, after: ObservedState) {
        self.label = Some(label.to_owned());
        self.info = Some(UndoInfo { label: label.to_owned(), before, after, seq: 0 });
    }

    /// Asks to fold this commit into the undo step below it instead of pushing a new step (a repeat of one action,
    /// such as nudging). The step keeps its label and its `before` state, and takes this commit's `after` state.
    /// It has no effect when the commit is not undoable, when there is no step to fold into, or when a collaborator's
    /// edit arrived since that step (Loro then keeps the two apart). [`LoroStore::last_commit_merged`] says what
    /// happened.
    ///
    /// [`LoroStore::last_commit_merged`]: crate::LoroStore::last_commit_merged
    pub fn merge_with_previous(&mut self) {
        // A commit that is not undoable has no step to fold into, whichever call came first.
        if self.history == History::Step {
            self.history = History::MergeIntoPrevious;
        }
    }

    /// Makes this commit invisible to undo (loading a document, applying a migration). It still syncs.
    pub fn not_undoable(&mut self) {
        self.history = History::Hidden;
    }
}

impl SceneRead for LoroTx<'_> {
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

fn normalize(e: &mut Element) {
    if let ElementKind::Text(t) = &mut e.kind {
        text::normalize_runs(&mut t.runs);
    }
}

impl SceneTx for LoroTx<'_> {
    fn insert(&mut self, mut element: Element) -> Result<(), StoreError> {
        normalize(&mut element);
        let id = element.id.clone();
        self.inner.insert(element)?;
        self.touched.insert(id);
        Ok(())
    }

    fn update<F: FnOnce(&mut Element)>(&mut self, id: &ElementId, f: F) -> Result<(), StoreError> {
        self.inner.update(id, |e| {
            f(e);
            normalize(e);
        })?;
        self.touched.insert(id.clone());
        Ok(())
    }

    fn remove(&mut self, id: &ElementId) -> Result<(), StoreError> {
        self.inner.remove(id)?;
        self.touched.insert(id.clone());
        Ok(())
    }

    fn set_name(&mut self, name: &str) -> Result<(), StoreError> {
        self.inner.set_name(name)
    }

    fn set_background(&mut self, color: Color) {
        self.inner.set_background(color);
    }

    fn put_file(&mut self, entry: FileEntry) -> Result<FileId, StoreError> {
        let id = self.inner.put_file(entry)?;
        self.touched_files.insert(id.clone());
        Ok(id)
    }

    fn remove_file(&mut self, id: &FileId) -> bool {
        let removed = self.inner.remove_file(id);
        if removed {
            self.touched_files.insert(id.clone());
        }
        removed
    }

    fn commit(self) -> Result<ChangeSet, StoreError> {
        let LoroTx { inner, core, touched, touched_files, label, info, history } = self;
        if let Some(found) = core.blocked {
            return Err(DocError::NewerSchema { found, max: crate::schema::SCHEMA_VERSION }.into());
        }
        if core.poisoned.get() {
            return Err(DocError::Poisoned.into());
        }
        // Capture the final values before the view consumes the transaction.
        let news: Vec<(ElementId, Option<Element>)> = touched
            .into_iter()
            .map(|id| {
                let e = inner.get(&id).cloned();
                (id, e)
            })
            .collect();
        let meta = inner.meta().clone();
        let files: Vec<(FileId, Option<FileEntry>)> = touched_files
            .into_iter()
            .map(|id| {
                let f = inner.file(&id).cloned();
                (id, f)
            })
            .collect();
        let cs = inner.commit()?;
        if cs.is_empty() {
            return Ok(cs);
        }
        let info = info.or_else(|| {
            Some(UndoInfo { label: label.clone().unwrap_or_else(|| "edit".to_owned()), ..UndoInfo::default() })
        });
        let commit = Commit { cs: &cs, news: &news, meta: &meta, files: &files, label, info, history };
        let flag = std::rc::Rc::clone(&core.poisoned);
        let written = crate::store::guard(&flag, || core.write(&commit)).and_then(|r| r);
        if let Err(e) = written {
            core.needs_resync = true;
            return Err(e.into());
        }
        profile_count(cs.elements.len());
        core.notify(&cs);
        Ok(cs)
    }
}

#[cfg(feature = "profile")]
fn profile_count(n: usize) {
    drafftink_profile::count(drafftink_profile::Counter::SyncOpsApplied, u64::try_from(n).unwrap_or(u64::MAX));
}

#[cfg(not(feature = "profile"))]
fn profile_count(_n: usize) {}

struct Commit<'a> {
    cs: &'a ChangeSet,
    news: &'a [(ElementId, Option<Element>)],
    meta: &'a DocMeta,
    files: &'a [(FileId, Option<FileEntry>)],
    label: Option<String>,
    info: Option<UndoInfo>,
    history: History,
}

impl Core {
    /// Writes one transaction's net change into Loro and commits it once.
    fn write(&mut self, c: &Commit<'_>) -> Result<(), DocError> {
        let before = self.doc.oplog_frontiers();
        let result = self.write_ops(c);
        // Always commit what was written, even after an error, so that the pending Loro transaction does not leak
        // into the next commit. The caller resyncs the view after an error.
        let undoable = c.history != History::Hidden;
        let origin = if undoable { "local".to_owned() } else { format!("{SYSTEM_ORIGIN_PREFIX}local") };
        let msg = c.label.as_deref().unwrap_or("edit");
        let merge = c.history == History::MergeIntoPrevious;
        self.last_merged = false;
        self.commit_seq += 1;
        let seq = self.commit_seq;
        {
            let mut g = lock(&self.shared);
            g.pending = if undoable {
                c.info.clone().map(|i| UndoInfo { seq, ..i }).as_ref().and_then(UndoInfo::to_loro)
            } else {
                None
            };
        }
        if merge {
            self.set_merge_interval(i64::MAX);
        }
        self.doc.commit_with(CommitOptions::new().origin(&origin).commit_msg(msg));
        if merge {
            self.set_merge_interval(0);
        }
        lock(&self.shared).pending = None;
        result?;
        self.stats.commits += 1;
        if undoable && self.doc.oplog_frontiers() != before {
            self.note_merge(merge, seq, c.info.as_ref());
            self.marks.push_back(before);
            let cap = self.config.undo_depth.saturating_add(1).max(2);
            while self.marks.len() > cap {
                self.marks.pop_front();
            }
        }
        Ok(())
    }

    fn set_merge_interval(&mut self, ms: i64) {
        let flag = std::rc::Rc::clone(&self.poisoned);
        let undo = &mut self.undo;
        let _ = crate::store::guard(&flag, || undo.set_merge_interval(ms));
    }

    /// After an undoable commit: records whether it was folded into the step below. A step pushed by this commit
    /// carries `seq`; if the top of the stack carries another number, the commit was merged.
    fn note_merge(&mut self, asked: bool, seq: u64, info: Option<&UndoInfo>) {
        self.last_merged = false;
        self.merged_after = None;
        if !asked {
            return;
        }
        let top = self.guard(|| self.undo.top_undo_meta()).ok().flatten();
        let top_seq = top.map(|m| UndoInfo::from_loro(&m.value).seq);
        if top_seq.is_some_and(|t| t != seq) {
            self.last_merged = true;
            self.merged_after = info.map(|i| i.after.clone());
        }
    }

    fn write_ops(&mut self, c: &Commit<'_>) -> Result<(), DocError> {
        let elements = self.doc.get_map(ROOT_ELEMENTS);
        for ch in &c.cs.elements {
            match ch.kind {
                ChangeKind::Removed => {
                    elements.delete(ch.id.as_str())?;
                    self.stats.fields_written += 1;
                }
                ChangeKind::Added | ChangeKind::Updated => {
                    let Some(e) = c
                        .news
                        .binary_search_by(|(id, _)| id.cmp(&ch.id))
                        .ok()
                        .and_then(|i| c.news.get(i))
                        .and_then(|(_, e)| e.as_ref())
                    else {
                        continue;
                    };
                    self.write_element(&elements, ch, e)?;
                }
            }
        }
        if c.cs.meta {
            let m = self.doc.get_map(ROOT_META);
            put_str(&m, META_NAME, &c.meta.name, "Untitled")?;
            put_str(&m, META_BACKGROUND, &c.meta.background.to_hex(), "#ffffff")?;
        }
        if !c.cs.files.is_empty() {
            self.write_files(c.files)?;
        }
        Ok(())
    }

    /// Writes the fields of one added or updated element that differ from what Loro holds.
    fn write_element(&mut self, elements: &LoroMap, ch: &Change, e: &Element) -> Result<(), DocError> {
        let reset = ch.kind == ChangeKind::Added && self.invalid.contains(&ch.id);
        let map = match elements.ensure_mergeable_map(ch.id.as_str()) {
            Ok(m) => m,
            Err(_) => {
                // The key holds something that is not an element container (a hostile or foreign write).
                elements.delete(ch.id.as_str())?;
                elements.ensure_mergeable_map(ch.id.as_str())?
            }
        };
        let flat = codec::flatten(e, ch.fields)?;
        let mut stale: Vec<String> = Vec::new();
        map.for_each(|k, _| {
            let in_scope = reset || codec::group_of(k).intersects(ch.fields);
            if in_scope && !flat.contains_key(k) {
                stale.push(k.to_owned());
            }
        });
        for k in &stale {
            map.delete(k)?;
            self.stats.fields_written += 1;
        }
        for (k, v) in &flat {
            match v {
                Flat::Value(val) => {
                    let same = matches!(map.get(k), Some(loro::ValueOrContainer::Value(cur)) if cur == *val);
                    if !same {
                        map.insert(k, val.clone())?;
                        self.stats.fields_written += 1;
                    }
                }
                Flat::Text { content, runs } => {
                    let t = match map.ensure_mergeable_text(k) {
                        Ok(t) => t,
                        Err(_) => {
                            map.delete(k)?;
                            map.ensure_mergeable_text(k)?
                        }
                    };
                    if text::write_text(&t, content, runs)? {
                        self.stats.fields_written += 1;
                    }
                }
            }
        }
        self.stats.elements_written += 1;
        self.invalid.remove(&ch.id);
        Ok(())
    }

    fn write_files(&self, files: &[(FileId, Option<FileEntry>)]) -> Result<(), DocError> {
        let map = self.doc.get_map(ROOT_FILES);
        for (id, entry) in files {
            match entry {
                Some(f) => {
                    let blob = HashMap::from([
                        ("mime".to_owned(), LoroValue::from(f.mime.as_str())),
                        ("bytes".to_owned(), LoroValue::from(f.bytes.to_vec())),
                    ]);
                    map.insert(id.as_str(), LoroValue::from(blob))?;
                }
                None => map.delete(id.as_str())?,
            }
        }
        Ok(())
    }
}

/// Stores `value` under `key`, or removes the key when it equals the default.
fn put_str(map: &loro::LoroMap, key: &str, value: &str, default: &str) -> Result<(), DocError> {
    let current = match map.get(key) {
        Some(loro::ValueOrContainer::Value(LoroValue::String(s))) => Some(s.to_string()),
        _ => None,
    };
    if value == default {
        if current.is_some() {
            map.delete(key)?;
        }
    } else if current.as_deref() != Some(value) {
        map.insert(key, value)?;
    }
    Ok(())
}
