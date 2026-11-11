//! From an element to its geometry: the exact outline used for hit tests, marquee tests and the spatial index.
//!
//! The geometry is in world space and ignores the stroke width (callers add half of it as reach). The kind decides
//! the shape:
//!
//! | Kind | Shape | Fill rule |
//! |---|---|---|
//! | rectangle, diamond, ellipse | the shape, rotated about the box centre | hit inside only when it has a visible fill |
//! | line, arrow | polyline through the points; `round` paths use the same Catmull-Rom spline as the sketch crate | inside only for a closed line with a visible fill |
//! | freehand | polyline through the points | inside only for a closed stroke with a visible fill |
//! | text, math, image | the rotated box | always the whole box |
//! | frame | the rotated box | border only, never the inside |

use drafftink_geom::kurbo::{Point, Rect, Size};
use drafftink_geom::spline::catmull_rom_path;
use drafftink_geom::{
    DiamondShape, ElementFrame, EllipseShape, OrientedBox, Outline, PathShape, Polyline, Primitive, RectShape,
};
use drafftink_scene::{Element, ElementKind, PathType};

/// Default size of an arrowhead in world units when the element does not set one.
pub const DEFAULT_HEAD_SIZE: f64 = 15.0;

/// The rotated box of an element (position, size, angle).
pub fn element_box(e: &Element) -> OrientedBox {
    let t = &e.transform;
    OrientedBox::from_xywh(t.x, t.y, t.w, t.h, t.angle)
}

fn element_frame(e: &Element) -> ElementFrame {
    let t = &e.transform;
    // Only images keep a mirror flag and a box is its own mirror image, so flips never change a hit test.
    ElementFrame::new(Point::new(t.x, t.y), Size::new(t.w, t.h), t.angle)
}

/// The world-space outline of an element. `None` when the element has no usable geometry (non-finite numbers, a
/// line without two points).
pub fn shape_of(e: &Element) -> Option<Primitive> {
    let b = element_box(e);
    let shape: Primitive = match &e.kind {
        ElementKind::Rectangle => RectShape::new(b).with_corner_radius(e.style.corner_radius.max(0.0)).into(),
        ElementKind::Diamond => DiamondShape::new(b).with_corner_radius(e.style.corner_radius.max(0.0)).into(),
        ElementKind::Ellipse => EllipseShape::new(b).into(),
        ElementKind::Text(_) | ElementKind::Math(_) | ElementKind::Image(_) | ElementKind::Frame(_) => {
            RectShape::new(b).into()
        }
        ElementKind::Line(d) | ElementKind::Arrow(d) => {
            if d.points.len() < 2 {
                return None;
            }
            if d.path == PathType::Round {
                PathShape::from_frame(&element_frame(e), &catmull_rom_path(&d.points, d.closed)).into()
            } else {
                Polyline::from_frame(&element_frame(e), &d.points, d.closed).into()
            }
        }
        ElementKind::Freehand(d) => {
            if d.points.is_empty() {
                return None;
            }
            Polyline::from_frame(&element_frame(e), &d.points, d.closed).into()
        }
    };
    shape.is_valid().then_some(shape)
}

/// True when a click inside the outline hits the element: the element has a visible fill and a closed outline, or
/// it is a box-like content kind (text, math, image).
pub fn hits_inside(e: &Element) -> bool {
    match &e.kind {
        ElementKind::Text(_) | ElementKind::Math(_) | ElementKind::Image(_) => true,
        ElementKind::Frame(_) => false,
        ElementKind::Rectangle | ElementKind::Diamond | ElementKind::Ellipse => has_fill(e),
        ElementKind::Line(d) | ElementKind::Arrow(d) => d.closed && has_fill(e),
        ElementKind::Freehand(d) => d.closed && has_fill(e),
    }
}

/// The element has a fill colour that is not fully transparent.
pub fn has_fill(e: &Element) -> bool {
    e.style.fill_color.is_some_and(|c| c.a > 0)
}

/// How far past the outline the element paints, beyond half the stroke width: arrowheads.
fn paint_margin(e: &Element) -> f64 {
    match &e.kind {
        ElementKind::Arrow(d) | ElementKind::Line(d) => {
            let heads = d.start_head != drafftink_scene::Head::None || d.end_head != drafftink_scene::Head::None;
            if heads { d.head_size.filter(|s| s.is_finite() && *s > 0.0).unwrap_or(DEFAULT_HEAD_SIZE) } else { 0.0 }
        }
        _ => 0.0,
    }
}

/// The world bounds of the outline, exact under rotation, without stroke. `None` for an element without geometry.
pub fn outline_bounds(e: &Element) -> Option<Rect> {
    shape_of(e).map(|s| s.bounds())
}

/// The bounds the spatial index stores: the outline bounds grown by half the stroke width and by arrowheads.
/// `None` for an element without geometry (it is not indexed and cannot be hit).
pub fn index_bounds(e: &Element) -> Option<Rect> {
    let shape = shape_of(e)?;
    let w = if e.style.stroke_width.is_finite() { e.style.stroke_width.max(0.0) } else { 0.0 };
    let r = shape.bounds().inflate(w * 0.5 + paint_margin(e), w * 0.5 + paint_margin(e));
    drafftink_geom::aabb::is_finite(&r).then_some(r)
}

/// The stroke width used as reach when hit testing: the element's, clamped to something finite.
pub fn stroke_reach_width(e: &Element) -> f64 {
    if e.style.stroke_width.is_finite() { e.style.stroke_width.clamp(0.0, 1e6) } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use drafftink_scene::{Color, ElementId, LinearData, ZIndex};

    fn id(s: &str) -> ElementId {
        ElementId::new(s).unwrap()
    }

    #[test]
    fn rotated_rect_bounds_are_exact() {
        let mut e = Element::new(id("a"), ElementKind::Rectangle, ZIndex::first()).with_bounds(0.0, 0.0, 100.0, 20.0);
        e.transform.angle = std::f64::consts::FRAC_PI_2;
        let b = outline_bounds(&e).unwrap();
        assert!((b.x0 - 40.0).abs() < 1e-9 && (b.x1 - 60.0).abs() < 1e-9, "{b:?}");
        assert!((b.y0 + 40.0).abs() < 1e-9 && (b.y1 - 60.0).abs() < 1e-9, "{b:?}");
    }

    #[test]
    fn index_bounds_include_stroke_and_heads() {
        let mut e = Element::linear(
            id("l"),
            drafftink_scene::Kind::Arrow,
            ZIndex::first(),
            &[Point::new(0.0, 0.0), Point::new(100.0, 0.0)],
        )
        .unwrap();
        e.style.stroke_width = 4.0;
        let plain = index_bounds(&e).unwrap();
        assert_eq!((plain.x0, plain.x1), (-2.0, 102.0));
        if let ElementKind::Arrow(d) = &mut e.kind {
            d.end_head = drafftink_scene::Head::Triangle;
        }
        let with_head = index_bounds(&e).unwrap();
        assert_eq!((with_head.x0, with_head.x1), (-17.0, 117.0));
    }

    #[test]
    fn fill_rules_per_kind() {
        let filled = |kind| {
            let mut e = Element::new(id("a"), kind, ZIndex::first()).with_bounds(0.0, 0.0, 10.0, 10.0);
            e.style.fill_color = Some(Color::rgb(1, 2, 3));
            e
        };
        let open = |kind| Element::new(id("a"), kind, ZIndex::first()).with_bounds(0.0, 0.0, 10.0, 10.0);
        assert!(hits_inside(&filled(ElementKind::Rectangle)));
        assert!(!hits_inside(&open(ElementKind::Rectangle)));
        assert!(hits_inside(&open(ElementKind::Text(drafftink_scene::TextData::new("x")))));
        assert!(!hits_inside(&filled(ElementKind::Frame(Default::default()))));
        let mut transparent = filled(ElementKind::Ellipse);
        transparent.style.fill_color = Some(Color::TRANSPARENT);
        assert!(!hits_inside(&transparent));
        let mut line = filled(ElementKind::Line(LinearData::new(vec![Point::ZERO, Point::new(5.0, 5.0)])));
        assert!(!hits_inside(&line), "an open line has no inside");
        if let ElementKind::Line(d) = &mut line.kind {
            d.closed = true;
        }
        assert!(hits_inside(&line));
    }

    #[test]
    fn degenerate_elements_have_no_shape() {
        let mut e = Element::new(id("a"), ElementKind::Rectangle, ZIndex::first());
        e.transform.x = f64::NAN;
        assert!(shape_of(&e).is_none() && index_bounds(&e).is_none());
        let line = Element::new(id("b"), ElementKind::Line(LinearData::new(vec![Point::ZERO])), ZIndex::first());
        assert!(shape_of(&line).is_none());
    }

    #[test]
    fn round_paths_follow_the_spline() {
        let mut e = Element::linear(
            id("l"),
            drafftink_scene::Kind::Line,
            ZIndex::first(),
            &[Point::new(0.0, 0.0), Point::new(50.0, 50.0), Point::new(100.0, 0.0)],
        )
        .unwrap();
        if let ElementKind::Line(d) = &mut e.kind {
            d.path = PathType::Round;
        }
        let s = shape_of(&e).unwrap();
        // The curve passes through the middle point and bulges less than the sharp corner would allow.
        assert!(s.outline_distance(Point::new(50.0, 50.0)) < 1e-6);
    }
}
