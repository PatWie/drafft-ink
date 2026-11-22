//! The chisel tip of a highlighter marker.
//!
//! The nib is a flat rectangle held at a fixed angle to the page. A stroke is the nib swept along the path of
//! the pen, so the mark is wide when the pen moves across the nib and thin when it moves along it, and the ends
//! of a stroke are flat cuts of the nib instead of round caps.
//!
//! The sweep of a convex nib along one segment is the convex hull of the nib at both ends of the segment (the
//! Minkowski sum of the nib and the segment). The outline of a whole stroke is those hulls, one sub-path per
//! segment, all wound the same way, so a single non-zero fill gives their union. Nothing here is random or
//! depends on the zoom: the same points always give the same outline.

use kurbo::{BezPath, Point, Vec2};

use crate::obox::OrientedBox;

/// The angle of the long edge of the nib to the page, in degrees. The edge rises to the right on screen, as the
/// nib of a right-handed marker does.
pub const NIB_ANGLE_DEGREES: f64 = 40.0;

/// The thickness of the nib (its short side) as a fraction of its length (its long side).
pub const NIB_THICKNESS_RATIO: f64 = 0.25;

/// Segments shorter than this (in world units) are treated as a standing nib.
const MIN_SEGMENT: f64 = 1e-9;

/// A flat rectangular nib. Build it with [`Nib::new`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Nib {
    /// Half of the long side, as a vector from the centre of the nib to the middle of a short side.
    long: Vec2,
    /// Half of the short side, as a vector from the centre of the nib to the middle of a long side.
    thin: Vec2,
}

impl Nib {
    /// A nib whose long side is `length` (a highlighter's stroke width), or `None` when `length` is not a usable
    /// positive number.
    pub fn new(length: f64) -> Option<Self> {
        if !(length > 0.0 && crate::num_is_sane(length)) {
            return None;
        }
        let (sin, cos) = NIB_ANGLE_DEGREES.to_radians().sin_cos();
        let along = Vec2::new(cos, -sin);
        let across = Vec2::new(sin, cos);
        Some(Self { long: along * (length / 2.0), thin: across * (length * NIB_THICKNESS_RATIO / 2.0) })
    }

    /// The long side of the nib: the widest the mark gets.
    pub fn length(&self) -> f64 {
        self.long.hypot() * 2.0
    }

    /// The short side of the nib: the thinnest the mark gets.
    pub fn thickness(&self) -> f64 {
        self.thin.hypot() * 2.0
    }

    /// The distance from the centre of the nib to its farthest corner. A stroke never reaches further than this
    /// from its path.
    pub fn reach(&self) -> f64 {
        self.long.hypot().hypot(self.thin.hypot())
    }

    /// The width of the mark, measured across its direction, when the pen moves along `direction`. `None` when
    /// `direction` has no length.
    pub fn width_moving_along(&self, direction: Vec2) -> Option<f64> {
        let len = direction.hypot();
        if !(len > MIN_SEGMENT && len.is_finite()) {
            return None;
        }
        let normal = Vec2::new(-direction.y, direction.x) / len;
        Some(2.0 * (self.long.dot(normal).abs() + self.thin.dot(normal).abs()))
    }

    /// The four corners of the nib centred on `at`.
    fn corners(&self, at: Point) -> [Point; 4] {
        [at - self.long - self.thin, at + self.long - self.thin, at + self.long + self.thin, at - self.long + self.thin]
    }

    /// The convex hull of the nib at `from` and at `to`, counter-clockwise in the maths sense (so clockwise on
    /// screen, where y points down). `scratch` is reused between calls.
    fn hull(&self, from: Point, to: Point, scratch: &mut Vec<Point>) -> Vec<Point> {
        scratch.clear();
        scratch.extend(self.corners(from));
        if (to - from).hypot() > MIN_SEGMENT {
            scratch.extend(self.corners(to));
        }
        convex_hull(scratch)
    }

    /// The finite points of `path` as the nib's resting places: each segment between two of them, or the single
    /// point alone.
    fn hulls(&self, path: &[Point]) -> Vec<Vec<Point>> {
        let pts: Vec<Point> = path.iter().copied().filter(|p| crate::point_is_finite(*p)).collect();
        let mut scratch = Vec::with_capacity(8);
        match pts.as_slice() {
            [] => Vec::new(),
            [only] => vec![self.hull(*only, *only, &mut scratch)],
            _ => pts
                .windows(2)
                .filter_map(|w| match w {
                    [a, b] => Some(self.hull(*a, *b, &mut scratch)),
                    _ => None,
                })
                .collect(),
        }
    }

    /// The outline of the stroke the nib draws along `path`: one closed polygon per segment (the whole nib for a
    /// single point). Fill it with the non-zero rule, in one opaque colour, to get the mark. Overlapping parts
    /// of the stroke are one shape, so a translucent layer over it does not darken where the path crosses
    /// itself.
    pub fn outline(&self, path: &[Point]) -> BezPath {
        let mut out = BezPath::new();
        for hull in self.hulls(path) {
            let mut points = hull.into_iter();
            if let Some(first) = points.next() {
                out.move_to(first);
                points.for_each(|p| out.line_to(p));
                out.close_path();
            }
        }
        out
    }

    /// Does the stroke the nib draws along `path` touch `target`?
    pub fn touches(&self, path: &[Point], target: &OrientedBox) -> bool {
        if !target.is_valid() {
            return false;
        }
        let corners = target.corners();
        self.hulls(path).iter().any(|hull| !separated(hull, &corners))
    }
}

/// Andrew's monotone chain. The result has no repeated or collinear points and winds counter-clockwise in the
/// maths sense. Fewer than three distinct points come back as they are.
fn convex_hull(points: &mut [Point]) -> Vec<Point> {
    points.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    let cross = |o: Point, a: Point, b: Point| (a - o).cross(b - o);
    let half = |iter: &mut dyn Iterator<Item = Point>| {
        let mut chain: Vec<Point> = Vec::new();
        for p in iter {
            while let [.., a, b] = chain.as_slice()
                && cross(*a, *b, p) <= 0.0
            {
                chain.pop();
            }
            chain.push(p);
        }
        chain.pop();
        chain
    };
    let mut hull = half(&mut points.iter().copied());
    hull.extend(half(&mut points.iter().rev().copied()));
    hull
}

/// True when a line separates the convex polygons `a` and `b`: one of their edge directions does (separating
/// axis test).
fn separated(a: &[Point], b: &[Point]) -> bool {
    [a, b].into_iter().any(|poly| {
        poly.iter().zip(poly.iter().cycle().skip(1)).any(|(p, q)| {
            let edge = *q - *p;
            let axis = Vec2::new(-edge.y, edge.x);
            let (a0, a1) = project(a, axis);
            let (b0, b1) = project(b, axis);
            a1 < b0 || b1 < a0
        })
    })
}

fn project(poly: &[Point], axis: Vec2) -> (f64, f64) {
    poly.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
        let d = p.to_vec2().dot(axis);
        (lo.min(d), hi.max(d))
    })
}

#[cfg(test)]
mod tests {
    use kurbo::{PathEl, Shape, Size};

    use super::*;

    const LENGTH: f64 = 12.0;

    fn nib() -> Nib {
        Nib::new(LENGTH).expect("a usable length")
    }

    /// The direction of the long side of the nib.
    fn along() -> Vec2 {
        let a = NIB_ANGLE_DEGREES.to_radians();
        Vec2::new(a.cos(), -a.sin())
    }

    fn vertices(path: &BezPath) -> Vec<Point> {
        path.elements()
            .iter()
            .filter_map(|el| match el {
                PathEl::MoveTo(p) | PathEl::LineTo(p) => Some(*p),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_nib_needs_a_positive_finite_length() {
        for bad in [0.0, -3.0, f64::NAN, f64::INFINITY, 1e200] {
            assert!(Nib::new(bad).is_none(), "{bad}");
        }
        let n = nib();
        assert!((n.length() - LENGTH).abs() < 1e-9);
        assert!((n.thickness() - LENGTH * NIB_THICKNESS_RATIO).abs() < 1e-9);
    }

    #[test]
    fn the_mark_is_wide_across_the_nib_and_thin_along_it() {
        let n = nib();
        let along_nib = n.width_moving_along(along()).expect("a direction");
        let across_nib = n.width_moving_along(Vec2::new(-along().y, along().x)).expect("a direction");
        assert!((along_nib - n.thickness()).abs() < 1e-9, "moving along the nib draws its thickness: {along_nib}");
        assert!((across_nib - n.length()).abs() < 1e-9, "moving across the nib draws its length: {across_nib}");
        let horizontal = n.width_moving_along(Vec2::new(1.0, 0.0)).expect("a direction");
        assert!(along_nib < horizontal && horizontal < across_nib, "{along_nib} < {horizontal} < {across_nib}");
        assert_eq!(n.width_moving_along(Vec2::ZERO), None);
    }

    #[test]
    fn the_outline_of_a_straight_stroke_has_the_width_the_nib_gives() {
        // Move along the long side: the stroke is as wide as the nib is thick.
        let n = nib();
        let start = Point::new(10.0, 10.0);
        let end = start + along() * 100.0;
        let outline = n.outline(&[start, end]);
        let normal = Vec2::new(-along().y, along().x);
        let widths: Vec<f64> = vertices(&outline).iter().map(|p| (*p - start).dot(normal)).collect();
        let (lo, hi) = widths.iter().fold((f64::MAX, f64::MIN), |(lo, hi), w| (lo.min(*w), hi.max(*w)));
        assert!((hi - lo - n.thickness()).abs() < 1e-9, "{}", hi - lo);
    }

    #[test]
    fn the_ends_are_flat_cuts_of_the_nib() {
        let n = nib();
        // Move straight across the nib: each end is the whole long side, a straight edge square to the motion.
        let direction = Vec2::new(-along().y, along().x);
        let (start, end) = (Point::new(0.0, 0.0), Point::new(0.0, 0.0) + direction * 80.0);
        let outline = n.outline(&[start, end]);
        assert!(
            outline.elements().iter().all(|el| matches!(el, PathEl::MoveTo(_) | PathEl::LineTo(_) | PathEl::ClosePath)),
            "no arcs or curves anywhere"
        );
        let ahead: Vec<f64> = vertices(&outline).iter().map(|p| (*p - start).dot(direction)).collect();
        let far = ahead.iter().copied().fold(f64::MIN, f64::max);
        let tip: Vec<Point> =
            vertices(&outline).into_iter().filter(|p| ((*p - start).dot(direction) - far).abs() < 1e-9).collect();
        assert_eq!(tip.len(), 2, "a flat end has two corners at the farthest distance");
        assert!(((tip[0] - tip[1]).hypot() - n.length()).abs() < 1e-9, "and they are the ends of the long side");
    }

    #[test]
    fn a_single_point_is_the_nib_itself() {
        let n = nib();
        let at = Point::new(5.0, 5.0);
        let outline = n.outline(&[at]);
        let corners = n.corners(at);
        let mut got = vertices(&outline);
        got.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
        let mut want = corners.to_vec();
        want.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
        assert_eq!(got.len(), 4);
        assert!(got.iter().zip(&want).all(|(g, w)| (*g - *w).hypot() < 1e-9));
        assert!((outline.area().abs() - n.length() * n.thickness()).abs() < 1e-9);
    }

    #[test]
    fn the_outline_is_the_same_every_time() {
        let n = nib();
        let path: Vec<Point> =
            (0..200).map(|i| Point::new(f64::from(i) * 1.7, 30.0 * (f64::from(i) * 0.09).sin())).collect();
        assert_eq!(n.outline(&path), n.outline(&path));
        assert_eq!(n.outline(&path), Nib::new(LENGTH).expect("a nib").outline(&path));
    }

    #[test]
    fn every_piece_winds_the_same_way_so_one_fill_unions_them() {
        let n = nib();
        let path = [Point::new(0.0, 0.0), Point::new(40.0, 10.0), Point::new(20.0, 50.0), Point::new(0.0, 5.0)];
        let outline = n.outline(&path);
        let pieces: Vec<f64> = outline
            .elements()
            .split_inclusive(|el| matches!(el, PathEl::ClosePath))
            .map(|els| BezPath::from_vec(els.to_vec()).area())
            .collect();
        assert_eq!(pieces.len(), path.len() - 1);
        assert!(pieces.iter().all(|a| *a > 0.0) || pieces.iter().all(|a| *a < 0.0), "{pieces:?}");
    }

    #[test]
    fn hostile_points_are_skipped() {
        let n = nib();
        let outline = n.outline(&[Point::new(f64::NAN, 0.0), Point::new(0.0, 0.0), Point::new(f64::INFINITY, 1.0)]);
        assert_eq!(vertices(&outline).len(), 4, "the one usable point is a dot");
        assert!(n.outline(&[]).elements().is_empty());
        assert!(n.outline(&[Point::new(f64::NAN, f64::NAN)]).elements().is_empty());
    }

    #[test]
    fn a_stroke_touches_the_boxes_it_crosses_and_not_those_it_misses() {
        let n = nib();
        let stroke = [Point::new(0.0, 50.0), Point::new(200.0, 50.0)];
        let crossed = OrientedBox::from_xywh(80.0, 0.0, 40.0, 100.0, 0.0);
        let beside = OrientedBox::from_xywh(80.0, 80.0, 40.0, 40.0, 0.0);
        let beyond = OrientedBox::from_xywh(300.0, 0.0, 40.0, 100.0, 0.0);
        assert!(n.touches(&stroke, &crossed));
        assert!(!n.touches(&stroke, &beside));
        assert!(!n.touches(&stroke, &beyond));
        // A rotated box whose corner box overlaps the stroke's but whose body does not.
        let tilted = OrientedBox::new(Point::new(100.0, 90.0), Size::new(60.0, 6.0), std::f64::consts::FRAC_PI_4);
        assert!(!n.touches(&stroke, &tilted));
        // A dot touches what lies under it.
        assert!(n.touches(&[Point::new(100.0, 50.0)], &crossed));
    }

    #[test]
    fn a_diagonal_stroke_does_not_touch_a_box_in_the_empty_corner_of_its_bounds() {
        let n = nib();
        let stroke = [Point::new(0.0, 0.0), Point::new(200.0, 200.0)];
        let corner = OrientedBox::from_xywh(150.0, 10.0, 30.0, 30.0, 0.0);
        assert!(!n.touches(&stroke, &corner));
        let on_it = OrientedBox::from_xywh(90.0, 90.0, 20.0, 20.0, 0.0);
        assert!(n.touches(&stroke, &on_it));
    }
}
