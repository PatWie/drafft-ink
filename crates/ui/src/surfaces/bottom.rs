//! The bar at the bottom left: undo and redo, grid, background, zoom, centre, fit, snapping. Each
//! control is a command; the toggles read their state from the settings in the model.

use egui::{Align2, Area, Context, Id, Order, vec2};
use serde_json::json;

use crate::action::Verb;
use crate::colors;
use crate::icons::Icon;
use crate::state::UiState;
use crate::theme::MARGIN;
use crate::widgets::{Ctl, Cx, ScrollDir, panel_frame, scroll_if, vertical_separator};

/// The icon for a grid style setting value.
pub fn grid_icon(style: &str) -> Icon {
    match style {
        "none" => Icon::GridNone,
        "horizontal_lines" => Icon::GridHorizontal,
        "crosses" => Icon::GridCrosses,
        "dots" => Icon::GridDots,
        _ => Icon::GridLines,
    }
}

/// Draws the bar.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    let side = cx.layout.small_side();
    // The panels around it leave this much of the window width to the bar; past that it scrolls sideways.
    let max_width = (ctx.content_rect().width() - 2.0 * MARGIN - 20.0).max(160.0);
    Area::new(Id::unique("bottom_bar")).anchor(Align2::LEFT_BOTTOM, vec2(MARGIN, -MARGIN)).order(Order::Middle).show(
        ctx,
        |ui| {
            panel_frame(&cx.pal).show(ui, |ui| {
                if cx.layout.compact {
                    ui.set_max_width(max_width);
                }
                scroll_if(ui, cx.layout.compact, ScrollDir::Horizontal, "bottom_scroll", 0.0, |ui| {
                    bar(ui, cx, st, side);
                });
            });
        },
    );
}

fn bar(ui: &mut egui::Ui, cx: &mut Cx<'_>, st: &mut UiState, side: f32) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = vec2(3.0, 0.0);
        cx.icon_button(ui, &Ctl::new("history.undo"), Icon::Undo, side);
        cx.icon_button(ui, &Ctl::new("history.redo"), Icon::Redo, side);
        vertical_separator(ui, &cx.pal, 20.0);
        let grid = cx.model.setting_str("view.grid_style").unwrap_or("lines").to_owned();
        let ctl = Ctl::new("settings.cycle").args(json!({ "key": "view.grid_style" }));
        cx.icon_button(ui, &ctl, grid_icon(&grid), side);
        background_button(ui, cx, st, side);
        vertical_separator(ui, &cx.pal, 20.0);
        zoom_group(ui, cx, side);
        vertical_separator(ui, &cx.pal, 20.0);
        for (icon, key, label) in [
            (Icon::SnapGrid, "snap.grid", "Snap to grid"),
            (Icon::SnapShapes, "snap.smart_guides", "Smart guides"),
            (Icon::SnapAngle, "snap.angle", "Angle snap"),
        ] {
            let on = cx.model.setting_bool(key);
            let ctl = Ctl::new("settings.toggle").args(json!({ "key": key })).label(label).selected(on);
            cx.icon_button(ui, &ctl, icon, side);
        }
    });
}

fn background_button(ui: &mut egui::Ui, cx: &mut Cx<'_>, st: &UiState, side: f32) {
    let pal = cx.pal;
    // The page as the canvas draws it in this theme.
    let colour = colors::displayed(&cx.model.background, pal.dark);
    let (rect, resp) = ui.allocate_exact_size(vec2(side, side), egui::Sense::click());
    let open = st.color_popover == Some(crate::state::ColorTarget::Background);
    if ui.is_rect_visible(rect) {
        crate::widgets::paint_swatch(ui.painter(), rect.center(), 8.0, colour, open, resp.hovered(), &pal, true);
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Background colour"));
    crate::surfaces::color::remember_anchor(ui.ctx(), crate::surfaces::color::PopoverKey::Background, rect);
    if resp.on_hover_text("Background colour").clicked() {
        cx.verb(Verb::OpenColorBackground);
    }
}

fn zoom_group(ui: &mut egui::Ui, cx: &mut Cx<'_>, side: f32) {
    cx.icon_button(ui, &Ctl::new("view.zoom_out"), Icon::Minus, side);
    let percent = format!("{}%", cx.model.zoom_percent.round());
    cx.toggle_button(ui, &Ctl::new("view.zoom_reset").label("Reset zoom to 100%"), &percent);
    cx.icon_button(ui, &Ctl::new("view.zoom_in"), Icon::Plus, side);
    let centre = Ctl::new("view.center").args(json!({ "on": "origin" })).label("Centre the canvas at the origin");
    cx.icon_button(ui, &centre, Icon::Center, side);
    let fit = Ctl::new("view.zoom_to_fit").args(json!({ "scope": "auto" })).label("Zoom to fit");
    cx.icon_button(ui, &fit, Icon::ZoomFit, side);
}
