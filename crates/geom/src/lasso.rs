//! The lasso: a free-hand path that closes itself and selects the shapes it surrounds or crosses.
//!
//! It is the free-form counterpart of the marquee rectangle ([`crate::select::marquee_selects`]) and takes the same
//! [`MarqueeMode`]:
//!
//! * `Touch`: a shape is selected when the lasso outline crosses it, when it lies inside the lasso, or (for a
//!   shape with a fill) when the lasso lies inside it. A stroke-only shape is not selected by a lasso drawn
//!   inside it, as with the marquee.
//! * `Contain`: a shape is selected only when its whole outline lies inside the lasso.
//!
//! "Inside" is the non-zero winding rule, so a path that crosses itself still encloses its loops. The path is
//! simplified first (Ramer-Douglas-Peucker), because a pointer path has one point per event and a shape test
//! costs one segment test per lasso edge.

use kurbo::{Point, Rect};

use crate::aabb;
use crate::outline::{Fill, Outline, Probe};
use crate::select::MarqueeMode;

/// Why a path cannot be a lasso.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LassoError {
    /// A point is not a usable number.
    #[error("a point of the path is not a usable number")]
    NotFinite,
    /// The path has fewer than three distinct points after simplifying.
    #[error("the path has only {have} distinct point(s) after simplifying, and a lasso needs 3")]
    TooFew {
        /// Points left.
        have: usize,
    },
    /// All points lie on one line, so the path encloses nothing.
    #[error("the points lie on one line, so the path encloses no area")]
    NoArea,
}

/// A closed free-hand outline. The last point joins the first.
#[derive(Debug, Clone, PartialEq)]
pub struct Lasso {
    points: Vec<Point>,
    bounds: Rect,
}

impl Lasso {
    /// A lasso through `points`. Repeated points are dropped; with `tolerance` above zero the path is simplified so
    /// that no dropped point is farther than `tolerance` from the result.
    pub fn new(points: &[Point], tolerance: f64) -> Result<Self, LassoError> {
        if points.iter().any(|p| !crate::point_is_finite(*p)) {
            return Err(LassoError::NotFinite);
        }
        let mut path: Vec<Point> = Vec::with_capacity(points.len());
        for p in points {
            if path.last() != Some(p) {
                path.push(*p);
            }
        }
        if path.len() > 1 && path.first() == path.last() {
            path.pop();
        }
        check_area(&path)?;
        if tolerance.is_finite() && tolerance > 0.0 {
            path = simplify(&path, tolerance);
            check_area(&path)?;
        }
        let bounds = aabb::from_points(path.iter().copied()).ok_or(LassoError::TooFew { have: 0 })?;
        Ok(Self { points: path, bounds })
    }

    /// The corners of the outline, in drawing order (the closing edge is implied).
    pub fn points(&self) -> &[Point] {
        &self.points
    }

    /// The bounds of the outline.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// The edges, the closing one last.
    fn edges(&self) -> impl Iterator<Item = (Point, Point)> + '_ {
        let next = self.points.iter().copied().cycle().skip(1);
        self.points.iter().copied().zip(next)
    }

    /// Is `p` inside the outline (non-zero winding), or on it?
    pub fn contains(&self, p: Point) -> bool {
        if !crate::point_is_finite(p) || !aabb::contains_point(&self.bounds, p) {
            return false;
        }
        let mut winding = 0_i32;
        for (a, b) in self.edges() {
            let side = cross(a, b, p);
            if side == 0.0 && on_segment(a, b, p) {
                return true;
            }
            if a.y <= p.y {
                if b.y > p.y && side > 0.0 {
                    winding += 1;
                }
            } else if b.y <= p.y && side < 0.0 {
                winding -= 1;
            }
        }
        winding != 0
    }

    /// Does any edge of the lasso cross the outline of `shape`?
    fn crosses<O: Outline + ?Sized>(&self, shape: &O) -> bool {
        let sb = shape.bounds();
        self.edges().any(|(a, b)| {
            let probe = Probe::segment(a, b);
            aabb::segment_intersects_rect(a, b, &sb) && probe.is_valid() && !shape.intersect(&probe).is_empty()
        })
    }
}

/// Does the lasso select `shape`? `fill` says whether the shape's interior counts as part of it (see the module docs).
pub fn lasso_selects<O: Outline + ?Sized>(shape: &O, lasso: &Lasso, mode: MarqueeMode, fill: Fill) -> bool {
    if !shape.is_valid() {
        return false;
    }
    let bounds = shape.bounds();
    // One point of the outline decides whether the whole outline is inside when no lasso edge crosses it.
    let outline_inside = || shape.nearest(aabb::center(&bounds)).is_some_and(|n| lasso.contains(n.point));
    match mode {
        MarqueeMode::Contain => {
            aabb::contains_rect(&lasso.bounds, &bounds) && !lasso.crosses(shape) && outline_inside()
        }
        MarqueeMode::Touch => {
            aabb::intersects(&lasso.bounds, &bounds)
                && (lasso.crosses(shape)
                    || outline_inside()
                    || (fill.is_filled()
                        && shape.is_closed()
                        && lasso.points.first().is_some_and(|first| shape.contains(*first))))
        }
    }
}

/// A path needs three points that do not lie on one line.
fn check_area(path: &[Point]) -> Result<(), LassoError> {
    if path.len() < 3 {
        Err(LassoError::TooFew { have: path.len() })
    } else if is_flat(path) {
        Err(LassoError::NoArea)
    } else {
        Ok(())
    }
}

/// The signed area of the triangle `a`, `b`, `p`, doubled: positive when `p` is left of `a -> b` in y-down space.
fn cross(a: Point, b: Point, p: Point) -> f64 {
    (b.x - a.x) * (p.y - a.y) - (p.x - a.x) * (b.y - a.y)
}

fn on_segment(a: Point, b: Point, p: Point) -> bool {
    p.x >= a.x.min(b.x) && p.x <= a.x.max(b.x) && p.y >= a.y.min(b.y) && p.y <= a.y.max(b.y)
}

/// True when every point lies on the line through the first two (within a relative tolerance).
fn is_flat(path: &[Point]) -> bool {
    let (Some(&a), Some(&b)) = (path.first(), path.get(1)) else { return true };
    let scale = (b - a).hypot();
    path.iter().skip(2).all(|&p| cross(a, b, p).abs() <= 1e-9 * scale * scale.max((p - a).hypot()))
}

/// Ramer-Douglas-Peucker on an open path, keeping both ends. Iterative, so a long path cannot overflow the stack.
fn simplify(path: &[Point], tolerance: f64) -> Vec<Point> {
    if path.len() < 3 {
        return path.to_vec();
    }
    let mut keep = vec![false; path.len()];
    if let Some(k) = keep.first_mut() {
        *k = true;
    }
    if let Some(k) = keep.last_mut() {
        *k = true;
    }
    let mut stack = vec![(0_usize, path.len() - 1)];
    while let Some((first, last)) = stack.pop() {
        let (Some(&a), Some(&b)) = (path.get(first), path.get(last)) else { continue };
        let far = path
            .iter()
            .enumerate()
            .take(last)
            .skip(first + 1)
            .map(|(i, p)| (i, distance_to_segment(*p, a, b)))
            .max_by(|x, y| x.1.total_cmp(&y.1));
        if let Some((i, d)) = far
            && d > tolerance
        {
            if let Some(k) = keep.get_mut(i) {
                *k = true;
            }
            stack.push((first, i));
            stack.push((i, last));
        }
    }
    path.iter().zip(&keep).filter(|(_, k)| **k).map(|(p, _)| *p).collect()
}

fn distance_to_segment(p: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let len2 = ab.hypot2();
    if len2 <= f64::EPSILON {
        return (p - a).hypot();
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    (p - (a + ab * t)).hypot()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shapes::{EllipseShape, Polyline, RectShape};

    fn square(x0: f64, y0: f64, x1: f64, y1: f64) -> Lasso {
        let pts = [Point::new(x0, y0), Point::new(x1, y0), Point::new(x1, y1), Point::new(x0, y1)];
        Lasso::new(&pts, 0.0).unwrap()
    }

    fn rect(x: f64, y: f64, w: f64, h: f64) -> RectShape {
        RectShape::from_rect(Rect::new(x, y, x + w, y + h))
    }

    #[test]
    fn a_path_needs_three_points_that_enclose_an_area() {
        let p = Point::new;
        assert_eq!(Lasso::new(&[p(0.0, 0.0), p(1.0, 1.0)], 0.0), Err(LassoError::TooFew { have: 2 }));
        assert_eq!(Lasso::new(&[p(0.0, 0.0), p(0.0, 0.0), p(0.0, 0.0)], 0.0), Err(LassoError::TooFew { have: 1 }));
        assert_eq!(Lasso::new(&[p(0.0, 0.0), p(5.0, 5.0), p(10.0, 10.0)], 0.0), Err(LassoError::NoArea));
        assert_eq!(Lasso::new(&[p(0.0, 0.0), p(f64::NAN, 1.0), p(3.0, 4.0)], 0.0), Err(LassoError::NotFinite));
        assert_eq!(Lasso::new(&[p(0.0, 0.0), p(1e200, 1.0), p(3.0, 4.0)], 0.0), Err(LassoError::NotFinite));
        assert!(Lasso::new(&[p(0.0, 0.0), p(10.0, 0.0), p(0.0, 10.0)], 0.0).is_ok());
    }

    #[test]
    fn a_closing_point_equal_to_the_first_is_dropped() {
        let p = Point::new;
        let l = Lasso::new(&[p(0.0, 0.0), p(10.0, 0.0), p(0.0, 10.0), p(0.0, 0.0)], 0.0).unwrap();
        assert_eq!(l.points().len(), 3);
    }

    #[test]
    fn simplifying_drops_points_within_the_tolerance_and_keeps_the_corners() {
        let mut path = vec![Point::new(0.0, 0.0)];
        path.extend((1..100).map(|i| Point::new(f64::from(i), 0.2 * f64::from(i % 2))));
        path.extend([Point::new(100.0, 0.0), Point::new(100.0, 50.0), Point::new(0.0, 50.0)]);
        let l = Lasso::new(&path, 1.0).unwrap();
        assert_eq!(l.points().len(), 4, "{:?}", l.points());
        assert_eq!(l.bounds(), Rect::new(0.0, 0.0, 100.0, 50.0));
    }

    #[test]
    fn contains_uses_the_non_zero_winding_rule_and_counts_the_outline() {
        let l = square(0.0, 0.0, 10.0, 10.0);
        assert!(l.contains(Point::new(5.0, 5.0)));
        assert!(l.contains(Point::new(0.0, 5.0)), "on the outline");
        assert!(l.contains(Point::new(10.0, 10.0)), "on a corner");
        assert!(!l.contains(Point::new(10.1, 5.0)));
        assert!(!l.contains(Point::new(f64::NAN, 5.0)));
        // A bow tie: both loops are inside, the crossing point too.
        let p = Point::new;
        let bow = Lasso::new(&[p(0.0, 0.0), p(10.0, 10.0), p(10.0, 0.0), p(0.0, 10.0)], 0.0).unwrap();
        assert!(bow.contains(p(8.0, 5.0)) && bow.contains(p(2.0, 5.0)));
        assert!(!bow.contains(p(5.0, 1.0)), "above the crossing, between the loops");
    }

    #[test]
    fn touch_selects_what_the_lasso_crosses_or_surrounds() {
        let l = square(0.0, 0.0, 100.0, 100.0);
        let surrounded = rect(10.0, 10.0, 20.0, 20.0);
        let crossed = rect(80.0, 80.0, 50.0, 50.0);
        let apart = rect(200.0, 0.0, 20.0, 20.0);
        for filled in [false, true] {
            assert!(lasso_selects(&surrounded, &l, MarqueeMode::Touch, Fill::from(filled)));
            assert!(lasso_selects(&crossed, &l, MarqueeMode::Touch, Fill::from(filled)));
            assert!(!lasso_selects(&apart, &l, MarqueeMode::Touch, Fill::from(filled)));
        }
    }

    #[test]
    fn touch_in_a_filled_shape_selects_it_and_in_an_empty_one_does_not() {
        let l = square(40.0, 40.0, 60.0, 60.0);
        let big = rect(0.0, 0.0, 100.0, 100.0);
        assert!(lasso_selects(&big, &l, MarqueeMode::Touch, Fill::Filled));
        assert!(!lasso_selects(&big, &l, MarqueeMode::Touch, Fill::Stroke));
    }

    #[test]
    fn contain_needs_the_whole_outline_inside() {
        let l = square(0.0, 0.0, 100.0, 100.0);
        let inside = rect(10.0, 10.0, 20.0, 20.0);
        let crossed = rect(80.0, 80.0, 50.0, 50.0);
        let around = rect(-10.0, -10.0, 200.0, 200.0);
        assert!(lasso_selects(&inside, &l, MarqueeMode::Contain, Fill::Filled));
        assert!(!lasso_selects(&crossed, &l, MarqueeMode::Contain, Fill::Filled));
        assert!(!lasso_selects(&around, &l, MarqueeMode::Contain, Fill::Filled));
    }

    #[test]
    fn a_concave_lasso_does_not_contain_what_sits_in_its_notch() {
        // A "U": the notch between the arms is outside the lasso although inside its bounds.
        let p = Point::new;
        let u = Lasso::new(
            &[
                p(0.0, 0.0),
                p(30.0, 0.0),
                p(30.0, 70.0),
                p(70.0, 70.0),
                p(70.0, 0.0),
                p(100.0, 0.0),
                p(100.0, 100.0),
                p(0.0, 100.0),
            ],
            0.0,
        )
        .unwrap();
        let in_notch = rect(40.0, 10.0, 20.0, 20.0);
        let in_arm = rect(5.0, 10.0, 20.0, 20.0);
        let across = rect(20.0, 10.0, 30.0, 20.0);
        assert!(!lasso_selects(&in_notch, &u, MarqueeMode::Contain, Fill::Filled));
        assert!(!lasso_selects(&in_notch, &u, MarqueeMode::Touch, Fill::Stroke));
        assert!(lasso_selects(&in_arm, &u, MarqueeMode::Contain, Fill::Filled));
        assert!(!lasso_selects(&across, &u, MarqueeMode::Contain, Fill::Filled));
        assert!(lasso_selects(&across, &u, MarqueeMode::Touch, Fill::Stroke));
    }

    #[test]
    fn lines_and_ellipses_are_tested_by_their_outline() {
        let l = square(0.0, 0.0, 100.0, 100.0);
        let line_in = Polyline::new(vec![Point::new(10.0, 10.0), Point::new(90.0, 20.0)], false);
        let line_out = Polyline::new(vec![Point::new(110.0, 10.0), Point::new(190.0, 20.0)], false);
        let line_cross = Polyline::new(vec![Point::new(50.0, 50.0), Point::new(150.0, 50.0)], false);
        assert!(lasso_selects(&line_in, &l, MarqueeMode::Contain, Fill::Stroke));
        assert!(!lasso_selects(&line_out, &l, MarqueeMode::Touch, Fill::Stroke));
        assert!(lasso_selects(&line_cross, &l, MarqueeMode::Touch, Fill::Stroke));
        assert!(!lasso_selects(&line_cross, &l, MarqueeMode::Contain, Fill::Stroke));
        let ellipse = EllipseShape::new(crate::OrientedBox::from_xywh(20.0, 20.0, 60.0, 40.0, 0.0));
        assert!(lasso_selects(&ellipse, &l, MarqueeMode::Contain, Fill::Filled));
    }

    #[test]
    fn a_shape_that_is_not_valid_is_never_selected() {
        let l = square(0.0, 0.0, 100.0, 100.0);
        let bad = rect(f64::NAN, 0.0, 10.0, 10.0);
        assert!(!lasso_selects(&bad, &l, MarqueeMode::Touch, Fill::Filled));
        assert!(!lasso_selects(&bad, &l, MarqueeMode::Contain, Fill::Filled));
    }
}
