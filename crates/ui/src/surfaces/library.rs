//! The library panel (`ui.toggle_library`): the shapes the user keeps. The model reads `library.list` while
//! the panel is open. A click on an item inserts it in the middle of the view (`library.insert`); a drag
//! onto the canvas inserts it where the pointer is released. Add stores the selection (`library.add`),
//! Export writes an `.excalidrawlib` download (`library.export`), Import asks the app for a file
//! ([`HostRequest::PickLibraryImport`], which ends in `library.import`).

use egui::{Align2, Area, Context, Id, Order, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::{Value, json};

use crate::action::{HostRequest, UiAction};
use crate::icons::Icon;
use crate::model::{LibraryItem, ViewView};
use crate::state::{PanelKind, RenameTarget, UiState};
use crate::theme::{self, MARGIN};
use crate::widgets::{Ctl, Cx, panel_frame, section_label};

/// The file `library.export` writes.
pub const EXPORT_FILE: &str = "drafftink.excalidrawlib";
/// Width of the panel.
const WIDTH: f32 = 220.0;
/// Side of an item's preview box.
const PREVIEW: f32 = 40.0;

/// `library.insert` for a click: the engine puts the item in the middle of the view.
pub(crate) fn insert_args(item: &str) -> Value {
    json!({ "item": item })
}

/// `library.insert` for a drop at a screen point.
pub(crate) fn drop_args(item: &str, view: &ViewView, screen: egui::Pos2) -> Value {
    json!({ "item": item, "at": view.screen_to_world(screen) })
}

/// `library.add`: the selection under the typed name (no name: the engine names it).
pub(crate) fn add_args(name: &str) -> Value {
    let name = name.trim();
    if name.is_empty() { json!({}) } else { json!({ "name": name }) }
}

/// `library.export` of the whole library as a download.
pub(crate) fn export_args() -> Value {
    json!({ "to": { "download": EXPORT_FILE } })
}

/// Draws the panel when it is open.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    if !st.library_open {
        return;
    }
    let screen = ctx.content_rect();
    let side = cx.layout.tool_side();
    // To the right of the tool column; in the touch layout with collapsed tools, at the edge.
    let x = if st.panel_collapsed(PanelKind::Tools, screen.width()) { MARGIN } else { MARGIN + side + 24.0 };
    let max_height = (screen.height() - 160.0).max(160.0);
    let mut panel = Rect::NOTHING;
    Area::new(Id::unique("library_panel")).anchor(Align2::LEFT_CENTER, vec2(x, 0.0)).order(Order::Middle).show(
        ctx,
        |ui| {
            let out = panel_frame(&cx.pal).show(ui, |ui| {
                ui.set_width(WIDTH);
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                header(ui, cx);
                actions(ui, cx, st);
                items(ui, cx, st, max_height);
            });
            panel = out.response.rect;
        },
    );
    drop_target(ctx, cx, panel);
}

fn header(ui: &mut Ui, cx: &mut Cx<'_>) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Library").font(theme::hand(16.0)).color(cx.pal.text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            cx.verb_button(
                ui,
                crate::action::Verb::ToggleLibrary,
                Icon::Close,
                cx.layout.small_side() - 6.0,
                "Close the library",
                false,
            );
        });
    });
}

fn actions(ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState) {
    let field = crate::surfaces::dialogs::text_field(
        ui,
        &mut st.library_name,
        "Name (optional)",
        None,
        Some(256),
        "Library item name",
    );
    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    ui.horizontal_wrapped(|ui| {
        let add = Ctl::new("library.add").args(add_args(&st.library_name)).label("Add selection to library");
        let clicked = cx.text_button(ui, &add, "Add", true).clicked();
        if enter && !clicked && cx.model.is_available("library.add") {
            cx.run("library.add", add_args(&st.library_name));
        }
        // The file goes to the downloads: there is nothing more to do here.
        cx.text_button(ui, &Ctl::new("library.export").args(export_args()).label("Export library"), "Export", false);
        if crate::surfaces::dialogs::plain_button(ui, &cx.pal, "Import…", false).clicked() {
            cx.emit(UiAction::Host(HostRequest::PickLibraryImport));
        }
    });
}

fn items(ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState, max_height: f32) {
    if let Some(why) = &cx.model.library_error {
        ui.label(egui::RichText::new(why.as_str()).size(11.0).color(cx.pal.muted));
        return;
    }
    if cx.model.library.is_empty() {
        ui.label(egui::RichText::new("Nothing here yet. Select shapes and press Add.").size(11.0).color(cx.pal.muted));
        return;
    }
    section_label(ui, &cx.pal, "Click to insert, or drag onto the canvas");
    let library = cx.model.library.clone();
    egui::ScrollArea::vertical().max_height(max_height).auto_shrink([true, true]).show(ui, |ui| {
        for item in &library {
            item_row(ui, cx, st, item);
        }
    });
}

fn item_row(ui: &mut Ui, cx: &mut Cx<'_>, st: &mut UiState, item: &LibraryItem) {
    let pal = cx.pal;
    ui.horizontal(|ui| {
        let (rect, resp) = ui.allocate_exact_size(
            vec2(WIDTH - 2.0 * (cx.layout.small_side() - 6.0) - 24.0, PREVIEW),
            Sense::click_and_drag(),
        );
        let enabled = cx.model.is_available("library.insert");
        if ui.is_rect_visible(rect) {
            if resp.hovered() && enabled {
                ui.painter().rect_filled(rect, theme::BUTTON_RADIUS, pal.hover);
            }
            paint_preview(ui, rect, item, &pal);
            ui.painter().text(
                pos2(rect.left() + PREVIEW + 8.0, rect.center().y - 7.0),
                Align2::LEFT_CENTER,
                &item.name,
                egui::FontId::proportional(12.0),
                pal.text,
            );
            ui.painter().text(
                pos2(rect.left() + PREVIEW + 8.0, rect.center().y + 8.0),
                Align2::LEFT_CENTER,
                format!("{} elements", item.element_count),
                egui::FontId::proportional(10.0),
                pal.muted,
            );
        }
        let name = format!("Insert {}", item.name);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, name.as_str()));
        let resp = match cx.refusal("library.insert") {
            Some(why) => resp.on_hover_text(why),
            None => resp.on_hover_text("Click to insert. Drag onto the canvas to place it."),
        };
        if enabled && resp.clicked() {
            cx.run("library.insert", insert_args(&item.id));
        }
        if resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        if enabled && resp.drag_stopped() {
            // The release point decides: over the panel it is a cancelled drag.
            if let Some(at) = ui.ctx().pointer_latest_pos().filter(|p| !panel_rect(ui).contains(*p)) {
                cx.run("library.insert", drop_args(&item.id, &cx.model.view, at));
            }
        }
        let rename = ui.add(
            egui::Button::new(egui::RichText::new("Aa").size(11.0))
                .min_size(vec2(cx.layout.small_side() - 6.0, cx.layout.small_side() - 6.0)),
        );
        rename.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Rename {}", item.name)));
        if rename.on_hover_text("Rename").clicked() {
            st.open_rename(RenameTarget::LibraryItem(item.id.clone()), item.name.clone());
        }
        let remove =
            Ctl::new("library.remove").args(json!({ "items": [item.id] })).label(format!("Remove {}", item.name));
        cx.icon_button(ui, &remove, Icon::Delete, cx.layout.small_side() - 6.0);
    });
}

/// The panel's rectangle, remembered by the last frame (a drop over the panel is not a drop on the canvas).
fn panel_rect(ui: &Ui) -> Rect {
    ui.ctx().data(|d| d.get_temp::<Rect>(Id::unique("library_panel_rect"))).unwrap_or(Rect::NOTHING)
}

fn drop_target(ctx: &Context, _cx: &mut Cx<'_>, panel: Rect) {
    ctx.data_mut(|d| d.insert_temp(Id::unique("library_panel_rect"), panel));
}

/// A box with the item's proportions, as a stand-in for a thumbnail (`library.list` has no drawing).
fn paint_preview(ui: &Ui, rect: Rect, item: &LibraryItem, pal: &theme::Palette) {
    let inner = Rect::from_min_size(rect.min + vec2(2.0, 2.0), vec2(PREVIEW - 4.0, PREVIEW - 4.0));
    let (w, h) = (item.width.max(1.0) as f32, item.height.max(1.0) as f32);
    let k = (inner.width() / w).min(inner.height() / h);
    let shape = Rect::from_center_size(inner.center(), vec2(w * k, h * k).max(vec2(4.0, 4.0)));
    ui.painter().rect_stroke(shape, 2.0, Stroke::new(1.5, pal.muted), StrokeKind::Inside);
}
