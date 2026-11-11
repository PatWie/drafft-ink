//! An axis-aligned ellipse centred on the origin, in the shape's local frame.

use kurbo::{Point, Vec2};

use crate::outline::{Hit, clamp_to_range, quadratic_roots, unit};

#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalEllipse {
    pub a: f64,
    pub b: f64,
}

impl LocalEllipse {
    /// Semi-axes `a`, `b`, grown by `offset` (an ellipse with the axes enlarged, which is what
    /// Excalidraw uses for the binding gap; it is not the exact parallel curve).
    pub(crate) fn new(a: f64, b: f64, offset: f64) -> Self {
        let off = if offset.is_finite() { offset.max(0.0) } else { 0.0 };
        Self { a: a.max(crate::MIN_DIM * 0.5) + off, b: b.max(crate::MIN_DIM * 0.5) + off }
    }

    pub(crate) fn contains(&self, p: Point) -> bool {
        let (x, y) = (p.x / self.a, p.y / self.b);
        x * x + y * y <= 1.0 + 1e-12
    }

    pub(crate) fn support(&self, u: Vec2) -> f64 {
        ((self.a * u.x).powi(2) + (self.b * u.y).powi(2)).sqrt()
    }

    fn normal_at(&self, q: Point) -> Vec2 {
        unit(Vec2::new(q.x / (self.a * self.a), q.y / (self.b * self.b)))
    }

    pub(crate) fn nearest(&self, p: Point) -> (Point, f64, Vec2) {
        let (qx, qy) = closest_on_ellipse(self.a, self.b, p.x, p.y);
        let q = Point::new(qx, qy);
        (q, (p - q).hypot(), self.normal_at(q))
    }

    pub(crate) fn intersect(&self, o: Point, d: Vec2, t0: f64, t1: f64, out: &mut Vec<Hit>) {
        // Scale to the unit circle; t is unchanged by the scaling.
        let (ox, oy) = (o.x / self.a, o.y / self.b);
        let (dx, dy) = (d.x / self.a, d.y / self.b);
        let a2 = dx * dx + dy * dy;
        let b2 = 2.0 * (ox * dx + oy * dy);
        let c2 = ox * ox + oy * oy - 1.0;
        for t in quadratic_roots(a2, b2, c2).into_iter().flatten() {
            if let Some(t) = clamp_to_range(t, t0, t1) {
                let q = o + d * t;
                out.push(Hit { point: q, t, normal: self.normal_at(q) });
            }
        }
    }
}

/// Closest point on the ellipse `(x/a)^2 + (y/b)^2 = 1` to `(px, py)`.
///
/// Eberly's robust bisection ("Distance from a Point to an Ellipse", Geometric Tools), done in the
/// first quadrant with `a >= b` and mapped back by symmetry.
fn closest_on_ellipse(a: f64, b: f64, px: f64, py: f64) -> (f64, f64) {
    let swap = a < b;
    let (e0, e1, y0, y1) = if swap { (b, a, py.abs(), px.abs()) } else { (a, b, px.abs(), py.abs()) };
    let (x0, x1) = closest_first_quadrant(e0, e1, y0, y1);
    let (rx, ry) = if swap { (x1, x0) } else { (x0, x1) };
    (rx.copysign(if px == 0.0 { 1.0 } else { px }), ry.copysign(if py == 0.0 { 1.0 } else { py }))
}

fn closest_first_quadrant(e0: f64, e1: f64, y0: f64, y1: f64) -> (f64, f64) {
    if y1 > 0.0 {
        if y0 > 0.0 {
            let (z0, z1) = (y0 / e0, y1 / e1);
            let g = z0 * z0 + z1 * z1 - 1.0;
            if g != 0.0 {
                let r0 = (e0 / e1) * (e0 / e1);
                let sbar = robust_root(r0, z0, z1, g);
                (r0 * y0 / (sbar + r0), y1 / (sbar + 1.0))
            } else {
                (y0, y1)
            }
        } else {
            (0.0, e1)
        }
    } else {
        let numer0 = e0 * y0;
        let denom0 = e0 * e0 - e1 * e1;
        if numer0 < denom0 {
            let xde0 = numer0 / denom0;
            (e0 * xde0, e1 * (1.0 - xde0 * xde0).max(0.0).sqrt())
        } else {
            (e0, 0.0)
        }
    }
}

fn robust_root(r0: f64, z0: f64, z1: f64, g0: f64) -> f64 {
    let n0 = r0 * z0;
    let mut s0 = z1 - 1.0;
    let mut s1 = if g0 < 0.0 { 0.0 } else { n0.hypot(z1) - 1.0 };
    let mut s = 0.0;
    for _ in 0..200 {
        s = (s0 + s1) * 0.5;
        if s == s0 || s == s1 {
            break;
        }
        let (r_a, r_b) = (n0 / (s + r0), z1 / (s + 1.0));
        let g = r_a * r_a + r_b * r_b - 1.0;
        if g > 0.0 {
            s0 = s;
        } else if g < 0.0 {
            s1 = s;
        } else {
            break;
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circle_nearest() {
        let e = LocalEllipse::new(10.0, 10.0, 0.0);
        let (q, d, n) = e.nearest(Point::new(30.0, 0.0));
        assert!((q.x - 10.0).abs() < 1e-9 && q.y.abs() < 1e-9);
        assert!((d - 20.0).abs() < 1e-9);
        assert!((n.x - 1.0).abs() < 1e-9);
    }

    #[test]
    fn nearest_inside_and_on_axes() {
        let e = LocalEllipse::new(10.0, 4.0, 0.0);
        let (_, d, _) = e.nearest(Point::new(0.0, 0.0));
        assert!((d - 4.0).abs() < 1e-9, "centre to the nearest vertex is b: {d}");
        let (q, d, _) = e.nearest(Point::new(0.0, 9.0));
        assert!((q.y - 4.0).abs() < 1e-9 && (d - 5.0).abs() < 1e-9);
        let (q, _, _) = e.nearest(Point::new(-3.0, -1.0));
        assert!(q.x < 0.0 && q.y < 0.0);
    }

    #[test]
    fn nearest_is_on_the_curve_and_minimal() {
        let e = LocalEllipse::new(12.0, 5.0, 0.0);
        for &(px, py) in &[(20.0, 7.0), (3.0, 1.0), (-8.0, 6.0), (0.0, 0.0), (1.0, -30.0), (11.9, 0.0)] {
            let (q, d, _) = e.nearest(Point::new(px, py));
            let on = (q.x / 12.0).powi(2) + (q.y / 5.0).powi(2);
            assert!((on - 1.0).abs() < 1e-9, "({px},{py}) -> {q:?} on={on}");
            // brute force over the parametrisation
            let best = (0..20_000)
                .map(|i| {
                    let t = i as f64 / 20_000.0 * std::f64::consts::TAU;
                    ((12.0 * t.cos() - px).powi(2) + (5.0 * t.sin() - py).powi(2)).sqrt()
                })
                .fold(f64::INFINITY, f64::min);
            assert!(d <= best + 1e-9 && best - d < 1e-2, "({px},{py}) d={d} brute={best}");
        }
    }

    #[test]
    fn thin_ellipse_is_finite() {
        let e = LocalEllipse::new(10.0, 0.0, 0.0);
        let (q, d, n) = e.nearest(Point::new(3.0, 2.0));
        assert!(q.x.is_finite() && d.is_finite() && n.x.is_finite());
        assert!((d - 2.0).abs() < 1e-3);
    }

    #[test]
    fn line_through_the_centre() {
        let e = LocalEllipse::new(10.0, 4.0, 0.0);
        let mut hits = Vec::new();
        e.intersect(Point::new(-20.0, 0.0), Vec2::new(1.0, 0.0), 0.0, f64::INFINITY, &mut hits);
        assert_eq!(hits.len(), 2);
        assert!((hits[0].point.x + 10.0).abs() < 1e-9 && (hits[1].t - 30.0).abs() < 1e-9);
        // ray starting inside hits once
        hits.clear();
        e.intersect(Point::ZERO, Vec2::new(0.0, 1.0), 0.0, f64::INFINITY, &mut hits);
        assert_eq!(hits.len(), 1);
        assert!((hits[0].point.y - 4.0).abs() < 1e-9);
    }
}
