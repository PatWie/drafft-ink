//! The primitive shapes: rectangle, ellipse, diamond, polyline or polygon, and bezier path.
//!
//! Box-like shapes (rectangle, ellipse, diamond) are an [`OrientedBox`] plus a few numbers, and
//! every query is computed exactly in the box's local frame and mapped back. Polylines and paths
//! live in world space; use [`Polyline::transformed`] and [`PathShape::transformed`] or
//! [`ElementFrame`] to place points that are stored in element-local coordinates.

use kurbo::{Affine, BezPath, Line, ParamCurve, ParamCurveNearest, PathEl, PathSeg, Point, Rect, Shape, Vec2};

use crate::aabb;
use crate::convex::Convex;
use crate::ellipse::LocalEllipse;
use crate::obox::OrientedBox;
use crate::outline::{Hit, Nearest, Outline, Probe, cross, finish_hits, unit};
use crate::transform::ElementFrame;

// ---------------------------------------------------------------------------------------------
// Box-like shapes
// ---------------------------------------------------------------------------------------------

/// Shared plumbing for shapes defined by a frame and a local convex or elliptical outline.
macro_rules! framed_outline {
    ($ty:ty, |$s:ident| $local:expr) => {
        impl Outline for $ty {
            fn bounds(&self) -> Rect {
                if !self.is_valid() {
                    return Rect::ZERO;
                }
                let $s = self;
                let l = $local;
                let f = &self.frame;
                let ux = f.dir_to_local(Vec2::new(1.0, 0.0));
                let uy = f.dir_to_local(Vec2::new(0.0, 1.0));
                Rect::new(
                    f.center.x - l.support(-ux),
                    f.center.y - l.support(-uy),
                    f.center.x + l.support(ux),
                    f.center.y + l.support(uy),
                )
            }

            fn is_closed(&self) -> bool {
                true
            }

            fn is_valid(&self) -> bool {
                self.frame.is_valid() && crate::num_is_sane(self.offset) && self.extra_is_finite()
            }

            fn contains(&self, p: Point) -> bool {
                if !self.is_valid() || !crate::point_is_finite(p) {
                    return false;
                }
                let $s = self;
                let l = $local;
                l.contains(self.frame.to_local(p))
            }

            fn nearest(&self, p: Point) -> Option<Nearest> {
                if !self.is_valid() || !crate::point_is_finite(p) {
                    return None;
                }
                let $s = self;
                let l = $local;
                let (q, distance, n) = l.nearest(self.frame.to_local(p));
                Some(Nearest { point: self.frame.to_world(q), distance, normal: self.frame.dir_to_world(n) })
            }

            fn intersect(&self, probe: &Probe) -> Vec<Hit> {
                if !self.is_valid() || !probe.is_valid() {
                    return Vec::new();
                }
                let $s = self;
                let l = $local;
                let f = &self.frame;
                let mut out = Vec::new();
                l.intersect(f.to_local(probe.origin), f.dir_to_local(probe.dir), probe.t_min, probe.t_max, &mut out);
                for h in &mut out {
                    h.point = f.to_world(h.point);
                    h.normal = f.dir_to_world(h.normal);
                }
                finish_hits(out)
            }
        }
    };
}

/// A rectangle, optionally with rounded corners and an outward offset.
///
/// `offset` grows the outline outward by that distance (corners become arcs). It is the "binding
/// gap" outline: an arrow end bound in orbit mode sits on the rectangle offset by the gap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectShape {
    /// The oriented box the shape is inscribed in.
    pub frame: OrientedBox,
    /// Corner radius; clamped to half the shorter side.
    pub corner_radius: f64,
    /// Distance the outline is pushed outward (0 for the shape itself).
    pub offset: f64,
}

impl RectShape {
    /// A sharp-cornered rectangle filling `frame`.
    pub fn new(frame: OrientedBox) -> Self {
        Self { frame, corner_radius: 0.0, offset: 0.0 }
    }

    /// An unrotated rectangle covering `r`.
    pub fn from_rect(r: Rect) -> Self {
        Self::new(OrientedBox::from_rect(r))
    }

    /// The same rectangle with rounded corners.
    pub fn with_corner_radius(mut self, r: f64) -> Self {
        self.corner_radius = r;
        self
    }

    /// The same rectangle with its outline pushed outward by `d`.
    pub fn with_offset(mut self, d: f64) -> Self {
        self.offset = d;
        self
    }

    fn extra_is_finite(&self) -> bool {
        crate::num_is_sane(self.corner_radius)
    }

    fn convex(&self) -> Convex {
        Convex::rect(self.frame.half(), self.corner_radius, self.offset)
    }
}

framed_outline!(RectShape, |s| s.convex());

/// A diamond (rhombus) whose vertices are the midpoints of the sides of its box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiamondShape {
    /// The oriented box the diamond is inscribed in.
    pub frame: OrientedBox,
    /// Corner radius of the vertices.
    pub corner_radius: f64,
    /// Distance the outline is pushed outward (0 for the shape itself).
    pub offset: f64,
}

impl DiamondShape {
    /// A sharp-cornered diamond inscribed in `frame`.
    pub fn new(frame: OrientedBox) -> Self {
        Self { frame, corner_radius: 0.0, offset: 0.0 }
    }

    /// The same diamond with rounded vertices.
    pub fn with_corner_radius(mut self, r: f64) -> Self {
        self.corner_radius = r;
        self
    }

    /// The same diamond with its outline pushed outward by `d`.
    pub fn with_offset(mut self, d: f64) -> Self {
        self.offset = d;
        self
    }

    fn extra_is_finite(&self) -> bool {
        crate::num_is_sane(self.corner_radius)
    }

    fn convex(&self) -> Convex {
        Convex::diamond(self.frame.half(), self.corner_radius, self.offset)
    }

    /// The four vertices in world space: top, right, bottom, left of the unrotated box.
    pub fn vertices(&self) -> [Point; 4] {
        self.frame.side_midpoints()
    }
}

framed_outline!(DiamondShape, |s| s.convex());

/// An ellipse inscribed in its box. `offset` enlarges both semi-axes by that distance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EllipseShape {
    /// The oriented box the ellipse is inscribed in.
    pub frame: OrientedBox,
    /// Distance both semi-axes are enlarged by (0 for the shape itself).
    pub offset: f64,
}

impl EllipseShape {
    /// An ellipse inscribed in `frame`.
    pub fn new(frame: OrientedBox) -> Self {
        Self { frame, offset: 0.0 }
    }

    /// The same ellipse with both semi-axes enlarged by `d`.
    pub fn with_offset(mut self, d: f64) -> Self {
        self.offset = d;
        self
    }

    fn extra_is_finite(&self) -> bool {
        true
    }

    fn local(&self) -> LocalEllipse {
        let h = self.frame.half();
        LocalEllipse::new(h.x, h.y, self.offset)
    }
}

framed_outline!(EllipseShape, |s| s.local());

// ---------------------------------------------------------------------------------------------
// Polyline / polygon
// ---------------------------------------------------------------------------------------------

/// A polyline, or a polygon when `closed`. A closed polyline needs three points to enclose area.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Polyline {
    /// The vertices.
    pub points: Vec<Point>,
    /// True for a polygon: the last point joins the first.
    pub closed: bool,
}

impl Polyline {
    /// A polyline, or a polygon when `closed`.
    pub fn new(points: Vec<Point>, closed: bool) -> Self {
        Self { points, closed }
    }

    /// Place points stored in element-local space into world space.
    pub fn from_frame(frame: &ElementFrame, local_points: &[Point], closed: bool) -> Self {
        let a = frame.to_world();
        Self::new(local_points.iter().map(|p| a * *p).collect(), closed)
    }

    /// The polyline with every point mapped through `a`.
    pub fn transformed(&self, a: Affine) -> Self {
        Self::new(self.points.iter().map(|p| a * *p).collect(), self.closed)
    }

    fn encloses(&self) -> bool {
        self.closed && self.points.len() >= 3
    }

    /// Number of segments, including the closing one.
    fn segment_count(&self) -> usize {
        match self.points.len() {
            0 | 1 => 0,
            n if self.encloses() => n,
            n => n - 1,
        }
    }

    fn segment(&self, i: usize) -> (Point, Point) {
        let n = self.points.len();
        (self.points[i], self.points[(i + 1) % n])
    }

    /// +1 when the polygon runs clockwise on screen (positive shoelace sum in y-down), else -1.
    fn orientation(&self) -> f64 {
        let n = self.points.len();
        let mut sum = 0.0;
        for i in 0..n {
            let (a, b) = (self.points[i], self.points[(i + 1) % n]);
            sum += a.x * b.y - b.x * a.y;
        }
        if sum >= 0.0 { 1.0 } else { -1.0 }
    }

    /// Total length of the outline.
    pub fn length(&self) -> f64 {
        (0..self.segment_count())
            .map(|i| {
                let (a, b) = self.segment(i);
                (b - a).hypot()
            })
            .sum()
    }

    fn winding(&self, p: Point) -> i32 {
        let n = self.points.len();
        let mut w = 0;
        for i in 0..n {
            let (a, b) = (self.points[i], self.points[(i + 1) % n]);
            if a.y <= p.y {
                if b.y > p.y && cross(b - a, p - a) > 0.0 {
                    w += 1;
                }
            } else if b.y <= p.y && cross(b - a, p - a) < 0.0 {
                w -= 1;
            }
        }
        w
    }
}

impl Outline for Polyline {
    fn bounds(&self) -> Rect {
        if !self.is_valid() {
            return Rect::ZERO;
        }
        aabb::from_points(self.points.iter().copied()).unwrap_or(Rect::ZERO)
    }

    fn is_closed(&self) -> bool {
        self.encloses()
    }

    fn is_valid(&self) -> bool {
        !self.points.is_empty() && self.points.iter().all(|p| crate::point_is_finite(*p))
    }

    fn contains(&self, p: Point) -> bool {
        if !self.is_valid() || !self.encloses() || !crate::point_is_finite(p) {
            return false;
        }
        if self.winding(p) != 0 {
            return true;
        }
        // The boundary counts as inside.
        self.outline_distance(p) <= 1e-12
    }

    fn nearest(&self, p: Point) -> Option<Nearest> {
        if !self.is_valid() || !crate::point_is_finite(p) {
            return None;
        }
        if self.points.len() == 1 {
            let q = self.points[0];
            return Some(Nearest { point: q, distance: (p - q).hypot(), normal: Vec2::ZERO });
        }
        let orient = if self.encloses() { self.orientation() } else { 1.0 };
        let mut best: Option<Nearest> = None;
        for i in 0..self.segment_count() {
            let (a, b) = self.segment(i);
            let e = b - a;
            let l2 = e.hypot2();
            let s = if l2 > 0.0 { ((p - a).dot(e) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let q = a + e * s;
            let d = (p - q).hypot();
            if best.is_none_or(|n| d < n.distance) {
                let u = unit(e);
                best = Some(Nearest { point: q, distance: d, normal: Vec2::new(u.y, -u.x) * orient });
            }
        }
        best
    }

    fn intersect(&self, probe: &Probe) -> Vec<Hit> {
        if !self.is_valid() || !probe.is_valid() {
            return Vec::new();
        }
        let orient = if self.encloses() { self.orientation() } else { 1.0 };
        let (o, d) = (probe.origin, probe.dir);
        let tol = 1e-12;
        let mut out = Vec::new();
        for i in 0..self.segment_count() {
            let (a, b) = self.segment(i);
            let e = b - a;
            let denom = cross(d, e);
            if e.hypot2() == 0.0 || denom.abs() <= 1e-14 * d.hypot() * e.hypot() {
                continue;
            }
            let ao = a - o;
            let t = cross(ao, e) / denom;
            let s = cross(ao, d) / denom;
            if (-tol..=1.0 + tol).contains(&s) && t >= probe.t_min - tol && t <= probe.t_max + tol {
                let t = t.clamp(probe.t_min, probe.t_max);
                let u = unit(e);
                out.push(Hit { point: o + d * t, t, normal: Vec2::new(u.y, -u.x) * orient });
            }
        }
        finish_hits(out)
    }
}

// ---------------------------------------------------------------------------------------------
// Bezier path
// ---------------------------------------------------------------------------------------------

/// A bezier path in world space. Filled queries use the non-zero winding rule; an open subpath is
/// closed implicitly for the fill, as in SVG.
#[derive(Debug, Clone, PartialEq)]
pub struct PathShape {
    /// The path, in world space.
    pub path: BezPath,
}

impl PathShape {
    /// A shape over `path`.
    pub fn new(path: BezPath) -> Self {
        Self { path }
    }

    /// The path mapped through `a`.
    pub fn transformed(&self, a: Affine) -> Self {
        Self::new(a * self.path.clone())
    }

    /// Place a path stored in element-local space into world space.
    pub fn from_frame(frame: &ElementFrame, local: &BezPath) -> Self {
        Self::new(frame.to_world() * local.clone())
    }

    fn el_finite(el: &PathEl) -> bool {
        match el {
            PathEl::MoveTo(a) | PathEl::LineTo(a) => crate::point_is_finite(*a),
            PathEl::QuadTo(a, b) => crate::point_is_finite(*a) && crate::point_is_finite(*b),
            PathEl::CurveTo(a, b, c) => {
                crate::point_is_finite(*a) && crate::point_is_finite(*b) && crate::point_is_finite(*c)
            }
            PathEl::ClosePath => true,
        }
    }

    fn tangent_normal(&self, seg: &PathSeg, t: f64) -> Vec2 {
        let (t0, t1) = ((t - 1e-4).max(0.0), (t + 1e-4).min(1.0));
        let d = seg.eval(t1) - seg.eval(t0);
        let u = unit(d);
        Vec2::new(u.y, -u.x)
    }

    fn outward(&self, seg: &PathSeg, t: f64, q: Point) -> Vec2 {
        let n = self.tangent_normal(seg, t);
        if n == Vec2::ZERO || !self.is_closed() {
            return n;
        }
        let probe = q + n * (self.bounds().width().max(self.bounds().height()) * 1e-4 + 1e-6);
        if self.path.winding(probe) != 0 { -n } else { n }
    }
}

impl Outline for PathShape {
    fn bounds(&self) -> Rect {
        if !self.is_valid() || self.path.elements().is_empty() {
            return Rect::ZERO;
        }
        self.path.bounding_box()
    }

    fn is_closed(&self) -> bool {
        self.path.elements().iter().any(|e| matches!(e, PathEl::ClosePath))
    }

    fn is_valid(&self) -> bool {
        self.path.elements().iter().all(Self::el_finite)
    }

    fn contains(&self, p: Point) -> bool {
        if !self.is_valid() || !crate::point_is_finite(p) || !self.is_closed() {
            return false;
        }
        self.path.winding(p) != 0
    }

    fn nearest(&self, p: Point) -> Option<Nearest> {
        if !self.is_valid() || !crate::point_is_finite(p) {
            return None;
        }
        let mut best: Option<(f64, PathSeg, f64)> = None;
        for seg in self.path.segments() {
            let n = seg.nearest(p, 1e-6);
            if best.is_none_or(|(d2, _, _)| n.distance_sq < d2) {
                best = Some((n.distance_sq, seg, n.t));
            }
        }
        let (d2, seg, t) = best?;
        let q = seg.eval(t);
        Some(Nearest { point: q, distance: d2.sqrt(), normal: self.outward(&seg, t, q) })
    }

    fn intersect(&self, probe: &Probe) -> Vec<Hit> {
        if !self.is_valid() || !probe.is_valid() {
            return Vec::new();
        }
        let Some((t0, t1)) = probe.clip_to_rect(&aabb::inflate(self.bounds(), 1.0)) else {
            return Vec::new();
        };
        if !t0.is_finite() || !t1.is_finite() {
            return Vec::new();
        }
        let line = Line::new(probe.at(t0), probe.at(t1));
        let mut out = Vec::new();
        for seg in self.path.segments() {
            for x in seg.intersect_line(line) {
                let t = t0 + x.line_t * (t1 - t0);
                let q = seg.eval(x.segment_t);
                out.push(Hit { point: q, t, normal: self.outward(&seg, x.segment_t, q) });
            }
        }
        finish_hits(out)
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::{FRAC_PI_2, FRAC_PI_4};

    use kurbo::{Circle, Size};

    use super::*;
    use crate::outline::HitOpts;

    fn rect(x: f64, y: f64, w: f64, h: f64, a: f64) -> RectShape {
        RectShape::new(OrientedBox::from_xywh(x, y, w, h, a))
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn rect_bounds_under_rotation() {
        let r = rect(0.0, 0.0, 10.0, 4.0, FRAC_PI_2);
        let b = r.bounds();
        assert!(near(b.width(), 4.0) && near(b.height(), 10.0));
        let r = rect(0.0, 0.0, 10.0, 10.0, FRAC_PI_4);
        assert!(near(r.bounds().width(), 10.0 * 2.0_f64.sqrt()));
    }

    #[test]
    fn rounded_rect_bounds_under_rotation() {
        // A circle-like rounded rect (radius = half side) has the same bounds at any angle.
        let r = rect(0.0, 0.0, 10.0, 10.0, 0.7).with_corner_radius(5.0);
        let b = r.bounds();
        assert!(near(b.width(), 10.0) && near(b.height(), 10.0), "{b:?}");
    }

    #[test]
    fn rect_filled_and_stroke_hits() {
        let r = rect(0.0, 0.0, 100.0, 60.0, 0.0);
        let stroke = HitOpts::stroke(2.0, 2.0);
        let filled = HitOpts::filled(2.0, 2.0);
        assert!(r.hit_test(Point::new(50.0, 30.0), filled));
        assert!(!r.hit_test(Point::new(50.0, 30.0), stroke), "stroke-only does not hit the middle");
        assert!(r.hit_test(Point::new(50.0, 0.0), stroke));
        assert!(r.hit_test(Point::new(50.0, -2.9), stroke), "tolerance 2 + half stroke 1");
        assert!(!r.hit_test(Point::new(50.0, -3.1), stroke));
        assert!(!r.hit_test(Point::new(150.0, 30.0), filled));
    }

    #[test]
    fn rotated_rect_hit_ignores_the_unrotated_corner() {
        // The old app ignored rotation in hit tests. A 100x10 bar turned 90 degrees is vertical.
        let r = rect(0.0, 0.0, 100.0, 10.0, FRAC_PI_2);
        let filled = HitOpts::filled(0.0, 0.0);
        assert!(r.hit_test(Point::new(50.0, 50.0), filled));
        assert!(r.hit_test(Point::new(50.0, 0.0), filled));
        assert!(!r.hit_test(Point::new(5.0, 5.0), filled), "unrotated top-left corner is empty now");
    }

    #[test]
    fn signed_distance_sign() {
        let r = rect(0.0, 0.0, 10.0, 10.0, 0.0);
        assert!(near(r.signed_distance(Point::new(5.0, 5.0)), -5.0));
        assert!(near(r.signed_distance(Point::new(15.0, 5.0)), 5.0));
    }

    #[test]
    fn rect_ray_and_offset_ray() {
        let r = rect(0.0, 0.0, 10.0, 10.0, 0.0);
        let p = Probe::ray(Point::new(-10.0, 5.0), Vec2::new(1.0, 0.0));
        let hits = r.intersect(&p);
        assert_eq!(hits.len(), 2);
        assert!(near(hits[0].point.x, 0.0) && near(hits[1].point.x, 10.0));
        let g = r.with_offset(5.0);
        let hits = g.intersect(&p);
        assert!(near(hits[0].point.x, -5.0) && near(hits[1].point.x, 15.0));
        // Starting inside: one hit, the exit.
        let hits = r.intersect(&Probe::ray(Point::new(5.0, 5.0), Vec2::new(0.0, -1.0)));
        assert_eq!(hits.len(), 1);
        assert!(near(hits[0].point.y, 0.0) && near(hits[0].normal.y, -1.0));
    }

    #[test]
    fn segment_probe_is_bounded() {
        let r = rect(0.0, 0.0, 10.0, 10.0, 0.0);
        assert!(r.intersect(&Probe::segment(Point::new(-10.0, 5.0), Point::new(-1.0, 5.0))).is_empty());
        assert_eq!(r.intersect(&Probe::segment(Point::new(-10.0, 5.0), Point::new(5.0, 5.0))).len(), 1);
        assert_eq!(r.intersect(&Probe::line(Point::new(5.0, 5.0), Vec2::new(1.0, 1.0))).len(), 2);
    }

    #[test]
    fn rotated_rect_ray_matches_world_geometry() {
        let r = rect(0.0, 0.0, 20.0, 20.0, FRAC_PI_4);
        let hits = r.intersect(&Probe::ray(Point::new(-50.0, 10.0), Vec2::new(1.0, 0.0)));
        assert_eq!(hits.len(), 2);
        let half_diag = 10.0 * 2.0_f64.sqrt();
        assert!(near(hits[0].point.x, 10.0 - half_diag) && near(hits[1].point.x, 10.0 + half_diag));
    }

    #[test]
    fn ellipse_basics() {
        let e = EllipseShape::new(OrientedBox::new(Point::ZERO, Size::new(40.0, 20.0), 0.0));
        assert!(e.contains(Point::new(19.0, 0.0)));
        assert!(!e.contains(Point::new(15.0, 9.0)));
        assert!(near(e.outline_distance(Point::new(30.0, 0.0)), 10.0));
        let rot = EllipseShape::new(OrientedBox::new(Point::ZERO, Size::new(40.0, 20.0), FRAC_PI_2));
        let b = rot.bounds();
        assert!(near(b.width(), 20.0) && near(b.height(), 40.0));
        assert!(rot.contains(Point::new(0.0, 19.0)) && !rot.contains(Point::new(19.0, 0.0)));
        let off = e.with_offset(5.0);
        let hits = off.intersect(&Probe::ray(Point::new(-50.0, 0.0), Vec2::new(1.0, 0.0)));
        assert!(near(hits[0].point.x, -25.0));
    }

    #[test]
    fn diamond_basics() {
        let d = DiamondShape::new(OrientedBox::new(Point::ZERO, Size::new(20.0, 20.0), 0.0));
        assert!(d.contains(Point::new(4.0, 4.0)) && !d.contains(Point::new(6.0, 6.0)));
        let hits = d.intersect(&Probe::ray(Point::new(-20.0, 0.0), Vec2::new(1.0, 0.0)));
        assert_eq!(hits.len(), 2);
        assert!(near(hits[0].point.x, -10.0) && near(hits[1].point.x, 10.0));
        // Through a vertex exactly: no duplicate hits.
        let hits = d.intersect(&Probe::ray(Point::new(0.0, -20.0), Vec2::new(0.0, 1.0)));
        assert_eq!(hits.len(), 2, "{hits:?}");
        let b = DiamondShape::new(OrientedBox::new(Point::ZERO, Size::new(20.0, 20.0), FRAC_PI_4)).bounds();
        assert!(near(b.width(), 10.0 * 2.0_f64.sqrt()));
    }

    #[test]
    fn offset_diamond_has_round_vertices() {
        let d = DiamondShape::new(OrientedBox::new(Point::ZERO, Size::new(20.0, 20.0), 0.0)).with_offset(3.0);
        let hits = d.intersect(&Probe::ray(Point::new(-20.0, 0.0), Vec2::new(1.0, 0.0)));
        assert!(near(hits[0].point.x, -13.0));
    }

    #[test]
    fn polyline_open_and_closed() {
        let open = Polyline::new(vec![Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(10.0, 10.0)], false);
        assert!(!open.is_closed() && !open.contains(Point::new(5.0, 1.0)));
        assert!(near(open.outline_distance(Point::new(5.0, 3.0)), 3.0));
        assert!(open.hit_test(Point::new(5.0, 3.0), HitOpts::stroke(2.0, 2.0)));
        assert!(!open.hit_test(Point::new(5.0, 3.5), HitOpts::stroke(2.0, 2.0)));
        let tri = Polyline::new(open.points.clone(), true);
        assert!(tri.is_closed() && tri.contains(Point::new(8.0, 2.0)) && !tri.contains(Point::new(2.0, 8.0)));
        assert!(tri.hit_test(Point::new(8.0, 2.0), HitOpts::filled(0.0, 0.0)));
        assert!(near(tri.length(), 10.0 + 10.0 + 200.0_f64.sqrt()));
    }

    #[test]
    fn polyline_degenerate_inputs() {
        assert_eq!(Polyline::default().bounds(), Rect::ZERO);
        assert!(Polyline::default().nearest(Point::ZERO).is_none());
        let one = Polyline::new(vec![Point::new(3.0, 4.0)], false);
        assert!(near(one.outline_distance(Point::ZERO), 5.0));
        let dup = Polyline::new(vec![Point::new(1.0, 1.0), Point::new(1.0, 1.0)], false);
        assert!(near(dup.outline_distance(Point::new(1.0, 4.0)), 3.0));
        assert!(dup.intersect(&Probe::ray(Point::ZERO, Vec2::new(1.0, 1.0))).is_empty());
        let nan = Polyline::new(vec![Point::new(f64::NAN, 0.0), Point::new(1.0, 1.0)], false);
        assert!(!nan.is_valid() && nan.nearest(Point::ZERO).is_none());
    }

    #[test]
    fn polyline_outward_normals() {
        let sq = Polyline::new(
            vec![Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(10.0, 10.0), Point::new(0.0, 10.0)],
            true,
        );
        let n = sq.nearest(Point::new(5.0, -3.0)).unwrap();
        assert!(near(n.normal.y, -1.0));
        let ccw = Polyline::new(sq.points.iter().rev().copied().collect(), true);
        let n = ccw.nearest(Point::new(5.0, -3.0)).unwrap();
        assert!(near(n.normal.y, -1.0), "orientation does not matter: {n:?}");
    }

    #[test]
    fn path_circle() {
        let path = Circle::new((0.0, 0.0), 10.0).to_path(0.01);
        let s = PathShape::new(path);
        assert!(s.is_closed() && s.contains(Point::new(5.0, 5.0)) && !s.contains(Point::new(8.0, 8.0)));
        assert!((s.outline_distance(Point::new(20.0, 0.0)) - 10.0).abs() < 1e-3);
        let hits = s.intersect(&Probe::ray(Point::new(-30.0, 0.0), Vec2::new(1.0, 0.0)));
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert!((hits[0].point.x + 10.0).abs() < 1e-3);
        assert!(hits[0].normal.x < -0.99, "outward normal on the left side: {:?}", hits[0].normal);
        let b = s.bounds();
        assert!((b.width() - 20.0).abs() < 1e-2);
    }

    #[test]
    fn path_open_curve_has_no_interior() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.curve_to((10.0, 20.0), (20.0, 20.0), (30.0, 0.0));
        let s = PathShape::new(p);
        assert!(!s.is_closed() && !s.contains(Point::new(15.0, 10.0)));
        // Exact bounds use curve extrema, not control points.
        assert!((s.bounds().y1 - 15.0).abs() < 1e-9, "{:?}", s.bounds());
        assert!(s.hit_test(Point::new(15.0, 15.5), HitOpts::stroke(1.0, 0.0)));
    }

    #[test]
    fn empty_and_hostile_paths() {
        let s = PathShape::new(BezPath::new());
        assert_eq!(s.bounds(), Rect::ZERO);
        assert!(s.nearest(Point::ZERO).is_none());
        let mut p = BezPath::new();
        p.move_to((f64::NAN, 0.0));
        p.line_to((1.0, 1.0));
        assert!(!PathShape::new(p).is_valid());
    }

    #[test]
    fn intersects_rect_cases() {
        let r = rect(0.0, 0.0, 100.0, 100.0, 0.0);
        let inside = Rect::new(40.0, 40.0, 60.0, 60.0);
        assert!(r.intersects_rect(&inside, true));
        assert!(!r.intersects_rect(&inside, false), "stroke-only shape is not touched by a rect inside it");
        assert!(r.intersects_rect(&Rect::new(-10.0, 40.0, 10.0, 60.0), false), "crossing the outline");
        assert!(r.intersects_rect(&Rect::new(-10.0, -10.0, 200.0, 200.0), false), "marquee around the shape");
        assert!(!r.intersects_rect(&Rect::new(200.0, 0.0, 300.0, 100.0), true));
        let e = EllipseShape::new(OrientedBox::new(Point::new(50.0, 50.0), Size::new(100.0, 100.0), 0.0));
        assert!(!e.intersects_rect(&Rect::new(0.0, 0.0, 10.0, 10.0), true), "corner of the box is outside the ellipse");
    }

    #[test]
    fn hostile_shapes_do_not_panic() {
        let bad = [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0, -5.0, 1e300];
        for &a in &bad {
            for &b in &bad {
                let f = OrientedBox::new(Point::new(a, b), Size::new(b, a), a);
                let probe = Probe::ray(Point::new(a, 0.0), Vec2::new(1.0, b));
                for p in [
                    Primitive::from(RectShape::new(f).with_corner_radius(a).with_offset(b)),
                    EllipseShape::new(f).with_offset(a).into(),
                    DiamondShape::new(f).with_corner_radius(b).into(),
                    Polyline::new(vec![Point::new(a, b), Point::new(b, a), Point::new(0.0, 0.0)], true).into(),
                ] {
                    let _ = (
                        p.bounds(),
                        p.contains(Point::new(a, b)),
                        p.nearest(Point::new(b, a)),
                        p.intersect(&probe),
                        p.signed_distance(Point::new(a, a)),
                    );
                    let _ = p.hit_test(Point::new(a, b), HitOpts::filled(a, b));
                    let _ = p.intersects_rect(&Rect::new(a, b, b, a), true);
                }
            }
        }
    }

    use crate::outline::Primitive;
}
