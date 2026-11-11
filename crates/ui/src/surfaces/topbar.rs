//! The style controls in the middle of the top strip: stroke colour, fill colour, stroke width. Every control is
//! `style.set`; with nothing selected the engine writes the defaults for new elements, so they are always useful.
//! `surfaces::strip` places them.

use egui::vec2;
use serde_json::{Value, json};

use crate::action::Verb;
use crate::colors::{self, QUICK_FAMILIES, QUICK_FILL_SHADE, QUICK_STROKE_SHADE};
use crate::icons::Icon;
use crate::state::{ColorTarget, UiState};
use crate::surfaces::color::{PopoverKey, remember_anchor_in};
use crate::theme::{Control, space};
use crate::widgets::{Ctl, Cx, section_label, vertical_separator};

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

/// The stroke, fill and width controls in one row: the centre of the top strip.
pub(super) fn style_row(ui: &mut egui::Ui, cx: &mut Cx<'_>, st: &UiState) {
    ui.spacing_mut().item_spacing = vec2(space::SM, 0.0);
    let height = cx.layout.control(Control::Small) - 2.0 * space::SM;
    colour_row(ui, cx, "Stroke", "stroke_color", QUICK_STROKE_SHADE, false, st.last_picked.of(ColorTarget::Stroke));
    ui.add_space(space::MD);
    vertical_separator(ui, &cx.pal, height);
    ui.add_space(space::MD);
    colour_row(ui, cx, "Fill", "fill_color", QUICK_FILL_SHADE, true, st.last_picked.of(ColorTarget::Fill));
    ui.add_space(space::MD);
    vertical_separator(ui, &cx.pal, height);
    ui.add_space(space::MD);
    width_row(ui, cx);
}

/// The same controls as rows one under the other, for the popover of a strip with no room for [`style_row`].
pub(super) fn style_column(ui: &mut egui::Ui, cx: &mut Cx<'_>, st: &UiState) {
    ui.spacing_mut().item_spacing = vec2(space::SM, space::MD);
    ui.horizontal_wrapped(|ui| {
        colour_row(ui, cx, "Stroke", "stroke_color", QUICK_STROKE_SHADE, false, st.last_picked.of(ColorTarget::Stroke));
    });
    ui.horizontal_wrapped(|ui| {
        colour_row(ui, cx, "Fill", "fill_color", QUICK_FILL_SHADE, true, st.last_picked.of(ColorTarget::Fill));
    });
    ui.horizontal_wrapped(|ui| width_row(ui, cx));
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
    ui.add_space(space::SM);
    // The swatch cell is a toggle-high square, the same in every row of the strip.
    let swatch = cx.layout.control(Control::Toggle) - 4.0;
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
    let wheel = cx.verb_button(ui, verb, Icon::Wheel, cx.layout.control(Control::Small), &wheel_tip, false);
    remember_anchor_in(ui, key, wheel.rect);
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
        let side = cx.layout.control(Control::Small);
        cx.command_widget(ui, &ctl, vec2(side, side), move |p, rect, look| {
            crate::widgets::paint_button_bg(p, rect, look, &pal, true);
            let c = crate::widgets::icon_color(look, &pal);
            let y = rect.center().y;
            let inset = side * 0.25;
            p.line_segment(
                [egui::pos2(rect.left() + inset, y), egui::pos2(rect.right() - inset, y)],
                egui::Stroke::new(thickness, c),
            );
        });
    }
}
