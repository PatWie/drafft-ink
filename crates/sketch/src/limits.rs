//! Hard limits and input sanitising. Every input is hostile: nothing here may panic, and every
//! limit keeps the output size bounded.

use kurbo::Point;

/// Coordinates, sizes and offsets are clamped to `[-MAX_COORD, MAX_COORD]`. At this size all the
/// arithmetic in the crate (squares, sums of squares) stays finite in `f64`.
pub const MAX_COORD: f64 = 1.0e9;

/// Point lists longer than this are decimated (every n-th point plus the last one) before
/// sketching. Output size is linear in the number of points kept, so this bounds the path length
/// to about 4 elements per point (two sketch passes plus the pressure outline).
pub const MAX_POINTS: usize = 65_536;

/// Hachure-style fills draw at most this many lines per direction. The fill gap is widened until
/// the bounding box of the shape needs no more lines than this. Cross-hatch draws two directions,
/// so at most twice this many.
pub const MAX_HACHURE_LINES: f64 = 1_000.0;

/// Dashed, zigzag-line fills draw at most this many pieces in total (lines times pieces per
/// line). The fill gap is widened until the estimate fits.
pub const MAX_FILL_SEGMENTS: f64 = 20_000.0;

/// Dot fills place at most this many dots (each dot is a small ellipse of about 8 curves).
pub const MAX_FILL_DOTS: f64 = 4_000.0;

/// Smallest fill gap in local units. Guards against zero and denormal gaps.
pub const MIN_FILL_GAP: f64 = 0.5;

/// Upper bound for roughness and bowing. The old app's highest level is 3.5.
pub const MAX_ROUGHNESS: f64 = 10.0;

/// Upper bound for the stroke width.
pub const MAX_STROKE_WIDTH: f64 = 10_000.0;

/// Upper bound for arrowhead size.
pub const MAX_HEAD_SIZE: f64 = 100_000.0;

/// Replace NaN with 0 and clamp to [`MAX_COORD`]. Also turns `-0.0` into `0.0`.
pub(crate) fn coord(x: f64) -> f64 {
    if x.is_nan() { 0.0 } else { x.clamp(-MAX_COORD, MAX_COORD) + 0.0 }
}

/// A length: NaN and negative values become 0, large values are clamped.
pub(crate) fn length(x: f64) -> f64 {
    if x.is_nan() || x <= 0.0 { 0.0 } else { x.min(MAX_COORD) }
}

/// Clamp `x` to `[0, max]`; NaN becomes `default`.
pub(crate) fn bounded(x: f64, default: f64, max: f64) -> f64 {
    if x.is_nan() { default } else { x.clamp(0.0, max) + 0.0 }
}

/// Keep the finite points (clamped), and decimate when there are more than [`MAX_POINTS`].
/// Returns the kept points and, for each, its index in the input (so per-point data such as
/// pressure follows the same selection).
pub(crate) fn clean_points(points: &[Point]) -> Vec<(usize, Point)> {
    let finite = |p: &Point| p.x.is_finite() && p.y.is_finite();
    let n = points.iter().filter(|p| finite(p)).count();
    let stride = n.div_ceil(MAX_POINTS).max(1);
    let mut out = Vec::with_capacity(n.min(MAX_POINTS + 1));
    let mut seen = 0usize;
    let mut last: Option<(usize, Point)> = None;
    for (i, p) in points.iter().enumerate() {
        if !finite(p) {
            continue;
        }
        let q = Point::new(coord(p.x), coord(p.y));
        if seen.is_multiple_of(stride) {
            out.push((i, q));
            last = None;
        } else {
            last = Some((i, q));
        }
        seen += 1;
    }
    if let Some(l) = last {
        out.push(l);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coord_handles_hostile_values() {
        assert_eq!(coord(f64::NAN), 0.0);
        assert_eq!(coord(f64::INFINITY), MAX_COORD);
        assert_eq!(coord(f64::NEG_INFINITY), -MAX_COORD);
        assert_eq!(coord(-0.0).to_bits(), 0.0f64.to_bits());
    }

    #[test]
    fn clean_points_drops_non_finite_and_decimates() {
        let pts =
            vec![Point::new(0.0, 0.0), Point::new(f64::NAN, 1.0), Point::new(1.0, f64::INFINITY), Point::new(2.0, 2.0)];
        let c = clean_points(&pts);
        assert_eq!(c.len(), 2);
        assert_eq!(c[1].0, 3);

        let many: Vec<Point> = (0..MAX_POINTS * 3 + 7).map(|i| Point::new(i as f64, 0.0)).collect();
        let c = clean_points(&many);
        assert!(c.len() <= MAX_POINTS + 1, "{}", c.len());
        assert_eq!(c.last().map(|p| p.0), Some(many.len() - 1));
    }
}
