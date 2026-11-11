//! The bar at the top: stroke colour, fill colour, stroke width. Every control is `style.set`; with
//! nothing selected the engine writes the defaults for new elements, so the bar is always useful.

use egui::{Align2, Area, Context, Id, Order, vec2};
use serde_json::{Value, json};

use crate::action::Verb;
use crate::colors::{self, QUICK_FAMILIES, QUICK_FILL_SHADE, QUICK_STROKE_SHADE};
use crate::icons::Icon;
use crate::state::UiState;
use crate::surfaces::color::{PopoverKey, remember_anchor};
use crate::theme::MARGIN;
use crate::widgets::{Ctl, Cx, panel_frame, section_label, vertical_separator};

/// The stroke widths of the old app: name and value.
pub const STROKE_WIDTHS: [(&str, f64); 4] = [("Thin", 1.0), ("Normal", 2.0), ("Bold", 4.0), ("Extra bold", 8.0)];

/// Why a property control is greyed, when the selection has no element it applies to.
pub fn not_applicable(cx: &Cx<'_>, prop: &str) -> Option<String> {
    if cx.model.style.props.is_empty() || cx.model.style.applies(prop) {
        None
    } else {
        Some(format!("`{prop}` does not apply to the selected elements."))
    }
}

/// Draws the bar.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, _st: &mut UiState) {
    Area::new(Id::unique("properties_bar")).anchor(Align2::CENTER_TOP, vec2(0.0, MARGIN)).order(Order::Middle).show(
        ctx,
        |ui| {
            panel_frame(&cx.pal).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = vec2(3.0, 0.0);
                    colour_row(ui, cx, "Stroke", "stroke_color", QUICK_STROKE_SHADE, false);
                    ui.add_space(6.0);
                    vertical_separator(ui, &cx.pal, 24.0);
                    ui.add_space(6.0);
                    colour_row(ui, cx, "Fill", "fill_color", QUICK_FILL_SHADE, true);
                    ui.add_space(6.0);
                    vertical_separator(ui, &cx.pal, 24.0);
                    ui.add_space(6.0);
                    width_row(ui, cx);
                });
            });
        },
    );
}

fn colour_row(ui: &mut egui::Ui, cx: &mut Cx<'_>, title: &str, prop: &str, shade: u16, allow_none: bool) {
    section_label(ui, &cx.pal, title);
    ui.add_space(4.0);
    let current = cx.model.style.prop(prop).and_then(|p| p.value.clone());
    let blocked = not_applicable(cx, prop);
    let is_none = allow_none && matches!(current, Some(Value::Null));
    if allow_none {
        let ctl = Ctl::new("style.set")
            .args(json!({ "props": { prop: Value::Null } }))
            .label("No fill")
            .selected(is_none)
            .blocked(blocked.clone());
        cx.swatch(ui, &ctl, None, 18.0);
    }
    for family in QUICK_FAMILIES {
        let name = colors::quick_name(family, shade);
        let selected = current.as_ref().and_then(Value::as_str).is_some_and(|v| colors::same(v, &name));
        let ctl = Ctl::new("style.set")
            .args(json!({ "props": { prop: name } }))
            .label(format!("{title} {family}"))
            .selected(selected)
            .blocked(blocked.clone());
        cx.swatch(ui, &ctl, colors::parse(&name), 18.0);
    }
    let verb = if prop == "stroke_color" { Verb::OpenColorStroke } else { Verb::OpenColorFill };
    let wheel_tip = format!("More {} colours", title.to_lowercase());
    let key = if prop == "stroke_color" { PopoverKey::Stroke } else { PopoverKey::Fill };
    let wheel = cx.verb_button(ui, verb, Icon::Wheel, 26.0, &wheel_tip, false);
    remember_anchor(ui.ctx(), key, wheel.rect);
}

fn width_row(ui: &mut egui::Ui, cx: &mut Cx<'_>) {
    section_label(ui, &cx.pal, "Width");
    let current = cx.model.style.num_value("stroke_width");
    let blocked = not_applicable(cx, "stroke_width");
    for (i, (name, w)) in STROKE_WIDTHS.iter().enumerate() {
        let selected = current.is_some_and(|c| (c - w).abs() < 0.01);
        let ctl = Ctl::new("style.set")
            .args(json!({ "props": { "stroke_width": w } }))
            .label(*name)
            .selected(selected)
            .blocked(blocked.clone());
        let thickness = [1.0_f32, 2.5, 4.5, 7.0][i];
        let pal = cx.pal;
        cx.command_widget(ui, &ctl, vec2(30.0, 24.0), move |p, rect, look| {
            crate::widgets::paint_button_bg(p, rect, look, &pal, false);
            let c = crate::widgets::icon_color(look, &pal);
            let y = rect.center().y;
            p.line_segment(
                [egui::pos2(rect.left() + 7.0, y), egui::pos2(rect.right() - 7.0, y)],
                egui::Stroke::new(thickness, c),
            );
        });
    }
}
