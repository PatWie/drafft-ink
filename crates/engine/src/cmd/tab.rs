//! `tab`: canvas tabs (docs/commands.md §2.2).
//!
//! A tab holds one document with its own selection, camera and history. The tab list, the active tab
//! and per-tab metadata are outside the rollback transaction (docs/commands.md §1.7). Every handler
//! checks its input before the first change.
//!
//! The workspace record (all tabs saved to storage and restored at start) is in [`workspace`].

pub mod workspace;
mod workspace_save;

use drafftink_scene::limits::MAX_NAME_CHARS;
use drafftink_scene::{Color, Document, SceneRead};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::refusal::{Refusal, echo};
use crate::session::Session;
use crate::spec::{Batch, CommandSpec, Facts, Gate, Mutates, NoArgs, PortSet, Redraw, RingMode, Selection, TextGate, Typed, Undo};
use crate::state::{Tab, TabId};
use crate::store::{StepMeta, TabDoc};

/// Most tabs one session holds.
pub const MAX_TABS: usize = 50;
/// `Tab::saved_revision` of a document that was never saved: no revision equals it, so it stays dirty.
pub const NEVER_SAVED: u64 = u64::MAX;

/// Set by the `collab` family on a tab that has joined a room. `tab.close` refuses to drop such a tab
/// without `leave_room`, and `tab.list` shows the room.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoomBinding(pub Option<String>);

/// Names a tab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum TabRef {
    /// The position shown by `tab.list`, counted from 0.
    Index(u32),
    /// A tab id such as `t2`, else a tab name (the first tab with that name).
    Name(String),
}

impl TabRef {
    fn shown(&self) -> String {
        match self {
            Self::Index(i) => i.to_string(),
            Self::Name(n) => echo(n),
        }
    }
}

/// What to do with unsaved changes when a tab closes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OnUnsaved {
    /// Refuse with `unsaved_changes`.
    #[default]
    Refuse,
    /// Drop the changes.
    Discard,
    /// Save with `doc.save` first (the active tab only).
    Save,
}

/// A document name in normal form: trimmed and cut to [`MAX_NAME_CHARS`] characters. `empty_name` when
/// nothing is left, `bad_args` when it has a control character such as a line break (the document
/// refuses those). `doc.rename` uses the same rule.
pub fn normalize_name(id: &str, raw: &str) -> Result<String, Refusal> {
    let t = raw.trim();
    if t.is_empty() {
        return Err(Refusal::custom(
            "empty_name",
            id,
            format!("The name is empty. Give a name of 1 to {MAX_NAME_CHARS} characters."),
        ));
    }
    if t.chars().any(char::is_control) {
        return Err(Refusal::bad_arg(id, "name", "must not contain control characters such as line breaks"));
    }
    Ok(t.chars().take(MAX_NAME_CHARS).collect())
}

fn no_such_tab(id: &str, which: &TabRef) -> Refusal {
    Refusal::custom("no_such_tab", id, format!("There is no tab `{}`. List tabs with `tab.list`.", which.shown()))
        .suggesting("tab.list", None)
}

pub(crate) fn name_of(t: &Tab) -> String {
    t.doc.store().meta().name.clone()
}

fn room_of(t: &Tab) -> Option<String> {
    t.ext.get::<RoomBinding>().and_then(|r| r.0.clone())
}

/// The index of the named tab, or of the active tab when none is named.
fn resolve(s: &Session, id: &str, which: Option<&TabRef>) -> Result<usize, Refusal> {
    let Some(w) = which else {
        return Ok(s.active_index());
    };
    let found = match w {
        TabRef::Index(i) => usize::try_from(*i).ok().filter(|i| *i < s.tabs.len()),
        TabRef::Name(n) => s
            .tabs
            .iter()
            .position(|t| t.id.to_string() == *n)
            .or_else(|| s.tabs.iter().position(|t| name_of(t) == *n)),
    };
    found.ok_or_else(|| no_such_tab(id, w))
}

pub(crate) fn too_many_tabs(id: &str) -> Refusal {
    Refusal::custom(
        "too_many_tabs",
        id,
        format!("There are already {MAX_TABS} tabs, which is the limit. Close one first with `tab.close`."),
    )
    .suggesting("tab.close", None)
}

/// The tab after a tab command.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct TabOut {
    /// False when the state was already as requested.
    pub changed: bool,
    /// The tab the command acted on (for `tab.close`, the tab that was closed).
    pub id: String,
    /// Its position, counted from 0 (before it was closed, for `tab.close`).
    pub index: usize,
    /// Its name.
    pub name: String,
    /// The active tab after the command.
    pub active: String,
    /// The undo step of a rename in a tab that is not the active one (the executor reports the others).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub undo_label: Option<String>,
}

fn out(s: &Session, changed: bool, id: TabId, index: usize, name: String) -> TabOut {
    TabOut { changed, id: id.to_string(), index, name, active: s.tab().id.to_string(), undo_label: None }
}

// ---- building tabs ----

/// A new tab around `doc`. It is clean: nothing in it differs from what it was made from.
pub(crate) fn tab_from(s: &mut Session, id: &str, doc: &Document) -> Result<Tab, Refusal> {
    let td = TabDoc::from_document(doc).map_err(|e| Refusal::internal(id, format!("building the tab failed: {e}")))?;
    let tid = s.alloc_tab_id();
    let mut t = Tab::new(tid);
    t.saved_revision = td.revision();
    t.doc = td;
    Ok(t)
}

/// An empty document with the default name and the background of the setting `doc.default_background`.
pub(crate) fn blank_document(s: &Session, name: Option<String>) -> Document {
    let background = s
        .settings
        .get("doc.default_background")
        .and_then(serde_json::Value::as_str)
        .and_then(crate::cmd::settings::keys::parse_color)
        .unwrap_or(Color::WHITE);
    Document { name: name.unwrap_or_else(|| Document::default().name), background, ..Document::default() }
}

pub(crate) fn blank_tab(s: &mut Session, id: &str, name: Option<String>) -> Result<Tab, Refusal> {
    let doc = blank_document(s, name);
    tab_from(s, id, &doc)
}

/// A tab holding a copy of the document of tab `src`: same content, selection and camera, no history, and
/// no connection to a room or a save target. A copy of a document with unsaved changes has unsaved changes.
fn copy_tab(s: &mut Session, id: &str, src: usize, name: Option<String>) -> Result<Tab, Refusal> {
    let Some(from) = s.tabs.get(src) else {
        return Err(Refusal::internal(id, "the source tab disappeared"));
    };
    let mut doc = from.doc.snapshot();
    doc.name = name.unwrap_or_else(|| {
        let copy = format!("{} copy", doc.name);
        copy.chars().take(MAX_NAME_CHARS).collect()
    });
    let (observed, view, dirty) = (from.observed.clone(), from.view, from.dirty());
    let mut t = tab_from(s, id, &doc)?;
    t.observed = observed;
    t.observed.linear_editor = None;
    t.view = view;
    if dirty {
        t.saved_revision = NEVER_SAVED;
    }
    Ok(t)
}

/// Puts `tab` after the active tab and makes it the active one. The canvas is one widget, so the new
/// tab takes over the viewport size of the tab being left.
pub(crate) fn activate_new(s: &mut Session, tab: Tab) {
    let at = s.active_index() + 1;
    let viewport = s.view().viewport;
    s.tab_mut().observed.linear_editor = None;
    let mut tab = tab;
    tab.view.viewport = viewport;
    s.tabs.insert(at.min(s.tabs.len()), tab);
    s.set_active_index(at);
}

fn optional_name(id: &str, name: Option<&str>) -> Result<Option<String>, Refusal> {
    match name {
        Some(n) => normalize_name(id, n).map(Some),
        None => Ok(None),
    }
}

// ---- tab.new ----

/// Where the content of a new tab comes from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NewFrom {
    /// An empty document with the default background.
    #[default]
    Blank,
    /// The built-in intro document.
    Intro,
    /// A copy of the active document.
    Active,
}

/// Arguments of `tab.new`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct NewArgs {
    /// Name of the new document (1 to 256 characters). Default: "Untitled", or "<name> copy" for `active`.
    pub name: Option<String>,
    /// What the new tab holds: `blank` (default), `intro` or `active`.
    pub from: NewFrom,
}

fn new_tab(s: &mut Session, a: NewArgs) -> Result<TabOut, Refusal> {
    let id = s.cmd();
    let name = optional_name(id, a.name.as_deref())?;
    if s.tabs.len() >= MAX_TABS {
        return Err(too_many_tabs(id));
    }
    let tab = match a.from {
        NewFrom::Blank => blank_tab(s, id, name)?,
        NewFrom::Active => copy_tab(s, id, s.active_index(), name)?,
        NewFrom::Intro => {
            let mut doc = crate::cmd::doc::intro_document(s, id)?;
            if let Some(n) = name {
                doc.name = n;
            }
            tab_from(s, id, &doc)?
        }
    };
    let (tid, label) = (tab.id, name_of(&tab));
    activate_new(s, tab);
    Ok(out(s, true, tid, s.active_index(), label))
}

// ---- tab.switch ----

/// Arguments of `tab.switch`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SwitchArgs {
    /// The tab: its position (counted from 0), its id such as `t2`, or its name.
    pub tab: TabRef,
}

fn switch(s: &mut Session, a: SwitchArgs) -> Result<TabOut, Refusal> {
    let i = resolve(s, s.cmd(), Some(&a.tab))?;
    let Some(t) = s.tabs.get(i) else {
        return Err(no_such_tab(s.cmd(), &a.tab));
    };
    let (tid, label) = (t.id, name_of(t));
    if i == s.active_index() {
        return Ok(out(s, false, tid, i, label));
    }
    let viewport = s.view().viewport;
    s.tab_mut().observed.linear_editor = None;
    s.set_active_index(i);
    s.tab_mut().view.viewport = viewport;
    Ok(out(s, true, tid, i, label))
}

// ---- tab.close ----

/// Arguments of `tab.close`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct CloseArgs {
    /// The tab to close. Default: the active tab.
    pub tab: Option<TabRef>,
    /// What to do when the tab has unsaved changes: `refuse` (default), `discard` or `save`.
    pub on_unsaved: OnUnsaved,
    /// Leave the room the tab is in. Default false: such a tab is not closed.
    pub leave_room: bool,
}

fn close(s: &mut Session, a: CloseArgs) -> Result<TabOut, Refusal> {
    let id = s.cmd();
    let i = resolve(s, id, a.tab.as_ref())?;
    let active = s.active_index();
    if s.tabs.len() <= 1 {
        return Err(Refusal::custom(
            "last_tab",
            id,
            "The last tab cannot be closed. Open another tab first with `tab.new`, or clear this document with `doc.clear`.",
        )
        .suggesting("tab.new", None));
    }
    let Some(t) = s.tabs.get(i) else {
        return Err(Refusal::internal(id, "the tab disappeared"));
    };
    let (tid, label, dirty, room) = (t.id, name_of(t), t.dirty(), room_of(t));
    if dirty && a.on_unsaved == OnUnsaved::Refuse {
        return Err(Refusal::unsaved_changes(id));
    }
    if dirty && a.on_unsaved == OnUnsaved::Save && i != active {
        return Err(Refusal::bad_arg(
            id,
            "on_unsaved",
            format!("\"save\" works on the active tab only. Switch to `{}` with `tab.switch`, save it with `doc.save`, then close it", echo(&label)),
        ));
    }
    if let Some(r) = &room
        && !a.leave_room
    {
        return Err(Refusal::custom(
            "in_room",
            id,
            format!(
                "Tab `{}` is connected to room {}. Leave the room first with `collab.leave_room`, or pass leave_room=true.",
                echo(&label),
                echo(r)
            ),
        )
        .suggesting("collab.leave_room", None));
    }
    // Everything is checked. The nested commands below can still refuse (no save target); they run
    // before the tab is touched.
    if dirty && a.on_unsaved == OnUnsaved::Save {
        if s.registry().find("doc.save").is_none() {
            return Err(Refusal::not_supported(id, "on_unsaved=\"save\" needs `doc.save`, which is not registered in this session"));
        }
        s.run_nested("doc.save", json!({}))?;
        // The write is queued: make it now, so that the tab is not closed over a failed save.
        s.settle_saves(id)?;
    }
    if room.is_some() && i == active && s.registry().find("collab.leave_room").is_some() {
        s.run_nested("collab.leave_room", json!({}))?;
    }
    let viewport = s.view().viewport;
    s.tabs.remove(i);
    let left = s.tabs.len();
    let now = if i == active {
        i.min(left.saturating_sub(1))
    } else if i < active {
        active - 1
    } else {
        active
    };
    s.set_active_index(now);
    s.tab_mut().view.viewport = viewport;
    Ok(out(s, true, tid, i, label))
}

// ---- tab.rename ----

/// Arguments of `tab.rename`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameArgs {
    /// The tab to rename. Default: the active tab.
    #[serde(default)]
    pub tab: Option<TabRef>,
    /// The new name, 1 to 256 characters after trimming.
    pub name: String,
}

fn rename(s: &mut Session, a: RenameArgs) -> Result<TabOut, Refusal> {
    let id = s.cmd();
    let name = normalize_name(id, &a.name)?;
    let i = resolve(s, id, a.tab.as_ref())?;
    let Some(t) = s.tabs.get(i) else {
        return Err(Refusal::internal(id, "the tab disappeared"));
    };
    let tid = t.id;
    if name_of(t) == name {
        return Ok(out(s, false, tid, i, name));
    }
    if i == s.active_index() {
        // The executor commits the change of the active tab as one undo step.
        s.tab_mut()
            .doc
            .edit::<(), drafftink_scene::StoreError>(|tx| {
                use drafftink_scene::SceneTx;
                tx.set_name(&name)
            })
            .map_err(|e| Refusal::internal(id, e.to_string()))?;
        return Ok(out(s, true, tid, i, name));
    }
    // A tab in the background: the executor only commits the active tab, so this tab's step is
    // written here, as one undo step in its own history.
    let depth = s.settings.history_depth();
    let label = s.spec(id).map_or("Rename tab", |sp| sp.title);
    let t = s.tabs.get_mut(i).ok_or_else(|| Refusal::internal(id, "the tab disappeared"))?;
    let mark = t.doc.mark();
    let edited = t.doc.edit::<(), drafftink_scene::StoreError>(|tx| {
        use drafftink_scene::SceneTx;
        tx.set_name(&name)
    });
    if let Err(e) = edited {
        return Err(Refusal::internal(id, e.to_string()));
    }
    let meta = StepMeta { label, undoable: true, before: &t.observed, after: &t.observed, depth, coalesce: None };
    if let Err(e) = t.doc.commit_since(mark, &meta) {
        let _ = t.doc.rollback_to(mark);
        return Err(Refusal::internal(id, format!("saving the change failed: {e}")));
    }
    s.mark_dirty(Redraw::Scene);
    let mut done = out(s, true, tid, i, name);
    done.undo_label = Some(label.to_owned());
    Ok(done)
}

// ---- tab.move ----

/// Arguments of `tab.move`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveArgs {
    /// The tab to move. Default: the active tab.
    #[serde(default)]
    pub tab: Option<TabRef>,
    /// Its new position, counted from 0.
    pub to_index: i64,
}

fn move_tab(s: &mut Session, a: MoveArgs) -> Result<TabOut, Refusal> {
    let id = s.cmd();
    let i = resolve(s, id, a.tab.as_ref())?;
    let last = s.tabs.len().saturating_sub(1);
    let to = usize::try_from(a.to_index)
        .ok()
        .filter(|t| *t <= last)
        .ok_or_else(|| Refusal::bad_arg(id, "to_index", format!("must be a position from 0 to {last}")))?;
    let (tid, label) = match s.tabs.get(i) {
        Some(t) => (t.id, name_of(t)),
        None => return Err(Refusal::internal(id, "the tab disappeared")),
    };
    if i == to {
        return Ok(out(s, false, tid, i, label));
    }
    let active = s.tab().id;
    let t = s.tabs.remove(i);
    s.tabs.insert(to, t);
    if let Some(k) = s.tabs.iter().position(|t| t.id == active) {
        s.set_active_index(k);
    }
    Ok(out(s, true, tid, to, label))
}

// ---- tab.duplicate ----

/// Arguments of `tab.duplicate`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct DuplicateArgs {
    /// The tab to copy. Default: the active tab.
    pub tab: Option<TabRef>,
    /// Name of the copy. Default: "<name> copy".
    pub name: Option<String>,
}

fn duplicate(s: &mut Session, a: DuplicateArgs) -> Result<TabOut, Refusal> {
    let id = s.cmd();
    let name = optional_name(id, a.name.as_deref())?;
    let i = resolve(s, id, a.tab.as_ref())?;
    if s.tabs.len() >= MAX_TABS {
        return Err(too_many_tabs(id));
    }
    let tab = copy_tab(s, id, i, name)?;
    let (tid, label) = (tab.id, name_of(&tab));
    // The copy goes right after its source and becomes the active tab.
    let viewport = s.view().viewport;
    s.tab_mut().observed.linear_editor = None;
    let mut tab = tab;
    tab.view.viewport = viewport;
    let at = (i + 1).min(s.tabs.len());
    s.tabs.insert(at, tab);
    s.set_active_index(at);
    Ok(out(s, true, tid, at, label))
}

// ---- tab.list ----

/// One tab in `tab.list`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct TabInfo {
    /// The tab id, such as `t2`.
    pub id: String,
    /// The position, counted from 0.
    pub index: usize,
    /// The document name.
    pub name: String,
    /// Whether it is the active tab.
    pub active: bool,
    /// Whether the document has changes that were not saved.
    pub dirty: bool,
    /// The room the tab is connected to, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
}

/// Output of `tab.list`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ListOut {
    /// Always false.
    pub changed: bool,
    /// The tabs, left to right.
    pub tabs: Vec<TabInfo>,
}

fn list(s: &mut Session, _: NoArgs) -> Result<ListOut, Refusal> {
    let active = s.active_index();
    let tabs = s
        .tabs
        .iter()
        .enumerate()
        .map(|(index, t)| TabInfo {
            id: t.id.to_string(),
            index,
            name: name_of(t),
            active: index == active,
            dirty: t.dirty(),
            room: room_of(t),
        })
        .collect();
    Ok(ListOut { changed: false, tabs })
}

// ---- registration ----

/// A tab command: tabs only, no undo, allowed in view-only mode, refused during a drag, ends a text edit
/// first.
const TABS: Facts = Facts {
    undo: Undo::NoUndo,
    reads_only: false,
    mutates: Mutates::TABS,
    needs_selection: Selection::No,
    ports: PortSet::NONE,
    batch: Batch::Allowed,
    view_only: Gate::Allowed,
    while_gesture: Gate::Refused,
    while_text_edit: TextGate::Commit,
    redraw: Redraw::Scene,
    ring: RingMode::Always,
    palette: true,
    agent: true,
};

/// The tab family.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "tab.new",
        "tab",
        "New tab",
        "Adds a tab after the active one and activates it: a blank document, the intro, or a copy of the active one.",
        &Typed::new(new_tab),
        TABS,
    )
    .refusals(&["empty_name", "too_many_tabs"])
    .example(r#"{"name":"Sketch","from":"blank"}"#),
    CommandSpec::new(
        "tab.switch",
        "tab",
        "Switch tab",
        "Activates a tab, named by position, id or name. Each tab keeps its own selection, camera and history.",
        &Typed::new(switch),
        TABS,
    )
    .refusals(&["no_such_tab"])
    .example(r#"{"tab":"t2"}"#),
    CommandSpec::new(
        "tab.close",
        "tab",
        "Close tab",
        "Closes a tab and activates its right neighbour. Refuses for unsaved changes, a room, and the last tab.",
        &Typed::new(close),
        Facts { batch: Batch::Last, ..TABS },
    )
    .refusals(&["no_such_tab", "last_tab", "in_room"])
    .example(r#"{"tab":1,"on_unsaved":"discard"}"#),
    CommandSpec::new(
        "tab.rename",
        "tab",
        "Rename tab",
        "Renames the document of a tab. One undo step in that tab's history.",
        &Typed::new(rename),
        Facts { undo: Undo::Undoable, mutates: Mutates::DOC.with(Mutates::TABS), view_only: Gate::Refused, ..TABS },
    )
    .refusals(&["no_such_tab", "empty_name"])
    .example(r#"{"name":"Architecture"}"#),
    CommandSpec::new(
        "tab.move",
        "tab",
        "Move tab",
        "Moves a tab to another position in the tab bar.",
        &Typed::new(move_tab),
        TABS,
    )
    .refusals(&["no_such_tab"])
    .example(r#"{"tab":"t2","to_index":0}"#),
    CommandSpec::new(
        "tab.duplicate",
        "tab",
        "Duplicate tab",
        "Copies the document of a tab into a new tab right after it and activates the copy.",
        &Typed::new(duplicate),
        TABS,
    )
    .refusals(&["no_such_tab", "empty_name", "too_many_tabs"])
    .example(r#"{"name":"Draft 2"}"#),
    CommandSpec::new(
        "tab.list",
        "tab",
        "List tabs",
        "Lists the tabs with their id, position, name, active and unsaved state, and room.",
        &Typed::new(list),
        Facts { palette: true, ..Facts::READ_ONLY },
    ),
];

#[cfg(test)]
#[path = "tab_tests.rs"]
mod tests;
