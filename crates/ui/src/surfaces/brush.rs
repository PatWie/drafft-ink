//! The colour group of the strip while the paint editor is open: the colour of the brush over its mode.
//!
//! It takes the place of the stroke and fill rows (`topbar.rs`), whose `style.set` would only change the outline of
//! the image being painted on. The first row has the same cells as a colour row of the strip (the six quick
//! colours, the colour last picked, the eyedropper, the wheel); a pick is `image.paint_brush` with a `color`. The
//! second row is the Paint and Erase choice (`image.paint_brush` with a `mode`). The sliders and the buttons that
//! end the editor are the strip's "Edit picture" group (`ribbon/paint.rs`).
//!
//! The colour of a stroke is a colour of the picture, so the swatches show it as it is: the canvas does not map
//! pictures for the dark theme (`ColorTarget::is_themed`).

use drafftink_scene::paint::PaintMode;
use egui::{Sense, Ui, vec2};
use serde_json::json;

use crate::action::Verb;
use crate::colors::{self, QUICK_FAMILIES, QUICK_STROKE_SHADE};
use crate::icons::Icon;
use crate::state::{ColorTarget, UiState};
use crate::surfaces::color::{PopoverKey, remember_anchor_in};
use crate::theme::{Control, TextSize, space};
use crate::widgets::{Ctl, Cx, strip_row};

/// The row labels, which are also what the tooltips are made of.
const COLOUR_LABEL: &str = "Brush";
const MODE_LABEL: &str = "Mode";
/// The two modes: the text of the button and the value of `mode`.
const MODES: [(&str, &str, PaintMode); 2] =
    [("Paint", "paint", PaintMode::Paint), ("Erase", "erase", PaintMode::Erase)];

/// The two rows in the strip: the colours over the mode.
pub(super) fn rows(ui: &mut Ui, cx: &mut Cx<'_>, st: &UiState) {
    let height = cx.layout.small_side();
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = vec2(space::SM, space::SM);
        strip_row(ui, height, |ui| colour_row(ui, cx, st));
        strip_row(ui, height, |ui| mode_row(ui, cx));
    });
}

/// The same rows one under the other, for the popover of a strip with no room for [`rows`].
pub(super) fn column(ui: &mut Ui, cx: &mut Cx<'_>, st: &UiState) {
    ui.spacing_mut().item_spacing = vec2(space::SM, space::MD);
    // The colours, then the colour last picked with the eyedropper and the wheel under them, so that a row is no
    // wider than a phone's popover.
    ui.horizontal(|ui| {
        label(ui, cx, COLOUR_LABEL);
        swatches(ui, cx);
    });
    ui.horizontal(|ui| {
        ui.allocate_space(vec2(label_width(ui, cx), 0.0));
        tools(ui, cx, st);
    });
    ui.horizontal(|ui| mode_row(ui, cx));
}

/// The width both labels share, so that the cells of the two rows start at the same x.
fn label_width(ui: &Ui, cx: &Cx<'_>) -> f32 {
    [COLOUR_LABEL, MODE_LABEL]
        .into_iter()
        .map(|text| ui.painter().layout_no_wrap(text.to_owned(), TextSize::Body.hand(), cx.pal.muted).size().x)
        .fold(0.0, f32::max)
}

/// A row label in the same face as "Stroke" and "Fill".
fn label(ui: &mut Ui, cx: &Cx<'_>, text: &str) {
    let size = vec2(label_width(ui, cx), cx.layout.small_side());
    let text = egui::RichText::new(text).font(TextSize::Body.hand()).color(cx.pal.muted);
    ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_min_size(size);
        ui.add(egui::Label::new(text).wrap_mode(egui::TextWrapMode::Extend));
    });
}

/// The quick colours, the colour last picked, the eyedropper and the wheel.
fn colour_row(ui: &mut Ui, cx: &mut Cx<'_>, st: &UiState) {
    label(ui, cx, COLOUR_LABEL);
    swatches(ui, cx);
    tools(ui, cx, st);
}

fn brush_color(color: &str) -> Ctl<'_> {
    Ctl::new("image.paint_brush").args(json!({ "color": color }))
}

/// The side of a colour cell.
fn swatch_side(cx: &Cx<'_>) -> f32 {
    cx.layout.control(Control::Toggle) - 4.0
}

/// The six quick colours.
fn swatches(ui: &mut Ui, cx: &mut Cx<'_>) {
    let now = cx.model.brush.color;
    for family in QUICK_FAMILIES {
        let name = colors::quick_name(family, QUICK_STROKE_SHADE);
        let selected = colors::resolve(&name) == Some(now);
        let ctl = brush_color(&name).label(format!("Brush {family}")).selected(selected);
        cx.swatch(ui, &ctl, colors::displayed(&name, false), swatch_side(cx));
    }
}

/// The colour last picked, the eyedropper and the wheel.
fn tools(ui: &mut Ui, cx: &mut Cx<'_>, st: &UiState) {
    let swatch = swatch_side(cx);
    let now = cx.model.brush.color;
    // The place of the colour last picked stays reserved, so the buttons after it do not move.
    match st.last_picked.of(ColorTarget::Brush) {
        Some(name) => {
            let selected = colors::resolve(name) == Some(now);
            let ctl = brush_color(name).label("Last picked brush colour").selected(selected);
            cx.swatch(ui, &ctl, colors::displayed(name, false), swatch);
        }
        None => {
            ui.allocate_exact_size(vec2(swatch + 4.0, swatch + 4.0), Sense::hover());
        }
    }
    let side = cx.layout.control(Control::Small);
    let picking = st.eyedropper_target() == Some(ColorTarget::Brush);
    let tip = "Pick the brush colour from the canvas";
    cx.verb_button_gated(ui, Verb::PickColorBrush, Icon::Eyedropper, side, tip, picking, None);
    let wheel = cx.verb_button(ui, Verb::OpenColorBrush, Icon::Wheel, side, "More brush colours", false);
    remember_anchor_in(ui, PopoverKey::Brush, wheel.rect);
}

/// Paint or erase: two text buttons, the one in use filled with the accent.
fn mode_row(ui: &mut Ui, cx: &mut Cx<'_>) {
    label(ui, cx, MODE_LABEL);
    for (text, value, mode) in MODES {
        let ctl = Ctl::new("image.paint_brush")
            .args(json!({ "mode": value }))
            .label(text)
            .selected(cx.model.brush.mode == mode);
        super::ribbon::chip(cx, ui, &ctl, text);
    }
}
