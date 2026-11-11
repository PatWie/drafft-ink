//! Align and distribute, as pure functions over rects.
//!
//! Each function takes the boxes to move (one per element or group) and returns one translation
//! per box, in the same order. The caller applies the translations as one batch. A box that is
//! not finite gets a zero translation and does not influence the others.
//!
//! The old app had align buttons in the UI and no distribute. Align follows the usual meaning;
//! distribute follows Excalidraw: equal gaps between boxes with the outer two fixed, falling back
//! to equal centre spacing when the boxes are wider than the space they span.

use kurbo::{Rect, Vec2};

use crate::aabb;

/// Which edge or centre line the boxes align to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlignMode {
    /// Left edges on one vertical line.
    Left,
    /// Right edges on one vertical line.
    Right,
    /// Top edges on one horizontal line.
    Top,
    /// Bottom edges on one horizontal line.
    Bottom,
    /// Horizontal centres on one vertical line ("center horizontally").
    CenterX,
    /// Vertical centres on one horizontal line ("center vertically").
    CenterY,
}

/// The axis along which boxes are distributed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// The horizontal axis: boxes are spread left to right.
    X,
    /// The vertical axis: boxes are spread top to bottom.
    Y,
}

/// Translations that align the boxes to the bounds of all of them.
pub fn align(rects: &[Rect], mode: AlignMode) -> Vec<Vec2> {
    match aabb::union_all(rects.iter().copied()) {
        Some(reference) => align_to(rects, mode, reference),
        None => vec![Vec2::ZERO; rects.len()],
    }
}

/// Translations that align the boxes to `reference` (the selection bounds, a frame, or the box of
/// a key element).
pub fn align_to(rects: &[Rect], mode: AlignMode, reference: Rect) -> Vec<Vec2> {
    if !aabb::is_finite(&reference) {
        return vec![Vec2::ZERO; rects.len()];
    }
    let reference = aabb::normalize(reference);
    rects
        .iter()
        .map(|r| {
            if !aabb::is_finite(r) {
                return Vec2::ZERO;
            }
            let r = aabb::normalize(*r);
            match mode {
                AlignMode::Left => Vec2::new(reference.x0 - r.x0, 0.0),
                AlignMode::Right => Vec2::new(reference.x1 - r.x1, 0.0),
                AlignMode::Top => Vec2::new(0.0, reference.y0 - r.y0),
                AlignMode::Bottom => Vec2::new(0.0, reference.y1 - r.y1),
                AlignMode::CenterX => Vec2::new(aabb::center(&reference).x - aabb::center(&r).x, 0.0),
                AlignMode::CenterY => Vec2::new(0.0, aabb::center(&reference).y - aabb::center(&r).y),
            }
        })
        .collect()
}

/// Translations that space the boxes evenly along `axis`.
///
/// Boxes are ordered by centre. The first and last (by position) stay where they are.
/// - If the boxes fit inside their common extent, the gaps between neighbours become equal.
/// - If they overlap so much that the gap would be negative, the centres become evenly spaced
///   between the centres of the two outermost boxes instead.
///
/// Fewer than three boxes need no movement and give zero translations.
pub fn distribute(rects: &[Rect], axis: Axis) -> Vec<Vec2> {
    let mut out = vec![Vec2::ZERO; rects.len()];
    // (original index, start, end, mid)
    let mut items: Vec<(usize, f64, f64, f64)> = rects
        .iter()
        .enumerate()
        .filter(|(_, r)| aabb::is_finite(r))
        .map(|(i, r)| {
            let r = aabb::normalize(*r);
            let mid = aabb::center(&r);
            match axis {
                Axis::X => (i, r.x0, r.x1, mid.x),
                Axis::Y => (i, r.y0, r.y1, mid.y),
            }
        })
        .collect();
    let n = items.len();
    if n < 3 {
        return out;
    }
    items.sort_by(|a, b| a.3.total_cmp(&b.3).then(a.0.cmp(&b.0)));

    let min_start = items.iter().map(|i| i.1).fold(f64::INFINITY, f64::min);
    let max_end = items.iter().map(|i| i.2).fold(f64::NEG_INFINITY, f64::max);
    let span: f64 = items.iter().map(|i| i.2 - i.1).sum();
    let step = ((max_end - min_start) - span) / (n - 1) as f64;
    if !step.is_finite() {
        return out;
    }

    let put = |out: &mut Vec<Vec2>, idx: usize, delta: f64| {
        out[idx] = match axis {
            Axis::X => Vec2::new(delta, 0.0),
            Axis::Y => Vec2::new(0.0, delta),
        };
    };

    if step < 0.0 {
        // Distribute centres between the boxes that define the extent.
        let first = items.iter().position(|i| i.1 == min_start).unwrap_or(0);
        let last = items.iter().position(|i| i.2 == max_end).unwrap_or(n - 1);
        let step = (items[last].3 - items[first].3) / (n - 1) as f64;
        let mut pos = items[first].3;
        for (k, item) in items.iter().enumerate() {
            if k != first && k != last {
                pos += step;
                put(&mut out, item.0, pos - item.3);
            }
        }
    } else {
        let mut pos = min_start;
        for item in &items {
            put(&mut out, item.0, pos - item.1);
            pos += (item.2 - item.1) + step;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect::new(x, y, x + w, y + h)
    }

    fn apply(rects: &[Rect], d: &[Vec2]) -> Vec<Rect> {
        rects.iter().zip(d).map(|(r, v)| aabb::translated(*r, *v)).collect()
    }

    #[test]
    fn align_edges_and_centres() {
        let rs = [r(0.0, 0.0, 10.0, 10.0), r(30.0, 20.0, 20.0, 40.0), r(-5.0, 100.0, 5.0, 5.0)];
        let l = apply(&rs, &align(&rs, AlignMode::Left));
        assert!(l.iter().all(|x| x.x0 == -5.0));
        let rt = apply(&rs, &align(&rs, AlignMode::Right));
        assert!(rt.iter().all(|x| x.x1 == 50.0));
        let t = apply(&rs, &align(&rs, AlignMode::Top));
        assert!(t.iter().all(|x| x.y0 == 0.0));
        let b = apply(&rs, &align(&rs, AlignMode::Bottom));
        assert!(b.iter().all(|x| x.y1 == 105.0));
        let cx = apply(&rs, &align(&rs, AlignMode::CenterX));
        assert!(cx.iter().all(|x| (aabb::center(x).x - 22.5).abs() < 1e-12));
        let cy = apply(&rs, &align(&rs, AlignMode::CenterY));
        assert!(cy.iter().all(|x| (aabb::center(x).y - 52.5).abs() < 1e-12));
    }

    #[test]
    fn align_moves_one_axis_only() {
        let rs = [r(0.0, 0.0, 10.0, 10.0), r(30.0, 20.0, 10.0, 10.0)];
        let d = align(&rs, AlignMode::Left);
        assert!(d.iter().all(|v| v.y == 0.0));
        let d = align(&rs, AlignMode::Top);
        assert!(d.iter().all(|v| v.x == 0.0));
    }

    #[test]
    fn align_to_a_reference() {
        let rs = [r(0.0, 0.0, 10.0, 10.0)];
        let d = align_to(&rs, AlignMode::Right, Rect::new(0.0, 0.0, 100.0, 100.0));
        assert_eq!(d[0], Vec2::new(90.0, 0.0));
        assert_eq!(align_to(&rs, AlignMode::Right, Rect::new(f64::NAN, 0.0, 1.0, 1.0)), vec![Vec2::ZERO]);
    }

    #[test]
    fn align_is_idempotent() {
        let rs = [r(0.0, 0.0, 10.0, 10.0), r(30.0, 20.0, 20.0, 40.0)];
        let once = apply(&rs, &align(&rs, AlignMode::CenterX));
        let again = align(&once, AlignMode::CenterX);
        assert!(again.iter().all(|v| v.hypot() < 1e-12));
    }

    #[test]
    fn distribute_equal_gaps() {
        // widths 10, 20, 30 inside extent 0..200: gaps (200 - 60) / 2 = 70
        let rs = [r(0.0, 0.0, 10.0, 10.0), r(100.0, 0.0, 20.0, 10.0), r(170.0, 0.0, 30.0, 10.0)];
        let moved = apply(&rs, &distribute(&rs, Axis::X));
        assert_eq!((moved[0].x0, moved[0].x1), (0.0, 10.0));
        assert_eq!((moved[1].x0, moved[1].x1), (80.0, 100.0));
        assert_eq!((moved[2].x0, moved[2].x1), (170.0, 200.0));
        assert!((moved[1].x0 - moved[0].x1 - 70.0).abs() < 1e-12 && (moved[2].x0 - moved[1].x1 - 70.0).abs() < 1e-12);
    }

    #[test]
    fn distribute_orders_by_centre_not_by_input() {
        let rs = [r(170.0, 0.0, 30.0, 10.0), r(0.0, 0.0, 10.0, 10.0), r(150.0, 0.0, 20.0, 10.0)];
        let d = distribute(&rs, Axis::X);
        let moved = apply(&rs, &d);
        assert_eq!(moved[1].x0, 0.0);
        assert_eq!(moved[0].x1, 200.0);
        assert_eq!(moved[2].x0, 80.0);
        assert!(d.iter().all(|v| v.y == 0.0));
    }

    #[test]
    fn distribute_y() {
        let rs = [r(0.0, 0.0, 10.0, 10.0), r(0.0, 15.0, 10.0, 10.0), r(0.0, 90.0, 10.0, 10.0)];
        let moved = apply(&rs, &distribute(&rs, Axis::Y));
        assert_eq!(moved[1].y0, 45.0);
    }

    #[test]
    fn distribute_overlapping_falls_back_to_centres() {
        // widths 100 in extent 0..150: negative gap, so centres become evenly spaced
        let rs = [r(0.0, 0.0, 100.0, 10.0), r(10.0, 0.0, 100.0, 10.0), r(50.0, 0.0, 100.0, 10.0)];
        let moved = apply(&rs, &distribute(&rs, Axis::X));
        let c: Vec<f64> = moved.iter().map(|m| aabb::center(m).x).collect();
        assert_eq!(c[0], 50.0);
        assert_eq!(c[2], 100.0);
        assert_eq!(c[1], 75.0);
    }

    #[test]
    fn distribute_needs_three() {
        let rs = [r(0.0, 0.0, 10.0, 10.0), r(100.0, 0.0, 10.0, 10.0)];
        assert_eq!(distribute(&rs, Axis::X), vec![Vec2::ZERO; 2]);
        assert!(distribute(&[], Axis::X).is_empty());
    }

    #[test]
    fn distribute_keeps_extent_and_outer_boxes() {
        let rs = [r(0.0, 0.0, 10.0, 10.0), r(13.0, 0.0, 7.0, 10.0), r(40.0, 0.0, 12.0, 10.0), r(90.0, 0.0, 10.0, 10.0)];
        let moved = apply(&rs, &distribute(&rs, Axis::X));
        assert_eq!(moved[0], rs[0]);
        assert_eq!(moved[3], rs[3]);
        let gaps: Vec<f64> = (0..3).map(|i| moved[i + 1].x0 - moved[i].x1).collect();
        assert!(gaps.iter().all(|g| (g - gaps[0]).abs() < 1e-9), "{gaps:?}");
    }

    #[test]
    fn hostile_input() {
        let rs = [
            r(0.0, 0.0, 10.0, 10.0),
            Rect::new(f64::NAN, 0.0, 1.0, 1.0),
            r(100.0, 0.0, 10.0, 10.0),
            r(50.0, 0.0, 10.0, 10.0),
        ];
        let d = distribute(&rs, Axis::X);
        assert_eq!(d.len(), 4);
        assert_eq!(d[1], Vec2::ZERO);
        let a = align(&rs, AlignMode::Right);
        assert_eq!(a[1], Vec2::ZERO);
        let inf = [r(0.0, 0.0, 10.0, 10.0), Rect::new(0.0, 0.0, f64::INFINITY, 1.0)];
        assert_eq!(align(&inf, AlignMode::Left), vec![Vec2::ZERO; 2]);
        let all_bad = [Rect::new(f64::NAN, f64::NAN, f64::NAN, f64::NAN); 3];
        assert_eq!(distribute(&all_bad, Axis::Y), vec![Vec2::ZERO; 3]);
        assert_eq!(align(&all_bad, AlignMode::Top), vec![Vec2::ZERO; 3]);
    }
}
