//! Arrowheads. Heads are appended to the shaft as extra subpaths, so the whole arrow goes through
//! one sketch pass and one random stream, exactly like the old app's arrow (shaft first, then the
//! head).

use kurbo::{BezPath, Circle, Point, Shape, Vec2};

use crate::input::Arrowhead;

/// The most of its end segment a head may cover: at most half. The head of the other end of a two-point arrow then
/// still fits in the other half. Excalidraw uses the same half (`getArrowheadPoints`, `lengthMultiplier`).
pub const HEAD_MAX_SEGMENT_SHARE: f64 = 0.5;

/// The smallest a head is shrunk to, in world units. A segment shorter than twice this still gets a head of this
/// size, so a tiny arrow keeps a visible head. A head that was asked to be smaller than this is not enlarged.
pub const HEAD_MIN_SIZE: f64 = 4.0;

/// The size a head is drawn at, given the size it was asked for and the length of the segment that ends in its tip
/// (the last segment for an end head, the first for a start head; for a two-point arrow the whole arrow).
///
/// A segment of at least `size / HEAD_MAX_SEGMENT_SHARE` keeps the full `size`. A shorter one gets
/// `HEAD_MAX_SEGMENT_SHARE` of its length, but never less than [`HEAD_MIN_SIZE`] and never more than `size`. The
/// result grows with the segment length and never exceeds `size`, so bounds computed from `size` always hold the
/// drawn head.
///
/// A `size` that is not a positive finite number gives `0.0`. A negative or zero `segment_len` counts as no segment
/// (the minimum); one that is not finite (NaN, infinite) counts as unknown and keeps `size`.
pub fn fit_head_size(size: f64, segment_len: f64) -> f64 {
    if !(size.is_finite() && size > 0.0) {
        return 0.0;
    }
    if segment_len.is_nan() {
        return size;
    }
    let floor = HEAD_MIN_SIZE.min(size);
    (segment_len.max(0.0) * HEAD_MAX_SEGMENT_SHARE).max(floor).min(size)
}

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
    let count = match head {
        Arrowhead::None => return None,
        Arrowhead::Arrow => {
            path.move_to(tip);
            path.line_to(left);
            path.move_to(tip);
            path.line_to(right);
            2
        }
        Arrowhead::Triangle(_) => {
            path.move_to(tip);
            path.line_to(left);
            path.line_to(right);
            path.close_path();
            1
        }
        Arrowhead::Bar => {
            path.move_to(tip + perp * (size * 0.5));
            path.line_to(tip - perp * (size * 0.5));
            1
        }
        Arrowhead::Diamond(_) => {
            path.move_to(tip);
            path.line_to(tip - dir * (size * 0.5) + perp * (size * 0.3));
            path.line_to(back);
            path.line_to(tip - dir * (size * 0.5) - perp * (size * 0.3));
            path.close_path();
            1
        }
        Arrowhead::Circle(_) => {
            let d = size * 0.7;
            let c = Circle::new(tip - dir * (d / 2.0), d / 2.0);
            for el in c.path_elements(0.1) {
                path.push(el);
            }
            1
        }
    };
    Some(HeadSpan { first: subpaths_so_far, count, filled: head.is_filled() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_segment_keeps_the_head_and_a_short_one_shrinks_it() {
        assert_eq!(fit_head_size(15.0, 100.0), 15.0);
        assert_eq!(fit_head_size(15.0, 30.0), 15.0);
        assert_eq!(fit_head_size(15.0, 20.0), 10.0);
        assert_eq!(fit_head_size(15.0, 10.0), 5.0);
        // The floor holds below twice the minimum, and a head asked for smaller than the floor stays as asked.
        assert_eq!(fit_head_size(15.0, 2.0), HEAD_MIN_SIZE);
        assert_eq!(fit_head_size(15.0, 0.0), HEAD_MIN_SIZE);
        assert_eq!(fit_head_size(3.0, 1.0), 3.0);
        assert_eq!(fit_head_size(3.0, 100.0), 3.0);
    }

    #[test]
    fn hostile_numbers_give_a_finite_head_that_never_exceeds_the_request() {
        for size in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -5.0, 0.0, -0.0] {
            assert_eq!(fit_head_size(size, 10.0), 0.0, "size {size}");
        }
        assert_eq!(fit_head_size(15.0, -10.0), HEAD_MIN_SIZE);
        assert_eq!(fit_head_size(15.0, f64::NEG_INFINITY), HEAD_MIN_SIZE);
        assert_eq!(fit_head_size(15.0, f64::NAN), 15.0);
        assert_eq!(fit_head_size(15.0, f64::INFINITY), 15.0);
        assert_eq!(fit_head_size(f64::MAX, f64::MAX), f64::MAX * HEAD_MAX_SEGMENT_SHARE);
    }

    #[test]
    fn the_fitted_head_grows_with_the_segment_and_stays_within_bounds() {
        for size in [1.0, 4.0, 15.0, 60.0] {
            let mut last = 0.0;
            for i in 0..=400 {
                let fitted = fit_head_size(size, f64::from(i) * 0.5);
                assert!(fitted >= last, "size {size} segment {}: {fitted} < {last}", f64::from(i) * 0.5);
                assert!(fitted <= size && fitted >= HEAD_MIN_SIZE.min(size));
                last = fitted;
            }
            assert_eq!(last, size);
        }
    }

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
        let at = |h| append(&mut BezPath::new(), 1, h, Point::ZERO, Vec2::new(1.0, 0.0), 10.0);
        // An open head has no fill to ask for.
        assert!(!at(Arrowhead::ARROW).is_some_and(|s| s.filled));
        assert!(!at(Arrowhead::BAR).is_some_and(|s| s.filled));
        for filled in [Arrowhead::TRIANGLE, Arrowhead::CIRCLE, Arrowhead::DIAMOND] {
            assert!(at(filled).is_some_and(|s| s.filled), "{filled:?}");
        }
        for outline in [Arrowhead::TRIANGLE_OUTLINE, Arrowhead::CIRCLE_OUTLINE, Arrowhead::DIAMOND_OUTLINE] {
            assert!(at(outline).is_some_and(|s| !s.filled), "{outline:?}");
        }
    }
}
