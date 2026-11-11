//! The hamburger menu and the collaboration button at the top left. Menu rows are commands (or UI
//! verbs that end in a command); they carry the origin `menu`.

use drafftink_engine::Origin;
use egui::{Align2, Area, Context, Id, Order, Rect, vec2};
use serde_json::json;

use crate::action::{HostRequest, UiAction, Verb};
use crate::icons::{self, Icon};
use crate::model::CollabStatus;
use crate::state::UiState;
use crate::theme::MARGIN;
use crate::widgets::{Ctl, Cx, panel_frame, separator};

const MENU_WIDTH: f32 = 220.0;

/// Draws the two buttons and, when open, the menu.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let mut button_rect = Rect::NOTHING;
    Area::new(Id::unique("menu_buttons")).anchor(Align2::LEFT_TOP, vec2(MARGIN, MARGIN)).order(Order::Middle).show(
        ctx,
        |ui| {
            panel_frame(&cx.pal).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = vec2(3.0, 0.0);
                    let side = cx.layout.small_side() + 4.0;
                    let r = cx.verb_button(ui, Verb::ToggleMenu, Icon::Menu, side, "Menu", st.menu_open);
                    button_rect = r.rect;
                    collab_button(ui, cx, st);
                    title_button(ui, cx);
                });
            });
        },
    );
    if !st.menu_open {
        return;
    }
    let mut close_after = false;
    let area = Area::new(Id::unique("main_menu"))
        .anchor(Align2::LEFT_TOP, vec2(MARGIN, MARGIN + 52.0))
        .order(Order::Foreground)
        .show(ctx, |ui| {
            panel_frame(&cx.pal).show(ui, |ui| {
                ui.set_width(MENU_WIDTH);
                ui.spacing_mut().item_spacing = vec2(0.0, 1.0);
                close_after = cx.with_origin(Origin::Menu, |cx| menu_rows(ui, cx));
            });
        });
    if close_after {
        st.menu_open = false;
    }
    // A click anywhere outside the menu and its button closes it. The rectangle is the real one.
    let outside = ctx.input(|i| {
        i.pointer.any_pressed()
            && i.pointer.interact_pos().is_some_and(|p| !area.response.rect.contains(p) && !button_rect.contains(p))
    });
    if outside {
        st.menu_open = false;
    }
}

/// Collects which kind of row was chosen, so the menu closes after one.
struct Rows {
    /// A command ran: the menu closes with `ui.close_overlays`, which comes after the command.
    command: bool,
    /// A verb or a request for the app: the verb opens what it opens (and closes the menu itself), so the menu is
    /// closed without a second verb that would close the dialog the first one opened.
    local: bool,
}

impl Rows {
    fn command(&mut self, ui: &mut egui::Ui, cx: &mut Cx<'_>, ctl: Ctl<'_>, text: &str) {
        if cx.menu_item(ui, &ctl, text).clicked() {
            self.command = true;
        }
    }

    fn verb(&mut self, ui: &mut egui::Ui, cx: &mut Cx<'_>, verb: Verb, text: &str) {
        if cx.verb_item(ui, verb, text).clicked() {
            self.local = true;
        }
    }

    fn host(&mut self, ui: &mut egui::Ui, cx: &mut Cx<'_>, request: HostRequest, text: &str) {
        let (rect, resp) =
            ui.allocate_exact_size(vec2(ui.available_width().max(160.0), cx.layout.row_height()), egui::Sense::click());
        if ui.is_rect_visible(rect) {
            if resp.hovered() {
                ui.painter().rect_filled(rect, crate::theme::BUTTON_RADIUS, cx.pal.hover);
            }
            ui.painter().text(
                egui::pos2(rect.left() + 10.0, rect.center().y),
                Align2::LEFT_CENTER,
                text,
                egui::FontId::proportional(13.0),
                cx.pal.text,
            );
        }
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, text));
        if resp.clicked() {
            cx.emit(UiAction::Host(request));
            self.local = true;
        }
    }
}

/// Draws the rows. Returns true when a verb or app request was chosen and the menu must be closed by the caller.
fn menu_rows(ui: &mut egui::Ui, cx: &mut Cx<'_>) -> bool {
    let mut rows = Rows { command: false, local: false };
    rows.command(ui, cx, Ctl::new("tab.new"), "New tab");
    rows.verb(ui, cx, Verb::OpenFile, "Open…");
    rows.verb(ui, cx, Verb::OpenRecent, "Open recent…");
    rows.command(ui, cx, Ctl::new("doc.save"), "Save");
    rows.verb(ui, cx, Verb::SaveAs, "Save as…");
    rows.verb(ui, cx, Verb::RenameDocument, "Rename document…");
    separator(ui, &cx.pal);
    rows.command(
        ui,
        cx,
        Ctl::new("doc.export").args(json!({ "format": "png" })).label("Export as PNG"),
        "Export as PNG",
    );
    rows.verb(ui, cx, Verb::OpenExport, "Export…");
    rows.command(
        ui,
        cx,
        Ctl::new("clipboard.copy_as").args(json!({ "format": "png" })).label("Copy selection as PNG"),
        "Copy selection as PNG",
    );
    export_scale_row(ui, cx);
    separator(ui, &cx.pal);
    rows.verb(ui, cx, Verb::InsertImage, "Insert image…");
    rows.host(ui, cx, HostRequest::PickLibrary, "Open a library as a tab…");
    rows.command(
        ui,
        cx,
        Ctl::new("diagram.import_mermaid")
            .args(json!({ "source": "clipboard" }))
            .label("Import Mermaid from clipboard"),
        "Import Mermaid from clipboard",
    );
    separator(ui, &cx.pal);
    rows.command(ui, cx, Ctl::new("doc.clear"), "Clear the canvas");
    rows.command(ui, cx, Ctl::new("doc.show_intro"), "Show the intro");
    let view_only = cx.model.view_only;
    rows.command(ui, cx, Ctl::new("view.set_view_only").selected(view_only), "View-only mode");
    separator(ui, &cx.pal);
    theme_row(ui, cx);
    rows.verb(ui, cx, Verb::ToggleLibrary, "Library");
    rows.verb(ui, cx, Verb::OpenPreferences, "Preferences…");
    rows.verb(ui, cx, Verb::OpenPalette, "Command palette…");
    rows.verb(ui, cx, Verb::OpenShortcuts, "Keyboard shortcuts");
    if rows.command {
        cx.verb(Verb::CloseOverlays);
    }
    rows.local
}

fn export_scale_row(ui: &mut egui::Ui, cx: &mut Cx<'_>) {
    let current = cx.model.setting_f64("export.scale").unwrap_or(2.0);
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.label(egui::RichText::new("Export scale").size(11.0).color(cx.pal.muted));
        for scale in [1.0, 2.0, 3.0] {
            let ctl = Ctl::new("settings.set")
                .args(json!({ "key": "export.scale", "value": scale }))
                .label(format!("Export scale {scale}x"))
                .selected((current - scale).abs() < 0.01);
            cx.toggle_button(ui, &ctl, &format!("{scale}x"));
        }
    });
}

fn theme_row(ui: &mut egui::Ui, cx: &mut Cx<'_>) {
    let current = cx.model.setting_str("theme").unwrap_or("system").to_owned();
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.label(egui::RichText::new("Theme").size(11.0).color(cx.pal.muted));
        for (label, value) in [("Light", "light"), ("Dark", "dark"), ("System", "system")] {
            let ctl = Ctl::new("settings.set")
                .args(json!({ "key": "theme", "value": value }))
                .label(format!("{label} theme"))
                .selected(current == value);
            cx.toggle_button(ui, &ctl, label);
        }
    });
}

fn collab_button(ui: &mut egui::Ui, cx: &mut Cx<'_>, st: &UiState) {
    let pal = cx.pal;
    let side = cx.layout.small_side() + 4.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(side, side), egui::Sense::click());
    let (dot, tip) = match &cx.model.collab.status {
        CollabStatus::Connected => (pal.ok, "Connected. Manage collaboration".to_owned()),
        CollabStatus::Connecting => (pal.warn, "Connecting…".to_owned()),
        CollabStatus::Failed(why) => (pal.danger, format!("Connection failed: {why}")),
        CollabStatus::Disconnected => (pal.muted, "Not connected. Collaborate".to_owned()),
    };
    if ui.is_rect_visible(rect) {
        let look =
            crate::widgets::Look { hovered: resp.hovered(), pressed: false, enabled: true, selected: st.collab_open };
        crate::widgets::paint_button_bg(ui.painter(), rect, look, &pal, true);
        icons::paint(ui.painter(), rect.shrink(side * 0.2), Icon::Collab, crate::widgets::icon_color(look, &pal));
        ui.painter().circle_filled(rect.right_top() + vec2(-6.0, 6.0), 4.0, dot);
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Collaboration"));
    if resp.on_hover_text(tip).clicked() {
        cx.verb(Verb::OpenCollab);
    }
}

/// The longest title shown on the chip, in characters.
const TITLE_CHARS: usize = 24;

/// The name of the document, next to the menu. A click renames it (`doc.rename`).
fn title_button(ui: &mut egui::Ui, cx: &mut Cx<'_>) {
    let Some(active) = cx.model.tabs.iter().find(|t| t.active) else {
        return;
    };
    let pal = cx.pal;
    let full = if active.dirty { format!("{} •", active.name) } else { active.name.clone() };
    let shown: String = if full.chars().count() > TITLE_CHARS {
        full.chars().take(TITLE_CHARS - 1).chain(std::iter::once('~')).collect()
    } else {
        full
    };
    let galley = ui.painter().layout_no_wrap(shown, egui::FontId::proportional(13.0), pal.text);
    let height = cx.layout.small_side() + 4.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 16.0, height), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        if resp.hovered() || resp.has_focus() {
            ui.painter().rect_filled(rect, crate::theme::BUTTON_RADIUS, pal.hover);
        }
        ui.painter().galley(egui::pos2(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0), galley, pal.text);
    }
    let label = format!("Document name: {}", active.name);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label.as_str()));
    if resp.on_hover_text("Rename the document").clicked() {
        cx.verb(Verb::RenameDocument);
    }
}
