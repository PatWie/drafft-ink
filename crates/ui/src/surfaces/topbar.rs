//! The colour group of the top strip: stroke colour over fill colour. Every control is
//! `style.set`; with nothing selected the engine writes the defaults for new elements, so they are always useful.
//! `surfaces::strip` places them.

use egui::vec2;
use serde_json::{Value, json};

use crate::action::Verb;
use crate::colors::{self, QUICK_FAMILIES, QUICK_FILL_SHADE, QUICK_STROKE_SHADE};
use crate::icons::Icon;
use crate::state::{ColorTarget, UiState};
use crate::surfaces::color::{PopoverKey, remember_anchor_in};
use crate::theme::{Control, TextSize, space};
use crate::widgets::{Ctl, Cx, strip_row};

/// Why a property control is greyed, when the selection has no element it applies to: the engine's text, as
/// `style.get` reports it (the refusal that `style.set` would give).
pub fn not_applicable(cx: &Cx<'_>, prop: &str) -> Option<String> {
    if cx.model.style.props.is_empty() || cx.model.style.applies(prop) {
        None
    } else {
        cx.model.style.reason(prop).map(str::to_owned)
    }
}

/// The colour group of the top strip: stroke over fill, two rows that start at the same x. Every control is
/// `style.set`; with nothing selected the engine writes the defaults for new elements, so they are always useful.
pub(super) fn style_rows(ui: &mut egui::Ui, cx: &mut Cx<'_>, st: &UiState) {
    let height = cx.layout.small_side();
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = vec2(space::SM, space::SM);
        strip_row(ui, height, |ui| {
            colour_row(
                ui,
                cx,
                "Stroke",
                "stroke_color",
                QUICK_STROKE_SHADE,
                false,
                st.last_picked.of(ColorTarget::Stroke),
            );
        });
        strip_row(ui, height, |ui| {
            colour_row(ui, cx, "Fill", "fill_color", QUICK_FILL_SHADE, true, st.last_picked.of(ColorTarget::Fill));
        });
    });
}

/// The same controls as rows one under the other, for the popover of a strip with no room for [`style_rows`].
pub(super) fn style_column(ui: &mut egui::Ui, cx: &mut Cx<'_>, st: &UiState) {
    ui.spacing_mut().item_spacing = vec2(space::SM, space::MD);
    ui.horizontal_wrapped(|ui| {
        colour_row(ui, cx, "Stroke", "stroke_color", QUICK_STROKE_SHADE, false, st.last_picked.of(ColorTarget::Stroke));
    });
    ui.horizontal_wrapped(|ui| {
        colour_row(ui, cx, "Fill", "fill_color", QUICK_FILL_SHADE, true, st.last_picked.of(ColorTarget::Fill));
    });
}

/// The width the row labels share, so that the swatches of both rows start at the same x.
fn label_width(ui: &egui::Ui, cx: &Cx<'_>) -> f32 {
    ["Stroke", "Fill"]
        .into_iter()
        .map(|title| ui.painter().layout_no_wrap(title.to_owned(), TextSize::Body.hand(), cx.pal.muted).size().x)
        .fold(0.0, f32::max)
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
    let size = vec2(label_width(ui, cx), cx.layout.small_side());
    let label = egui::RichText::new(title).font(TextSize::Body.hand()).color(cx.pal.muted);
    ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_min_size(size);
        ui.add(egui::Label::new(label).wrap_mode(egui::TextWrapMode::Extend));
    });
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
