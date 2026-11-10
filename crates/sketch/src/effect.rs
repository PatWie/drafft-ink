//! The sketchy outline: the old app's `apply_hand_drawn_effect`, ported.
//!
//! Each path element is perturbed with deterministic noise: start points and end points move by up
//! to `2 * roughness`, straight segments become quadratic curves with a bowed control point, and
//! curve control points move by a smaller amount. The old app scaled the noise with
//! `1 / sqrt(zoom)`; here the noise is fixed in local units (the old output at zoom 1), so the
//! result does not depend on the camera and can be cached across pan and zoom.

use kurbo::{BezPath, PathEl, Point};

use crate::rng::SimpleRng;

/// Large prime that separates the random streams of the two strokes.
const STROKE_SEED_STEP: u32 = 99_991;

/// One sketch pass over `path`.
///
/// * `roughness`: 0 returns the path unchanged.
/// * `bowing`: multiplier for the bow of straight segments (1.0 is the old app).
/// * `stroke_index`: 0 and 1 give the two different strokes of the double-stroke look.
pub(crate) fn sketch_pass(path: &BezPath, roughness: f64, bowing: f64, seed: u32, stroke_index: u32) -> BezPath {
    if roughness <= 0.0 {
        return path.clone();
    }
    let max_offset = roughness * 2.0;
    let bowing = roughness * bowing;
    let mut rng = SimpleRng::new(seed.wrapping_add(stroke_index.wrapping_mul(STROKE_SEED_STEP)));

    let mut result = BezPath::new();
    let mut last = Point::ZERO;
    for el in path.elements() {
        match el {
            PathEl::MoveTo(p) => {
                let w = Point::new(p.x + rng.offset(max_offset), p.y + rng.offset(max_offset));
                result.move_to(w);
                last = *p;
            }
            PathEl::LineTo(p) => {
                let dx = p.x - last.x;
                let dy = p.y - last.y;
                let len = (dx * dx + dy * dy).sqrt();
                let bow_offset = bowing * roughness * len / 200.0;
                let bow = rng.offset(bow_offset);
                let (px, py) = if len > 0.001 { (-dy / len, dx / len) } else { (0.0, 0.0) };
                let mid_x = (last.x + p.x) / 2.0 + px * bow;
                let mid_y = (last.y + p.y) / 2.0 + py * bow;
                let end = Point::new(p.x + rng.offset(max_offset), p.y + rng.offset(max_offset));
                result.quad_to(Point::new(mid_x, mid_y), end);
                last = *p;
            }
            PathEl::QuadTo(p1, p2) => {
                let a = Point::new(p1.x + rng.offset(max_offset * 0.7), p1.y + rng.offset(max_offset * 0.7));
                let b = Point::new(p2.x + rng.offset(max_offset), p2.y + rng.offset(max_offset));
                result.quad_to(a, b);
                last = *p2;
            }
            PathEl::CurveTo(p1, p2, p3) => {
                let a = Point::new(p1.x + rng.offset(max_offset * 0.5), p1.y + rng.offset(max_offset * 0.5));
                let b = Point::new(p2.x + rng.offset(max_offset * 0.5), p2.y + rng.offset(max_offset * 0.5));
                let c = Point::new(p3.x + rng.offset(max_offset), p3.y + rng.offset(max_offset));
                result.curve_to(a, b, c);
                last = *p3;
            }
            // Kept as is: the closing edge is a straight segment back to the (moved) start point.
            PathEl::ClosePath => result.close_path(),
        }
    }
    result
}

/// Split a path into its subpaths (one per `MoveTo`).
pub(crate) fn split_subpaths(path: &BezPath) -> Vec<BezPath> {
    let mut out: Vec<BezPath> = Vec::new();
    for el in path.elements() {
        if matches!(el, PathEl::MoveTo(_)) || out.is_empty() {
            out.push(BezPath::new());
        }
        if let Some(cur) = out.last_mut() {
            cur.push(*el);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line() -> BezPath {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((100.0, 0.0));
        p
    }

    #[test]
    fn zero_roughness_is_identity() {
        assert_eq!(sketch_pass(&line(), 0.0, 1.0, 5, 0), line());
    }

    #[test]
    fn same_inputs_same_output_and_strokes_differ() {
        let a = sketch_pass(&line(), 1.0, 1.0, 5, 0);
        let b = sketch_pass(&line(), 1.0, 1.0, 5, 0);
        let c = sketch_pass(&line(), 1.0, 1.0, 5, 1);
        let d = sketch_pass(&line(), 1.0, 1.0, 6, 0);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
    }

    #[test]
    fn lines_become_quads_and_stay_near_the_original() {
        let a = sketch_pass(&line(), 1.0, 1.0, 5, 0);
        assert!(matches!(a.elements()[1], PathEl::QuadTo(..)));
        for el in a.elements() {
            for p in match el {
                PathEl::MoveTo(p) | PathEl::LineTo(p) => vec![*p],
                PathEl::QuadTo(a, b) => vec![*a, *b],
                _ => vec![],
            } {
                assert!(p.y.abs() <= 2.0 + 1.0, "{p:?}");
            }
        }
    }

    #[test]
    fn splits_at_move_to() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((1.0, 0.0));
        p.move_to((5.0, 5.0));
        p.line_to((6.0, 5.0));
        p.close_path();
        let s = split_subpaths(&p);
        assert_eq!(s.len(), 2);
        assert_eq!(s[1].elements().len(), 3);
        assert!(split_subpaths(&BezPath::new()).is_empty());
    }
}
