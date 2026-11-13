//! The colour group of the top strip: stroke colour over fill colour. Every control is
//! `style.set`; with nothing selected the engine writes the defaults for new elements, so they are always useful.
//! `surfaces::strip` places them.

use drafftink_scene::Color;
use egui::{Sense, vec2};
use serde_json::{Value, json};

use crate::action::Verb;
use crate::colors::{self, QUICK_FAMILIES, QUICK_FILL_SHADE, QUICK_STROKE_SHADE};
use crate::icons::Icon;
use crate::state::{ColorTarget, UiState};
use crate::surfaces::color::{self, Pick, PopoverKey, remember_anchor_in};
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

/// The colour group of the top strip: stroke over fill, two rows with the same cells in the same order, so
/// the swatches and the two wheels line up column by column. Every control is `style.set`; with nothing selected
/// the engine writes the defaults for new elements, so they are always useful.
pub(super) fn style_rows(ui: &mut egui::Ui, cx: &mut Cx<'_>, st: &UiState) {
    let height = cx.layout.small_side();
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = vec2(space::SM, space::SM);
        for paint in Paint::BOTH {
            strip_row(ui, height, |ui| colour_row(ui, cx, st, paint));
        }
    });
}

/// The same controls as rows one under the other, for the popover of a strip with no room for [`style_rows`].
pub(super) fn style_column(ui: &mut egui::Ui, cx: &mut Cx<'_>, st: &UiState) {
    ui.spacing_mut().item_spacing = vec2(space::SM, space::MD);
    for paint in Paint::BOTH {
        ui.horizontal_wrapped(|ui| colour_row(ui, cx, st, paint));
    }
}

/// Why "no stroke" or "no fill" cannot be picked now: the engine's text, from `style.get`. It is also the reason
/// when the property does not apply to the selection.
pub fn clear_blocked(cx: &Cx<'_>, target: ColorTarget) -> Option<String> {
    let prop = match target {
        ColorTarget::Stroke => "stroke_color",
        ColorTarget::Fill => "fill_color",
        ColorTarget::Background => return None,
    };
    not_applicable(cx, prop).or_else(|| cx.model.style.prop(prop).and_then(|p| p.clear_reason.clone()))
}

/// Which of the two colours a row of the colour group sets.
#[derive(Clone, Copy)]
enum Paint {
    Stroke,
    Fill,
}

impl Paint {
    const BOTH: [Paint; 2] = [Paint::Stroke, Paint::Fill];

    fn target(self) -> ColorTarget {
        match self {
            Paint::Stroke => ColorTarget::Stroke,
            Paint::Fill => ColorTarget::Fill,
        }
    }

    /// The row label, which is also what the tooltips and accessible names are made of.
    fn title(self) -> &'static str {
        match self {
            Paint::Stroke => "Stroke",
            Paint::Fill => "Fill",
        }
    }

    fn prop(self) -> &'static str {
        match self {
            Paint::Stroke => "stroke_color",
            Paint::Fill => "fill_color",
        }
    }

    fn shade(self) -> u16 {
        match self {
            Paint::Stroke => QUICK_STROKE_SHADE,
            Paint::Fill => QUICK_FILL_SHADE,
        }
    }

    fn open_verb(self) -> Verb {
        match self {
            Paint::Stroke => Verb::OpenColorStroke,
            Paint::Fill => Verb::OpenColorFill,
        }
    }

    fn pick_verb(self) -> Verb {
        match self {
            Paint::Stroke => Verb::PickColorStroke,
            Paint::Fill => Verb::PickColorFill,
        }
    }

    fn popover(self) -> PopoverKey {
        match self {
            Paint::Stroke => PopoverKey::Stroke,
            Paint::Fill => PopoverKey::Fill,
        }
    }
}

/// The width the row labels share, so that the swatches of both rows start at the same x.
fn label_width(ui: &egui::Ui, cx: &Cx<'_>) -> f32 {
    Paint::BOTH
        .into_iter()
        .map(|paint| {
            ui.painter().layout_no_wrap(paint.title().to_owned(), TextSize::Body.hand(), cx.pal.muted).size().x
        })
        .fold(0.0, f32::max)
}

/// One row: label, the "none" cell, the quick colours, the slot of the colour last picked, the eyedropper, the wheel. Both rows
/// have every cell, in the same order and of the same size, so nothing moves when a colour is picked or the
/// selection changes.
fn colour_row(ui: &mut egui::Ui, cx: &mut Cx<'_>, st: &UiState, paint: Paint) {
    let title = paint.title();
    let size = vec2(label_width(ui, cx), cx.layout.small_side());
    let label = egui::RichText::new(title).font(TextSize::Body.hand()).color(cx.pal.muted);
    ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_min_size(size);
        ui.add(egui::Label::new(label).wrap_mode(egui::TextWrapMode::Extend));
    });
    // The swatch cell is a toggle-high square, the same in every row of the strip.
    let swatch = cx.layout.control(Control::Toggle) - 4.0;
    let dark = cx.pal.dark;
    let current = cx.model.style.prop(paint.prop()).and_then(|p| p.value.clone());
    let blocked = not_applicable(cx, paint.prop());
    let (none_label, none_selected) = match paint {
        Paint::Stroke => ("No stroke", current.as_ref().and_then(Value::as_str).is_some_and(is_clear)),
        Paint::Fill => ("No fill", matches!(current, Some(Value::Null))),
    };
    let (id, args) = color::pick_command(paint.target(), &Pick::Clear);
    let none =
        Ctl::new(id).args(args).label(none_label).selected(none_selected).blocked(clear_blocked(cx, paint.target()));
    cx.swatch(ui, &none, None, swatch);
    for family in QUICK_FAMILIES {
        let name = colors::quick_name(family, paint.shade());
        let selected = current.as_ref().and_then(Value::as_str).is_some_and(|v| colors::same(v, &name));
        let ctl = Ctl::new("style.set")
            .args(json!({ "props": { paint.prop(): name } }))
            .label(format!("{title} {family}"))
            .selected(selected)
            .blocked(blocked.clone());
        cx.swatch(ui, &ctl, colors::displayed(&name, dark), swatch);
    }
    // The colour last picked in the full popover, next to the wheel that opens it. Its place stays empty until
    // there is one, so the wheel does not move.
    match st.last_picked.of(paint.target()) {
        Some(name) => {
            let selected = current.as_ref().and_then(Value::as_str).is_some_and(|v| colors::same(v, name));
            let ctl = Ctl::new("style.set")
                .args(json!({ "props": { paint.prop(): name } }))
                .label(format!("Last picked {}", title.to_lowercase()))
                .selected(selected)
                .blocked(blocked.clone());
            cx.swatch(ui, &ctl, colors::displayed(name, dark), swatch);
        }
        None => {
            ui.allocate_exact_size(vec2(swatch + 4.0, swatch + 4.0), Sense::hover());
        }
    }
    let wheel_tip = format!("More {} colours", title.to_lowercase());
    let side = cx.layout.control(Control::Small);
    // The eyedropper: pick the colour from the canvas. It is on while pick mode is on for this row.
    let picking = st.eyedropper_target() == Some(paint.target());
    let tip = format!("Pick the {} colour from the canvas", title.to_lowercase());
    cx.verb_button_gated(ui, paint.pick_verb(), Icon::Eyedropper, side, &tip, picking, blocked);
    let wheel = cx.verb_button(ui, paint.open_verb(), Icon::Wheel, side, &wheel_tip, false);
    remember_anchor_in(ui, paint.popover(), wheel.rect);
}

/// True for a colour with no alpha: the stored "no stroke".
fn is_clear(text: &str) -> bool {
    Color::parse(text).is_ok_and(|c| c.a == 0)
}
