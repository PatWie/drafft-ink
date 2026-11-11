//! Geometry for arrow binding and elbow routing: headings, fixed points, offset outlines.
//!
//! These are the pure pieces of Excalidraw's binding and elbow algorithms (`binding.ts`,
//! `heading.ts`, `utils.ts`, `elbowArrow.ts`). The solver that decides what to bind and when
//! lives in the edit layer; it calls these. Constants and tie-breaking follow Excalidraw.
//!
//! A bind target is a rectangle, ellipse or diamond with an [`OrientedBox`] frame. Its *fixed
//! point* is a position inside (or just outside) the unrotated box, as a ratio of the box size.
//! The *orbit* end of an arrow sits where the segment from the fixed point to the other end of
//! the arrow crosses the target outline pushed outward by the binding gap.

use kurbo::{Point, Rect, Vec2};

use crate::aabb;
use crate::obox::OrientedBox;
use crate::outline::{Outline, Primitive, Probe, unit};
use crate::shapes::{DiamondShape, EllipseShape, RectShape};

/// Gap between an arrow end and the outline of the shape it is bound to (scene units).
pub const BASE_BINDING_GAP: f64 = 5.0;
/// Reach for binding at zoom 1 or above (scene units).
pub const BASE_BINDING_DISTANCE: f64 = 15.0;
/// An arrow shorter than this has its end snapped to the focus point to avoid inversion.
pub const BASE_ARROW_MIN_LENGTH: f64 = 10.0;
/// Fixed point ratios are clamped to plus or minus this.
pub const FIXED_POINT_BOUND: f64 = 10.0;
/// A target thinner than this on either side gets the centre as its fixed point.
pub const MIN_BINDABLE_SIZE: f64 = 1.0;
/// Padding around bound shapes when routing elbow arrows (scene units).
pub const ELBOW_BASE_PADDING: f64 = 40.0;

/// The gap for a target with the given stroke width: `5 + stroke / 2`.
pub fn binding_gap(stroke_width: f64) -> f64 {
    BASE_BINDING_GAP + if stroke_width.is_finite() { stroke_width.max(0.0) } else { 0.0 } / 2.0
}

/// How far from a shape's outline a dragged arrow end still binds, in scene units:
/// `clamp(15 / (min(zoom, 1) * 1.5), 15, 30)`.
pub fn max_binding_distance(zoom: f64) -> f64 {
    let z = if zoom.is_finite() && zoom > 0.0 && zoom < 1.0 { zoom } else { 1.0 };
    (BASE_BINDING_DISTANCE / (z * 1.5)).clamp(BASE_BINDING_DISTANCE, BASE_BINDING_DISTANCE * 2.0)
}

/// Clamp fixed point ratios to the allowed range and move an exact 0.5 to 0.5001, so that the
/// side an arrow leaves from does not flip on rounding noise. Non-finite input gives the centre.
pub fn normalize_fixed_point(fp: Vec2) -> Vec2 {
    if !fp.x.is_finite() || !fp.y.is_finite() {
        return Vec2::new(0.5001, 0.5001);
    }
    let f = |v: f64| {
        let v = v.clamp(-FIXED_POINT_BOUND, FIXED_POINT_BOUND);
        if (v - 0.5).abs() < 1e-4 { 0.5001 } else { v }
    };
    Vec2::new(f(fp.x), f(fp.y))
}

// ---------------------------------------------------------------------------------------------
// Headings
// ---------------------------------------------------------------------------------------------

/// One of the four axis directions an elbow arrow can leave or enter a shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Heading {
    /// Towards negative y (up on screen).
    Up,
    /// Towards positive x.
    Right,
    /// Towards positive y (down on screen).
    Down,
    /// Towards negative x.
    Left,
}

impl Heading {
    /// All four headings, clockwise from [`Heading::Up`].
    pub const ALL: [Heading; 4] = [Heading::Up, Heading::Right, Heading::Down, Heading::Left];

    /// The unit vector (y points down).
    pub fn vector(self) -> Vec2 {
        match self {
            Heading::Up => Vec2::new(0.0, -1.0),
            Heading::Right => Vec2::new(1.0, 0.0),
            Heading::Down => Vec2::new(0.0, 1.0),
            Heading::Left => Vec2::new(-1.0, 0.0),
        }
    }

    /// The heading closest to a vector. Ties between a horizontal and a vertical heading go to
    /// `Up` (when `y` is negative or the vector is on a diagonal with `y <= x`), as in Excalidraw.
    pub fn from_vector(v: Vec2) -> Heading {
        let (ax, ay) = (v.x.abs(), v.y.abs());
        if v.x > ay {
            Heading::Right
        } else if v.x <= -ay {
            Heading::Left
        } else if v.y > ax {
            Heading::Down
        } else {
            Heading::Up
        }
    }

    /// The heading of the vector from `origin` to `p`.
    pub fn from_points(p: Point, origin: Point) -> Heading {
        Heading::from_vector(p - origin)
    }

    /// The opposite heading.
    pub fn flip(self) -> Heading {
        match self {
            Heading::Up => Heading::Down,
            Heading::Down => Heading::Up,
            Heading::Left => Heading::Right,
            Heading::Right => Heading::Left,
        }
    }

    /// True for [`Heading::Left`] and [`Heading::Right`].
    pub fn is_horizontal(self) -> bool {
        matches!(self, Heading::Left | Heading::Right)
    }

    /// True for [`Heading::Up`] and [`Heading::Down`].
    pub fn is_vertical(self) -> bool {
        !self.is_horizontal()
    }
}

/// Padding offsets `[top, right, bottom, left]` for a box around a point that leaves in
/// `heading`: `head` on the side it leaves from, `side` on the other three.
pub fn offset_from_heading(heading: Heading, head: f64, side: f64) -> [f64; 4] {
    match heading {
        Heading::Up => [head, side, side, side],
        Heading::Right => [side, head, side, side],
        Heading::Down => [side, side, head, side],
        Heading::Left => [side, side, side, head],
    }
}

/// Add `[top, right, bottom, left]` offsets to a rect.
pub fn grow_rect(r: Rect, offsets: [f64; 4]) -> Rect {
    Rect::new(r.x0 - offsets[3], r.y0 - offsets[0], r.x1 + offsets[1], r.y1 + offsets[2])
}

/// Which outline a bind target has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BindKind {
    /// A rectangle, possibly with rounded corners.
    Rect,
    /// An ellipse inscribed in the box.
    Ellipse,
    /// A diamond whose vertices are the midpoints of the box sides.
    Diamond,
}

/// A shape an arrow can bind to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BindTarget {
    /// Which outline the target has.
    pub kind: BindKind,
    /// The oriented box of the target.
    pub frame: OrientedBox,
    /// Corner radius of a rectangle or diamond. Ignored for an ellipse.
    pub corner_radius: f64,
}

impl BindTarget {
    /// A target of `kind` in `frame` with sharp corners.
    pub fn new(kind: BindKind, frame: OrientedBox) -> Self {
        Self { kind, frame, corner_radius: 0.0 }
    }

    /// The same target with the given corner radius.
    pub fn with_corner_radius(mut self, r: f64) -> Self {
        self.corner_radius = r;
        self
    }

    /// True when the frame and the corner radius are usable numbers.
    pub fn is_valid(&self) -> bool {
        self.frame.is_valid() && crate::num_is_sane(self.corner_radius)
    }

    /// The outline pushed outward by `offset` (0 for the shape itself).
    pub fn shape(&self, offset: f64) -> Primitive {
        match self.kind {
            BindKind::Rect => {
                RectShape::new(self.frame).with_corner_radius(self.corner_radius).with_offset(offset).into()
            }
            BindKind::Ellipse => EllipseShape::new(self.frame).with_offset(offset).into(),
            BindKind::Diamond => {
                DiamondShape::new(self.frame).with_corner_radius(self.corner_radius).with_offset(offset).into()
            }
        }
    }

    /// The centre of the target.
    pub fn center(&self) -> Point {
        self.frame.center
    }

    /// False for targets too small to bind to (a side under [`MIN_BINDABLE_SIZE`]).
    pub fn is_bindable_size(&self) -> bool {
        self.frame.size.width >= MIN_BINDABLE_SIZE && self.frame.size.height >= MIN_BINDABLE_SIZE
    }

    /// Is the point inside the shape (the shape treated as filled)?
    pub fn contains(&self, p: Point) -> bool {
        self.is_valid() && self.shape(0.0).contains(p)
    }

    /// Distance to the outline: negative inside, positive outside. Excalidraw's
    /// `distanceToElement` has the opposite sign convention for the inside.
    pub fn signed_distance(&self, p: Point) -> f64 {
        if !self.is_valid() { f64::INFINITY } else { self.shape(0.0).signed_distance(p) }
    }

    /// Is `p` within the binding reach of the target: inside it, or no further than `reach` from
    /// its outline?
    pub fn within_reach(&self, p: Point, reach: f64) -> bool {
        self.is_valid() && self.signed_distance(p) <= reach.max(0.0)
    }

    /// The world position of a fixed point ratio: the ratio applied to the unrotated box, then
    /// rotated about the centre. The ratio is normalised first.
    pub fn global_fixed_point(&self, ratio: Vec2) -> Point {
        let r = normalize_fixed_point(ratio);
        let h = self.frame.half();
        self.frame.to_world(Point::new(-h.x + 2.0 * h.x * r.x, -h.y + 2.0 * h.y * r.y))
    }

    /// The fixed point ratio of a world point. Each ratio divides by `max(side, gap)` so tiny
    /// targets stay bounded (Excalidraw). For targets larger than `gap` this is the exact inverse
    /// of [`BindTarget::global_fixed_point`]. Targets under [`MIN_BINDABLE_SIZE`] give the centre.
    pub fn fixed_point_from_global(&self, p: Point, gap: f64) -> Vec2 {
        if !self.is_valid() || !self.is_bindable_size() || !crate::point_is_finite(p) {
            return Vec2::new(0.5001, 0.5001);
        }
        let h = self.frame.half();
        let l = self.frame.to_local(p);
        let gap = if gap.is_finite() { gap.max(0.0) } else { 0.0 };
        let (w, hh) = ((h.x * 2.0).max(gap), (h.y * 2.0).max(gap));
        normalize_fixed_point(Vec2::new((l.x + h.x) / w, (l.y + h.y) / hh))
    }

    /// The four side midpoints (diamond: its vertices, moved onto the outline when rounded):
    /// top, right, bottom, left.
    pub fn side_midpoints(&self) -> [Point; 4] {
        let m = self.frame.side_midpoints();
        if self.kind == BindKind::Diamond && self.corner_radius > 0.0 && self.is_valid() {
            let s = self.shape(0.0);
            return m.map(|v| s.nearest(v).map_or(v, |n| n.point));
        }
        m
    }

    /// The midpoint, if any, a dragged end at `p` should snap to for a simple (non-elbow) arrow:
    /// the first side midpoint within `threshold` of `p`, when `p` is outside the shape.
    pub fn snap_side_midpoint(&self, p: Point, threshold: f64) -> Option<Point> {
        if !self.is_valid() || !crate::point_is_finite(p) || self.contains(p) {
            return None;
        }
        // Excalidraw's order is right, bottom, left, top.
        let [top, right, bottom, left] = self.side_midpoints();
        [right, bottom, left, top].into_iter().find(|m| (*m - p).hypot() <= threshold)
    }

    /// The two diagonals used to project a pointer onto the shape, as world segments. For a
    /// rectangle they are the corner diagonals shrunk by 15 units at both ends; for an ellipse or
    /// diamond they are the two centre lines.
    pub fn diagonals(&self) -> [(Point, Point); 2] {
        let h = self.frame.half();
        let f = &self.frame;
        let (a, b) = match self.kind {
            BindKind::Rect => {
                ((Point::new(-h.x, -h.y), Point::new(h.x, h.y)), (Point::new(h.x, -h.y), Point::new(-h.x, h.y)))
            }
            _ => ((Point::new(0.0, -h.y), Point::new(0.0, h.y)), (Point::new(-h.x, 0.0), Point::new(h.x, 0.0))),
        };
        let shrink = if self.kind == BindKind::Rect { 15.0 } else { 0.0 };
        let seg = |(p, q): (Point, Point)| {
            let v = unit(q - p) * shrink;
            (f.to_world(p + v), f.to_world(q - v))
        };
        [seg(a), seg(b)]
    }

    /// Project the pointer onto the shape's diagonals, for an orbit anchor.
    ///
    /// `a` is the reference end: the arrow point next to the dragged end, or the other end's focus
    /// point. The ray from `a` through `p` is intersected with both diagonals; the crossing
    /// nearest to `a` that lies inside the shape wins. `None` when the ray misses.
    pub fn project_onto_diagonals(&self, p: Point, a: Point) -> Option<Point> {
        if !self.is_valid() || !crate::point_is_finite(p) || !crate::point_is_finite(a) {
            return None;
        }
        let dist = (p - a).hypot();
        if dist == 0.0 {
            return None;
        }
        let diagonals = self.diagonals();
        let longest = diagonals.iter().map(|(q, r)| (*r - *q).hypot()).fold(0.0, f64::max);
        let dir = (p - a) / dist;
        let reach = 2.0 * dist + longest;
        let mut best: Option<(f64, Point)> = None;
        for (q, r) in diagonals {
            let seg = Probe::segment(q, r);
            if !seg.is_valid() {
                continue;
            }
            // Intersect the finite ray with the diagonal segment.
            let e = r - q;
            let denom = dir.x * e.y - dir.y * e.x;
            if denom.abs() < 1e-12 {
                continue;
            }
            let qa = q - a;
            let t = (qa.x * e.y - qa.y * e.x) / denom;
            let s = (qa.x * dir.y - qa.y * dir.x) / denom;
            if (0.0..=reach).contains(&t) && (0.0..=1.0).contains(&s) {
                let hit = a + dir * t;
                if best.is_none_or(|(d, _)| t < d) {
                    best = Some((t, hit));
                }
            }
        }
        best.map(|(_, hit)| hit).filter(|hit| self.contains(*hit))
    }

    /// Crossings of the segment `a`-`b` with the outline pushed out by `offset`, nearest to `a` first.
    pub fn intersect_segment(&self, a: Point, b: Point, offset: f64) -> Vec<Point> {
        if !self.is_valid() {
            return Vec::new();
        }
        let probe = Probe::segment(a, b);
        if !probe.is_valid() {
            return Vec::new();
        }
        self.shape(offset).intersect(&probe).into_iter().map(|h| h.point).collect()
    }

    /// Where an orbit-bound arrow end sits: the crossing of the segment `focus` to `other_ref`
    /// with the outline offset by `gap`, nearest to `other_ref`. `None` when the segment misses
    /// the offset outline (callers then fall back to `focus`).
    pub fn orbit_point(&self, focus: Point, other_ref: Point, gap: f64) -> Option<Point> {
        let mut hits = self.intersect_segment(focus, other_ref, gap);
        hits.sort_by(|p, q| (*p - other_ref).hypot2().total_cmp(&(*q - other_ref).hypot2()));
        hits.into_iter().next()
    }

    /// The point on the outline pushed out by `gap` that is closest to `p`.
    pub fn nearest_on_offset_outline(&self, p: Point, gap: f64) -> Option<Point> {
        if !self.is_valid() {
            return None;
        }
        self.shape(gap).nearest(p).map(|n| n.point)
    }

    /// The axis-aligned bounds of the rotated target with `[top, right, bottom, left]` offsets.
    pub fn aabb_with_offsets(&self, offsets: [f64; 4]) -> Rect {
        grow_rect(self.shape(0.0).bounds(), offsets)
    }

    /// If `p` lies in a corner zone of a rectangle (outside both the left/right and top/bottom
    /// ranges), move it to the nearer side, `gap` outside the edge, so an elbow arrow does not
    /// leave a corner diagonally. Other points and other shapes are returned unchanged.
    pub fn avoid_rectangular_corner(&self, p: Point, gap: f64) -> Point {
        if self.kind != BindKind::Rect || !self.is_valid() || !crate::point_is_finite(p) {
            return p;
        }
        let h = self.frame.half();
        let l = self.frame.to_local(p);
        let (left, right, top, bottom) = (-h.x, h.x, -h.y, h.y);
        let moved = if l.x < left && l.y < top {
            if l.y - top > -gap { Point::new(left - gap, top) } else { Point::new(left, top - gap) }
        } else if l.x < left && l.y > bottom {
            if l.x - left > -gap { Point::new(left, bottom + gap) } else { Point::new(left - gap, bottom) }
        } else if l.x > right && l.y > bottom {
            if l.x - right < gap { Point::new(right, bottom + gap) } else { Point::new(right + gap, bottom) }
        } else if l.x > right && l.y < top {
            if l.x - right < gap { Point::new(right, top - gap) } else { Point::new(right + gap, top) }
        } else {
            return p;
        };
        self.frame.to_world(moved)
    }

    /// The heading an elbow arrow takes when it leaves the target towards `p`.
    ///
    /// `aabb` is the axis-aligned box of the rotated target, usually grown by the binding
    /// distance. The box is scaled by 2 about its centre and split into four cones by its
    /// diagonals; the cone containing `p` gives the heading (default `Left`). Diamonds use their
    /// own cones around the vertices and sides.
    pub fn heading_for_point(&self, p: Point, aabb: Rect) -> Heading {
        if self.kind == BindKind::Diamond {
            return self.diamond_heading(p, aabb);
        }
        let mid = aabb::center(&aabb);
        let scale = |x: f64, y: f64| Point::new(mid.x + (x - mid.x) * 2.0, mid.y + (y - mid.y) * 2.0);
        let (tl, tr) = (scale(aabb.x0, aabb.y0), scale(aabb.x1, aabb.y0));
        let (bl, br) = (scale(aabb.x0, aabb.y1), scale(aabb.x1, aabb.y1));
        if in_triangle([tl, tr, mid], p) {
            Heading::Up
        } else if in_triangle([tr, br, mid], p) {
            Heading::Right
        } else if in_triangle([br, bl, mid], p) {
            Heading::Down
        } else {
            Heading::Left
        }
    }

    fn diamond_heading(&self, p: Point, aabb: Rect) -> Heading {
        let mid = aabb::center(&aabb);
        let [top0, right0, bottom0, left0] = self.frame.side_midpoints();
        let shrink = |v: Point| mid + (v - mid) * 0.95;
        let (top, right, bottom, left) = (shrink(top0), shrink(right0), shrink(bottom0), shrink(left0));
        let cross = |a: Vec2, b: Vec2| a.x * b.y - a.y * b.x;
        let v = |a: Point, b: Point| a - b;

        // Corner cones.
        if cross(v(p, top), v(top, right)) <= 0.0 && cross(v(p, top), v(top, left)) > 0.0 {
            return Heading::from_points(top, mid);
        }
        if cross(v(p, right), v(right, bottom)) <= 0.0 && cross(v(p, right), v(right, top)) > 0.0 {
            return Heading::from_points(right, mid);
        }
        if cross(v(p, bottom), v(bottom, left)) <= 0.0 && cross(v(p, bottom), v(bottom, right)) > 0.0 {
            return Heading::from_points(bottom, mid);
        }
        if cross(v(p, left), v(left, top)) <= 0.0 && cross(v(p, left), v(left, bottom)) > 0.0 {
            return Heading::from_points(left, mid);
        }
        // Side cones: a wide diamond exits up or down, a tall one left or right.
        let wide = self.frame.size.width > self.frame.size.height;
        let pm = v(p, mid);
        if cross(pm, v(top, mid)) <= 0.0 && cross(pm, v(right, mid)) > 0.0 {
            return Heading::from_points(if wide { top } else { right }, mid);
        }
        if cross(pm, v(right, mid)) <= 0.0 && cross(pm, v(bottom, mid)) > 0.0 {
            return Heading::from_points(if wide { bottom } else { right }, mid);
        }
        if cross(pm, v(bottom, mid)) <= 0.0 && cross(pm, v(left, mid)) > 0.0 {
            return Heading::from_points(if wide { bottom } else { left }, mid);
        }
        Heading::from_points(if wide { top } else { left }, mid)
    }
}

/// Is `p` in the triangle, edges included? (Excalidraw's `triangleIncludesPoint`.)
fn in_triangle(t: [Point; 3], p: Point) -> bool {
    let sign = |a: Point, b: Point, c: Point| (a.x - c.x) * (b.y - c.y) - (b.x - c.x) * (a.y - c.y);
    let d1 = sign(p, t[0], t[1]);
    let d2 = sign(p, t[1], t[2]);
    let d3 = sign(p, t[2], t[0]);
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

#[cfg(test)]
mod tests {
    use std::f64::consts::FRAC_PI_2;

    use kurbo::Size;

    use super::*;

    fn close(a: Point, b: Point) -> bool {
        (a - b).hypot() < 1e-9
    }

    fn rect_target(x: f64, y: f64, w: f64, h: f64, angle: f64) -> BindTarget {
        BindTarget::new(BindKind::Rect, OrientedBox::from_xywh(x, y, w, h, angle))
    }

    #[test]
    fn constants() {
        assert_eq!((BASE_BINDING_GAP, BASE_ARROW_MIN_LENGTH, FIXED_POINT_BOUND), (5.0, 10.0, 10.0));
        assert_eq!(binding_gap(2.0), 6.0);
        assert_eq!(binding_gap(f64::NAN), 5.0);
    }

    #[test]
    fn max_distance_by_zoom() {
        assert_eq!(max_binding_distance(2.0), 15.0);
        assert_eq!(max_binding_distance(1.0), 15.0);
        assert_eq!(max_binding_distance(0.5), 20.0);
        assert_eq!(max_binding_distance(0.25), 30.0);
        assert_eq!(max_binding_distance(0.01), 30.0);
        assert_eq!(max_binding_distance(f64::NAN), 15.0);
        assert_eq!(max_binding_distance(-1.0), 15.0);
    }

    #[test]
    fn fixed_point_normalisation() {
        assert_eq!(normalize_fixed_point(Vec2::new(0.5, 0.2)), Vec2::new(0.5001, 0.2));
        assert_eq!(normalize_fixed_point(Vec2::new(50.0, -50.0)), Vec2::new(10.0, -10.0));
        assert_eq!(normalize_fixed_point(Vec2::new(f64::NAN, 0.0)), Vec2::new(0.5001, 0.5001));
        assert_eq!(normalize_fixed_point(Vec2::new(0.50005, 0.9)), Vec2::new(0.5001, 0.9));
    }

    #[test]
    fn heading_vectors_and_ties() {
        assert_eq!(Heading::from_vector(Vec2::new(5.0, 1.0)), Heading::Right);
        assert_eq!(Heading::from_vector(Vec2::new(-5.0, 1.0)), Heading::Left);
        assert_eq!(Heading::from_vector(Vec2::new(1.0, 5.0)), Heading::Down);
        assert_eq!(Heading::from_vector(Vec2::new(1.0, -5.0)), Heading::Up);
        // Diagonals: x == y goes to Up (Excalidraw), x == -y goes to Left? check both axes.
        assert_eq!(Heading::from_vector(Vec2::new(3.0, 3.0)), Heading::Up);
        assert_eq!(Heading::from_vector(Vec2::new(-3.0, 3.0)), Heading::Left);
        assert_eq!(Heading::from_vector(Vec2::new(3.0, -3.0)), Heading::Up);
        assert_eq!(Heading::from_vector(Vec2::ZERO), Heading::Left);
        for h in Heading::ALL {
            assert_eq!(h.flip().flip(), h);
            assert_eq!(Heading::from_vector(h.vector()), h);
            assert_eq!(h.is_horizontal(), !h.is_vertical());
        }
        assert_eq!(Heading::from_vector(Vec2::new(f64::NAN, f64::NAN)), Heading::Up);
        assert_eq!(Heading::from_points(Point::new(10.0, 0.0), Point::ZERO), Heading::Right);
    }

    #[test]
    fn offsets() {
        assert_eq!(offset_from_heading(Heading::Up, 12.0, 1.0), [12.0, 1.0, 1.0, 1.0]);
        assert_eq!(offset_from_heading(Heading::Left, 12.0, 1.0), [1.0, 1.0, 1.0, 12.0]);
        assert_eq!(grow_rect(Rect::new(0.0, 0.0, 10.0, 10.0), [1.0, 2.0, 3.0, 4.0]), Rect::new(-4.0, -1.0, 12.0, 13.0));
    }

    #[test]
    fn global_fixed_point_rotates_with_the_target() {
        let t = rect_target(0.0, 0.0, 100.0, 50.0, 0.0);
        assert!(close(t.global_fixed_point(Vec2::new(0.0, 0.0)), Point::new(0.0, 0.0)));
        assert!(close(t.global_fixed_point(Vec2::new(1.0, 1.0)), Point::new(100.0, 50.0)));
        let t = rect_target(0.0, 0.0, 100.0, 50.0, FRAC_PI_2);
        // centre (50,25); the local top-left (-50,-25) turns a quarter turn to (25,-50)
        assert!(close(t.global_fixed_point(Vec2::new(0.0, 0.0)), Point::new(75.0, -25.0)));
        // 0.5 is nudged to 0.5001 so the centre is 0.0001 * size off.
        let c = t.global_fixed_point(Vec2::new(0.5, 0.5));
        assert!((c - Point::new(50.0, 25.0)).hypot() < 0.02);
    }

    #[test]
    fn fixed_point_round_trip() {
        let t = rect_target(10.0, 20.0, 100.0, 50.0, 0.7);
        for r in [Vec2::new(0.2, 0.8), Vec2::new(1.1, -0.1), Vec2::new(0.3, 0.3)] {
            let p = t.global_fixed_point(r);
            let back = t.fixed_point_from_global(p, 6.0);
            assert!((back - r).hypot() < 1e-9, "{r:?} -> {back:?}");
        }
        let tiny = rect_target(0.0, 0.0, 0.5, 50.0, 0.0);
        assert_eq!(tiny.fixed_point_from_global(Point::new(3.0, 3.0), 5.0), Vec2::new(0.5001, 0.5001));
    }

    #[test]
    fn orbit_point_sits_on_the_gap_outline() {
        let t = rect_target(0.0, 0.0, 100.0, 100.0, 0.0);
        // focus at the centre, other end far to the right: exit at x = 100 + gap
        let p = t.orbit_point(Point::new(50.0, 50.0), Point::new(400.0, 50.0), 5.0).unwrap();
        assert!(close(p, Point::new(105.0, 50.0)));
        // the segment stops short of the outline: no orbit point
        assert!(t.orbit_point(Point::new(50.0, 50.0), Point::new(103.0, 50.0), 5.0).is_none());
        // rotated target: still on the offset outline
        let r = rect_target(0.0, 0.0, 100.0, 100.0, 0.5);
        let p = r.orbit_point(Point::new(50.0, 50.0), Point::new(400.0, 50.0), 5.0).unwrap();
        assert!((r.signed_distance(p) - 5.0).abs() < 1e-9);
    }

    #[test]
    fn orbit_on_ellipse_and_diamond() {
        let e = BindTarget::new(BindKind::Ellipse, OrientedBox::new(Point::ZERO, Size::new(100.0, 50.0), 0.0));
        let p = e.orbit_point(Point::ZERO, Point::new(500.0, 0.0), 5.0).unwrap();
        assert!(close(p, Point::new(55.0, 0.0)));
        let d = BindTarget::new(BindKind::Diamond, OrientedBox::new(Point::ZERO, Size::new(100.0, 100.0), 0.0));
        let p = d.orbit_point(Point::ZERO, Point::new(500.0, 0.0), 5.0).unwrap();
        assert!(close(p, Point::new(55.0, 0.0)));
        let p = d.orbit_point(Point::ZERO, Point::new(500.0, 500.0), 5.0).unwrap();
        // 45 degrees: the diamond edge is at 25*sqrt(2) from the centre... distance from edge is 5
        assert!((d.signed_distance(p) - 5.0).abs() < 1e-9);
    }

    #[test]
    fn distance_sign_and_reach() {
        let t = rect_target(0.0, 0.0, 100.0, 100.0, 0.0);
        assert!(t.signed_distance(Point::new(50.0, 50.0)) < 0.0);
        assert!((t.signed_distance(Point::new(110.0, 50.0)) - 10.0).abs() < 1e-9);
        assert!(t.within_reach(Point::new(110.0, 50.0), 15.0));
        assert!(!t.within_reach(Point::new(120.0, 50.0), 15.0));
        assert!(t.within_reach(Point::new(50.0, 50.0), 0.0), "inside counts");
        assert!(!t.within_reach(Point::new(f64::NAN, 0.0), 15.0));
    }

    #[test]
    fn midpoint_snap_only_outside() {
        let t = rect_target(0.0, 0.0, 100.0, 100.0, 0.0);
        assert!(close(t.snap_side_midpoint(Point::new(112.0, 48.0), 15.0).unwrap(), Point::new(100.0, 50.0)));
        assert!(t.snap_side_midpoint(Point::new(95.0, 50.0), 15.0).is_none(), "inside the shape");
        assert!(t.snap_side_midpoint(Point::new(160.0, 50.0), 15.0).is_none());
    }

    #[test]
    fn diagonal_projection() {
        let t = rect_target(0.0, 0.0, 200.0, 100.0, 0.0);
        // other end far left at the same height; pointer inside the shape up and to the right
        let p = t.project_onto_diagonals(Point::new(150.0, 30.0), Point::new(-100.0, 50.0));
        let p = p.expect("hits a diagonal");
        assert!(t.contains(p));
        // collinear with a and the pointer
        let a = Point::new(-100.0, 50.0);
        let c = (Point::new(150.0, 30.0) - a).cross(p - a);
        assert!(c.abs() < 1e-6);
        assert!(t.project_onto_diagonals(Point::new(1.0, 1.0), Point::new(1.0, 1.0)).is_none());
    }

    #[test]
    fn avoid_corner() {
        let t = rect_target(0.0, 0.0, 100.0, 100.0, 0.0);
        // just outside the top-left corner, closer to the top: goes to the left side top
        let p = t.avoid_rectangular_corner(Point::new(-2.0, -3.0), 5.0);
        assert!(close(p, Point::new(-5.0, 0.0)), "{p:?}");
        // well inside a side zone: unchanged
        let q = Point::new(50.0, -4.0);
        assert_eq!(t.avoid_rectangular_corner(q, 5.0), q);
        let e = BindTarget::new(BindKind::Ellipse, t.frame);
        assert_eq!(e.avoid_rectangular_corner(Point::new(-2.0, -3.0), 5.0), Point::new(-2.0, -3.0));
    }

    #[test]
    fn rect_heading_cones() {
        let t = rect_target(0.0, 0.0, 100.0, 50.0, 0.0);
        let aabb = grow_rect(t.shape(0.0).bounds(), [15.0; 4]);
        assert_eq!(t.heading_for_point(Point::new(50.0, -30.0), aabb), Heading::Up);
        assert_eq!(t.heading_for_point(Point::new(150.0, 25.0), aabb), Heading::Right);
        assert_eq!(t.heading_for_point(Point::new(50.0, 90.0), aabb), Heading::Down);
        assert_eq!(t.heading_for_point(Point::new(-70.0, 25.0), aabb), Heading::Left);
        // Rotated by a quarter turn the bounding box is tall, cones follow the box.
        let r = rect_target(0.0, 0.0, 100.0, 50.0, FRAC_PI_2);
        let aabb = r.shape(0.0).bounds();
        assert_eq!(r.heading_for_point(Point::new(50.0, -60.0), aabb), Heading::Up);
    }

    #[test]
    fn diamond_heading() {
        let d = BindTarget::new(BindKind::Diamond, OrientedBox::new(Point::ZERO, Size::new(100.0, 100.0), 0.0));
        let aabb = grow_rect(d.shape(0.0).bounds(), [10.0; 4]);
        assert_eq!(d.heading_for_point(Point::new(0.0, -80.0), aabb), Heading::Up);
        assert_eq!(d.heading_for_point(Point::new(80.0, 0.0), aabb), Heading::Right);
        assert_eq!(d.heading_for_point(Point::new(0.0, 80.0), aabb), Heading::Down);
        assert_eq!(d.heading_for_point(Point::new(-80.0, 0.0), aabb), Heading::Left);
        // a wide diamond leaves a side hit up or down
        let w = BindTarget::new(BindKind::Diamond, OrientedBox::new(Point::ZERO, Size::new(200.0, 100.0), 0.0));
        let aabb = grow_rect(w.shape(0.0).bounds(), [10.0; 4]);
        let h = w.heading_for_point(Point::new(60.0, -45.0), aabb);
        assert!(h == Heading::Up || h == Heading::Right, "{h:?}");
    }

    #[test]
    fn intersect_segment_orders_and_handles_bad_input() {
        let t = rect_target(0.0, 0.0, 100.0, 100.0, 0.0);
        let hits = t.intersect_segment(Point::new(-50.0, 50.0), Point::new(150.0, 50.0), 0.0);
        assert_eq!(hits.len(), 2);
        assert!(hits[0].x < hits[1].x);
        assert!(t.intersect_segment(Point::new(f64::NAN, 0.0), Point::new(1.0, 1.0), 0.0).is_empty());
        assert!(t.intersect_segment(Point::ZERO, Point::ZERO, 0.0).is_empty());
        let bad =
            BindTarget::new(BindKind::Rect, OrientedBox::new(Point::new(f64::NAN, 0.0), Size::new(1.0, 1.0), 0.0));
        assert!(bad.orbit_point(Point::ZERO, Point::new(1.0, 1.0), 5.0).is_none());
        assert!(!bad.contains(Point::ZERO) && bad.signed_distance(Point::ZERO).is_infinite());
    }

    #[test]
    fn diagonals_are_shrunk_for_rects_only() {
        let r = rect_target(0.0, 0.0, 200.0, 100.0, 0.0);
        let [d1, _] = r.diagonals();
        let len = (d1.1 - d1.0).hypot();
        assert!((len - (200.0_f64.hypot(100.0) - 30.0)).abs() < 1e-9);
        let e = BindTarget::new(BindKind::Ellipse, r.frame);
        let [c1, c2] = e.diagonals();
        assert!((((c1.1 - c1.0).hypot()) - 100.0).abs() < 1e-9 && (((c2.1 - c2.0).hypot()) - 200.0).abs() < 1e-9);
    }
}
