//! The workspace record: every tab saved to the `Storage` port and restored at start
//! (docs/commands.md §9 item 6 and §2.2).
//!
//! Layout in the `Autosave` namespace:
//!
//! * `workspace`: the index, JSON, schema-versioned.
//!   `{ "format": "drafftink-workspace", "schema": 1, "saved_ms": 1730000000000, "active": 3,
//!      "tabs": [ { "id": 3, "key": "workspace.t3", "name": "Plan", "dirty": true, "revision": 12,
//!                  "view": { "scroll": [0, 0], "zoom": 1.68 }, "selection": ["e1"] } ] }`
//! * `workspace.t<id>`: one native document (the same JSON as a saved file) per tab.
//!
//! Documents are separate keys so that a save writes only the tabs that changed since the last write.
//! The index is written last, so a crash between the writes leaves the previous index pointing at
//! documents that are at worst newer. History is not saved: a restored tab starts with an empty undo
//! stack, as an opened file does.
//!
//! Nothing here starts a timer. The shell calls [`Session::workspace_tick`] when the dirty reason says
//! state changed and when the timer it set from [`Session::workspace_next_due_ms`] fires. Debouncing runs
//! on the monotonic `Clock` port: a write happens once the document has been quiet for
//! [`QUIET_MS`], and in any case no later than `autosave.interval_s` after the first unsaved change.

use std::collections::BTreeMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use drafftink_geom::kurbo::Point;
use drafftink_scene::{Document, ElementId, SceneRead};
use serde::Serialize;
use serde_json::Value;

use super::workspace_save::{Finished, SaveJob, TICK_BYTES};
use super::{NEVER_SAVED, name_of};
use crate::ports::{Ns, PortKind};
use crate::refusal::Refusal;
use crate::session::Session;
use crate::spec::Redraw;
use crate::state::{Tab, TabId, clamp_zoom};
use crate::store::TabDoc;

/// The schema version of the index this build reads and writes.
pub const SCHEMA: u32 = 1;
/// The storage key of the index.
pub const INDEX_KEY: &str = "workspace";
/// The prefix of the per-tab document keys.
pub const TAB_KEY_PREFIX: &str = "workspace.t";
/// Quiet time before a write, in milliseconds. A shorter `autosave.interval_s` wins.
pub const QUIET_MS: u64 = 1000;
/// Largest tab id a record may name. Restoring walks the id counter up to it.
const MAX_RESTORED_ID: u32 = 1_000_000;

fn tab_key(id: TabId) -> String {
    format!("{TAB_KEY_PREFIX}{}", id.0)
}

/// What a save wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct WorkspaceSaved {
    /// Tabs in the record.
    pub tabs: usize,
    /// Documents written (tabs that changed since the last write).
    pub documents_written: usize,
    /// Size of the index in bytes.
    pub index_bytes: usize,
}

/// A tab of the record that was not restored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkippedTab {
    /// The tab's id in the record.
    pub id: u32,
    /// Why.
    pub reason: String,
}

/// What a restore did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct WorkspaceRestore {
    /// An index was found in storage.
    pub found: bool,
    /// Tabs now open from the record. Zero leaves the session as it was.
    pub restored: usize,
    /// Tabs of the record that could not be read.
    pub skipped: Vec<SkippedTab>,
    /// The index could not be read at all.
    pub unreadable: Option<String>,
    /// The record was written by a newer build with this schema. It is not read, and not overwritten.
    pub newer_schema: Option<u32>,
}

/// Autosave bookkeeping, kept in `Session::ext`.
#[derive(Debug, Default)]
struct AutosaveState {
    /// What was last written: revision and name per tab.
    written: BTreeMap<TabId, (u64, String)>,
    /// The signature of the session when the record was last written or restored.
    written_sig: Option<u64>,
    /// The signature seen by the latest observation.
    seen_sig: Option<u64>,
    /// Monotonic time of the first change since the last write.
    first_change: Option<u64>,
    /// Monotonic time of the latest change.
    last_change: Option<u64>,
    /// The last write failed with this.
    last_error: Option<String>,
    /// Never write: the stored record is from a newer build.
    protect: Option<u32>,
    /// A write that is done in pieces, one per `workspace_tick`.
    job: Option<SaveJob>,
}

/// A number that changes whenever something the record holds changes: the tab order, the active tab, and
/// per tab its revision, name and unsaved state. The camera and the selection are saved along with a
/// write but do not cause one.
fn signature(s: &Session) -> u64 {
    let mut h = DefaultHasher::new();
    s.tab().id.hash(&mut h);
    for t in &s.tabs {
        t.id.hash(&mut h);
        t.doc.revision().hash(&mut h);
        name_of(t).hash(&mut h);
        t.dirty().hash(&mut h);
    }
    h.finish()
}

/// A session that nobody has used: one tab with no change in it. There is nothing to save.
fn pristine(s: &Session) -> bool {
    s.tabs.len() == 1 && s.tab().doc.revision() == 0
}

/// How often a write held back by a running gesture is looked at again, in milliseconds.
const GESTURE_RECHECK_MS: u64 = 500;

impl Session {
    /// Writes the workspace now: the tabs that changed since the last write, then the index, then removes
    /// the documents of closed tabs. Call it on window close, or let `workspace_tick` do it. It does the whole
    /// work in this call (a big document takes a while); `workspace_tick` does it in pieces of about 5 ms.
    pub fn save_workspace(&mut self) -> Result<WorkspaceSaved, Refusal> {
        self.ext.with::<AutosaveState, _>(|st| st.job = None);
        let r = self.begin_workspace_write().and_then(|(mut job, _)| {
            job.work(usize::MAX)?;
            self.finish_workspace_write(job, true)
        });
        let msg = r.as_ref().err().map(|e| e.message.clone());
        self.ext.with::<AutosaveState, _>(|st| st.last_error = msg);
        r
    }

    /// Checks that a write is possible and takes the document of every tab that changed. The cost is in the
    /// unit of [`TICK_BYTES`].
    fn begin_workspace_write(&mut self) -> Result<(SaveJob, usize), Refusal> {
        const ID: &str = "autosave";
        if !self.has_port(PortKind::Storage) {
            return Err(Refusal::port_unavailable(
                ID,
                "storage",
                "the workspace is saved to storage",
                "Without storage nothing survives a restart.",
            ));
        }
        if let Some(n) = self.ext.with::<AutosaveState, _>(|st| st.protect) {
            return Err(Refusal::custom(
                "workspace_newer",
                ID,
                format!("The stored workspace was written by a newer drafft.ink (format {n}), so it is left alone. Update the app to use it."),
            ));
        }
        let written = self.ext.with::<AutosaveState, _>(|st| st.written.clone());
        let sig = signature(self);
        Ok(SaveJob::begin(self, &written, sig))
    }

    /// The last step of a write: the finished documents and the index go to the storage port, and the
    /// bookkeeping says what was written.
    fn finish_workspace_write(&mut self, job: SaveJob, cleanup: bool) -> Result<WorkspaceSaved, Refusal> {
        let sig = job.sig;
        let tabs: Vec<TabId> = job.tabs().to_vec();
        let Finished { saved, written } = job.finish(self, cleanup)?;
        self.ext.with::<AutosaveState, _>(|st| {
            for (id, revision, name) in written {
                st.written.insert(id, (revision, name));
            }
            st.written.retain(|id, _| tabs.contains(id));
            // The session may have changed while the job ran: then the signature differs and a write is still due.
            st.written_sig = Some(sig);
            st.last_error = None;
        });
        self.observe_workspace();
        Ok(saved)
    }

    /// Opens the tabs of the stored workspace in place of the current ones. Call it once at start, before
    /// the first command. A session without storage, without a record, or with a record that has no
    /// readable tab stays as it is. A damaged record is reported, not an error.
    pub fn restore_workspace(&mut self) -> Result<WorkspaceRestore, Refusal> {
        const ID: &str = "autosave";
        let Some(storage) = self.ports.storage.as_ref() else {
            return Ok(WorkspaceRestore::default());
        };
        let bytes = storage
            .get(Ns::Autosave, INDEX_KEY)
            .map_err(|e| Refusal::io_failed(ID, "read", "the workspace index", &e.to_string()))?;
        let Some(bytes) = bytes else {
            return Ok(WorkspaceRestore::default());
        };
        let mut report = WorkspaceRestore { found: true, ..WorkspaceRestore::default() };
        let index: Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => {
                report.unreadable = Some(format!("not valid JSON ({e})"));
                return Ok(report);
            }
        };
        let schema = index.get("schema").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).unwrap_or(0);
        if schema == 0 || index.get("format").and_then(Value::as_str) != Some("drafftink-workspace") {
            report.unreadable = Some("it is not a workspace index".to_owned());
            return Ok(report);
        }
        if schema > SCHEMA {
            report.newer_schema = Some(schema);
            self.ext.with::<AutosaveState, _>(|st| st.protect = Some(schema));
            return Ok(report);
        }
        let entries = index.get("tabs").and_then(Value::as_array).cloned().unwrap_or_default();
        let mut restored: Vec<Tab> = Vec::new();
        for e in &entries {
            let id = e.get("id").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).unwrap_or(0);
            match self.read_tab(e, id, &restored) {
                Ok(t) => restored.push(t),
                Err(reason) => report.skipped.push(SkippedTab { id, reason }),
            }
        }
        if restored.is_empty() {
            return Ok(report);
        }
        let active_id = index.get("active").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok());
        let active = restored.iter().position(|t| Some(t.id.0) == active_id).unwrap_or(0);
        let max_id = restored.iter().map(|t| t.id.0).max().unwrap_or(0);
        self.cancel_gesture_if_running();
        self.text_edit = None;
        report.restored = restored.len();
        self.tabs = restored;
        self.set_active_index(active);
        // New ids must not collide with restored ones.
        while self.alloc_tab_id().0 <= max_id {}
        let written: BTreeMap<TabId, (u64, String)> =
            self.tabs.iter().map(|t| (t.id, (t.doc.revision(), name_of(t)))).collect();
        let sig = signature(self);
        self.ext.with::<AutosaveState, _>(|st| {
            st.written = written;
            st.written_sig = Some(sig);
            st.seen_sig = Some(sig);
            st.first_change = None;
            st.last_change = None;
        });
        self.mark_dirty(Redraw::Scene);
        Ok(report)
    }

    /// One tab of the index, read from storage. `Err` is the reason it is skipped.
    fn read_tab(&self, entry: &Value, id: u32, done: &[Tab]) -> Result<Tab, String> {
        if id == 0 || id > MAX_RESTORED_ID {
            return Err("its id is missing or out of range".to_owned());
        }
        if done.iter().any(|t| t.id.0 == id) {
            return Err("its id is used twice".to_owned());
        }
        let key = tab_key(TabId(id));
        let Some(storage) = self.ports.storage.as_ref() else {
            return Err("there is no storage".to_owned());
        };
        let bytes = storage
            .get(Ns::Autosave, &key)
            .map_err(|e| format!("reading `{key}` failed: {e}"))?
            .ok_or_else(|| format!("`{key}` is missing from storage"))?;
        let text = String::from_utf8(bytes).map_err(|_| format!("`{key}` is not text"))?;
        let doc = Document::from_json(&text).map_err(|e| format!("`{key}` is not a document: {e}"))?;
        let td = TabDoc::from_document(&doc).map_err(|e| format!("`{key}` is not a valid document: {e}"))?;
        let mut t = Tab::new(TabId(id));
        t.doc = td;
        if entry.get("dirty").and_then(Value::as_bool).unwrap_or(false) {
            t.saved_revision = NEVER_SAVED;
        } else {
            t.mark_saved();
        }
        if let Some(v) = entry.get("view") {
            let sc = v.get("scroll").and_then(Value::as_array);
            let (x, y) = (
                sc.and_then(|a| a.first()).and_then(Value::as_f64),
                sc.and_then(|a| a.get(1)).and_then(Value::as_f64),
            );
            if let (Some(x), Some(y)) = (x, y)
                && x.is_finite()
                && y.is_finite()
                && x.abs() <= crate::args::MAX_WORLD
                && y.abs() <= crate::args::MAX_WORLD
            {
                t.view.scroll = Point::new(x, y);
            }
            if let Some(z) = v.get("zoom").and_then(Value::as_f64) {
                t.view.zoom = clamp_zoom(z);
            }
        }
        if let Some(sel) = entry.get("selection").and_then(Value::as_array) {
            for id in sel.iter().filter_map(Value::as_str).filter_map(|s| ElementId::new(s).ok()) {
                if t.doc.store().get(&id).is_some_and(|e| !e.deleted) {
                    t.observed.selection.insert(id);
                }
            }
        }
        Ok(t)
    }

    /// Notices changes and decides whether a write is due now. Starts one when it is, and does a piece of the
    /// work of one that is under way (about 5 ms, [`TICK_BYTES`]; a small document is written in one call).
    /// Returns true when a write ended in this call. Does nothing when `autosave.enabled` is off or the session has
    /// no storage.
    pub fn workspace_tick(&mut self) -> bool {
        self.observe_workspace();
        if !self.autosave_active() {
            return false;
        }
        self.note_late_write_failure();
        // Writing happens in pieces, but not in the middle of a drag: the pieces wait for the gesture to end
        // (`workspace_next_due_ms` keeps the wake-up coming).
        if self.running_gesture().is_some() {
            return false;
        }
        let now = self.ports.clock.mono_ms();
        let mut budget = TICK_BYTES;
        let mut job = match self.ext.with::<AutosaveState, _>(|st| st.job.take()) {
            Some(job) => job,
            None => match self.workspace_due_at() {
                Some(due) if now >= due => match self.begin_workspace_write() {
                    Ok((job, cost)) => {
                        budget = budget.saturating_sub(cost);
                        job
                    }
                    Err(e) => return self.write_failed(&e, now),
                },
                _ => return false,
            },
        };
        match job.work(budget) {
            Ok(false) => {
                self.ext.with::<AutosaveState, _>(|st| st.job = Some(job));
                false
            }
            Ok(true) => match self.finish_workspace_write(job, false) {
                Ok(_) => true,
                Err(e) => self.write_failed(&e, now),
            },
            Err(e) => self.write_failed(&e, now),
        }
    }

    /// A write failed: say why, and try again after a full interval, not on every call.
    fn write_failed(&mut self, e: &Refusal, now: u64) -> bool {
        let msg = e.message.clone();
        self.ext.with::<AutosaveState, _>(|st| {
            st.last_error = Some(msg);
            st.job = None;
            st.first_change = Some(now);
            st.last_change = Some(now);
        });
        false
    }

    /// A queued write (`Storage::put_later`) that failed after the call that started it returned: the record on disk
    /// is not what `written` says, so everything is written again.
    fn note_late_write_failure(&mut self) {
        let Some(e) = self.ports.storage.as_mut().and_then(|s| s.take_write_error()) else { return };
        let now = self.ports.clock.mono_ms();
        let msg = format!("The workspace could not be written: {e}");
        self.ext.with::<AutosaveState, _>(|st| {
            st.last_error = Some(msg);
            st.written.clear();
            st.written_sig = None;
            st.first_change = Some(now);
            st.last_change = Some(now);
        });
    }

    /// Milliseconds until the next write is due, or `None` when nothing is waiting to be written (or
    /// autosave is off). `Some(0)` means due now, or a write is under way and wants its next piece. The shell sets
    /// one timer from this and calls `workspace_tick` when it fires; with `None` it sets no timer at all.
    pub fn workspace_next_due_ms(&mut self) -> Option<u64> {
        self.observe_workspace();
        if !self.autosave_active() {
            return None;
        }
        let under_way = self.ext.get::<AutosaveState>().is_some_and(|st| st.job.is_some());
        let now = self.ports.clock.mono_ms();
        let wait = if under_way { 0 } else { self.workspace_due_at().map(|d| d.saturating_sub(now))? };
        // A write held back by a running gesture is looked at again soon, not on every frame.
        Some(if self.running_gesture().is_some() { wait.max(GESTURE_RECHECK_MS) } else { wait })
    }

    /// Writes any unsaved change now, for window close. Respects `autosave.enabled`. Returns true when it
    /// wrote.
    pub fn flush_workspace(&mut self) -> bool {
        self.observe_workspace();
        self.autosave_active() && self.workspace_due_at().is_some() && self.save_workspace().is_ok()
    }

    /// Why the last write failed, if it did.
    pub fn workspace_error(&mut self) -> Option<String> {
        self.ext.with::<AutosaveState, _>(|st| st.last_error.clone())
    }

    fn autosave_active(&self) -> bool {
        self.settings.bool_or("autosave.enabled", true)
            && self.has_port(PortKind::Storage)
            && self.ext.get::<AutosaveState>().is_none_or(|st| st.protect.is_none())
    }

    /// Records a change of the signature with the time it was seen.
    fn observe_workspace(&mut self) {
        let sig = signature(self);
        let now = self.ports.clock.mono_ms();
        let is_pristine = pristine(self);
        self.ext.with::<AutosaveState, _>(|st| {
            if st.written_sig.is_none() && is_pristine {
                // Nothing was ever written and nothing was ever done: no change to report.
                st.seen_sig = Some(sig);
                return;
            }
            if st.seen_sig != Some(sig) {
                st.seen_sig = Some(sig);
                st.last_change = Some(now);
                st.first_change.get_or_insert(now);
            }
            if st.written_sig == Some(sig) {
                st.first_change = None;
                st.last_change = None;
            } else if st.first_change.is_none() {
                // Changed before the first observation (the shell started late): count from now.
                st.first_change = Some(now);
                st.last_change = Some(now);
            }
        });
    }

    /// Monotonic time at which the pending change is written, if one is pending.
    fn workspace_due_at(&self) -> Option<u64> {
        let st = self.ext.get::<AutosaveState>()?;
        let (first, last) = (st.first_change?, st.last_change?);
        let interval = self.settings.u64_or("autosave.interval_s", 5).clamp(1, 300) * 1000;
        Some((last + QUIET_MS.min(interval)).min(first + interval))
    }
}
