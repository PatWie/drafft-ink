//! The canvas context menu. The canvas layer opens it with `UiState::open_context_menu` on a right click.
//!
//! The menu is built per target (the empty canvas, a selection, a locked element) as a list of entries, and the
//! entries the engine refuses are left out: availability is `Session::availability` (the model's refusal text), not
//! a rule of this file. `Paste` stays and greys, because the clipboard is not the document. A command the registry
//! does not have stays too, greyed with the model's "not available in this build" text.
//!
//! Labels come from the registry (`Model::label`), so a command reads the same here, in the panel, in the palette and
//! in the hamburger menu. The rows live in [`crate::menu_kit`]; groups of rows open as flyouts. `Paste` puts its
//! content at the point where the menu was opened. Choosing a row closes the menu; a click anywhere else closes it
//! too.

use drafftink_engine::Origin;
use egui::{Align2, Context};
use serde_json::{Value, json};

use crate::action::Verb;
use crate::icons::Icon;
use crate::menu_kit::{Menu, Popup, Row};
use crate::model::{Model, SelectedElement};
use crate::state::{RenameTarget, UiState};
use crate::widgets::{Ctl, Cx};

/// The arguments of `clipboard.paste`: the content lands where the menu was opened, when that point is
/// known, else in the middle of the canvas (the command's default).
pub(crate) fn paste_args(world: Option<[f64; 2]>) -> Value {
    world.map_or_else(|| json!({}), |at| json!({ "at": at }))
}

/// The edges of the Align block, in the order of the menu.
pub const ALIGN_EDGES: [&str; 6] = ["left", "center_x", "right", "top", "center_y", "bottom"];

/// The kinds that are shapes (`element.convert` turns one into another).
const SHAPES: [&str; 3] = ["rectangle", "diamond", "ellipse"];
/// The kinds that are lines or arrows.
const LINEAR: [&str; 2] = ["line", "arrow"];

fn all_of(sel: &[SelectedElement], kinds: &[&str]) -> bool {
    !sel.is_empty() && sel.iter().all(|e| kinds.contains(&e.kind.as_str()))
}

/// The only selected element, when exactly one is selected.
fn single(sel: &[SelectedElement]) -> Option<&SelectedElement> {
    match sel {
        [one] => Some(one),
        _ => None,
    }
}

/// The arguments of `text.bind` for one free text and one container in the selection.
pub(crate) fn bind_args(sel: &[SelectedElement]) -> Option<Value> {
    let texts: Vec<&SelectedElement> = sel.iter().filter(|e| e.kind == "text").collect();
    let containers: Vec<&SelectedElement> =
        sel.iter().filter(|e| SHAPES.contains(&e.kind.as_str()) || e.kind == "arrow").collect();
    match (texts.as_slice(), containers.as_slice(), sel.len()) {
        ([text], [container], 2) => Some(json!({ "text": text.id, "container": container.id })),
        _ => None,
    }
}

/// The arguments of `text.unbind`: the label of the single selected container.
pub(crate) fn unbind_args(sel: &[SelectedElement]) -> Option<Value> {
    match sel {
        [one] => one.label.as_ref().map(|label| json!({ "text": label })),
        _ => None,
    }
}

/// The ids of the selected elements that sit in a frame.
pub(crate) fn in_frame_ids(sel: &[SelectedElement]) -> Vec<&str> {
    sel.iter().filter(|e| e.frame.is_some()).map(|e| e.id.as_str()).collect()
}

/// The `element.convert` arguments that apply to this selection: shapes become shapes and lines and arrows become
/// lines and arrows, as the command says. The titles are the command's variants.
pub(crate) fn convert_targets(sel: &[SelectedElement]) -> Vec<Value> {
    if all_of(sel, &SHAPES) {
        vec![json!({ "to": "rectangle" }), json!({ "to": "diamond" }), json!({ "to": "ellipse" })]
    } else if all_of(sel, &LINEAR) {
        vec![
            json!({ "to": "line" }),
            json!({ "to": "arrow", "arrow_type": "sharp" }),
            json!({ "to": "arrow", "arrow_type": "round" }),
            json!({ "to": "arrow", "arrow_type": "elbow" }),
        ]
    } else {
        Vec::new()
    }
}

/// The arguments of the Unlock row: that one element (its whole group unlocks with it).
pub(crate) fn unlock_args(element: &str) -> Value {
    json!({ "ids": [element] })
}

// ---- the entries ----

/// When a row is left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Show {
    /// While the engine does not refuse the command.
    WhenAvailable,
    /// Always; a refused row greys.
    Always,
}

/// What a row does.
enum Act {
    /// Calls a command. The label is the registry's unless the row has its own.
    Command(String, Value),
    /// Applies a UI verb. The verb leads to `gate`, whose refusal leaves the row out.
    Verb { verb: Verb, label: &'static str, gate: Option<&'static str> },
    /// Opens the rename dialog of a frame.
    RenameFrame(String),
    /// Opens a flyout.
    Submenu { label: &'static str, entries: Vec<Entry> },
}

/// One entry of a menu.
struct Entry {
    act: Act,
    icon: Option<Icon>,
    /// A label of the row's own, for calls whose arguments no registry title names.
    label: Option<&'static str>,
    check: Option<bool>,
    show: Show,
    dialog: bool,
    /// Starts a new section.
    sep_before: bool,
}

impl Entry {
    fn new(act: Act) -> Self {
        Self { act, icon: None, label: None, check: None, show: Show::WhenAvailable, dialog: false, sep_before: false }
    }

    fn command(id: &str) -> Self {
        Self::with_args(id, json!({}))
    }

    fn with_args(id: &str, args: Value) -> Self {
        Self::new(Act::Command(id.to_owned(), args))
    }

    fn verb(verb: Verb, label: &'static str, gate: Option<&'static str>) -> Self {
        Self::new(Act::Verb { verb, label, gate })
    }

    fn submenu(label: &'static str, icon: Icon, entries: Vec<Entry>) -> Self {
        Self::new(Act::Submenu { label, entries }).icon(icon)
    }

    fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    fn label(mut self, label: &'static str) -> Self {
        self.label = Some(label);
        self
    }

    fn checked(mut self, on: bool) -> Self {
        self.check = Some(on);
        self
    }

    fn always(mut self) -> Self {
        self.show = Show::Always;
        self
    }

    fn dialog(mut self) -> Self {
        self.dialog = true;
        self
    }

    fn section(mut self) -> Self {
        self.sep_before = true;
        self
    }

    /// The command this entry stands on, if it has one.
    fn command_id(&self) -> Option<&str> {
        match &self.act {
            Act::Command(id, _) => Some(id),
            Act::Verb { gate, .. } => *gate,
            Act::RenameFrame(_) | Act::Submenu { .. } => None,
        }
    }
}

/// What the menu is about.
enum Target<'a> {
    /// Nothing under the pointer.
    Empty,
    /// The selection.
    Selection(&'a [SelectedElement]),
    /// A locked element, which cannot be selected.
    Locked(&'a str),
}

/// The entries of the menu for a target. `stats` and `zen` are the states of the two view toggles that live in the
/// UI.
fn entries(target: &Target<'_>, model: &Model, world: Option<[f64; 2]>, stats: bool, zen: bool) -> Vec<Entry> {
    let paste = || Entry::with_args("clipboard.paste", paste_args(world)).icon(Icon::Paste).always();
    let select_all = || Entry::command("selection.all").icon(Icon::SelectAll);
    let fit = || Entry::with_args("view.zoom_to_fit", json!({ "scope": "all" })).icon(Icon::ZoomFit);
    match target {
        Target::Empty => vec![
            paste(),
            select_all().section(),
            fit(),
            Entry::submenu("View", Icon::Eye, view_entries(model, stats, zen)).section(),
        ],
        Target::Locked(id) => vec![
            Entry::with_args("element.unlock_all", unlock_args(id)).label("Unlock").icon(Icon::Unlock).always(),
            paste().section(),
            select_all(),
            fit(),
        ],
        Target::Selection(sel) => selection_entries(sel, paste()),
    }
}

fn view_entries(model: &Model, stats: bool, zen: bool) -> Vec<Entry> {
    vec![
        Entry::with_args("settings.cycle", json!({ "key": "view.grid_style" })).icon(Icon::GridLines).always(),
        Entry::with_args("settings.toggle", json!({ "key": "snap.grid" }))
            .icon(Icon::SnapGrid)
            .checked(model.setting_bool("snap.grid"))
            .always(),
        Entry::verb(Verb::ToggleZen, "Zen mode", None).checked(zen),
        Entry::command("view.set_view_only").checked(model.view_only).always(),
        Entry::verb(Verb::ToggleStats, "Stats panel", None).checked(stats),
        Entry::command("element.unlock_all").icon(Icon::Unlock).section(),
    ]
}

fn selection_entries(sel: &[SelectedElement], paste: Entry) -> Vec<Entry> {
    let out = vec![
        Entry::command("clipboard.cut").icon(Icon::Cut),
        Entry::command("clipboard.copy").icon(Icon::Copy),
        paste,
        Entry::command("element.duplicate").icon(Icon::Duplicate),
        Entry::command("element.delete").icon(Icon::Delete),
        Entry::command("group.create").icon(Icon::Group).section(),
        Entry::command("group.ungroup").icon(Icon::Ungroup),
        Entry::submenu("Arrange", Icon::Layers, arrange_entries()).section(),
        Entry::submenu("Align", Icon::AlignLeft, align_entries()),
        Entry::submenu("Flip & convert", Icon::Convert, flip_entries(sel)),
        Entry::submenu("Frame", Icon::Frame, frame_entries(sel)),
        Entry::submenu("Text", Icon::Text, text_entries(sel)),
        Entry::submenu("Copy as", Icon::Export, copy_as_entries()),
        Entry::command("element.lock").icon(Icon::Lock).section(),
        Entry::verb(Verb::OpenLinkEditor, "Link", Some("element.set_link")).icon(Icon::Link).dialog(),
        Entry::command("element.open_link").icon(Icon::Link),
        Entry::command("library.add").icon(Icon::Library),
        Entry::command("clipboard.copy_style").icon(Icon::Brush).section(),
        Entry::command("clipboard.paste_style").icon(Icon::Brush),
    ];
    out
}

fn arrange_entries() -> Vec<Entry> {
    let z = |op: &str, icon: Icon| Entry::with_args("arrange.z_order", json!({ "op": op })).icon(icon);
    vec![
        z("front", Icon::LayerFront),
        z("forward", Icon::LayerForward),
        z("backward", Icon::LayerBackward),
        z("back", Icon::LayerBack),
        Entry::command("frame.wrap").icon(Icon::Frame).section(),
    ]
}

fn align_entries() -> Vec<Entry> {
    let icons =
        [Icon::AlignLeft, Icon::AlignCenterX, Icon::AlignRight, Icon::AlignTop, Icon::AlignCenterY, Icon::AlignBottom];
    let mut out: Vec<Entry> = ALIGN_EDGES
        .iter()
        .zip(icons)
        .map(|(edge, icon)| Entry::with_args("arrange.align", json!({ "edge": edge })).icon(icon))
        .collect();
    out.push(Entry::with_args("arrange.distribute", json!({ "axis": "horizontal" })).icon(Icon::DistributeH).section());
    out.push(Entry::with_args("arrange.distribute", json!({ "axis": "vertical" })).icon(Icon::DistributeV));
    out
}

fn flip_entries(sel: &[SelectedElement]) -> Vec<Entry> {
    let mut out = vec![
        Entry::with_args("element.flip", json!({ "axis": "horizontal" })).icon(Icon::FlipH),
        Entry::with_args("element.flip", json!({ "axis": "vertical" })).icon(Icon::FlipV),
    ];
    let targets = convert_targets(sel);
    for (i, args) in targets.into_iter().enumerate() {
        let entry = Entry::with_args("element.convert", args);
        out.push(if i == 0 { entry.section() } else { entry });
    }
    out.push(Entry::command("arrow.edit_points").section());
    out.push(Entry::command("text.wrap_in_container"));
    out
}

fn frame_entries(sel: &[SelectedElement]) -> Vec<Entry> {
    // With one frame selected the rows name it; otherwise the commands take "the selected frame" and the engine
    // says why they cannot.
    let frame = single(sel).filter(|e| e.kind == "frame");
    let frame_args = frame.map_or_else(|| json!({}), |e| json!({ "frame": e.id }));
    let inside = in_frame_ids(sel);
    let remove_args = if inside.is_empty() { json!({}) } else { json!({ "ids": inside }) };
    let mut out = vec![
        Entry::with_args("frame.unwrap", frame_args.clone()),
        Entry::with_args("frame.select_children", frame_args),
    ];
    if let Some(frame) = frame {
        out.push(Entry::new(Act::RenameFrame(frame.id.clone())).label("Rename frame").dialog().always());
    }
    out.push(Entry::with_args("frame.remove", remove_args));
    out
}

fn text_entries(sel: &[SelectedElement]) -> Vec<Entry> {
    let mut out = Vec::new();
    if let Some(args) = bind_args(sel) {
        out.push(Entry::with_args("text.bind", args));
    }
    if let Some(args) = unbind_args(sel) {
        out.push(Entry::with_args("text.unbind", args));
    }
    out
}

fn copy_as_entries() -> Vec<Entry> {
    // Whether the selection holds a text is checked when the copy runs, and a click on the row says so.
    ["png", "svg", "text"].map(|format| Entry::with_args("clipboard.copy_as", json!({ "format": format }))).into()
}

/// Leaves out what the engine refuses (see the module text), and the flyouts that end up empty.
fn prune(entries: Vec<Entry>, model: &Model) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    let mut section = false;
    for mut entry in entries {
        section |= entry.sep_before;
        if let Act::Submenu { entries, .. } = &mut entry.act {
            *entries = prune(std::mem::take(entries), model);
            if entries.is_empty() {
                continue;
            }
        }
        let registered_and_refused =
            entry.command_id().is_some_and(|id| model.command(id).is_some() && model.refusal_text(id).is_some());
        if entry.show == Show::WhenAvailable && registered_and_refused {
            continue;
        }
        entry.sep_before = section && !out.is_empty();
        section = false;
        out.push(entry);
    }
    out
}

/// Draws the entries into a menu.
fn draw(menu: &mut Menu<'_, '_, '_>, entries: Vec<Entry>, st: &mut UiState) {
    for entry in entries {
        if entry.sep_before {
            menu.separator();
        }
        let mut row = Row::new();
        if let Some(icon) = entry.icon {
            row = row.icon(icon);
        }
        if let Some(on) = entry.check {
            row = row.checked(on);
        }
        if entry.dialog {
            row = row.dialog();
        }
        match entry.act {
            Act::Command(id, args) => {
                if let Some(label) = entry.label {
                    row = row.label(label);
                }
                menu.command(Ctl::new(&id).args(args), row);
            }
            Act::Verb { verb, label, gate } => {
                let reason = gate.and_then(|id| menu.refusal(id));
                menu.verb(verb, label, row, reason);
            }
            Act::RenameFrame(frame) => {
                let label = entry.label.unwrap_or("Rename frame");
                if menu.local(label, row).clicked() {
                    st.open_rename(RenameTarget::Frame(frame), "");
                }
            }
            Act::Submenu { label, entries } => {
                menu.submenu(label, row, |flyout| draw(flyout, entries, st));
            }
        }
    }
}

/// Draws the menu when `st.context_menu` is set.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let Some(menu) = st.context_menu.clone() else {
        return;
    };
    let sel = cx.model.selection.clone();
    let target = match &menu.locked {
        Some(id) => Target::Locked(id),
        None if sel.is_empty() => Target::Empty,
        None => Target::Selection(&sel),
    };
    let list = prune(entries(&target, cx.model, menu.world, st.stats_open, st.zen), cx.model);
    let spec = Popup {
        key: egui::Id::unique(("context_menu", menu.opened)),
        label: "Context menu",
        pivot: Align2::LEFT_TOP,
        at: menu.screen,
    };
    let result =
        cx.with_origin(Origin::ContextMenu, |cx| crate::menu_kit::popup(ctx, cx, &spec, |m| draw(m, list, st)));
    if result.picked.any() {
        st.context_menu = None;
    } else if result.outside {
        cx.verb(Verb::CloseOverlays);
    }
}
