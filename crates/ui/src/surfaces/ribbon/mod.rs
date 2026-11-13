//! The contextual groups of the top strip: the properties of what is selected, or of the tool being drawn with.
//! `surfaces/strip.rs` places them; this module says which groups there are and draws each one.
//!
//! * **Where.** The strip is two control rows tall. At its left stand the menu, rooms and title, then the colour
//!   group (stroke over fill, `topbar.rs`), then these groups, each laid out in the same two rows, with a thin
//!   divider between two groups. What does not fit goes behind the "More" button at the right end of the strip.
//! * **What.** Groups of controls in a fixed order ([`Group::ALL`]). A group is there when `style.get` says its
//!   property applies to the selection (or, with nothing selected, to the tool: [`tool_shows`]), or when the
//!   engine says its command can run (align needs two elements, distribute three). Layering, flip and the
//!   actions need a selection. With nothing selected and no drawing tool there are no groups: the strip holds the
//!   colour group (the defaults for new elements) and nothing else, at the same height as always.
//! * **How it fits.** `layout.rs` plans it from the measured widths of the groups; `menus.rs` has the "More" popup
//!   (the overflowed groups and the properties that have no room of their own) and the head menus.
//!
//! Every control is a command; the scrub of the opacity slider is the one gesture (docs/exceptions.md E13).

mod controls;
mod layout;
mod menus;
mod pictograms;

use drafftink_engine::state::Tool;

pub(crate) use self::controls::{MORE_BUTTON_WIDTH, more_button};
pub(crate) use self::layout::{Widths, fit};
pub(crate) use self::menus::{has_more as has_extras, show as show_menus};
use crate::model::StyleProp;
use crate::theme::space;
use crate::widgets::Cx;

/// The width of the divider between two parts of the strip: space, the line, space.
pub(crate) const DIVIDER_WIDTH: f32 = 2.0 * space::MD + 1.0;

/// Whether a property is relevant to the tool being drawn with (used only when nothing is selected). This is
/// presentation: the engine skips and reports the properties that do not apply.
fn tool_shows(tool: Tool, prop: &str) -> bool {
    const TEXT: [&str; 4] = ["font_family", "font_weight", "font_size", "text_align"];
    const FREEHAND: [&str; 2] = ["freehand_style", "pressure_mode"];
    match tool {
        Tool::Rectangle | Tool::Diamond | Tool::Frame => !TEXT.contains(&prop) && !FREEHAND.contains(&prop),
        Tool::Ellipse => !TEXT.contains(&prop) && !FREEHAND.contains(&prop) && prop != "corner_radius",
        Tool::Line | Tool::Arrow => {
            matches!(prop, "stroke_width" | "stroke_style" | "sloppiness" | "corner_radius" | "opacity")
        }
        Tool::Freehand | Tool::Highlighter => {
            matches!(prop, "stroke_width" | "freehand_style" | "pressure_mode" | "opacity" | "fill_pattern")
        }
        Tool::Text | Tool::Math => TEXT.contains(&prop) || matches!(prop, "line_height" | "opacity"),
        Tool::Select | Tool::Lasso | Tool::Hand | Tool::Eraser | Tool::Laser => false,
    }
}

/// Whether a property has a control now: the engine's `style.get` for a selection, the tool for new elements.
fn shows(cx: &Cx<'_>, prop: &str) -> bool {
    let m = cx.model;
    if m.selection_count > 0 { m.style.applies(prop) } else { tool_shows(m.tool, prop) }
}

/// One row of controls of a group. Every row is as tall as a small button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    /// Thin to extra bold.
    Width,
    /// Solid, dashed, dotted.
    StrokeStyle,
    /// Solid, hatch, cross-hatch, dots and the others.
    FillPattern,
    /// Sharp or round corners.
    Corners,
    /// Straight, curved or elbow arrows and lines.
    ArrowPath,
    /// The start and end heads of an arrow.
    ArrowHeads,
    /// Normal or calligraphy.
    Brush,
    /// The font dropdown.
    FontFamily,
    /// Light, regular, heavy.
    FontWeight,
    /// S to XL.
    TextSize,
    /// Bold and italic.
    Emphasis,
    /// Left, centre, right.
    TextAlign,
    /// Top, middle, bottom.
    VerticalAlign,
    /// The opacity slider.
    Opacity,
    /// Align and distribute.
    Align,
    /// Z-order.
    Layer,
    /// Flip.
    Flip,
    /// Duplicate, group, ungroup, lock.
    Compose,
    /// The link and delete.
    LinkDelete,
    /// Crop the image, or finish cropping it.
    Crop,
    /// Show the whole picture of a cropped image again.
    ResetCrop,
}

/// The groups of the strip, in the order they have from left to right. The order never changes; groups come and go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Group {
    /// Width over style.
    Stroke,
    /// Fill pattern over corners.
    Shape,
    /// Path over heads.
    Arrow,
    /// Normal or calligraphy.
    Brush,
    /// Family over weight.
    Font,
    /// Size over bold and italic.
    TextSize,
    /// Text align over vertical align.
    TextAlign,
    /// Opacity over align and distribute.
    Opacity,
    /// Layering over flip.
    Layer,
    /// Duplicate, group, lock over link and delete.
    Actions,
    /// Crop over reset crop: for one selected image.
    Image,
}

impl Group {
    /// Every group, in order.
    pub(crate) const ALL: [Group; 11] = [
        Group::Stroke,
        Group::Shape,
        Group::Arrow,
        Group::Brush,
        Group::Font,
        Group::TextSize,
        Group::TextAlign,
        Group::Opacity,
        Group::Image,
        Group::Layer,
        Group::Actions,
    ];

    /// The rows the group has now (at most two): the controls that apply, packed to the top.
    fn rows(self, cx: &Cx<'_>) -> Vec<Row> {
        let model = cx.model;
        let selected = model.selection_count > 0;
        let candidates: Vec<(Row, bool)> = match self {
            Group::Stroke => {
                vec![(Row::Width, shows(cx, "stroke_width")), (Row::StrokeStyle, shows(cx, "stroke_style"))]
            }
            // Next to a path (sharp, round, elbow) two more "sharp" and "round" would only confuse: the corners of
            // arrows and lines are in the "More" menu.
            Group::Shape => vec![
                (Row::FillPattern, shows(cx, "fill_pattern") && has_fill(cx)),
                (Row::Corners, shows(cx, "corner_radius") && !controls::has_path(cx)),
            ],
            Group::Arrow => vec![
                (Row::ArrowPath, controls::has_path(cx)),
                (Row::ArrowHeads, controls::new_arrows(cx) || (selected && model.style.has_kind("arrow"))),
            ],
            Group::Brush => vec![(Row::Brush, shows(cx, "freehand_style"))],
            Group::Font => {
                vec![(Row::FontFamily, shows(cx, "font_family")), (Row::FontWeight, shows(cx, "font_weight"))]
            }
            Group::TextSize => vec![(Row::TextSize, shows(cx, "font_size")), (Row::Emphasis, model.faces().is_some())],
            Group::TextAlign => {
                vec![(Row::TextAlign, shows(cx, "text_align")), (Row::VerticalAlign, shows(cx, "vertical_align"))]
            }
            // Align needs two elements and distribute three; the engine knows, and the row follows its answer.
            Group::Opacity => {
                vec![
                    (Row::Opacity, shows(cx, "opacity")),
                    (Row::Align, selected && cx.refusal("arrange.align").is_none()),
                ]
            }
            Group::Layer => vec![(Row::Layer, selected), (Row::Flip, selected)],
            Group::Actions => vec![(Row::Compose, selected), (Row::LinkDelete, selected)],
            // One selected image offers the crop; the reset is there while the picture is cropped.
            Group::Image => vec![
                (Row::Crop, model.crop_editing || cx.refusal("image.crop_begin").is_none()),
                (Row::ResetCrop, cx.refusal("image.reset_crop").is_none()),
            ],
        };
        candidates.into_iter().filter_map(|(row, on)| on.then_some(row)).collect()
    }
}

/// The groups that have anything to offer now, in order.
pub(crate) fn visible_groups(cx: &Cx<'_>) -> Vec<Group> {
    Group::ALL.into_iter().filter(|g| !g.rows(cx).is_empty()).collect()
}

/// A fill pattern only matters while there is a fill (or the selection disagrees about it).
fn has_fill(cx: &Cx<'_>) -> bool {
    let fill = cx.model.style.prop("fill_color");
    !matches!(fill.and_then(|p| p.value.as_ref()), Some(serde_json::Value::Null))
        || fill.is_some_and(StyleProp::is_mixed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_order_lists_every_group_once() {
        let mut seen = Group::ALL.to_vec();
        seen.sort_by_key(|g| *g as u8);
        seen.dedup();
        assert_eq!(seen.len(), Group::ALL.len());
    }

    #[test]
    fn a_drawing_tool_gets_the_properties_that_suit_it() {
        assert!(tool_shows(Tool::Rectangle, "corner_radius") && !tool_shows(Tool::Rectangle, "font_family"));
        assert!(!tool_shows(Tool::Ellipse, "corner_radius"));
        assert!(tool_shows(Tool::Text, "font_family") && tool_shows(Tool::Text, "line_height"));
        assert!(tool_shows(Tool::Freehand, "pressure_mode") && !tool_shows(Tool::Freehand, "stroke_style"));
        assert!(tool_shows(Tool::Arrow, "stroke_style") && !tool_shows(Tool::Arrow, "font_size"));
        assert!(!tool_shows(Tool::Select, "opacity") && !tool_shows(Tool::Hand, "stroke_style"));
    }

    #[test]
    fn the_width_follows_the_tools_that_draw_a_stroke() {
        for tool in [Tool::Rectangle, Tool::Ellipse, Tool::Line, Tool::Arrow, Tool::Freehand, Tool::Highlighter] {
            assert!(tool_shows(tool, "stroke_width"), "{tool:?}");
        }
        for tool in [Tool::Text, Tool::Math, Tool::Select, Tool::Lasso, Tool::Hand, Tool::Eraser, Tool::Laser] {
            assert!(!tool_shows(tool, "stroke_width"), "{tool:?}");
        }
    }
}
