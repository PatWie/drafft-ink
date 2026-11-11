//! Building the preview element and writing the overlay parts.

use drafftink_edit::create::{self, LinearSpec, StrokeSpec};
use drafftink_geom::guides::SmartGuide;
use drafftink_geom::kurbo::Point;
use drafftink_scene::{Element, ElementId, Kind, PressureMode, ZIndex};

use crate::interact::ctx::Ctx;

/// The id of the preview element. It is valid, never in the document, and the same for every preview.
const PREVIEW_ID: &str = "drafftink-preview";

fn preview_id() -> Option<ElementId> {
    ElementId::new(PREVIEW_ID).ok()
}

/// A rectangle, diamond, ellipse or frame preview in the box `(x, y, w, h)`.
pub(super) fn shape(cx: &Ctx<'_>, kind: Kind, rect: (f64, f64, f64, f64)) -> Option<Element> {
    let style = cx.s.tool.defaults().style;
    create::shape_element(preview_id()?, kind, ZIndex::first(), rect, style).ok()
}

/// A line or arrow preview through `points` (at least two). The preview is always drawn with straight
/// segments, so an elbow arrow shows its two ends joined until the engine routes it.
pub(super) fn linear(cx: &Ctx<'_>, kind: Kind, points: &[Point]) -> Option<Element> {
    let style = cx.s.tool.defaults().style;
    create::linear_element(preview_id()?, ZIndex::first(), points, &LinearSpec::defaults(kind), style).ok()
}

/// A freehand or highlighter stroke through `points`, with `pressures` empty or one per point.
pub(super) fn stroke(cx: &Ctx<'_>, points: &[Point], pressures: &[f64], highlighter: bool) -> Option<Element> {
    let defaults = cx.s.tool.defaults();
    let style = if highlighter { create::highlighter_style(defaults.style, false, false) } else { defaults.style };
    // The same choice `element.create` makes, so the preview and the result use the same width model.
    let pressure_mode = if defaults.pressure_mode != PressureMode::Uniform {
        defaults.pressure_mode
    } else if pressures.is_empty() {
        PressureMode::Uniform
    } else {
        PressureMode::Device
    };
    let spec = StrokeSpec { pressures: pressures.to_vec(), pressure_mode, brush: defaults.brush, highlighter };
    create::freehand_element(preview_id()?, ZIndex::first(), points, spec, style).ok()
}

/// Shows (or removes) the preview and asks for an overlay redraw. Callers run this only when their input
/// changed.
pub(super) fn set(cx: &mut Ctx<'_>, element: Option<Element>) {
    cx.parts.preview = element;
    cx.overlay_changed();
}

/// Shows the guides, asking for a redraw only when they changed.
pub(super) fn set_guides(cx: &mut Ctx<'_>, guides: Vec<SmartGuide>) {
    if cx.parts.guides != guides {
        cx.parts.guides = guides;
        cx.overlay_changed();
    }
}

/// Shows the highlighted target (at most one), asking for a redraw only when it changed.
pub(super) fn set_highlight(cx: &mut Ctx<'_>, target: Option<&ElementId>) {
    let same = match (cx.parts.highlight.as_slice(), target) {
        ([], None) => true,
        ([a], Some(b)) => a == b,
        _ => false,
    };
    if !same {
        cx.parts.highlight = target.cloned().into_iter().collect();
        cx.overlay_changed();
    }
}

/// Takes back everything the drawing tools show: preview, faded elements, guides and highlight.
pub(super) fn clear(cx: &mut Ctx<'_>) {
    let parts = &mut *cx.parts;
    let shown =
        parts.preview.is_some() || !parts.faded.is_empty() || !parts.guides.is_empty() || !parts.highlight.is_empty();
    parts.preview = None;
    parts.faded.clear();
    parts.guides.clear();
    parts.highlight.clear();
    if shown {
        cx.overlay_changed();
    }
}
