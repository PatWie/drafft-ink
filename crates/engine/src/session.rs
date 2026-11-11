//! `Session`: everything a user can change, owned in one place (docs/commands.md §1.1).
//!
//! Commands see it as `&Session` (`available`) and `&mut Session` (the handler). Shells call
//! [`Session::execute`], [`Session::execute_batch`] and the `gesture_*` methods. See `docs/engine.md`.

use std::sync::Arc;

use drafftink_scene::{Element, ElementId, SceneRead};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::gesture::GestureState;
use crate::ports::{PortKind, PortWrite, Ports, SaveDone, WriteToken};
use crate::refusal::Refusal;
use crate::registry::Registry;
use crate::ring::{Origin, Ring};
use crate::services::Services;
use crate::spec::{CommandSpec, Redraw};
use crate::state::{Extensions, Observed, Settings, Tab, TabId, TextEdit, Tool, ToolState, View};
use crate::store::UndoOutcome;
use drafftink_profile::PerfStats;

/// An element reference in arguments: an id, or `"@name"` for the name set with `element.set_name`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct Ref(pub String);

/// The small non-document state that rollback restores (docs/commands.md §1.4 step 5).
#[derive(Debug, Clone)]
pub struct SoftState {
    pub(crate) tab: TabId,
    pub(crate) observed: Observed,
    view: View,
    tool: ToolState,
    text_edit: Option<TextEdit>,
    view_only: bool,
}

/// Bookkeeping for the command that is running. Only the executor and the gesture machinery write it.
#[derive(Default)]
pub(crate) struct RunState {
    /// Id of the command being run, `""` when idle.
    pub cmd: &'static str,
    /// Nesting depth of the executor. Above 0 while a handler runs.
    pub depth: u32,
    /// True while a gesture frame runs.
    pub live: bool,
    /// Non-fatal notes of the current command or batch.
    pub warnings: Vec<String>,
    /// Port writes queued by the current command or batch.
    pub writes: Vec<PortWrite>,
    /// The handler cleared the history, so no undo step may be pushed for this run.
    pub history_reset: bool,
    /// The gesture machinery already wrote the ring entry for this call.
    pub ring_handled: bool,
    /// Who made the call being run.
    pub origin: Origin,
    /// The pipeline is running the final command of a gesture (`gesture.end`): its step is not coalesced.
    pub gesture_end: bool,
    /// The running call is a [`Session::query`]: a success leaves no ring entry.
    pub quiet_ok: bool,
}

/// Which dirty reasons are pending since the shell last asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Dirty {
    /// The strongest pending reason.
    pub level: Option<Redraw>,
}

/// The one running session of an editor.
pub struct Session {
    registry: Arc<Registry>,
    /// Tabs in display order. Never empty.
    pub tabs: Vec<Tab>,
    active: usize,
    next_tab: u32,
    fallback: Tab,
    /// Tool state.
    pub tool: ToolState,
    /// Persisted settings.
    pub settings: Settings,
    /// While on, commands with `view_only = Refused` refuse. Not saved with the document.
    pub view_only: bool,
    /// Everything outside the process.
    pub ports: Ports,
    /// The text and formula engine behind the measuring traits of `edit` and `io`.
    pub services: Services,
    /// The text edit in progress, if any.
    pub text_edit: Option<TextEdit>,
    /// Family-private session state (clipboard buffers, library, recent list).
    pub ext: Extensions,
    pub(crate) gesture: Option<GestureState>,
    pub(crate) next_gesture: u32,
    pub(crate) ring: Ring,
    pub(crate) run: RunState,
    pub(crate) dirty: Dirty,
    pub(crate) perf_mark: Option<PerfStats>,
    /// Whether `perf.stats` reads the whole process or the calling thread.
    pub(crate) perf_scope: PerfScope,
    /// Document writes that the files port will finish later, with what to do when each is confirmed.
    pub(crate) saves: Vec<(WriteToken, SaveDone)>,
    /// Batch steps whose picture is still being drawn (`renders.rs`).
    pub(crate) renders: Vec<(crate::renders::RenderToken, crate::renders::DeferredRender)>,
    pub(crate) next_render: u64,
}

/// Which counters `perf.stats` reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PerfScope {
    /// Everything the process counted.
    Process,
    /// What the thread that calls `perf.stats` counted.
    Thread,
}

impl Session {
    /// Makes `perf.stats` report the work of the calling thread only. Many sessions that share a process (a
    /// test suite, the testkit) otherwise add their counters to the same process-wide totals, so a session
    /// could not tell its own work from the others'. The shell of an app leaves the default.
    pub fn count_this_thread_only(&mut self) {
        self.perf_scope = PerfScope::Thread;
    }

    /// A session with one empty tab over the global registry.
    pub fn new(ports: Ports) -> Self {
        Self::with_registry(ports, Registry::global())
    }

    /// A session over a given registry (tests use synthetic commands).
    pub fn with_registry(ports: Ports, registry: Arc<Registry>) -> Self {
        Self {
            registry,
            tabs: vec![Tab::new(TabId(1))],
            active: 0,
            next_tab: 2,
            fallback: Tab::new(TabId(0)),
            tool: ToolState::default(),
            settings: Settings::default(),
            view_only: false,
            ports,
            services: Services::new(),
            text_edit: None,
            ext: Extensions::default(),
            gesture: None,
            next_gesture: 1,
            ring: Ring::default(),
            run: RunState::default(),
            dirty: Dirty::default(),
            perf_mark: None,
            perf_scope: PerfScope::Process,
            saves: Vec::new(),
            renders: Vec::new(),
            next_render: 0,
        }
    }

    /// A session with the given services (tests pass `Services::fake()`).
    pub fn with_services(ports: Ports, services: Services) -> Self {
        let mut s = Self::new(ports);
        s.services = services;
        s
    }

    /// The registry this session runs.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// A shared handle on the registry.
    pub fn registry_arc(&self) -> Arc<Registry> {
        self.registry.clone()
    }

    // ---- tabs ----

    /// The active tab.
    pub fn tab(&self) -> &Tab {
        match self.tabs.get(self.active) {
            Some(t) => t,
            None => self.tabs.first().unwrap_or(&self.fallback),
        }
    }

    /// The active tab, mutable.
    pub fn tab_mut(&mut self) -> &mut Tab {
        if self.tabs.get(self.active).is_none() {
            self.active = 0;
            if self.tabs.is_empty() {
                let id = self.alloc_tab_id();
                self.tabs.push(Tab::new(id));
            }
        }
        match self.tabs.get_mut(self.active) {
            Some(t) => t,
            None => &mut self.fallback,
        }
    }

    /// A tab by id.
    pub fn tab_by_id(&self, id: TabId) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == id)
    }

    /// A tab by id, mutable.
    pub fn tab_by_id_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    /// Index of the active tab.
    pub fn active_index(&self) -> usize {
        self.active.min(self.tabs.len().saturating_sub(1))
    }

    /// Makes the tab at `index` active. Returns false for an index out of range.
    pub fn set_active_index(&mut self, index: usize) -> bool {
        if index < self.tabs.len() {
            self.active = index;
            true
        } else {
            false
        }
    }

    /// A fresh tab id.
    pub fn alloc_tab_id(&mut self) -> TabId {
        let id = TabId(self.next_tab);
        self.next_tab += 1;
        id
    }

    // ---- shorthand for handlers ----

    /// The id of the command being run (empty outside a command). Use it for refusal text.
    pub fn cmd(&self) -> &'static str {
        self.run.cmd
    }

    /// True while a gesture frame runs (`live = true` in docs/commands.md §1.10). A live frame skips work
    /// that only the final state needs; it does not skip the binding solver or snapping.
    pub fn live(&self) -> bool {
        self.run.live
    }

    /// Adds a non-fatal note to the output of the running command.
    pub fn warn(&mut self, note: impl Into<String>) {
        self.run.warnings.push(note.into());
    }

    /// Queues a port write. It is performed only after the command (or batch) succeeded.
    pub fn queue_write(&mut self, w: PortWrite) {
        self.run.writes.push(w);
    }

    /// Clears the active tab's undo history. The running command then pushes no undo step
    /// (`doc.new`, `doc.open`).
    pub fn clear_history(&mut self) {
        self.tab_mut().doc.clear_history();
        self.run.history_reset = true;
    }

    /// The selection of the active tab.
    pub fn selection(&self) -> &std::collections::BTreeSet<ElementId> {
        &self.tab().observed.selection
    }

    /// The active tab's view.
    pub fn view(&self) -> &View {
        &self.tab().view
    }

    /// Shorthand for the active tab's document store, read-only.
    pub fn store(&self) -> &crate::store::WorkStore {
        self.tab().doc.store()
    }

    /// The tool in effect.
    pub fn active_tool(&self) -> Tool {
        self.tool.effective()
    }

    /// True when the session has the port.
    pub fn has_port(&self, kind: PortKind) -> bool {
        self.ports.has(kind)
    }

    /// The refusal for a missing port, or `Ok`.
    pub fn require_port(&self, kind: PortKind, what: &str, alternative: &str) -> Result<(), Refusal> {
        if self.ports.has(kind) {
            Ok(())
        } else {
            Err(Refusal::port_unavailable(self.cmd(), kind.name(), what, alternative))
        }
    }

    /// Resolves a [`Ref`] (an id or `@name`) to a live element id, else `not_found`.
    pub fn resolve_ref(&self, r: &Ref) -> Result<ElementId, Refusal> {
        let store = self.store();
        if let Some(name) = r.0.strip_prefix('@') {
            return store
                .iter()
                .find(|e| e.name.as_deref() == Some(name))
                .map(|e| e.id.clone())
                .ok_or_else(|| Refusal::not_found(self.cmd(), &r.0));
        }
        let id = ElementId::new(&r.0).map_err(|_| Refusal::not_found(self.cmd(), &r.0))?;
        match store.get(&id) {
            Some(e) if !e.deleted => Ok(id),
            _ => Err(Refusal::not_found(self.cmd(), &r.0)),
        }
    }

    /// The target list of a command with `needs_selection = Required`: the explicit refs (each resolved,
    /// duplicates removed, order kept), else the selection. Empty gives `no_target`.
    pub fn targets(&self, explicit: Option<&[Ref]>) -> Result<Vec<ElementId>, Refusal> {
        let ids: Vec<ElementId> = match explicit {
            Some(refs) => {
                let mut out: Vec<ElementId> = Vec::with_capacity(refs.len());
                for r in refs {
                    let id = self.resolve_ref(r)?;
                    if !out.contains(&id) {
                        out.push(id);
                    }
                }
                out
            }
            None => {
                self.selection().iter().filter(|id| self.store().get(id).is_some_and(|e| !e.deleted)).cloned().collect()
            }
        };
        if ids.is_empty() { Err(Refusal::no_target(self.cmd())) } else { Ok(ids) }
    }

    /// A live element of the active tab.
    pub fn element(&self, id: &ElementId) -> Option<&Element> {
        self.store().get(id).filter(|e| !e.deleted)
    }

    // ---- undo and redo (used by the `history` family) ----

    /// Takes back up to `n` undo steps of the active tab and restores the selection and editing group
    /// from before them. It does not check gates; `history.undo` does.
    pub fn undo_steps(&mut self, n: usize) -> Result<UndoOutcome, Refusal> {
        let cmd = self.cmd();
        let t = self.tab_mut();
        let out = t.doc.undo(n).map_err(|e| Refusal::internal(cmd, format!("undo failed: {e}")))?;
        if let Some(obs) = &out.observed {
            t.observed = obs.clone();
        }
        if !out.labels.is_empty() {
            self.mark_dirty(Redraw::Scene);
        }
        Ok(out)
    }

    /// Re-applies up to `n` undone steps of the active tab.
    pub fn redo_steps(&mut self, n: usize) -> Result<UndoOutcome, Refusal> {
        let cmd = self.cmd();
        let t = self.tab_mut();
        let out = t.doc.redo(n).map_err(|e| Refusal::internal(cmd, format!("redo failed: {e}")))?;
        if let Some(obs) = &out.observed {
            t.observed = obs.clone();
        }
        if !out.labels.is_empty() {
            self.mark_dirty(Redraw::Scene);
        }
        Ok(out)
    }

    // ---- soft state (rollback) ----

    /// Copies the small non-document state of the active tab.
    pub fn soft_state(&self) -> SoftState {
        let t = self.tab();
        SoftState {
            tab: t.id,
            observed: t.observed.clone(),
            view: t.view,
            tool: self.tool.clone(),
            text_edit: self.text_edit.clone(),
            view_only: self.view_only,
        }
    }

    /// Puts back what [`Session::soft_state`] copied.
    pub fn restore_soft(&mut self, s: &SoftState) {
        if let Some(t) = self.tab_by_id_mut(s.tab) {
            t.observed = s.observed.clone();
            t.view = s.view;
        }
        self.tool = s.tool.clone();
        self.text_edit = s.text_edit.clone();
        self.view_only = s.view_only;
    }

    // ---- dirty reasons ----

    /// Raises the pending dirty reason.
    pub fn mark_dirty(&mut self, r: Redraw) {
        if r != Redraw::None && self.dirty.level.is_none_or(|cur| r > cur) {
            self.dirty.level = Some(r);
        }
    }

    /// The pending dirty reason, without clearing it.
    pub fn dirty(&self) -> Option<Redraw> {
        self.dirty.level
    }

    /// The text editor's latest result, as `text.edit_end` arguments (docs/exceptions.md E16). A command with the
    /// `Commit` gate ends the running text edit with these arguments instead of an empty commit. It does nothing
    /// when no text is being edited. It is not a command: it changes no document and no undo step.
    ///
    /// Non-command write N3 (docs/exceptions.md).
    pub fn stage_text_edit(&mut self, args: serde_json::Value) {
        if self.text_edit.is_some() {
            self.ext.with::<crate::state::StagedEdit, _>(|st| st.0 = Some(args));
        }
    }

    pub(crate) fn staged_text_edit(&self) -> Option<serde_json::Value> {
        self.ext.get::<crate::state::StagedEdit>().and_then(|st| st.0.clone())
    }

    pub(crate) fn clear_staged_text_edit(&mut self) {
        self.ext.with::<crate::state::StagedEdit, _>(|st| st.0 = None);
    }

    /// The pending dirty reason; clears it. A shell redraws only when this returns `Some`.
    pub fn take_dirty(&mut self) -> Option<Redraw> {
        self.dirty.level.take()
    }

    // ---- introspection ----

    /// The ring, oldest first.
    pub fn ring(&self) -> &Ring {
        &self.ring
    }

    /// True while a gesture runs.
    pub fn gesture_running(&self) -> bool {
        self.gesture.is_some()
    }

    /// A string that changes whenever any state class changes: documents, observed state, views, tool,
    /// settings, modes and history depth. Tests compare it before and after a refusal.
    pub fn fingerprint(&self) -> String {
        let mut out = String::new();
        for t in &self.tabs {
            out.push_str(&format!(
                "tab {} rev={} doc={:016x} obs={:?} view={:?} undo={}/{} saved={}\n",
                t.id,
                t.doc.revision(),
                doc_hash(t),
                t.observed,
                t.view,
                t.doc.undo_depth(),
                t.doc.redo_depth(),
                t.saved_revision
            ));
        }
        out.push_str(&format!(
            "active={} tool={:?} settings={:?} view_only={} text={:?} gesture={}\n",
            self.active_index(),
            self.tool,
            self.settings.revision,
            self.view_only,
            self.text_edit,
            self.gesture.is_some()
        ));
        out
    }

    /// `engine.state` payload.
    pub fn state_json(&self) -> Value {
        let t = self.tab();
        let collab = self.collab_view();
        json!({
            "tabs": self.tabs.iter().enumerate().map(|(i, t)| json!({
                "id": t.id.to_string(),
                "name": t.doc.store().meta().name,
                "active": i == self.active_index(),
                "dirty": t.dirty(),
                "blocked_by_schema": t.doc.blocked_by_schema(),
            })).collect::<Vec<_>>(),
            "tool": self.tool.effective().name(),
            "tool_lock": self.tool.lock,
            "view_only": self.view_only,
            "selection": {
                "count": t.observed.selection.len(),
                "groups": t.observed.selected_groups.iter().map(|g| g.as_str().to_owned()).collect::<Vec<_>>(),
                "editing_group": t.observed.editing_group.as_ref().map(|g| g.as_str().to_owned()),
            },
            "gesture": self.gesture.as_ref().map(GestureState::describe),
            "text_edit": self.text_edit.as_ref().map(|e| json!({
                "element": e.element.as_ref().map(|i| i.as_str().to_owned()),
                "draft": e.draft,
            })),
            "revision": t.doc.revision(),
            "dirty": t.dirty(),
            "blocked_by_schema": t.doc.blocked_by_schema(),
            "collab": { "state": collab.state, "room": collab.room },
            "settings_revision": self.settings.revision,
        })
    }

    /// Looks up a spec in this session's registry.
    pub fn spec(&self, id: &str) -> Option<&'static CommandSpec> {
        self.registry.find(id)
    }

    /// Wall-clock milliseconds from the clock port.
    pub fn now_ms(&self) -> u64 {
        self.ports.clock.now_ms()
    }

    /// The origin to use when a shell does not say (scripts).
    pub fn default_origin() -> Origin {
        Origin::Script
    }
}

/// A hash of the document content: every element (without its local change counter), the name, the
/// background and the blobs, in order.
fn doc_hash(t: &Tab) -> u64 {
    use std::hash::Hasher;
    struct W(std::collections::hash_map::DefaultHasher);
    impl std::io::Write for W {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.write(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let store = t.doc.store();
    let mut w = W(std::collections::hash_map::DefaultHasher::new());
    let m = store.meta();
    let _ = serde_json::to_writer(&mut w, &(&m.name, m.background));
    for e in store.iter_all() {
        let mut e = e.clone();
        e.version = 0;
        let _ = serde_json::to_writer(&mut w, &e);
    }
    for (id, f) in store.files() {
        let _ = serde_json::to_writer(&mut w, &(id, f.bytes.len(), &f.mime));
    }
    w.0.finish()
}
