//! Axis-aligned box helpers on top of `kurbo::Rect`.
//!
//! A `Rect` is the AABB type of the whole workspace. `Rect` is not guaranteed to be
//! normalised (`x0 <= x1`) or finite, so the helpers here say what they do with bad input.

use kurbo::{Point, Rect, Vec2};

/// The AABB type used across the workspace.
pub type Aabb = Rect;

/// True when all four coordinates are usable numbers (finite and at most [`crate::MAX_COORD`]).
#[inline]
pub fn is_finite(r: &Rect) -> bool {
    crate::num_is_sane(r.x0) && crate::num_is_sane(r.y0) && crate::num_is_sane(r.x1) && crate::num_is_sane(r.y1)
}

/// The rect with `x0 <= x1` and `y0 <= y1`.
#[inline]
pub fn normalize(r: Rect) -> Rect {
    Rect::new(r.x0.min(r.x1), r.y0.min(r.y1), r.x0.max(r.x1), r.y0.max(r.y1))
}

/// `r` when it is finite, else `Rect::ZERO`.
#[inline]
pub fn or_zero(r: Rect) -> Rect {
    if is_finite(&r) { r } else { Rect::ZERO }
}

/// Centre of the rect.
#[inline]
pub fn center(r: &Rect) -> Point {
    Point::new((r.x0 + r.x1) * 0.5, (r.y0 + r.y1) * 0.5)
}

/// The corners in the order top-left, top-right, bottom-right, bottom-left.
#[inline]
pub fn corners(r: &Rect) -> [Point; 4] {
    [Point::new(r.x0, r.y0), Point::new(r.x1, r.y0), Point::new(r.x1, r.y1), Point::new(r.x0, r.y1)]
}

/// The tightest rect around the finite points, or `None` when there are none.
pub fn from_points<I: IntoIterator<Item = Point>>(points: I) -> Option<Rect> {
    let mut out: Option<Rect> = None;
    for p in points {
        if !crate::point_is_finite(p) {
            continue;
        }
        out = Some(match out {
            None => Rect::new(p.x, p.y, p.x, p.y),
            Some(r) => Rect::new(r.x0.min(p.x), r.y0.min(p.y), r.x1.max(p.x), r.y1.max(p.y)),
        });
    }
    out
}

/// The union of the finite rects (normalised first), or `None` when there are none.
pub fn union_all<I: IntoIterator<Item = Rect>>(rects: I) -> Option<Rect> {
    let mut out: Option<Rect> = None;
    for r in rects {
        if !is_finite(&r) {
            continue;
        }
        let r = normalize(r);
        out = Some(match out {
            None => r,
            Some(u) => Rect::new(u.x0.min(r.x0), u.y0.min(r.y0), u.x1.max(r.x1), u.y1.max(r.y1)),
        });
    }
    out
}

/// True when the rects overlap or touch. Both are normalised first. False for non-finite input.
#[inline]
pub fn intersects(a: &Rect, b: &Rect) -> bool {
    if !is_finite(a) || !is_finite(b) {
        return false;
    }
    let (a, b) = (normalize(*a), normalize(*b));
    a.x0 <= b.x1 && b.x0 <= a.x1 && a.y0 <= b.y1 && b.y0 <= a.y1
}

/// True when the rects share a region of positive area.
#[inline]
pub fn overlaps(a: &Rect, b: &Rect) -> bool {
    if !is_finite(a) || !is_finite(b) {
        return false;
    }
    let (a, b) = (normalize(*a), normalize(*b));
    a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1
}

/// True when `inner` lies completely inside `outer` (touching edges count).
#[inline]
pub fn contains_rect(outer: &Rect, inner: &Rect) -> bool {
    if !is_finite(outer) || !is_finite(inner) {
        return false;
    }
    let (o, i) = (normalize(*outer), normalize(*inner));
    o.x0 <= i.x0 && o.y0 <= i.y0 && o.x1 >= i.x1 && o.y1 >= i.y1
}

/// Closed containment of a point. False for non-finite input.
#[inline]
pub fn contains_point(r: &Rect, p: Point) -> bool {
    if !is_finite(r) || !crate::point_is_finite(p) {
        return false;
    }
    let r = normalize(*r);
    p.x >= r.x0 && p.x <= r.x1 && p.y >= r.y0 && p.y <= r.y1
}

/// Grow (or, with a negative `d`, shrink) every side by `d`. A shrink never inverts the rect: it
/// stops at the centre line.
pub fn inflate(r: Rect, d: f64) -> Rect {
    if !is_finite(&r) || !d.is_finite() {
        return r;
    }
    let r = normalize(r);
    let hx = (r.width() * 0.5 + d).max(0.0);
    let hy = (r.height() * 0.5 + d).max(0.0);
    let c = center(&r);
    Rect::new(c.x - hx, c.y - hy, c.x + hx, c.y + hy)
}

/// Distance from the point to the rect (0 inside). Infinity for non-finite input.
pub fn distance_to_point(r: &Rect, p: Point) -> f64 {
    if !is_finite(r) || !crate::point_is_finite(p) {
        return f64::INFINITY;
    }
    let r = normalize(*r);
    let dx = (r.x0 - p.x).max(0.0).max(p.x - r.x1);
    let dy = (r.y0 - p.y).max(0.0).max(p.y - r.y1);
    dx.hypot(dy)
}

/// The AABB of `r` after rotating it by `angle` about `pivot`.
pub fn rotated_bounds(r: &Rect, pivot: Point, angle: f64) -> Rect {
    let rot = kurbo::Affine::rotate_about(angle, pivot);
    from_points(corners(r).map(|c| rot * c)).unwrap_or(Rect::ZERO)
}

/// Scale the rect about `pivot`. A negative factor mirrors it; the result is normalised.
pub fn scale_about(r: &Rect, pivot: Point, sx: f64, sy: f64) -> Rect {
    if !is_finite(r) || !crate::point_is_finite(pivot) || !sx.is_finite() || !sy.is_finite() {
        return *r;
    }
    let f = |x: f64, c: f64, s: f64| c + (x - c) * s;
    normalize(Rect::new(f(r.x0, pivot.x, sx), f(r.y0, pivot.y, sy), f(r.x1, pivot.x, sx), f(r.y1, pivot.y, sy)))
}

/// The rect moved by `v`.
#[inline]
pub fn translated(r: Rect, v: Vec2) -> Rect {
    Rect::new(r.x0 + v.x, r.y0 + v.y, r.x1 + v.x, r.y1 + v.y)
}

/// Does the segment `a`-`b` touch the rect? Liang-Barsky; touching an edge counts.
pub fn segment_intersects_rect(a: Point, b: Point, r: &Rect) -> bool {
    if !crate::point_is_finite(a) || !crate::point_is_finite(b) || !is_finite(r) {
        return false;
    }
    let r = normalize(*r);
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    for (p, q) in [(-dx, a.x - r.x0), (dx, r.x1 - a.x), (-dy, a.y - r.y0), (dy, r.y1 - a.y)] {
        if p == 0.0 {
            if q < 0.0 {
                return false;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
            if t0 > t1 {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn union_and_points() {
        let u = union_all([
            Rect::new(0.0, 0.0, 1.0, 1.0),
            Rect::new(5.0, -2.0, 3.0, 4.0),
            Rect::new(f64::NAN, 0.0, 1.0, 1.0),
        ]);
        assert_eq!(u, Some(Rect::new(0.0, -2.0, 5.0, 4.0)));
        assert_eq!(union_all([]), None);
        assert_eq!(
            from_points([Point::new(1.0, 2.0), Point::new(f64::INFINITY, 0.0), Point::new(-1.0, 5.0)]),
            Some(Rect::new(-1.0, 2.0, 1.0, 5.0))
        );
    }

    #[test]
    fn touching_vs_overlapping() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(10.0, 0.0, 20.0, 10.0);
        assert!(intersects(&a, &b));
        assert!(!overlaps(&a, &b));
        assert!(overlaps(&a, &Rect::new(9.0, 9.0, 20.0, 20.0)));
        assert!(!intersects(&a, &Rect::new(f64::NAN, 0.0, 1.0, 1.0)));
    }

    #[test]
    fn inflate_never_inverts() {
        let r = Rect::new(0.0, 0.0, 10.0, 4.0);
        assert_eq!(inflate(r, 1.0), Rect::new(-1.0, -1.0, 11.0, 5.0));
        assert_eq!(inflate(r, -3.0), Rect::new(3.0, 2.0, 7.0, 2.0));
        assert_eq!(inflate(r, f64::NAN), r);
    }

    #[test]
    fn rotated_bounds_quarter_turn() {
        let r = Rect::new(0.0, 0.0, 10.0, 4.0);
        let b = rotated_bounds(&r, center(&r), std::f64::consts::FRAC_PI_2);
        assert!((b.width() - 4.0).abs() < 1e-9 && (b.height() - 10.0).abs() < 1e-9);
        assert!((center(&b).x - 5.0).abs() < 1e-9);
    }

    #[test]
    fn segment_rect() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(segment_intersects_rect(Point::new(-5.0, 5.0), Point::new(15.0, 5.0), &r));
        assert!(segment_intersects_rect(Point::new(5.0, 5.0), Point::new(6.0, 6.0), &r));
        assert!(!segment_intersects_rect(Point::new(-5.0, -5.0), Point::new(-1.0, 20.0), &r));
        assert!(segment_intersects_rect(Point::new(-1.0, 0.0), Point::new(11.0, 0.0), &r), "along an edge");
        assert!(!segment_intersects_rect(Point::new(f64::NAN, 0.0), Point::new(1.0, 1.0), &r));
    }

    #[test]
    fn distance() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert_eq!(distance_to_point(&r, Point::new(5.0, 5.0)), 0.0);
        assert_eq!(distance_to_point(&r, Point::new(13.0, 14.0)), 5.0);
        assert!(distance_to_point(&r, Point::new(f64::NAN, 0.0)).is_infinite());
    }
}
