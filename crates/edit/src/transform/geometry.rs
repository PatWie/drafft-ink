//! Element geometry in world space: frames, bounds and outlines for the structural rules.

use drafftink_geom::kurbo::{Point, Rect, Size};
use drafftink_geom::shapes::{DiamondShape, EllipseShape, Polyline, RectShape};
use drafftink_geom::{ElementFrame, OrientedBox, Primitive, aabb};
use drafftink_scene::{Element, ElementKind};

/// The oriented box of an element (centre, size, angle).
pub fn obox(e: &Element) -> OrientedBox {
    let t = &e.transform;
    OrientedBox::from_xywh(t.x, t.y, t.w, t.h, t.angle)
}

/// The element frame: where its local space (origin at the box top-left) sits in the world.
pub fn frame_of(e: &Element) -> ElementFrame {
    let t = &e.transform;
    ElementFrame::new(Point::new(t.x, t.y), Size::new(t.w, t.h), t.angle)
}

/// The vertices of a line, arrow or freehand in world space.
pub fn world_points(e: &Element) -> Option<Vec<Point>> {
    let pts = e.points()?;
    let a = frame_of(e).to_world();
    Some(pts.iter().map(|p| a * *p).collect())
}

/// The axis-aligned bounds of an element in world space, exact under rotation. Stroke width is not included.
/// A line, arrow or freehand uses its points; every other kind uses its rotated box.
pub fn world_bounds(e: &Element) -> Rect {
    if let Some(pts) = world_points(e)
        && let Some(r) = aabb::from_points(pts)
    {
        return r;
    }
    obox(e).aabb()
}

/// The union of the bounds of `elements`, or `None` when there are none.
pub fn union_bounds<'a>(elements: impl IntoIterator<Item = &'a Element>) -> Option<Rect> {
    aabb::union_all(elements.into_iter().map(world_bounds))
}

/// The outline of an element for hit and marquee tests. `None` for a line with no points.
pub fn primitive(e: &Element) -> Option<Primitive> {
    let b = obox(e);
    Some(match &e.kind {
        ElementKind::Ellipse => EllipseShape::new(b).into(),
        ElementKind::Diamond => DiamondShape::new(b).with_corner_radius(e.style.corner_radius).into(),
        ElementKind::Line(d) => Polyline::from_frame(&frame_of(e), &d.points, d.closed).into(),
        ElementKind::Arrow(d) => Polyline::from_frame(&frame_of(e), &d.points, false).into(),
        ElementKind::Freehand(d) => Polyline::from_frame(&frame_of(e), &d.points, d.closed).into(),
        _ => RectShape::new(b).with_corner_radius(e.style.corner_radius).into(),
    })
}

/// True when a click or marquee inside the outline selects the element. Frames only react on their outline.
pub fn is_filled(e: &Element) -> bool {
    match &e.kind {
        ElementKind::Frame(_) => false,
        ElementKind::Text(_) | ElementKind::Math(_) | ElementKind::Image(_) => true,
        _ => e.style.fill_color.is_some(),
    }
}
