//! The canvas context menu. The canvas layer opens it with `UiState::open_context_menu` on a right click.
//! Every row is a command (`Ctl`) drawn with `menu_item`, so a command that cannot run now is greyed with
//! its refusal text as the tooltip, and a command the registry does not have is greyed with the
//! model's "not available in this build" text. `Paste` puts its content at the point where the menu was
//! opened. Choosing a row closes the menu; a click anywhere else
//! closes it too.

use std::collections::BTreeSet;

use drafftink_engine::Origin;
use egui::{Area, Context, Id, Order, Sense, Ui, pos2};
use serde_json::{Value, json};

use crate::action::Verb;
use crate::model::SelectedElement;
use crate::state::{RenameTarget, UiState};
use crate::theme::MARGIN;
use crate::widgets::{Ctl, Cx, panel_frame, separator};

/// Width of the menu.
const WIDTH: f32 = 230.0;
/// The least distance of the menu from the window edge.
const SCREEN_GAP: f32 = MARGIN;

/// One row: the text, the command and its arguments.
struct Item {
    text: &'static str,
    id: &'static str,
    args: Value,
}

fn item(text: &'static str, id: &'static str, args: Value) -> Item {
    Item { text, id, args }
}

/// The arguments of `clipboard.paste`: the content lands where the menu was opened, when that point is
/// known, else in the middle of the canvas (the command's default).
pub(crate) fn paste_args(world: Option<[f64; 2]>) -> Value {
    world.map_or_else(|| json!({}), |at| json!({ "at": at }))
}

/// The rows of the first block: the clipboard and deleting.
fn edit_items(world: Option<[f64; 2]>) -> Vec<Item> {
    vec![
        item("Cut", "clipboard.cut", json!({})),
        item("Copy", "clipboard.copy", json!({})),
        item("Paste", "clipboard.paste", paste_args(world)),
        item("Duplicate", "element.duplicate", json!({})),
        item("Delete", "element.delete", json!({})),
    ]
}

fn group_items() -> Vec<Item> {
    vec![item("Group", "group.create", json!({})), item("Ungroup", "group.ungroup", json!({}))]
}

fn order_items() -> Vec<Item> {
    vec![
        item("Bring to front", "arrange.z_order", json!({ "op": "front" })),
        item("Bring forward", "arrange.z_order", json!({ "op": "forward" })),
        item("Send backward", "arrange.z_order", json!({ "op": "backward" })),
        item("Send to back", "arrange.z_order", json!({ "op": "back" })),
    ]
}

/// The edges of the Align block, in the order of the menu.
pub const ALIGN_EDGES: [(&str, &str); 6] = [
    ("Align left", "left"),
    ("Align right", "right"),
    ("Align top", "top"),
    ("Align bottom", "bottom"),
    ("Align horizontal centres", "center_x"),
    ("Align vertical centres", "center_y"),
];

fn align_items() -> Vec<Item> {
    ALIGN_EDGES.iter().map(|&(text, edge)| item(text, "arrange.align", json!({ "edge": edge }))).collect()
}

fn distribute_items() -> Vec<Item> {
    vec![
        item("Distribute horizontally", "arrange.distribute", json!({ "axis": "horizontal" })),
        item("Distribute vertically", "arrange.distribute", json!({ "axis": "vertical" })),
    ]
}

fn style_items() -> Vec<Item> {
    vec![item("Copy style", "clipboard.copy_style", json!({})), item("Paste style", "clipboard.paste_style", json!({}))]
}

/// The blocks of rows the user opened (a block is a group of rows behind a row of its own), kept while the
/// menu is open.
#[derive(Clone, Default)]
struct OpenBlocks(BTreeSet<&'static str>);

fn blocks_id() -> Id {
    Id::unique("context_menu.blocks")
}

/// The kinds that are shapes (`element.convert` turns one into another).
const SHAPES: [&str; 3] = ["rectangle", "diamond", "ellipse"];
/// The kinds that are lines or arrows.
const LINEAR: [&str; 2] = ["line", "arrow"];

/// What the selection allows, worked out from the model once per frame.
struct Facts<'a> {
    sel: &'a [SelectedElement],
}

impl<'a> Facts<'a> {
    fn all_of(&self, kinds: &[&str]) -> bool {
        !self.sel.is_empty() && self.sel.iter().all(|e| kinds.contains(&e.kind.as_str()))
    }

    /// The only selected element, when exactly one is selected.
    fn single(&self) -> Option<&'a SelectedElement> {
        match self.sel {
            [one] => Some(one),
            _ => None,
        }
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

/// The `element.convert` targets that apply to this selection, as (row text, arguments). Shapes become
/// shapes and lines and arrows become lines and arrows, as the command says.
pub(crate) fn convert_targets(sel: &[SelectedElement]) -> Vec<(&'static str, Value)> {
    let f = Facts { sel };
    if f.all_of(&SHAPES) {
        vec![
            ("Convert to rectangle", json!({ "to": "rectangle" })),
            ("Convert to diamond", json!({ "to": "diamond" })),
            ("Convert to ellipse", json!({ "to": "ellipse" })),
        ]
    } else if f.all_of(&LINEAR) {
        vec![
            ("Convert to line", json!({ "to": "line" })),
            ("Convert to sharp arrow", json!({ "to": "arrow", "arrow_type": "sharp" })),
            ("Convert to round arrow", json!({ "to": "arrow", "arrow_type": "round" })),
            ("Convert to elbow arrow", json!({ "to": "arrow", "arrow_type": "elbow" })),
        ]
    } else {
        Vec::new()
    }
}

/// Draws the menu when `st.context_menu` is set.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let Some(menu) = st.context_menu.clone() else {
        ctx.data_mut(|d| d.remove::<OpenBlocks>(blocks_id()));
        return;
    };
    let screen = ctx.content_rect();
    // A click anywhere else closes the menu. The backdrop lies under the menu and over everything else.
    let backdrop = Area::new(Id::unique("context_menu.backdrop"))
        .order(Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| ui.allocate_rect(screen, Sense::click()));
    let outside = backdrop.inner.clicked() || backdrop.inner.secondary_clicked();

    let mut chosen = false;
    let pos = pos2(
        menu.screen.x.clamp(screen.left() + SCREEN_GAP, (screen.right() - WIDTH - SCREEN_GAP).max(screen.left())),
        menu.screen.y.max(screen.top() + SCREEN_GAP),
    );
    let max_height = (screen.height() - 2.0 * SCREEN_GAP).max(120.0);
    let compact = cx.layout.compact;
    Area::new(Id::unique("context_menu")).order(Order::Foreground).fixed_pos(pos).constrain(true).show(ctx, |ui| {
        panel_frame(&cx.pal).show(ui, |ui| {
            ui.set_width(WIDTH);
            ui.spacing_mut().item_spacing.y = 1.0;
            let mut draw_rows = |ui: &mut Ui| {
                chosen = cx.with_origin(Origin::ContextMenu, |cx| match &menu.locked {
                    Some(id) => locked_rows(ui, cx, id, menu.world),
                    None => rows(ui, cx, st, menu.world),
                });
            };
            // A short touch screen scrolls the menu; elsewhere it is as tall as its rows.
            if compact {
                egui::ScrollArea::vertical()
                    .max_height(max_height - 24.0)
                    .auto_shrink([true, true])
                    .show(ui, draw_rows);
            } else {
                draw_rows(ui);
            }
        });
    });
    if chosen {
        st.context_menu = None;
    } else if outside {
        cx.verb(Verb::CloseOverlays);
    }
}

/// The rows for a locked element, which cannot be selected: Unlock (which selects what it unlocks, as
/// Excalidraw does), then the rows that do not need a selection.
fn locked_rows(ui: &mut Ui, cx: &mut Cx<'_>, element: &str, world: Option<[f64; 2]>) -> bool {
    let mut chosen = false;
    let unlock = Ctl::new("element.unlock_all").args(unlock_args(element));
    chosen |= cx.menu_item(ui, &unlock, "Unlock").clicked();
    separator(ui, &cx.pal);
    let rest = [
        item("Paste", "clipboard.paste", paste_args(world)),
        item("Select all", "selection.all", json!({})),
        item("Zoom to fit", "view.zoom_to_fit", json!({ "scope": "all" })),
    ];
    for it in rest {
        chosen |= cx.menu_item(ui, &Ctl::new(it.id).args(it.args), it.text).clicked();
    }
    chosen
}

/// The arguments of the Unlock row: that one element (its whole group unlocks with it).
pub(crate) fn unlock_args(element: &str) -> Value {
    json!({ "ids": [element] })
}

/// Draws rows and remembers whether one was chosen.
struct Rows {
    chosen: bool,
}

impl Rows {
    fn block(&mut self, ui: &mut Ui, cx: &mut Cx<'_>, items: Vec<Item>, indent: bool) {
        for it in items {
            let ctl = Ctl::new(it.id).args(it.args);
            ui.horizontal(|ui| {
                if indent {
                    ui.add_space(12.0);
                }
                self.chosen |= cx.menu_item(ui, &ctl, it.text).clicked();
            });
        }
    }

    fn row(&mut self, ui: &mut Ui, cx: &mut Cx<'_>, ctl: Ctl<'_>, text: &str, indent: bool) {
        ui.horizontal(|ui| {
            if indent {
                ui.add_space(12.0);
            }
            self.chosen |= cx.menu_item(ui, &ctl, text).clicked();
        });
    }

    /// A row that applies a UI verb and is greyed, with the engine's text, while `command` is refused.
    fn gated_verb(&mut self, ui: &mut Ui, cx: &mut Cx<'_>, verb: Verb, text: &str, command: &str) {
        let reason = cx.refusal(command);
        self.chosen |= cx.verb_item_gated(ui, verb, text, reason).clicked();
    }

    fn verb(&mut self, ui: &mut Ui, cx: &mut Cx<'_>, verb: Verb, text: &str, indent: bool) {
        ui.horizontal(|ui| {
            if indent {
                ui.add_space(12.0);
            }
            self.chosen |= cx.verb_item(ui, verb, text).clicked();
        });
    }

    /// A row that opens or closes a block of rows; returns whether the block is open.
    fn toggle(&mut self, ui: &mut Ui, cx: &mut Cx<'_>, key: &'static str, title: &str) -> bool {
        let open = ui.data_mut(|d| d.get_temp_mut_or_default::<OpenBlocks>(blocks_id()).0.contains(key));
        if cx.plain_row(ui, title).clicked() {
            ui.data_mut(|d| {
                let blocks = &mut d.get_temp_mut_or_default::<OpenBlocks>(blocks_id()).0;
                if !blocks.remove(key) {
                    blocks.insert(key);
                }
            });
        }
        open
    }
}

/// Draws the rows of a menu over a selection (or over the empty canvas) and returns true when one was chosen.
fn rows(ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState, world: Option<[f64; 2]>) -> bool {
    let sel = cx.model.selection.clone();
    let facts = Facts { sel: &sel };
    let mut r = Rows { chosen: false };
    r.block(ui, cx, edit_items(world), false);
    separator(ui, &cx.pal);
    r.block(ui, cx, group_items(), false);
    separator(ui, &cx.pal);
    r.block(ui, cx, order_items(), false);
    separator(ui, &cx.pal);
    r.block(ui, cx, vec![item("Lock or unlock", "element.lock", json!({}))], false);
    // These rows need a selection (of one, of two, of three). They stay in the menu and grey out with the
    // engine's text when the command is refused; a count the engine only checks when it runs is refused then.
    r.gated_verb(ui, cx, Verb::OpenLinkEditor, "Link…", "element.set_link");
    r.block(ui, cx, vec![item("Open link", "element.open_link", json!({}))], false);
    separator(ui, &cx.pal);
    element_blocks(ui, cx, st, &mut r, &facts);
    separator(ui, &cx.pal);
    if r.toggle(ui, cx, "align", "Align and distribute…") {
        r.block(ui, cx, align_items(), true);
        r.block(ui, cx, distribute_items(), true);
    }
    separator(ui, &cx.pal);
    r.block(ui, cx, vec![item("Select all", "selection.all", json!({}))], false);
    r.block(ui, cx, style_items(), false);
    r.block(ui, cx, vec![item("Zoom to fit", "view.zoom_to_fit", json!({ "scope": "all" }))], false);
    separator(ui, &cx.pal);
    view_block(ui, cx, &mut r);
    r.chosen
}

/// The blocks that act on the selected elements: copy as, flip and convert, frame, text.
fn element_blocks(ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState, r: &mut Rows, f: &Facts<'_>) {
    if r.toggle(ui, cx, "copy_as", "Copy as…") {
        let mut rows = vec![("Copy as PNG", json!({ "format": "png" })), ("Copy as SVG", json!({ "format": "svg" }))];
        if f.sel.iter().any(|e| e.kind == "text") {
            rows.push(("Copy as text", json!({ "format": "text" })));
        }
        for (text, args) in rows {
            r.row(ui, cx, Ctl::new("clipboard.copy_as").args(args), text, true);
        }
    }
    if r.toggle(ui, cx, "transform", "Flip and convert…") {
        for (text, axis) in [("Flip horizontal", "horizontal"), ("Flip vertical", "vertical")] {
            r.row(ui, cx, Ctl::new("element.flip").args(json!({ "axis": axis })), text, true);
        }
        for (text, args) in convert_targets(f.sel) {
            r.row(ui, cx, Ctl::new("element.convert").args(args), text, true);
        }
        if f.single().is_some_and(|e| LINEAR.contains(&e.kind.as_str())) {
            r.row(ui, cx, Ctl::new("arrow.edit_points"), "Edit line points", true);
        }
        r.row(ui, cx, Ctl::new("library.add"), "Add to library", true);
    }
    if r.toggle(ui, cx, "frame", "Frame…") {
        r.row(ui, cx, Ctl::new("frame.wrap"), "Wrap in frame", true);
        if let Some(frame) = f.single().filter(|e| e.kind == "frame") {
            r.row(ui, cx, Ctl::new("frame.unwrap").args(json!({ "frame": frame.id })), "Unwrap frame", true);
            r.row(
                ui,
                cx,
                Ctl::new("frame.select_children").args(json!({ "frame": frame.id })),
                "Select all in frame",
                true,
            );
            if cx.plain_row(ui, "Rename frame…").clicked() {
                st.open_rename(RenameTarget::Frame(frame.id.clone()), "");
            }
        }
        let inside = in_frame_ids(f.sel);
        if !inside.is_empty() {
            r.row(ui, cx, Ctl::new("frame.remove").args(json!({ "ids": inside })), "Remove from frame", true);
        }
    }
    let has_text = f.sel.iter().any(|e| e.kind == "text");
    let bind = bind_args(f.sel);
    let unbind = unbind_args(f.sel);
    if (has_text || bind.is_some() || unbind.is_some()) && r.toggle(ui, cx, "text", "Text…") {
        if has_text {
            r.row(ui, cx, Ctl::new("text.wrap_in_container"), "Wrap text in a shape", true);
        }
        if let Some(args) = bind {
            r.row(ui, cx, Ctl::new("text.bind").args(args), "Bind text to shape", true);
        }
        if let Some(args) = unbind {
            r.row(ui, cx, Ctl::new("text.unbind").args(args), "Unbind text", true);
        }
    }
}

/// The view block: grid, snapping, zen, view-only, stats and unlock all.
fn view_block(ui: &mut Ui, cx: &mut Cx<'_>, r: &mut Rows) {
    if !r.toggle(ui, cx, "view", "View and snapping…") {
        return;
    }
    let grid = Ctl::new("settings.cycle").args(json!({ "key": "view.grid_style" })).label("Change the grid");
    r.row(ui, cx, grid, "Change the grid", true);
    let snap = Ctl::new("settings.toggle")
        .args(json!({ "key": "snap.grid" }))
        .label("Snap to grid")
        .selected(cx.model.setting_bool("snap.grid"));
    r.row(ui, cx, snap, "Snap to grid", true);
    r.verb(ui, cx, Verb::ToggleZen, "Zen mode", true);
    let view_only = Ctl::new("view.set_view_only").selected(cx.model.view_only);
    r.row(ui, cx, view_only, "View-only mode", true);
    r.verb(ui, cx, Verb::ToggleStats, "Stats panel", true);
    r.row(ui, cx, Ctl::new("element.unlock_all"), "Unlock all", true);
}
