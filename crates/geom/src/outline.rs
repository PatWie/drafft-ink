//! Outline queries: the common interface of every primitive shape.
//!
//! A shape answers six questions: its bounds, whether it contains a point, how far a point is from
//! its outline, which outline point is closest, where a ray or segment crosses the outline, and
//! whether a pointer with some tolerance and stroke width hits it.
//!
//! Distances and tolerances are in world units. Callers convert screen pixels with the zoom.

use kurbo::{Point, Rect, Vec2};

use crate::aabb;
use crate::shapes::{DiamondShape, EllipseShape, PathShape, Polyline, RectShape};

/// A ray, segment or line in the form `origin + t * dir` with `t` in `[t_min, t_max]`.
///
/// `dir` need not be a unit vector; the returned `t` is in units of `dir`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Probe {
    pub origin: Point,
    pub dir: Vec2,
    pub t_min: f64,
    pub t_max: f64,
}

impl Probe {
    /// A ray from `origin` along `dir`.
    pub fn ray(origin: Point, dir: Vec2) -> Self {
        Self { origin, dir, t_min: 0.0, t_max: f64::INFINITY }
    }

    /// The segment from `a` to `b` (`t` in `[0, 1]`).
    pub fn segment(a: Point, b: Point) -> Self {
        Self { origin: a, dir: b - a, t_min: 0.0, t_max: 1.0 }
    }

    /// The infinite line through `origin` along `dir`.
    pub fn line(origin: Point, dir: Vec2) -> Self {
        Self { origin, dir, t_min: f64::NEG_INFINITY, t_max: f64::INFINITY }
    }

    /// The point at parameter `t`.
    #[inline]
    pub fn at(&self, t: f64) -> Point {
        self.origin + self.dir * t
    }

    /// True when the probe can be intersected: finite origin and direction, a direction that is
    /// not zero, and a non-empty parameter range.
    pub fn is_valid(&self) -> bool {
        crate::point_is_finite(self.origin)
            && crate::vec_is_finite(self.dir)
            && self.dir.hypot2() > 0.0
            && !self.t_min.is_nan()
            && !self.t_max.is_nan()
            && self.t_min <= self.t_max
    }

    /// The part of the probe inside `r` as a `t` range (Liang-Barsky), or `None`.
    pub fn clip_to_rect(&self, r: &Rect) -> Option<(f64, f64)> {
        if !self.is_valid() || !aabb::is_finite(r) {
            return None;
        }
        let r = aabb::normalize(*r);
        let (mut t0, mut t1) = (self.t_min, self.t_max);
        for (p, q) in [
            (-self.dir.x, self.origin.x - r.x0),
            (self.dir.x, r.x1 - self.origin.x),
            (-self.dir.y, self.origin.y - r.y0),
            (self.dir.y, r.y1 - self.origin.y),
        ] {
            if p == 0.0 {
                if q < 0.0 {
                    return None;
                }
            } else {
                let t = q / p;
                if p < 0.0 {
                    t0 = t0.max(t);
                } else {
                    t1 = t1.min(t);
                }
            }
        }
        (t0 <= t1).then_some((t0, t1))
    }
}

/// An intersection of a [`Probe`] with an outline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    pub point: Point,
    /// Parameter along the probe.
    pub t: f64,
    /// Unit normal of the outline at the hit. For closed shapes it points outward. For open
    /// shapes it is the left-hand normal of the segment direction.
    pub normal: Vec2,
}

/// The closest point of an outline to a query point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Nearest {
    pub point: Point,
    /// Unsigned distance from the query point to `point`.
    pub distance: f64,
    /// Unit normal at `point` (see [`Hit::normal`]).
    pub normal: Vec2,
}

/// How a pointer hits a shape.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HitOpts {
    /// The shape has a fill: the interior hits too. Stroke-only shapes hit near the outline only.
    pub filled: bool,
    /// Extra reach around the outline, in world units (typically `pixels / zoom`).
    pub tolerance: f64,
    /// The stroke width. Half of it is added to the reach.
    pub stroke_width: f64,
}

impl HitOpts {
    pub const fn stroke(tolerance: f64, stroke_width: f64) -> Self {
        Self { filled: false, tolerance, stroke_width }
    }

    pub const fn filled(tolerance: f64, stroke_width: f64) -> Self {
        Self { filled: true, tolerance, stroke_width }
    }

    /// The distance from the outline within which the stroke is hit.
    #[inline]
    pub fn reach(&self) -> f64 {
        let t = if self.tolerance.is_finite() { self.tolerance.max(0.0) } else { 0.0 };
        let w = if self.stroke_width.is_finite() { self.stroke_width.max(0.0) } else { 0.0 };
        t + w * 0.5
    }
}

/// The queries every primitive shape supports.
pub trait Outline {
    /// Axis-aligned bounds of the geometry (no stroke), exact under rotation. `Rect::ZERO` for an
    /// invalid or empty shape.
    fn bounds(&self) -> Rect;

    /// True for shapes that enclose an area.
    fn is_closed(&self) -> bool;

    /// True when the shape has finite numbers and can be queried.
    fn is_valid(&self) -> bool;

    /// Is the point inside the enclosed area (boundary included)? Always false for open shapes.
    fn contains(&self, p: Point) -> bool;

    /// The outline point closest to `p`. `None` for an invalid or empty shape or a bad point.
    fn nearest(&self, p: Point) -> Option<Nearest>;

    /// All crossings of the probe with the outline, sorted by `t`.
    fn intersect(&self, probe: &Probe) -> Vec<Hit>;

    /// Unsigned distance from `p` to the outline. Infinity when it is undefined.
    fn outline_distance(&self, p: Point) -> f64 {
        self.nearest(p).map_or(f64::INFINITY, |n| n.distance)
    }

    /// Distance to the outline, negative inside a closed shape. Infinity when undefined.
    fn signed_distance(&self, p: Point) -> f64 {
        let d = self.outline_distance(p);
        if self.is_closed() && self.contains(p) { -d } else { d }
    }

    /// Does a pointer at `p` hit the shape?
    fn hit_test(&self, p: Point, opts: HitOpts) -> bool {
        if opts.filled && self.is_closed() && self.contains(p) {
            return true;
        }
        self.outline_distance(p) <= opts.reach()
    }

    /// Bounds grown by half the stroke width. Miter joins are not accounted for.
    fn bounds_with_stroke(&self, stroke_width: f64) -> Rect {
        let w = if stroke_width.is_finite() { stroke_width.max(0.0) } else { 0.0 };
        aabb::inflate(self.bounds(), w * 0.5)
    }

    /// The crossing nearest to the start of the probe.
    fn first_hit(&self, probe: &Probe) -> Option<Hit> {
        self.intersect(probe).into_iter().next()
    }

    /// Does the shape touch the rect? With `filled` the interior counts, otherwise only the
    /// outline does. Used for marquee selection.
    fn intersects_rect(&self, rect: &Rect, filled: bool) -> bool {
        if !self.is_valid() || !aabb::is_finite(rect) {
            return false;
        }
        let rect = aabb::normalize(*rect);
        if !aabb::intersects(&self.bounds(), &rect) {
            return false;
        }
        let c = aabb::corners(&rect);
        for i in 0..4 {
            let probe = Probe::segment(c[i], c[(i + 1) % 4]);
            if probe.is_valid() && !self.intersect(&probe).is_empty() {
                return true;
            }
        }
        // No edge crossing: either the outline is inside the rect or the rect is inside the shape
        // (or they are apart).
        if self.nearest(aabb::center(&rect)).is_some_and(|n| aabb::contains_point(&rect, n.point)) {
            return true;
        }
        filled && self.is_closed() && self.contains(aabb::center(&rect))
    }
}

/// Any primitive shape, for dispatch without generics.
#[derive(Debug, Clone, PartialEq)]
pub enum Primitive {
    Rect(RectShape),
    Ellipse(EllipseShape),
    Diamond(DiamondShape),
    Polyline(Polyline),
    Path(PathShape),
}

macro_rules! dispatch {
    ($self:ident, $s:ident => $e:expr) => {
        match $self {
            Primitive::Rect($s) => $e,
            Primitive::Ellipse($s) => $e,
            Primitive::Diamond($s) => $e,
            Primitive::Polyline($s) => $e,
            Primitive::Path($s) => $e,
        }
    };
}

impl Outline for Primitive {
    fn bounds(&self) -> Rect {
        dispatch!(self, s => s.bounds())
    }
    fn is_closed(&self) -> bool {
        dispatch!(self, s => s.is_closed())
    }
    fn is_valid(&self) -> bool {
        dispatch!(self, s => s.is_valid())
    }
    fn contains(&self, p: Point) -> bool {
        dispatch!(self, s => s.contains(p))
    }
    fn nearest(&self, p: Point) -> Option<Nearest> {
        dispatch!(self, s => s.nearest(p))
    }
    fn intersect(&self, probe: &Probe) -> Vec<Hit> {
        dispatch!(self, s => s.intersect(probe))
    }
}

impl From<RectShape> for Primitive {
    fn from(s: RectShape) -> Self {
        Primitive::Rect(s)
    }
}
impl From<EllipseShape> for Primitive {
    fn from(s: EllipseShape) -> Self {
        Primitive::Ellipse(s)
    }
}
impl From<DiamondShape> for Primitive {
    fn from(s: DiamondShape) -> Self {
        Primitive::Diamond(s)
    }
}
impl From<Polyline> for Primitive {
    fn from(s: Polyline) -> Self {
        Primitive::Polyline(s)
    }
}
impl From<PathShape> for Primitive {
    fn from(s: PathShape) -> Self {
        Primitive::Path(s)
    }
}

/// Sort hits by `t` and drop duplicates that come from a probe passing through a shared vertex.
pub(crate) fn finish_hits(mut hits: Vec<Hit>) -> Vec<Hit> {
    hits.retain(|h| h.t.is_finite() && crate::point_is_finite(h.point));
    hits.sort_by(|a, b| a.t.total_cmp(&b.t));
    hits.dedup_by(|b, a| (b.t - a.t).abs() <= 1e-9 * (1.0 + a.t.abs()));
    hits
}

/// A unit vector, or zero for a zero or non-finite input.
#[inline]
pub(crate) fn unit(v: Vec2) -> Vec2 {
    let l = v.hypot();
    if l > 0.0 && l.is_finite() { v / l } else { Vec2::ZERO }
}

/// 2D cross product.
#[inline]
pub(crate) fn cross(a: Vec2, b: Vec2) -> f64 {
    a.x * b.y - a.y * b.x
}
