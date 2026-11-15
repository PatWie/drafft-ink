//! The "Edit picture" group of the strip: the controls of the paint editor that are not colours.
//!
//! * Outside the editor the image section offers the chip that opens it (`image.paint_begin`) and, while the
//!   picture has paint, the chip that takes the paint off (`image.reset_paint`). Both labels are the registry's titles.
//! * Inside, the group has the brush's size, hardness and opacity as sliders (each named by a small picture and its
//!   tooltip), and the Reset and Done buttons (Done filled with the accent, like the tool in use). The colour and the
//!   Paint or Erase choice are the colour group (`surfaces/brush.rs`).
//!
//! A slider is a plain `image.paint_brush` call for each change. The brush is state of the session and not of the
//! document, so a drag has no undo step to share and is not a gesture (docs/exceptions.md E13 is for scrubs that
//! change the document); the command is `quiet` in the ring, so a drag leaves no trail there.

use egui::{Ui, WidgetInfo, vec2};
use serde_json::{Value, json};

use super::controls::{chip_with_icon, style_slider_rail};
use super::pictograms::{self, BrushProp};
use crate::icons::Icon;
use crate::theme::TextSize;
use crate::widgets::{Ctl, Cx};

/// The width of a brush slider.
const SLIDER_WIDTH: f32 = 64.0;
/// The width reserved for the value next to a slider (`400 px`, `100%`).
const VALUE_WIDTH: f32 = 40.0;

/// The sizes the size slider takes, in pixels of the picture: from a pen's width to a wide brush. The command takes
/// more (`image.paint_brush` allows up to 4096); the slider is the range people use.
pub(super) const SIZE_RANGE: std::ops::RangeInclusive<f32> = 1.0..=400.0;

/// What a brush slider is.
struct BrushSlider {
    /// The picture before the slider.
    prop: BrushProp,
    /// The accessible name and the tooltip.
    name: &'static str,
    range: std::ops::RangeInclusive<f32>,
    /// A log scale gives the small sizes room: 1 to 400 is two and a half decades.
    logarithmic: bool,
    value: f32,
    /// The value as the text after the slider shows it.
    text: String,
    /// The arguments of `image.paint_brush` for a value.
    args: fn(f32) -> Value,
}

/// The size as the command takes it: whole pixels, one decimal below 10 where a pixel is a large step.
fn size_arg(v: f32) -> f64 {
    let v = f64::from(v);
    if v >= 10.0 { v.round() } else { (v * 10.0).round() / 10.0 }
}

/// A fraction rounded to whole percent.
fn percent_arg(v: f32) -> f64 {
    (f64::from(v) * 100.0).round() / 100.0
}

fn brush_slider(ui: &mut Ui, cx: &mut Cx<'_>, spec: &BrushSlider) {
    let pal = cx.pal;
    let height = cx.layout.toggle_height();
    let (glyph, hover) = ui.allocate_exact_size(vec2(height, height), egui::Sense::hover());
    pictograms::brush_prop(ui.painter(), glyph.shrink(height * 0.2), spec.prop, pal.muted);
    hover.on_hover_text(spec.name);
    let blocked = cx.refusal("image.paint_brush");
    ui.spacing_mut().slider_width = SLIDER_WIDTH;
    style_slider_rail(ui, cx);
    let mut value = spec.value;
    let mut slider = egui::Slider::new(&mut value, spec.range.clone()).show_value(false);
    if spec.logarithmic {
        slider = slider.logarithmic(true);
    }
    let resp = ui.add_enabled(blocked.is_none(), slider);
    let shown = f64::from(value);
    resp.widget_info(|| WidgetInfo::slider(blocked.is_none(), shown, spec.name));
    let resp = match &blocked {
        Some(reason) => resp.on_disabled_hover_text(reason),
        None => resp.on_hover_text(spec.name),
    };
    if resp.changed() && !ui.is_sizing_pass() {
        cx.run("image.paint_brush", (spec.args)(value));
    }
    ui.add_sized(
        [VALUE_WIDTH, height],
        egui::Label::new(egui::RichText::new(&spec.text).size(TextSize::Caption.points()).color(pal.muted)),
    );
}

/// Size, then hardness.
pub(super) fn size_and_hardness(ui: &mut Ui, cx: &mut Cx<'_>) {
    let brush = cx.model.brush;
    let size = BrushSlider {
        prop: BrushProp::Size,
        name: "Brush size",
        range: SIZE_RANGE,
        logarithmic: true,
        value: brush.size as f32,
        text: format!("{} px", size_arg(brush.size as f32)),
        args: |v| json!({ "size": size_arg(v) }),
    };
    brush_slider(ui, cx, &size);
    let hardness = BrushSlider {
        prop: BrushProp::Hardness,
        name: "Brush hardness",
        range: 0.0..=1.0,
        logarithmic: false,
        value: brush.hardness as f32,
        text: format!("{}%", (brush.hardness * 100.0).round()),
        args: |v| json!({ "hardness": percent_arg(v) }),
    };
    brush_slider(ui, cx, &hardness);
}

/// Opacity, then Reset and Done.
pub(super) fn opacity_reset_done(ui: &mut Ui, cx: &mut Cx<'_>) {
    let brush = cx.model.brush;
    let opacity = BrushSlider {
        prop: BrushProp::Opacity,
        name: "Brush opacity",
        range: 0.0..=1.0,
        logarithmic: false,
        value: brush.opacity as f32,
        text: format!("{}%", (brush.opacity * 100.0).round()),
        args: |v| json!({ "opacity": percent_arg(v) }),
    };
    brush_slider(ui, cx, &opacity);
    // Icon buttons, so that the group fits the menu a narrow window puts it in; the registry's titles name them.
    let side = cx.layout.small_side();
    cx.icon_button(ui, &Ctl::new("image.reset_paint"), Icon::EditPictureReset, side);
    cx.icon_button(ui, &Ctl::new("image.paint_end").selected(true), Icon::Check, side);
}

/// Opens the paint editor on the selected image.
pub(super) fn edit_picture(ui: &mut Ui, cx: &mut Cx<'_>) {
    chip_with_icon(ui, cx, &Ctl::new("image.paint_begin"), Icon::EditPicture);
}

/// Takes the paint off the selected image.
pub(super) fn reset_paint(ui: &mut Ui, cx: &mut Cx<'_>) {
    chip_with_icon(ui, cx, &Ctl::new("image.reset_paint"), Icon::EditPictureReset);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_is_whole_pixels_from_ten_and_tenths_below() {
        assert_eq!(size_arg(24.4), 24.0);
        assert_eq!(size_arg(399.6), 400.0);
        assert_eq!(size_arg(1.04), 1.0);
        assert_eq!(size_arg(2.46), 2.5);
    }

    #[test]
    fn the_size_slider_reaches_from_a_hair_to_a_wide_brush_and_stays_within_what_the_command_takes() {
        assert_eq!((*SIZE_RANGE.start(), *SIZE_RANGE.end()), (1.0, 400.0));
        assert!(*SIZE_RANGE.start() >= 0.5 && *SIZE_RANGE.end() <= 4096.0);
    }

    #[test]
    fn a_fraction_is_whole_percent() {
        assert_eq!(percent_arg(0.8049), 0.8);
        assert_eq!(percent_arg(1.0), 1.0);
    }
}
