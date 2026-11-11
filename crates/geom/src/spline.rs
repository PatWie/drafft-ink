//! The curve of a rounded line: a Catmull-Rom spline through the points, tension 0.5.
//!
//! Drawing, hit testing and the point editor all follow this one curve, so they agree by construction.

use kurbo::{BezPath, CubicBez, Point, Vec2};

/// One cubic per segment of the Catmull-Rom spline through `points` (tension 0.5). The first and last point
/// repeat as their own neighbours, so the curve starts and ends straight. Fewer than two points give nothing;
/// two points give one straight cubic.
pub fn catmull_rom_cubics(points: &[Point]) -> impl Iterator<Item = CubicBez> + '_ {
    let last = points.len().saturating_sub(1);
    let at = move |i: usize| points.get(i.min(last)).copied().unwrap_or(Point::ZERO);
    (0..last).map(move |i| {
        let (p0, p1, p2, p3) = (at(i.saturating_sub(1)), at(i), at(i + 1), at(i + 2));
        let t1 = Vec2::new((p2.x - p0.x) * 0.5, (p2.y - p0.y) * 0.5);
        let t2 = Vec2::new((p3.x - p1.x) * 0.5, (p3.y - p1.y) * 0.5);
        CubicBez::new(p1, p1 + t1 / 3.0, p2 - t2 / 3.0, p2)
    })
}

/// The spline through `points` as a path, closed with a straight segment back to the start when `closed`.
/// Empty for fewer than two points.
pub fn catmull_rom_path(points: &[Point], closed: bool) -> BezPath {
    let mut path = BezPath::new();
    let Some((first, rest)) = points.split_first() else { return path };
    if rest.is_empty() {
        return path;
    }
    path.move_to(*first);
    for c in catmull_rom_cubics(points) {
        path.curve_to(c.p1, c.p2, c.p3);
    }
    if closed {
        path.close_path();
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::PathEl;

    #[test]
    fn two_points_make_one_straight_cubic() {
        let pts = [Point::new(0.0, 0.0), Point::new(30.0, 0.0)];
        let c: Vec<_> = catmull_rom_cubics(&pts).collect();
        assert_eq!(c.len(), 1);
        assert_eq!((c[0].p0, c[0].p3), (pts[0], pts[1]));
        assert!(c[0].p1.y == 0.0 && c[0].p2.y == 0.0);
    }

    #[test]
    fn the_curve_passes_through_every_point_and_is_continuous() {
        let pts = [Point::new(0.0, 0.0), Point::new(10.0, 20.0), Point::new(30.0, 10.0), Point::new(40.0, 40.0)];
        let c: Vec<_> = catmull_rom_cubics(&pts).collect();
        assert_eq!(c.len(), 3);
        for (i, seg) in c.iter().enumerate() {
            assert_eq!((seg.p0, seg.p3), (pts[i], pts[i + 1]));
        }
    }

    #[test]
    fn fewer_than_two_points_make_nothing() {
        assert_eq!(catmull_rom_cubics(&[]).count(), 0);
        assert_eq!(catmull_rom_cubics(&[Point::new(1.0, 1.0)]).count(), 0);
        assert!(catmull_rom_path(&[Point::new(1.0, 1.0)], true).elements().is_empty());
    }

    #[test]
    fn the_path_closes_only_when_asked() {
        let pts = [Point::new(0.0, 0.0), Point::new(10.0, 5.0), Point::new(20.0, 0.0)];
        assert!(matches!(catmull_rom_path(&pts, true).elements().last(), Some(PathEl::ClosePath)));
        assert!(matches!(catmull_rom_path(&pts, false).elements().last(), Some(PathEl::CurveTo(..))));
    }
}
