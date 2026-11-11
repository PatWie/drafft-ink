//! Conversions from scene data to the inputs of the layout and sketch crates, and the paint bounds of an element.
//!
//! `typeset` and `sketch` own their input types and know nothing about documents. This module is the single place
//! that turns an [`Element`] into those inputs. Everything that measures text must use [`text_spec`] and
//! [`math_spec`], because the box the editor stores in `transform` has to be the box the renderer draws.

use drafftink_scene::{
    Color, Element, ElementKind, FillPattern, FontFamily, FontWeight, Head, LinearData, MathData, PathType,
    PressureMode, Sloppiness, StrokeStyle, TextAlign, TextData,
};
use drafftink_sketch as sketch;
use drafftink_typeset as typeset;
use kurbo::{Affine, Rect};

/// Marker size of an arrow whose `head_size` is `None` and whose stroke is thin: the old app's constant.
pub const DEFAULT_HEAD_SIZE: f64 = 15.0;
/// Font size of a frame's name label, in logical pixels. The label has the same size on screen at every zoom
/// (Excalidraw does the same), so the composer draws it in device space after the frame's fragment.
pub const FRAME_LABEL_SIZE: f64 = 14.0;
/// Gap between the label and the frame's top edge, in logical pixels.
pub const FRAME_LABEL_GAP: f64 = 4.0;
/// Room reserved above a frame for its label (gap plus the tallest label line), in logical pixels.
pub const FRAME_LABEL_ROOM: f64 = 24.0;
/// The frame label is cut at the frame's width on screen, but never narrower than this many logical pixels.
pub const FRAME_LABEL_MIN_WIDTH: f64 = 48.0;
/// The lowest zoom at which the paint bounds of a frame still cover its label. A label has a fixed size on screen,
/// so its extent in world units grows as the zoom falls; the bounds (which cannot depend on the zoom) are sized for
/// this one. Below it, a label can be culled a few pixels early at the edge of the view.
pub const FRAME_LABEL_MIN_ZOOM: f64 = 0.25;

/// The colour of an element's text or ink as the layout engine sees it.
pub(crate) fn rgba8(c: Color) -> typeset::Rgba8 {
    typeset::Rgba8::new(c.r, c.g, c.b, c.a)
}

fn family(f: FontFamily) -> typeset::FontFamily {
    match f {
        FontFamily::GelPen => typeset::FontFamily::GelPen,
        FontFamily::NotoSans => typeset::FontFamily::NotoSans,
        FontFamily::GelPenSerif => typeset::FontFamily::GelPenSerif,
        FontFamily::VanillaExtract => typeset::FontFamily::VanillaExtract,
    }
}

fn weight(w: FontWeight) -> typeset::FontWeight {
    match w {
        FontWeight::Light => typeset::FontWeight::Light,
        FontWeight::Regular => typeset::FontWeight::Regular,
        FontWeight::Heavy => typeset::FontWeight::Heavy,
    }
}

/// The layout input of a text element.
///
/// `stroke` is the element's text colour as stored (no opacity, no theme mapping: both are applied when the layout
/// is drawn, so changing opacity or theme never re-lays-out). `box_width` is `transform.w`; it is the wrap width
/// unless the text sizes itself.
pub fn text_spec(t: &TextData, stroke: Color, box_width: f64) -> typeset::TextSpec {
    let mut style = typeset::TextStyle::with_size(t.font.size as f32);
    style.family = family(t.font.family);
    style.weight = weight(t.font.weight);
    style.color = rgba8(stroke);
    let colors: Vec<Option<typeset::Rgba8>> =
        if t.runs.is_empty() { Vec::new() } else { t.char_colors().into_iter().map(|c| c.map(rgba8)).collect() };
    let mut spec = typeset::TextSpec::styled(t.content.as_str(), style);
    spec.spans = typeset::Span::from_char_colors(&t.content, &colors);
    spec.align = match t.font.align {
        TextAlign::Left => typeset::Align::Left,
        TextAlign::Center => typeset::Align::Center,
        TextAlign::Right => typeset::Align::Right,
    };
    spec.wrap = if t.auto_resize { typeset::Wrap::None } else { typeset::Wrap::Width(box_width as f32) };
    spec.line_height = typeset::LineHeight::FontSize(t.font.line_height as f32);
    spec
}

/// The layout input of a math element.
pub fn math_spec(m: &MathData) -> typeset::MathSpec {
    typeset::MathSpec::new(m.latex.as_str()).with_size(m.font_size)
}

/// The layout input of a frame's name label.
pub(crate) fn frame_label_spec(name: &str) -> typeset::TextSpec {
    let style = typeset::TextStyle::with_size(FRAME_LABEL_SIZE as f32);
    typeset::TextSpec::styled(name, style)
}

fn dash(s: StrokeStyle) -> sketch::LineDash {
    match s {
        StrokeStyle::Solid => sketch::LineDash::Solid,
        StrokeStyle::Dashed => sketch::LineDash::Dashed,
        StrokeStyle::Dotted => sketch::LineDash::Dotted,
    }
}

fn fill_style(p: FillPattern) -> sketch::FillStyle {
    match p {
        FillPattern::Solid => sketch::FillStyle::Solid,
        FillPattern::Hachure => sketch::FillStyle::Hachure,
        FillPattern::ZigZag => sketch::FillStyle::ZigZag,
        FillPattern::CrossHatch => sketch::FillStyle::CrossHatch,
        FillPattern::Dots => sketch::FillStyle::Dots,
        FillPattern::Dashed => sketch::FillStyle::Dashed,
        FillPattern::ZigZagLine => sketch::FillStyle::ZigZagLine,
    }
}

fn params(e: &Element, fillable: bool) -> sketch::SketchParams {
    let s = &e.style;
    sketch::SketchParams {
        roughness: s.sloppiness.roughness(),
        seed: s.seed,
        stroke_width: s.stroke_width,
        line: dash(s.stroke_style),
        fill: if fillable && s.fill_color.is_some() { fill_style(s.fill_pattern) } else { sketch::FillStyle::None },
        ..sketch::SketchParams::default()
    }
}

/// How far, in head sizes, a head reaches from the end of its line. The farthest part is the ring of the
/// zero-or-many marker: its centre is 1.3 sizes behind the tip and its radius is 0.28 sizes (`cardinality_path`),
/// 1.58 in all. The stroke half-width is in the margin already. The value was 1.2 before, and a 14.5 x 3 arrow with
/// that marker painted 2.9 units outside its bounds.
const HEAD_REACH: f64 = 1.6;

/// The size of an arrow head: the element's own value, or a default that grows with the stroke.
pub fn head_size(d: &LinearData, stroke_width: f64) -> f64 {
    d.head_size.unwrap_or_else(|| (stroke_width * 7.5).max(DEFAULT_HEAD_SIZE))
}

fn arrowhead(h: Head) -> sketch::Arrowhead {
    match h {
        Head::Arrow => sketch::Arrowhead::ARROW,
        Head::Bar => sketch::Arrowhead::BAR,
        Head::Circle => sketch::Arrowhead::CIRCLE,
        Head::CircleOutline => sketch::Arrowhead::CIRCLE_OUTLINE,
        Head::Triangle => sketch::Arrowhead::TRIANGLE,
        Head::TriangleOutline => sketch::Arrowhead::TRIANGLE_OUTLINE,
        Head::Diamond => sketch::Arrowhead::DIAMOND,
        Head::DiamondOutline => sketch::Arrowhead::DIAMOND_OUTLINE,
        // The cardinality markers are drawn by the painter, not by the sketch generator.
        Head::None
        | Head::CardinalityOne
        | Head::CardinalityMany
        | Head::CardinalityOneOrMany
        | Head::CardinalityExactlyOne
        | Head::CardinalityZeroOrOne
        | Head::CardinalityZeroOrMany => sketch::Arrowhead::NONE,
    }
}

fn roundness(e: &Element) -> sketch::Roundness {
    if e.style.corner_radius > 0.0 {
        sketch::Roundness::Radius(e.style.corner_radius)
    } else {
        sketch::Roundness::Sharp
    }
}

fn curve(p: PathType) -> sketch::Curve {
    match p {
        PathType::Round => sketch::Curve::Smooth,
        PathType::Sharp | PathType::Elbow => sketch::Curve::Straight,
    }
}

/// The sketch input of an element that is drawn by the sketch generator, or `None` for other kinds (text, math,
/// image, frame) and for highlighter strokes, which have their own path.
pub fn sketch_input(e: &Element) -> Option<sketch::SketchInput> {
    let (w, h) = (e.transform.w, e.transform.h);
    let shape = match &e.kind {
        ElementKind::Rectangle => sketch::SketchShape::Rect { width: w, height: h, roundness: roundness(e) },
        ElementKind::Diamond => sketch::SketchShape::Diamond { width: w, height: h, roundness: roundness(e) },
        ElementKind::Ellipse => sketch::SketchShape::Ellipse { width: w, height: h },
        ElementKind::Line(d) => {
            sketch::SketchShape::Polyline { points: d.points.clone(), curve: curve(d.path), closed: d.closed }
        }
        ElementKind::Arrow(d) => sketch::SketchShape::Arrow {
            points: d.points.clone(),
            curve: curve(d.path),
            start: arrowhead(d.start_head),
            end: arrowhead(d.end_head),
            head_size: head_size(d, e.style.stroke_width),
        },
        ElementKind::Freehand(f) if !f.highlighter => {
            // Pressures shape the width unless the stroke is uniform, and only one per point is usable.
            let usable = f.pressure_mode != PressureMode::Uniform && f.pressures.len() == f.points.len();
            let pressures = if usable { f.pressures.clone() } else { Vec::new() };
            sketch::SketchShape::Freehand {
                points: f.points.clone(),
                pressures,
                closed: f.closed,
                profile: sketch::PressureProfile::default(),
            }
        }
        _ => return None,
    };
    let fillable = match &e.kind {
        ElementKind::Rectangle | ElementKind::Diamond | ElementKind::Ellipse => true,
        ElementKind::Line(d) => d.closed,
        ElementKind::Freehand(f) => f.closed,
        _ => false,
    };
    Some(sketch::SketchInput::new(shape, params(e, fillable)))
}

/// The transform from an element's local frame (origin at the top-left of its box) to world space.
///
/// Only images keep their mirror flags; for every other kind the flags are ignored, because those kinds mirror
/// their geometry instead.
pub fn element_to_world(e: &Element) -> Affine {
    let t = &e.transform;
    if matches!(e.kind, ElementKind::Image(_)) { t.mirrored_frame() } else { t.frame() }.to_world()
}

/// Space the element's paint may take outside its box, per side.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Margin {
    /// Left.
    pub left: f64,
    /// Top.
    pub top: f64,
    /// Right.
    pub right: f64,
    /// Bottom.
    pub bottom: f64,
}

impl Margin {
    fn all(m: f64) -> Self {
        Self { left: m, top: m, right: m, bottom: m }
    }
}

fn sane(v: f64) -> f64 {
    if v.is_finite() { v.abs().min(1.0e9) } else { 0.0 }
}

/// How far the paint of `e` can reach outside its box, in its local frame. Conservative: the sketch jitter, the
/// stroke, arrow heads and text overhang are all included, so an index of [`paint_bounds`] never culls a visible
/// element.
pub fn paint_margin(e: &Element) -> Margin {
    let t = &e.transform;
    let (w, h) = (sane(t.w), sane(t.h));
    let diag = w.hypot(h);
    let sw = sane(e.style.stroke_width);
    let rough = e.style.sloppiness.roughness();
    let sketchy = if e.style.sloppiness == Sloppiness::Architect {
        0.0
    } else {
        2.0 * rough + rough * rough * diag / 200.0 + 1.0
    };
    match &e.kind {
        ElementKind::Rectangle | ElementKind::Diamond | ElementKind::Ellipse => Margin::all(sw / 2.0 + sketchy + 1.0),
        ElementKind::Line(d) | ElementKind::Arrow(d) => {
            let mut m = sw / 2.0 + sketchy + 1.0;
            if d.path == PathType::Round {
                m += 0.2 * diag;
            }
            let heads =
                matches!(e.kind, ElementKind::Arrow(_)) && (d.start_head != Head::None || d.end_head != Head::None);
            if heads {
                m += sane(head_size(d, sw)) * HEAD_REACH;
            }
            Margin::all(m)
        }
        ElementKind::Freehand(f) => {
            let ink = if f.highlighter { sw } else { sw * 1.2 + 0.2 * sw };
            Margin::all(ink + sketchy + 1.0)
        }
        ElementKind::Text(td) => {
            let size = sane(td.font.size);
            Margin::all(0.3 * size + 1.0)
        }
        ElementKind::Math(m) => Margin::all(0.15 * sane(m.font_size) + 1.0),
        ElementKind::Image(_) => Margin::all(sw / 2.0 + 1.0),
        // The label is drawn by the composer at a constant size on screen: size the margin for the lowest zoom.
        ElementKind::Frame(_) => Margin {
            left: sw / 2.0 + 1.0,
            top: sw / 2.0 + 1.0 + FRAME_LABEL_ROOM / FRAME_LABEL_MIN_ZOOM,
            right: sw / 2.0 + 1.0 + (FRAME_LABEL_MIN_WIDTH / FRAME_LABEL_MIN_ZOOM - w).max(0.0),
            bottom: sw / 2.0 + 1.0,
        },
    }
}

/// The axis-aligned box of `local` after `to_world`.
pub(crate) fn world_aabb(to_world: Affine, local: Rect) -> Rect {
    let pts = [
        to_world * kurbo::Point::new(local.x0, local.y0),
        to_world * kurbo::Point::new(local.x1, local.y0),
        to_world * kurbo::Point::new(local.x1, local.y1),
        to_world * kurbo::Point::new(local.x0, local.y1),
    ];
    let mut r = Rect::new(pts[0].x, pts[0].y, pts[0].x, pts[0].y);
    for p in &pts[1..] {
        r = r.union_pt(*p);
    }
    r
}

/// The world-space bounding box of everything `e` can paint. This is what the spatial index handed to
/// [`crate::SceneComposer`] holds. It is not a hit-test box.
pub fn paint_bounds(e: &Element) -> Rect {
    let m = paint_margin(e);
    let t = &e.transform;
    let local = Rect::new(-m.left, -m.top, sane(t.w) + m.right, sane(t.h) + m.bottom);
    world_aabb(element_to_world(e), local)
}
