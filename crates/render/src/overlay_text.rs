//! Overlay parts made of text: the distance badges on spacing guides, and the caret and selection of the text
//! being edited on the canvas. Also the label chip that the names of collaborators use.

use std::sync::Arc;

use drafftink_geom::guides::{SmartGuide, SmartGuideKind};
use drafftink_typeset::{TextLayout, TextSelection, TextSpec, TextStyle};
use kurbo::{Affine, BezPath, Point, Rect, RoundedRect, Shape, Size, Stroke};
use peniko::{Color, Fill};
use vello::{Glyph, Scene};

use crate::overlay::Pen;

/// A label longer than this many characters is cut and ends with an ellipsis.
const MAX_LABEL_CHARS: usize = 32;
/// Horizontal and vertical padding of a chip around its text, in logical pixels.
const CHIP_PAD_X: f64 = 6.0;
const CHIP_PAD_Y: f64 = 3.0;
/// Corner radius of a chip, in logical pixels.
const CHIP_RADIUS: f64 = 4.0;
/// Gap between a spacing guide and its badge, in logical pixels.
const BADGE_GAP: f64 = 10.0;
/// Width of the caret, in logical pixels.
const CARET_WIDTH: f64 = 1.5;
/// The pale tint of the selection behind the text.
const SELECTION_ALPHA: f32 = 0.3;

/// The text of a label: no control characters, at most [`MAX_LABEL_CHARS`] characters.
pub(crate) fn clean_label(text: &str) -> String {
    let kept: String = text.chars().filter(|c| !c.is_control()).collect();
    let mut out: String = kept.trim().chars().take(MAX_LABEL_CHARS + 1).collect();
    if out.chars().count() > MAX_LABEL_CHARS {
        out = out.chars().take(MAX_LABEL_CHARS - 1).collect();
        out.push('\u{2026}');
    }
    out
}

/// Black or white, whichever reads better on `fill` (by relative luminance).
fn contrasting(fill: Color) -> Color {
    let [r, g, b, _] = fill.components;
    let luminance = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    if luminance > 0.6 { Color::BLACK } else { Color::WHITE }
}

/// Draws the glyph runs of `layout` with its top-left at `origin` (device pixels), `scale` device pixels per
/// layout unit, all in `color`.
pub(crate) fn draw_glyphs(scene: &mut Scene, layout: &TextLayout, origin: Point, scale: f64, color: Color) {
    let to = Affine::translate(origin.to_vec2()) * Affine::scale(scale);
    for run in layout.runs.iter().filter(|run| !run.glyphs.is_empty()) {
        scene
            .draw_glyphs(&run.font)
            .brush(color)
            .hint(false)
            .transform(to)
            .glyph_transform(run.skew.map(|s| Affine::skew(f64::from(s), 0.0)))
            .font_size(run.font_size)
            .normalized_coords(&run.normalized_coords)
            .draw(Fill::NonZero, run.glyphs.iter().map(|g| Glyph { id: g.id, x: g.x, y: g.y }));
    }
}

/// A laid-out label and its size in device pixels.
pub(crate) struct Label {
    layout: Arc<TextLayout>,
    size: Size,
}

impl Label {
    /// Lays out `text` at the label size of the style. `None` for text that is empty after cleaning.
    pub(crate) fn new(pen: &mut Pen, text: &str) -> Option<Self> {
        let text = clean_label(text);
        if text.is_empty() {
            return None;
        }
        let size = pen.style.label_size;
        let size = if size.is_finite() && size > 0.0 { size } else { 12.0 };
        let layout = pen.typesetter.text(&TextSpec::styled(text, TextStyle::with_size(size as f32)));
        let size = Size::new(layout.size.width * pen.px, layout.size.height * pen.px);
        Some(Self { layout, size })
    }

    /// Size of the chip around the text, in device pixels.
    pub(crate) fn chip_size(&self, px: f64) -> Size {
        Size::new(self.size.width + 2.0 * CHIP_PAD_X * px, self.size.height + 2.0 * CHIP_PAD_Y * px)
    }

    /// Draws the text on a rounded chip filled with `fill`; `top_left` is the chip's corner in device pixels.
    pub(crate) fn draw_chip(&self, scene: &mut Scene, px: f64, top_left: Point, fill: Color) {
        let size = self.chip_size(px);
        let chip = RoundedRect::new(
            top_left.x,
            top_left.y,
            top_left.x + size.width,
            top_left.y + size.height,
            CHIP_RADIUS * px,
        );
        scene.fill(Fill::NonZero, Affine::IDENTITY, fill, None, &chip);
        let origin = Point::new(top_left.x + CHIP_PAD_X * px, top_left.y + CHIP_PAD_Y * px);
        draw_glyphs(scene, &self.layout, origin, px, contrasting(fill));
    }
}

/// The distance shown on a spacing guide: its length, rounded to whole world units. `None` for other guides and
/// for lengths that are not worth a number.
pub(crate) fn badge_distance(g: &SmartGuide) -> Option<i64> {
    if !matches!(g.kind, SmartGuideKind::EqualSpacingH | SmartGuideKind::EqualSpacingV) {
        return None;
    }
    let length = (g.end - g.start).abs();
    (0.5..1.0e9).contains(&length).then_some(length.round() as i64)
}

/// Draws the gap on each spacing guide, in a chip beside the middle of the guide (above a horizontal one, to the
/// right of a vertical one). Returns the number of badges.
pub(crate) fn paint_guide_badges(scene: &mut Scene, pen: &mut Pen, guides: &[SmartGuide]) -> usize {
    let mut drawn = 0;
    for g in guides {
        let Some(distance) = badge_distance(g) else { continue };
        let Some(label) = Label::new(pen, &distance.to_string()) else { continue };
        let vertical = g.kind == SmartGuideKind::EqualSpacingV;
        let along = (g.start + g.end) / 2.0;
        let mid = pen.dev(if vertical { Point::new(g.position, along) } else { Point::new(along, g.position) });
        let size = label.chip_size(pen.px);
        let gap = BADGE_GAP * pen.px;
        let top_left = if vertical {
            Point::new(mid.x + gap, mid.y - size.height / 2.0)
        } else {
            Point::new(mid.x - size.width / 2.0, mid.y - gap - size.height)
        };
        label.draw_chip(scene, pen.px, top_left, pen.style.guide.with_alpha(1.0));
        drawn += 1;
    }
    drawn
}

/// The caret and selection of the text being edited. Rectangles are in the layout space of the text element, as
/// `drafftink_typeset` gives them; `to_world` is the element's transform (`element_to_world`), so a turned element
/// has a turned caret.
#[derive(Clone, Debug, PartialEq)]
pub struct TextEdit {
    /// Layout space of the text to world space.
    pub to_world: Affine,
    /// The caret: its left edge and height count, its width is set by the painter (1.5 logical pixels). `None`
    /// while it blinks off or while text is selected.
    pub caret: Option<Rect>,
    /// The selected stretches of text, one rectangle per line.
    pub selection: Vec<Rect>,
    /// Colour of the caret (the text colour).
    pub caret_color: Color,
}

impl TextEdit {
    /// The caret and selection of `selection` on `layout`. The caret shows when the selection is empty and
    /// `caret_on` is true (the caller blinks it by the clock).
    pub fn new(
        to_world: Affine,
        layout: &TextLayout,
        selection: &TextSelection,
        caret_on: bool,
        caret_color: Color,
    ) -> Self {
        let caret = (selection.is_collapsed() && caret_on).then(|| layout.caret_rect(selection.focus(), 0.0));
        Self { to_world, caret, selection: layout.selection_rects(selection), caret_color }
    }
}

/// Draws the selection under the caret. Returns 1 when something was drawn.
pub(crate) fn paint_text_edit(scene: &mut Scene, pen: &Pen, edit: Option<&TextEdit>) -> usize {
    let Some(edit) = edit else { return 0 };
    let to_dev = pen.to_dev * edit.to_world;
    let mut drawn = 0;
    let mut path = BezPath::new();
    for r in edit.selection.iter().filter(|r| r.is_finite()) {
        path.extend(r.path_elements(0.1));
    }
    if !path.elements().is_empty() {
        scene.fill(Fill::NonZero, to_dev, pen.style.accent.with_alpha(SELECTION_ALPHA), None, &path);
        drawn = 1;
    }
    if let Some(c) = edit.caret.filter(|c| c.is_finite() && c.height() > 0.0) {
        let mut line = BezPath::new();
        line.move_to(to_dev * Point::new(c.x0, c.y0));
        line.line_to(to_dev * Point::new(c.x0, c.y1));
        scene.stroke(&Stroke::new(CARET_WIDTH * pen.px), Affine::IDENTITY, edit.caret_color, None, &line);
        drawn = 1;
    }
    drawn
}
