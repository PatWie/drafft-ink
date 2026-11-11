//! Clean (unsketched) outlines. These match the old app's `Shape::to_path` for rectangles,
//! ellipses, lines and freehand strokes.

use drafftink_geom::spline::catmull_rom_cubics;
use kurbo::{BezPath, Ellipse, Point, Rect, RoundedRect, Shape, Vec2};

use crate::input::Curve;

/// Flattening tolerance of the old app's `to_path(0.1)`.
const TOLERANCE: f64 = 0.1;

pub(crate) fn rect(width: f64, height: f64, radius: f64) -> BezPath {
    let r = Rect::new(0.0, 0.0, width, height);
    if radius > 0.0 { RoundedRect::from_rect(r, radius).to_path(TOLERANCE) } else { r.to_path(TOLERANCE) }
}

pub(crate) fn ellipse(width: f64, height: f64) -> BezPath {
    Ellipse::new(Point::new(width / 2.0, height / 2.0), Vec2::new(width / 2.0, height / 2.0), 0.0).to_path(TOLERANCE)
}

/// A diamond through the middles of the sides. With a radius, each corner is cut off by a
/// quadratic curve; the cut length is the radius, but at most half the adjacent side.
pub(crate) fn diamond(width: f64, height: f64, radius: f64) -> BezPath {
    let v = [
        Point::new(width / 2.0, 0.0),
        Point::new(width, height / 2.0),
        Point::new(width / 2.0, height),
        Point::new(0.0, height / 2.0),
    ];
    let mut path = BezPath::new();
    if radius <= 0.0 {
        path.move_to(v[0]);
        for p in &v[1..] {
            path.line_to(*p);
        }
        path.close_path();
        return path;
    }
    // For each corner: the point where the rounding starts and where it ends.
    let corner = |i: usize| -> (Point, Point) {
        let prev = v[(i + 3) % 4];
        let cur = v[i];
        let next = v[(i + 1) % 4];
        let to_prev = prev - cur;
        let to_next = next - cur;
        let cut = |d: Vec2| -> Vec2 {
            let len = d.hypot();
            if len <= f64::EPSILON { Vec2::ZERO } else { d * (radius.min(len / 2.0) / len) }
        };
        (cur + cut(to_prev), cur + cut(to_next))
    };
    let (_, first_out) = corner(0);
    path.move_to(first_out);
    for i in 1..=4 {
        let (inp, out) = corner(i % 4);
        path.line_to(inp);
        path.quad_to(v[i % 4], out);
    }
    // The loop's last step rounds corner 0 and ends where the path started.
    path.close_path();
    path
}

/// Straight segments or a Catmull-Rom spline (tension 0.5, the old "flowing" style) through the
/// points. Needs at least two points, otherwise empty.
pub(crate) fn through_points(points: &[Point], curve: Curve, closed: bool) -> BezPath {
    let mut path = BezPath::new();
    let Some((first, rest)) = points.split_first() else { return path };
    if rest.is_empty() {
        return path;
    }
    path.move_to(*first);
    match curve {
        Curve::Straight => {
            for p in rest {
                path.line_to(*p);
            }
        }
        Curve::Smooth => {
            for c in catmull_rom_cubics(points) {
                path.curve_to(c.p1, c.p2, c.p3);
            }
        }
    }
    if closed {
        path.close_path();
    }
    path
}

/// True when all points are the same (nothing to draw).
pub(crate) fn all_equal(points: &[Point]) -> bool {
    points.windows(2).all(|w| w[0] == w[1])
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::PathEl;

    #[test]
    fn rect_has_the_old_element_layout() {
        let p = rect(10.0, 20.0, 0.0);
        assert_eq!(
            p.elements(),
            &[
                PathEl::MoveTo(Point::new(0.0, 0.0)),
                PathEl::LineTo(Point::new(10.0, 0.0)),
                PathEl::LineTo(Point::new(10.0, 20.0)),
                PathEl::LineTo(Point::new(0.0, 20.0)),
                PathEl::ClosePath,
            ]
        );
    }

    #[test]
    fn rounded_rect_and_ellipse_fit_their_box() {
        let b = rect(100.0, 50.0, 10.0).bounding_box();
        assert!((b.width() - 100.0).abs() < 1e-6 && (b.height() - 50.0).abs() < 1e-6);
        let b = ellipse(80.0, 40.0).bounding_box();
        assert!((b.width() - 80.0).abs() < 0.2 && (b.height() - 40.0).abs() < 0.2);
    }

    #[test]
    fn diamond_stays_in_its_box_with_and_without_rounding() {
        for r in [0.0, 5.0, 1000.0] {
            let b = diamond(60.0, 40.0, r).bounding_box();
            assert!(b.x0 >= -1e-9 && b.y0 >= -1e-9 && b.x1 <= 60.0 + 1e-9 && b.y1 <= 40.0 + 1e-9);
        }
        assert_eq!(diamond(60.0, 40.0, 0.0).elements().len(), 5);
    }

    #[test]
    fn spline_and_polyline() {
        let pts = [Point::new(0.0, 0.0), Point::new(10.0, 10.0), Point::new(20.0, 0.0)];
        assert_eq!(through_points(&pts, Curve::Straight, false).elements().len(), 3);
        let s = through_points(&pts, Curve::Smooth, true);
        assert_eq!(s.elements().len(), 4);
        assert!(through_points(&pts[..1], Curve::Straight, false).elements().is_empty());
        assert!(all_equal(&[pts[0], pts[0]]));
        assert!(!all_equal(&pts));
    }
}
