//! Convex polygons with rounded corners and an outward offset, in the shape's local frame.
//!
//! Rectangles (with a corner radius) and diamonds are both this. The outline is a chain of
//! straight sides and circular arcs, so distance, closest point and intersection are exact.
//! The offset outline (used for the binding gap) pushes every side out by `offset` and grows each
//! arc by the same amount, which is the true parallel curve.

#[cfg(test)]
use kurbo::Rect;
use kurbo::{Point, Vec2};

use crate::outline::{Hit, cross, unit};

const MAX_N: usize = 4;
const SECTOR_EPS: f64 = 1e-12;

#[derive(Debug, Clone, Copy)]
struct Corner {
    /// The polygon vertex.
    v: Point,
    /// Centre of the corner arc (the vertex itself when the corner is sharp).
    center: Point,
    /// Arc radius including the offset. Zero means a sharp corner.
    radius: f64,
    /// Outward normal of the side that arrives at this corner.
    n_in: Vec2,
    /// Outward normal of the side that leaves this corner.
    n_out: Vec2,
    /// True when the corner has an arc.
    arc: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Convex {
    n: usize,
    c: [Corner; MAX_N],
    offset: f64,
}

impl Convex {
    /// A rectangle centred on the origin.
    pub(crate) fn rect(half: Vec2, radius: f64, offset: f64) -> Self {
        let (hx, hy) = (half.x.max(crate::MIN_DIM * 0.5), half.y.max(crate::MIN_DIM * 0.5));
        let r = radius.clamp(0.0, hx.min(hy));
        Self::build(&[Point::new(-hx, -hy), Point::new(hx, -hy), Point::new(hx, hy), Point::new(-hx, hy)], r, offset)
    }

    /// A diamond centred on the origin: vertices at the middles of the four sides of the box.
    pub(crate) fn diamond(half: Vec2, radius: f64, offset: f64) -> Self {
        let (hx, hy) = (half.x.max(crate::MIN_DIM * 0.5), half.y.max(crate::MIN_DIM * 0.5));
        Self::build(
            &[Point::new(0.0, -hy), Point::new(hx, 0.0), Point::new(0.0, hy), Point::new(-hx, 0.0)],
            radius.max(0.0),
            offset,
        )
    }

    /// `verts` must be convex and in clockwise order on screen (y down).
    fn build(verts: &[Point], radius: f64, offset: f64) -> Self {
        let n = verts.len().min(MAX_N);
        let offset = if offset.is_finite() { offset.max(0.0) } else { 0.0 };
        let radius = if radius.is_finite() { radius } else { 0.0 };
        let blank = Corner {
            v: Point::ZERO,
            center: Point::ZERO,
            radius: 0.0,
            n_in: Vec2::ZERO,
            n_out: Vec2::ZERO,
            arc: false,
        };
        let mut c = [blank; MAX_N];
        for i in 0..n {
            let prev = verts[(i + n - 1) % n];
            let cur = verts[i];
            let next = verts[(i + 1) % n];
            let (d1, d2) = (cur - prev, next - cur);
            let (l1, l2) = (d1.hypot(), d2.hypot());
            let (e1, e2) = (unit(d1), unit(d2));
            let n_in = Vec2::new(e1.y, -e1.x);
            let n_out = Vec2::new(e2.y, -e2.x);
            let turn = cross(e1, e2).atan2(e1.dot(e2)).max(0.0);
            let half_tan = (turn * 0.5).tan();
            let (r, tangent_len) = if radius > 0.0 && turn > 1e-6 && half_tan.is_finite() && half_tan > 0.0 {
                let tl = (radius * half_tan).min(l1 * 0.5).min(l2 * 0.5);
                (tl / half_tan, tl)
            } else {
                (0.0, 0.0)
            };
            let center = if r > 0.0 { cur - e1 * tangent_len - n_in * r } else { cur };
            let big_r = r + offset;
            c[i] = Corner { v: cur, center, radius: big_r, n_in, n_out, arc: big_r > 0.0 && turn > 1e-6 };
        }
        Convex { n, c, offset }
    }

    #[inline]
    fn corners(&self) -> &[Corner] {
        &self.c[..self.n]
    }

    /// Is `w` (relative to a corner centre) in the sector swept by the arc?
    #[inline]
    fn in_sector(k: &Corner, w: Vec2) -> bool {
        cross(k.n_in, w) >= -SECTOR_EPS * w.hypot() && cross(w, k.n_out) >= -SECTOR_EPS * w.hypot()
    }

    pub(crate) fn contains(&self, p: Point) -> bool {
        for k in self.corners() {
            if k.n_out.dot(p - k.v) > self.offset + 1e-12 {
                return false;
            }
        }
        for k in self.corners() {
            if k.arc {
                let w = p - k.center;
                if Self::in_sector(k, w) && w.hypot() > k.radius + 1e-12 {
                    return false;
                }
            }
        }
        true
    }

    /// Largest value of `p . u` over the shape.
    pub(crate) fn support(&self, u: Vec2) -> f64 {
        let ul = u.hypot();
        let mut best = f64::NEG_INFINITY;
        for k in self.corners() {
            let a = k.center + k.n_in * k.radius;
            let b = k.center + k.n_out * k.radius;
            best = best.max(a.to_vec2().dot(u)).max(b.to_vec2().dot(u));
            if k.arc && ul > 0.0 {
                let dir = u / ul;
                if cross(k.n_in, dir) >= -SECTOR_EPS && cross(dir, k.n_out) >= -SECTOR_EPS {
                    best = best.max(k.center.to_vec2().dot(u) + k.radius * ul);
                }
            }
        }
        best
    }

    /// `(point, distance, normal)` of the closest outline point.
    pub(crate) fn nearest(&self, p: Point) -> (Point, f64, Vec2) {
        let mut best = (Point::ZERO, f64::INFINITY, Vec2::ZERO);
        let mut consider = |q: Point, normal: Vec2| {
            let d = (p - q).hypot();
            if d < best.1 {
                best = (q, d, normal);
            }
        };
        for i in 0..self.n {
            let k = &self.c[i];
            let k2 = &self.c[(i + 1) % self.n];
            if k.arc {
                let w = p - k.center;
                if Self::in_sector(k, w) {
                    let l = w.hypot();
                    let dir = if l > 0.0 { w / l } else { unit(k.n_in + k.n_out) };
                    consider(k.center + dir * k.radius, dir);
                }
            }
            let a = k.center + k.n_out * k.radius;
            let b = k2.center + k.n_out * k2.radius;
            let e = b - a;
            let l2 = e.hypot2();
            let s = if l2 > 0.0 { ((p - a).dot(e) / l2).clamp(0.0, 1.0) } else { 0.0 };
            consider(a + e * s, k.n_out);
        }
        best
    }

    pub(crate) fn intersect(&self, o: Point, d: Vec2, t0: f64, t1: f64, out: &mut Vec<Hit>) {
        let tol = 1e-12;
        for i in 0..self.n {
            let k = &self.c[i];
            let k2 = &self.c[(i + 1) % self.n];
            // Side from the end of this corner's arc to the start of the next one.
            let a = k.center + k.n_out * k.radius;
            let b = k2.center + k.n_out * k2.radius;
            let e = b - a;
            let denom = cross(d, e);
            if denom.abs() > 1e-14 * d.hypot() * e.hypot() && e.hypot2() > 0.0 {
                let ao = a - o;
                let t = cross(ao, e) / denom;
                let s = cross(ao, d) / denom;
                if (-tol..=1.0 + tol).contains(&s) && t >= t0 - tol && t <= t1 + tol {
                    let t = t.clamp(t0, t1);
                    out.push(Hit { point: o + d * t, t, normal: k.n_out });
                }
            }
            if k.arc {
                let a2 = d.hypot2();
                let oc = o - k.center;
                let b2 = 2.0 * d.dot(oc);
                let c2 = oc.hypot2() - k.radius * k.radius;
                let disc = b2 * b2 - 4.0 * a2 * c2;
                if disc >= 0.0 && a2 > 0.0 {
                    let sq = disc.sqrt();
                    let roots = if sq == 0.0 {
                        [Some(-b2 / (2.0 * a2)), None]
                    } else {
                        [Some((-b2 - sq) / (2.0 * a2)), Some((-b2 + sq) / (2.0 * a2))]
                    };
                    for t in roots.into_iter().flatten() {
                        if t >= t0 - tol && t <= t1 + tol {
                            let t = t.clamp(t0, t1);
                            let q = o + d * t;
                            let w = q - k.center;
                            if Self::in_sector(k, w) {
                                out.push(Hit { point: q, t, normal: unit(w) });
                            }
                        }
                    }
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn local_bounds(&self) -> Rect {
        Rect::new(
            -self.support(Vec2::new(-1.0, 0.0)),
            -self.support(Vec2::new(0.0, -1.0)),
            self.support(Vec2::new(1.0, 0.0)),
            self.support(Vec2::new(0.0, 1.0)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sharp_rect() {
        let c = Convex::rect(Vec2::new(10.0, 5.0), 0.0, 0.0);
        assert!(c.contains(Point::new(9.0, 4.0)));
        assert!(!c.contains(Point::new(11.0, 0.0)));
        let (q, d, n) = c.nearest(Point::new(15.0, 0.0));
        assert_eq!(q, Point::new(10.0, 0.0));
        assert!((d - 5.0).abs() < 1e-12);
        assert_eq!(n, Vec2::new(1.0, 0.0));
        assert_eq!(c.local_bounds(), Rect::new(-10.0, -5.0, 10.0, 5.0));
    }

    #[test]
    fn rounded_corner_cuts_the_tip() {
        let c = Convex::rect(Vec2::new(10.0, 10.0), 4.0, 0.0);
        assert!(!c.contains(Point::new(9.9, 9.9)), "the tip of a rounded corner is outside");
        assert!(c.contains(Point::new(8.5, 8.5)));
        let (q, d, _) = c.nearest(Point::new(20.0, 20.0));
        // arc centre (6,6), radius 4: nearest is centre + 4 * unit(1,1)
        let k = 6.0 + 4.0 / 2.0_f64.sqrt();
        assert!((q.x - k).abs() < 1e-9 && (q.y - k).abs() < 1e-9);
        assert!((d - (20.0 - k) * 2.0_f64.sqrt()).abs() < 1e-9);
    }

    #[test]
    fn offset_makes_round_corners() {
        let c = Convex::rect(Vec2::new(10.0, 10.0), 0.0, 5.0);
        assert!(c.contains(Point::new(14.0, 0.0)));
        assert!(!c.contains(Point::new(14.9, 14.9)));
        let (_, d, _) = c.nearest(Point::new(20.0, 20.0));
        assert!((d - (10.0 * 2.0_f64.sqrt() - 5.0)).abs() < 1e-9);
        assert_eq!(c.local_bounds(), Rect::new(-15.0, -15.0, 15.0, 15.0));
    }

    #[test]
    fn diamond() {
        let c = Convex::diamond(Vec2::new(10.0, 10.0), 0.0, 0.0);
        assert!(c.contains(Point::new(4.0, 4.0)));
        assert!(!c.contains(Point::new(6.0, 6.0)));
        let (q, d, _) = c.nearest(Point::new(10.0, 10.0));
        assert!((q.x - 5.0).abs() < 1e-9 && (q.y - 5.0).abs() < 1e-9);
        assert!((d - 5.0 * 2.0_f64.sqrt()).abs() < 1e-9);
    }

    #[test]
    fn ray_hits_both_sides() {
        let c = Convex::rect(Vec2::new(10.0, 5.0), 3.0, 0.0);
        let mut hits = Vec::new();
        c.intersect(Point::new(-20.0, 0.0), Vec2::new(1.0, 0.0), 0.0, f64::INFINITY, &mut hits);
        let hits = crate::outline::finish_hits(hits);
        assert_eq!(hits.len(), 2);
        assert!((hits[0].point.x + 10.0).abs() < 1e-9 && (hits[1].point.x - 10.0).abs() < 1e-9);
        assert_eq!(hits[0].normal, Vec2::new(-1.0, 0.0));
    }

    #[test]
    fn zero_size_stays_finite() {
        let c = Convex::rect(Vec2::ZERO, 5.0, 0.0);
        let (q, d, n) = c.nearest(Point::new(3.0, 4.0));
        assert!(q.x.is_finite() && d.is_finite() && n.x.is_finite());
        assert!((d - 5.0).abs() < 1e-3);
    }
}
