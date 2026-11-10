//! The hamburger menu and the collaboration button at the top left. Menu rows are commands (or UI
//! verbs that end in a command); they carry the origin `menu`.

use drafftink_engine::Origin;
use egui::{Align2, Area, Context, Id, Order, Rect, vec2};
use serde_json::json;

use crate::action::{HostRequest, UiAction, Verb};
use crate::icons::{self, Icon};
use crate::model::CollabStatus;
use crate::state::UiState;
use crate::theme::{MARGIN, SMALL_BUTTON};
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
                    let r = cx.verb_button(ui, Verb::ToggleMenu, Icon::Menu, SMALL_BUTTON + 4.0, "Menu", st.menu_open);
                    button_rect = r.rect;
                    collab_button(ui, cx, st);
                });
            });
        },
    );
    if !st.menu_open {
        return;
    }
    let area = Area::new(Id::unique("main_menu"))
        .anchor(Align2::LEFT_TOP, vec2(MARGIN, MARGIN + 52.0))
        .order(Order::Foreground)
        .show(ctx, |ui| {
            panel_frame(&cx.pal).show(ui, |ui| {
                ui.set_width(MENU_WIDTH);
                ui.spacing_mut().item_spacing = vec2(0.0, 1.0);
                cx.with_origin(Origin::Menu, |cx| menu_rows(ui, cx));
            });
        });
    // A click anywhere outside the menu and its button closes it. The rectangle is the real one.
    let outside = ctx.input(|i| {
        i.pointer.any_pressed()
            && i.pointer.interact_pos().is_some_and(|p| !area.response.rect.contains(p) && !button_rect.contains(p))
    });
    if outside {
        st.menu_open = false;
    }
}

/// Collects whether any row was chosen, so the menu closes after one.
struct Rows {
    chosen: bool,
}

impl Rows {
    fn command(&mut self, ui: &mut egui::Ui, cx: &mut Cx<'_>, ctl: Ctl<'_>, text: &str) {
        if cx.menu_item(ui, &ctl, text).clicked() {
            self.chosen = true;
        }
    }

    fn verb(&mut self, ui: &mut egui::Ui, cx: &mut Cx<'_>, verb: Verb, text: &str) {
        if cx.verb_item(ui, verb, text).clicked() {
            self.chosen = true;
        }
    }

    fn host(&mut self, ui: &mut egui::Ui, cx: &mut Cx<'_>, request: HostRequest, text: &str) {
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width().max(160.0), 28.0), egui::Sense::click());
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
            self.chosen = true;
        }
    }
}

fn menu_rows(ui: &mut egui::Ui, cx: &mut Cx<'_>) {
    let mut rows = Rows { chosen: false };
    rows.command(ui, cx, Ctl::new("tab.new"), "New tab");
    rows.verb(ui, cx, Verb::OpenFile, "Open…");
    rows.verb(ui, cx, Verb::OpenRecent, "Open recent…");
    rows.command(ui, cx, Ctl::new("doc.save"), "Save");
    rows.verb(ui, cx, Verb::SaveAs, "Save as…");
    separator(ui, &cx.pal);
    rows.command(
        ui,
        cx,
        Ctl::new("doc.export").args(json!({ "format": "png" })).label("Export as PNG"),
        "Export as PNG",
    );
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
    rows.verb(ui, cx, Verb::OpenPalette, "Command palette…");
    rows.verb(ui, cx, Verb::OpenShortcuts, "Keyboard shortcuts");
    if rows.chosen {
        cx.verb(Verb::CloseOverlays);
    }
}

fn export_scale_row(ui: &mut egui::Ui, cx: &mut Cx<'_>) {
    let current = cx.model.setting_f64("export.scale").unwrap_or(2.0);
    ui.horizontal(|ui| {
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
    let side = SMALL_BUTTON + 4.0;
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
