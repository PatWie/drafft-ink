//! The canvas context menu. The canvas layer opens it with `UiState::open_context_menu` on a right click.
//! Every row is a command (`Ctl`) drawn with `menu_item`, so a command that cannot run now is greyed with
//! its refusal text as the tooltip, and a command the registry does not have is greyed with the
//! model's "not available in this build" text. `Paste` puts its content at the point where the menu was
//! opened. Choosing a row closes the menu; a click anywhere else
//! closes it too.

use drafftink_engine::Origin;
use egui::{Align2, Area, Context, Id, Order, Response, Role, Sense, Ui, WidgetInfo, pos2, vec2};
use serde_json::{Value, json};

use crate::action::Verb;
use crate::state::UiState;
use crate::theme::{BUTTON_RADIUS, MARGIN, Palette};
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

/// Draws the menu when `st.context_menu` is set.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let align_id = Id::unique("context_menu.align_open");
    let Some(menu) = st.context_menu else {
        ctx.data_mut(|d| d.remove::<bool>(align_id));
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
    Area::new(Id::unique("context_menu")).order(Order::Foreground).fixed_pos(pos).constrain(true).show(ctx, |ui| {
        panel_frame(&cx.pal).show(ui, |ui| {
            ui.set_width(WIDTH);
            ui.spacing_mut().item_spacing.y = 1.0;
            chosen = cx.with_origin(Origin::ContextMenu, |cx| rows(ui, cx, align_id, menu.world));
        });
    });
    if chosen {
        st.context_menu = None;
    } else if outside {
        cx.verb(Verb::CloseOverlays);
    }
}

/// Draws the rows and returns true when one was chosen.
fn rows(ui: &mut Ui, cx: &mut Cx<'_>, align_id: Id, world: Option<[f64; 2]>) -> bool {
    let selected = cx.model.selection_count;
    let mut chosen = false;
    let mut block = |ui: &mut Ui, cx: &mut Cx<'_>, items: Vec<Item>, indent: bool| {
        for it in items {
            let ctl = Ctl::new(it.id).args(it.args);
            ui.horizontal(|ui| {
                if indent {
                    ui.add_space(12.0);
                }
                chosen |= cx.menu_item(ui, &ctl, it.text).clicked();
            });
        }
    };
    block(ui, cx, edit_items(world), false);
    separator(ui, &cx.pal);
    block(ui, cx, group_items(), false);
    separator(ui, &cx.pal);
    block(ui, cx, order_items(), false);
    separator(ui, &cx.pal);
    block(ui, cx, vec![item("Lock or unlock", "element.lock", json!({}))], false);
    if selected >= 2 {
        separator(ui, &cx.pal);
        let open = ui.data_mut(|d| *d.get_temp_mut_or_default::<bool>(align_id));
        if toggle_row(ui, &cx.pal, "Align and distribute…").clicked() {
            ui.data_mut(|d| d.insert_temp(align_id, !open));
        }
        if open {
            block(ui, cx, align_items(), true);
            if selected >= 3 {
                block(ui, cx, distribute_items(), true);
            }
        }
    }
    separator(ui, &cx.pal);
    block(ui, cx, vec![item("Select all", "selection.all", json!({}))], false);
    block(ui, cx, style_items(), false);
    block(ui, cx, vec![item("Zoom to fit", "view.zoom_to_fit", json!({ "scope": "all" }))], false);
    chosen
}

/// A row that opens or closes a block of rows. It only changes what the menu shows, so it is not a
/// command and is never greyed.
fn toggle_row(ui: &mut Ui, pal: &Palette, text: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width().max(160.0), 28.0), Sense::click());
    if ui.is_rect_visible(rect) {
        if resp.hovered() || resp.has_focus() {
            ui.painter().rect_filled(rect, BUTTON_RADIUS, pal.hover);
        }
        ui.painter().text(
            pos2(rect.left() + 10.0, rect.center().y),
            Align2::LEFT_CENTER,
            text,
            egui::FontId::proportional(13.0),
            pal.text,
        );
    }
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, text));
    resp
}
