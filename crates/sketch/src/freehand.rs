//! Variable-width freehand outline: the old app's `render_freehand_with_pressure`, ported.
//!
//! The old algorithm is a plain offset polygon, not perfect-freehand: no smoothing, no taper, no
//! streamline. Pressure `p` is eased with `sin(p * pi / 2)` and the width at a point is
//! `base * (1 - thinning * (1 - eased))` with `base = stroke_width * size_scale`. The outline goes
//! forward along the left offsets and back along the right offsets, and is filled once.

use kurbo::{BezPath, Point, Vec2};

use crate::input::PressureProfile;

/// `points` and `pressures` are index-aligned (missing pressure counts as 1). Returns `None` for
/// fewer than two points or when no point has a usable direction.
pub(crate) fn pressure_outline(
    points: &[Point],
    pressure: impl Fn(usize) -> f64,
    stroke_width: f64,
    profile: PressureProfile,
) -> Option<BezPath> {
    if points.len() < 2 {
        return None;
    }
    let base = stroke_width * profile.size_scale;
    let mut left: Vec<Point> = Vec::with_capacity(points.len());
    let mut right: Vec<Point> = Vec::with_capacity(points.len());
    for (i, &p) in points.iter().enumerate() {
        let eased = (pressure(i).clamp(0.0, 1.0) * std::f64::consts::FRAC_PI_2).sin();
        let width = base * (1.0 - profile.thinning * (1.0 - eased));
        // The direction runs from the previous to the next point; an end point uses itself for the
        // missing neighbour.
        let prev = i.checked_sub(1).and_then(|j| points.get(j)).copied().unwrap_or(p);
        let next = points.get(i + 1).copied().unwrap_or(p);
        let dir = next - prev;
        let len = dir.hypot();
        if len < f64::EPSILON {
            continue;
        }
        let perp = Vec2::new(-dir.y / len, dir.x / len);
        let half = width / 2.0;
        left.push(p + perp * half);
        right.push(p - perp * half);
    }
    let (first, rest) = left.split_first()?;
    let mut path = BezPath::new();
    path.move_to(*first);
    for q in rest {
        path.line_to(*q);
    }
    for q in right.iter().rev() {
        path.line_to(*q);
    }
    path.close_path();
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts() -> Vec<Point> {
        (0..10).map(|i| Point::new(f64::from(i) * 10.0, 0.0)).collect()
    }

    #[test]
    fn full_pressure_gives_a_band_of_base_width() {
        let p = pressure_outline(&pts(), |_| 1.0, 2.0, PressureProfile::default());
        let b = kurbo::Shape::bounding_box(&p.unwrap_or_default());
        assert!((b.height() - 4.6).abs() < 1e-9, "{b:?}");
        assert!((b.width() - 90.0).abs() < 1e-9);
    }

    #[test]
    fn low_pressure_is_thinner() {
        let full = pressure_outline(&pts(), |_| 1.0, 2.0, PressureProfile::default());
        let low = pressure_outline(&pts(), |_| 0.1, 2.0, PressureProfile::default());
        let h = |p: Option<BezPath>| kurbo::Shape::bounding_box(&p.unwrap_or_default()).height();
        assert!(h(low) < h(full));
    }

    #[test]
    fn degenerate_inputs() {
        assert!(pressure_outline(&[], |_| 1.0, 2.0, PressureProfile::default()).is_none());
        assert!(pressure_outline(&pts()[..1], |_| 1.0, 2.0, PressureProfile::default()).is_none());
        let same = [Point::new(1.0, 1.0), Point::new(1.0, 1.0)];
        assert!(pressure_outline(&same, |_| 1.0, 2.0, PressureProfile::default()).is_none());
    }
}
