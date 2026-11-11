//! The bar at the top: stroke colour, fill colour, stroke width. Every control is `style.set`; with
//! nothing selected the engine writes the defaults for new elements, so the bar is always useful.

use egui::{Align2, Area, Context, Id, Order, vec2};
use serde_json::{Value, json};

use crate::action::Verb;
use crate::colors::{self, QUICK_FAMILIES, QUICK_FILL_SHADE, QUICK_STROKE_SHADE};
use crate::icons::Icon;
use crate::state::{ColorTarget, LastPicked, UiState};
use crate::surfaces::color::{PopoverKey, remember_anchor};
use crate::theme::MARGIN;
use crate::widgets::{Ctl, Cx, ScrollDir, panel_frame, scroll_if, section_label, vertical_separator};

/// The stroke widths of the old app: name and value.
pub const STROKE_WIDTHS: [(&str, f64); 4] = [("Thin", 1.0), ("Normal", 2.0), ("Bold", 4.0), ("Extra bold", 8.0)];

/// Why a property control is greyed, when the selection has no element it applies to: the engine's text, as
/// `style.get` reports it (the refusal that `style.set` would give).
pub fn not_applicable(cx: &Cx<'_>, prop: &str) -> Option<String> {
    if cx.model.style.props.is_empty() || cx.model.style.applies(prop) {
        None
    } else {
        cx.model.style.reason(prop).map(str::to_owned)
    }
}

/// Draws the bar.
pub fn show(ctx: &Context, cx: &mut Cx<'_>, st: &mut UiState) {
    // On a narrow window the menu takes the top left, so the bar sits under it and scrolls sideways.
    let (anchor, offset) = if cx.layout.narrow {
        (Align2::LEFT_TOP, vec2(MARGIN, MARGIN + cx.layout.small_side() + 24.0))
    } else {
        (Align2::CENTER_TOP, vec2(0.0, MARGIN))
    };
    let max_width = (ctx.content_rect().width() - 2.0 * MARGIN - 20.0).max(160.0);
    Area::new(Id::unique("properties_bar")).anchor(anchor, offset).order(Order::Middle).show(ctx, |ui| {
        panel_frame(&cx.pal).show(ui, |ui| {
            if cx.layout.narrow {
                ui.set_max_width(max_width);
            }
            scroll_if(ui, cx.layout.narrow, ScrollDir::Horizontal, "properties_scroll", 0.0, |ui| {
                bar(ui, cx, &st.last_picked)
            });
        });
    });
}

fn bar(ui: &mut egui::Ui, cx: &mut Cx<'_>, last: &LastPicked) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = vec2(3.0, 0.0);
        colour_row(ui, cx, "Stroke", "stroke_color", QUICK_STROKE_SHADE, false, last.of(ColorTarget::Stroke));
        ui.add_space(6.0);
        vertical_separator(ui, &cx.pal, 24.0);
        ui.add_space(6.0);
        colour_row(ui, cx, "Fill", "fill_color", QUICK_FILL_SHADE, true, last.of(ColorTarget::Fill));
        ui.add_space(6.0);
        vertical_separator(ui, &cx.pal, 24.0);
        ui.add_space(6.0);
        width_row(ui, cx);
    });
}

fn colour_row(
    ui: &mut egui::Ui,
    cx: &mut Cx<'_>,
    title: &str,
    prop: &str,
    shade: u16,
    allow_none: bool,
    last_picked: Option<&str>,
) {
    section_label(ui, &cx.pal, title);
    ui.add_space(4.0);
    let swatch = if cx.layout.compact { 28.0 } else { 18.0 };
    let dark = cx.pal.dark;
    let current = cx.model.style.prop(prop).and_then(|p| p.value.clone());
    let blocked = not_applicable(cx, prop);
    let is_none = allow_none && matches!(current, Some(Value::Null));
    if allow_none {
        let ctl = Ctl::new("style.set")
            .args(json!({ "props": { prop: Value::Null } }))
            .label("No fill")
            .selected(is_none)
            .blocked(blocked.clone());
        cx.swatch(ui, &ctl, None, swatch);
    }
    for family in QUICK_FAMILIES {
        let name = colors::quick_name(family, shade);
        let selected = current.as_ref().and_then(Value::as_str).is_some_and(|v| colors::same(v, &name));
        let ctl = Ctl::new("style.set")
            .args(json!({ "props": { prop: name } }))
            .label(format!("{title} {family}"))
            .selected(selected)
            .blocked(blocked.clone());
        cx.swatch(ui, &ctl, colors::displayed(&name, dark), swatch);
    }
    // The colour last picked in the full popover, next to the wheel that opens it.
    if let Some(name) = last_picked {
        let selected = current.as_ref().and_then(Value::as_str).is_some_and(|v| colors::same(v, name));
        let ctl = Ctl::new("style.set")
            .args(json!({ "props": { prop: name } }))
            .label(format!("Last picked {}", title.to_lowercase()))
            .selected(selected)
            .blocked(blocked.clone());
        cx.swatch(ui, &ctl, colors::displayed(name, dark), swatch);
    }
    let verb = if prop == "stroke_color" { Verb::OpenColorStroke } else { Verb::OpenColorFill };
    let wheel_tip = format!("More {} colours", title.to_lowercase());
    let key = if prop == "stroke_color" { PopoverKey::Stroke } else { PopoverKey::Fill };
    let wheel = cx.verb_button(ui, verb, Icon::Wheel, swatch + 8.0, &wheel_tip, false);
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
        cx.command_widget(
            ui,
            &ctl,
            vec2(if cx.layout.compact { 40.0 } else { 30.0 }, cx.layout.toggle_height()),
            move |p, rect, look| {
                crate::widgets::paint_button_bg(p, rect, look, &pal, false);
                let c = crate::widgets::icon_color(look, &pal);
                let y = rect.center().y;
                p.line_segment(
                    [egui::pos2(rect.left() + 7.0, y), egui::pos2(rect.right() - 7.0, y)],
                    egui::Stroke::new(thickness, c),
                );
            },
        );
    }
}
