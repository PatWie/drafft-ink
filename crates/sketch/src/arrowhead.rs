//! Arrowheads. Heads are appended to the shaft as extra subpaths, so the whole arrow goes through
//! one sketch pass and one random stream, exactly like the old app's arrow (shaft first, then the
//! head).

use kurbo::{BezPath, Circle, Point, Shape, Vec2};

use crate::input::{Arrowhead, HeadKind};

/// Where the subpaths of one head are in the arrow path, and whether it is filled.
#[derive(Debug, Clone, Copy)]
pub(crate) struct HeadSpan {
    /// Index of the first subpath of the head. The shaft is subpath 0.
    pub first: usize,
    pub count: usize,
    pub filled: bool,
}

/// Append `head` at `tip` to `path`. `dir` is the unit vector pointing away from the line, out
/// of the tip. Returns the span, or `None` when nothing was added.
pub(crate) fn append(
    path: &mut BezPath,
    subpaths_so_far: usize,
    head: Arrowhead,
    tip: Point,
    dir: Vec2,
    size: f64,
) -> Option<HeadSpan> {
    let perp = Vec2::new(-dir.y, dir.x);
    // The old app's arrowhead: back = tip - dir * size, wings at +-size/2.
    let back = tip - dir * size;
    let left = back + perp * (size * 0.5);
    let right = back - perp * (size * 0.5);
    let count = match head.kind {
        HeadKind::None => return None,
        HeadKind::Arrow => {
            path.move_to(tip);
            path.line_to(left);
            path.move_to(tip);
            path.line_to(right);
            2
        }
        HeadKind::Triangle => {
            path.move_to(tip);
            path.line_to(left);
            path.line_to(right);
            path.close_path();
            1
        }
        HeadKind::Bar => {
            path.move_to(tip + perp * (size * 0.5));
            path.line_to(tip - perp * (size * 0.5));
            1
        }
        HeadKind::Diamond => {
            path.move_to(tip);
            path.line_to(tip - dir * (size * 0.5) + perp * (size * 0.3));
            path.line_to(back);
            path.line_to(tip - dir * (size * 0.5) - perp * (size * 0.3));
            path.close_path();
            1
        }
        HeadKind::Circle => {
            let d = size * 0.7;
            let c = Circle::new(tip - dir * (d / 2.0), d / 2.0);
            for el in c.path_elements(0.1) {
                path.push(el);
            }
            1
        }
    };
    let filled = head.filled && matches!(head.kind, HeadKind::Triangle | HeadKind::Diamond | HeadKind::Circle);
    Some(HeadSpan { first: subpaths_so_far, count, filled })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_arrow_head_is_two_wings() {
        let mut p = BezPath::new();
        let s = append(&mut p, 1, Arrowhead::ARROW, Point::new(100.0, 0.0), Vec2::new(1.0, 0.0), 15.0);
        assert_eq!(s.map(|s| (s.first, s.count, s.filled)), Some((1, 2, false)));
        assert_eq!(p.elements().len(), 4);
    }

    #[test]
    fn every_kind_adds_geometry_near_the_tip() {
        for h in [
            Arrowhead::ARROW,
            Arrowhead::TRIANGLE,
            Arrowhead::TRIANGLE_OUTLINE,
            Arrowhead::CIRCLE,
            Arrowhead::CIRCLE_OUTLINE,
            Arrowhead::BAR,
            Arrowhead::DIAMOND,
            Arrowhead::DIAMOND_OUTLINE,
        ] {
            let mut p = BezPath::new();
            let s = append(&mut p, 1, h, Point::new(50.0, 50.0), Vec2::new(0.0, -1.0), 20.0);
            assert!(s.is_some());
            let b = p.bounding_box();
            assert!(b.x0 >= 50.0 - 20.0 && b.x1 <= 50.0 + 20.0, "{h:?} {b:?}");
            assert!(b.y0 >= 50.0 - 1e-9 && b.y1 <= 50.0 + 20.0 + 1e-9, "{h:?} {b:?}");
        }
        let mut p = BezPath::new();
        assert!(append(&mut p, 1, Arrowhead::NONE, Point::ZERO, Vec2::new(1.0, 0.0), 10.0).is_none());
        assert!(p.elements().is_empty());
    }

    #[test]
    fn only_closed_kinds_can_be_filled() {
        let mut p = BezPath::new();
        let a = Arrowhead { kind: HeadKind::Arrow, filled: true };
        assert!(!append(&mut p, 1, a, Point::ZERO, Vec2::new(1.0, 0.0), 10.0).is_some_and(|s| s.filled));
        let t = append(&mut p, 2, Arrowhead::TRIANGLE, Point::ZERO, Vec2::new(1.0, 0.0), 10.0);
        assert!(t.is_some_and(|s| s.filled));
    }
}
