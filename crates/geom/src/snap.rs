//! Grid and angle snapping, as pure functions.
//!
//! Ported from the old app's `snap.rs` with the same constants and the same results for valid
//! input. Differences:
//! - a grid size or angle step that is not a positive finite number snaps nothing (the old code
//!   produced NaN);
//! - `AngleSnapResult::snapped` is true whenever angle snapping is enabled and the line has a
//!   length. The old flag was false for angles that rounded across 360 degrees;
//! - the ray-grid search uses `f64` cell indices, so a huge target cannot overflow an `i32`.
//!
//! Smart guides (alignment and equal spacing) are in [`crate::guides`].

use std::f64::consts::PI;

use kurbo::Point;

/// Grid size for snapping (matches the visual grid).
pub const GRID_SIZE: f64 = 20.0;

/// Threshold for smart guide detection, in world units.
pub const SMART_GUIDE_THRESHOLD: f64 = 8.0;

/// Snap zone radius for equal spacing detection (40 grid units).
pub const EQUAL_SPACING_SNAP_RADIUS: f64 = GRID_SIZE * 40.0;

/// How far along the row (or column) equal spacing has to look for the boxes of a pair: dragging a box next to a
/// pair with the largest gap that counts ([`EQUAL_SPACING_SNAP_RADIUS`]) puts the pair's far box up to two gaps
/// away from the dragged box.
pub const EQUAL_SPACING_REACH: f64 = EQUAL_SPACING_SNAP_RADIUS * 2.0;

/// Snap zone radius for line and arrow endpoint snapping (40 grid units).
pub const ENDPOINT_SNAP_RADIUS: f64 = GRID_SIZE * 40.0;

/// Snap zone radius for multi-shape move snapping (20 grid units).
pub const MULTI_MOVE_SNAP_RADIUS: f64 = GRID_SIZE * 20.0;

/// Angle snap increment in degrees.
pub const ANGLE_SNAP_INCREMENT: f64 = 15.0;

/// Rotation snap step for the rotate handle, in radians (15 degrees).
pub const ROTATION_SNAP_STEP: f64 = PI / 12.0;

/// Upper bound on the candidates a caller should feed into smart guides.
pub const MAX_SNAP_CANDIDATES: usize = 200;

/// Result of a point snap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnapResult {
    /// The point after snapping.
    pub point: Point,
    /// True when the x coordinate moved to a snap position.
    pub snapped_x: bool,
    /// True when the y coordinate moved to a snap position.
    pub snapped_y: bool,
}

impl SnapResult {
    /// A result with no snapping.
    pub fn none(point: Point) -> Self {
        Self { point, snapped_x: false, snapped_y: false }
    }

    /// Did any coordinate snap?
    pub fn is_snapped(&self) -> bool {
        self.snapped_x || self.snapped_y
    }
}

/// Result of an angle snap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AngleSnapResult {
    /// The snapped endpoint.
    pub point: Point,
    /// The snapped angle in degrees, `[0, 360)`.
    pub angle_degrees: f64,
    /// The unsnapped angle in degrees, `[0, 360)`.
    pub original_angle_degrees: f64,
    /// True when the angle is locked to a step.
    pub snapped: bool,
    /// Distance from the start point to `point`.
    pub distance: f64,
}

impl AngleSnapResult {
    /// A result with no snapping.
    pub fn none(point: Point, start: Point) -> Self {
        let (dx, dy) = (point.x - start.x, point.y - start.y);
        let a = angle_degrees(dx, dy);
        Self { point, angle_degrees: a, original_angle_degrees: a, snapped: false, distance: dx.hypot(dy) }
    }

    /// The result for a line too short (or too long to measure) to have a direction: the end point
    /// stays, the angle is 0, and `distance` is 0 unless it is not finite.
    fn degenerate(point: Point, distance: f64) -> Self {
        let distance = if distance.is_finite() { 0.0 } else { distance };
        Self { point, angle_degrees: 0.0, original_angle_degrees: 0.0, snapped: false, distance }
    }
}

/// A line shorter than this has no direction to snap.
const MIN_SNAP_LENGTH: f64 = 0.001;

fn valid_step(s: f64) -> bool {
    s.is_finite() && s > 0.0
}

/// Direction `(dx, dy)` as degrees in `[0, 360)`. 0 for non-finite input.
fn angle_degrees(dx: f64, dy: f64) -> f64 {
    let a = dy.atan2(dx).to_degrees();
    if !a.is_finite() {
        0.0
    } else if a < 0.0 {
        a + 360.0
    } else {
        a
    }
}

/// Snap a point to the nearest grid intersection. A bad grid size leaves the point alone.
pub fn snap_to_grid(point: Point, grid_size: f64) -> SnapResult {
    if !valid_step(grid_size) || !crate::point_is_finite(point) {
        return SnapResult::none(point);
    }
    SnapResult {
        point: Point::new((point.x / grid_size).round() * grid_size, (point.y / grid_size).round() * grid_size),
        snapped_x: true,
        snapped_y: true,
    }
}

/// Snap a point to the grid when `grid_enabled`.
pub fn snap_point(point: Point, grid_enabled: bool, grid_size: f64) -> SnapResult {
    if grid_enabled { snap_to_grid(point, grid_size) } else { SnapResult::none(point) }
}

/// Snap a coordinate to the grid.
pub fn snap_value(v: f64, grid_size: f64) -> f64 {
    if valid_step(grid_size) && v.is_finite() { (v / grid_size).round() * grid_size } else { v }
}

/// Snap an angle in degrees to the nearest multiple of `increment`, result in `[0, 360)`.
/// A bad increment returns the angle normalised and otherwise unchanged.
pub fn snap_angle(angle_degrees: f64, increment: f64) -> f64 {
    if !angle_degrees.is_finite() {
        return 0.0;
    }
    let snapped = if valid_step(increment) { (angle_degrees / increment).round() * increment } else { angle_degrees };
    let r = snapped.rem_euclid(360.0);
    if r >= 360.0 { 0.0 } else { r }
}

/// Snap a rotation in radians to a multiple of `step` (radians), result in `[0, 2π)`.
pub fn snap_rotation(angle: f64, step: f64) -> f64 {
    crate::transform::normalize_angle(if valid_step(step) && angle.is_finite() {
        (angle / step).round() * step
    } else {
        angle
    })
}

/// Snap the direction from `start` to `end` to [`ANGLE_SNAP_INCREMENT`], keeping the length.
pub fn snap_line_endpoint(start: Point, end: Point, angle_snap_enabled: bool) -> AngleSnapResult {
    let (dx, dy) = (end.x - start.x, end.y - start.y);
    let distance = dx.hypot(dy);
    if !distance.is_finite() || distance < MIN_SNAP_LENGTH {
        return AngleSnapResult::degenerate(end, distance);
    }
    let original = angle_degrees(dx, dy);
    if !angle_snap_enabled {
        return AngleSnapResult {
            point: end,
            angle_degrees: original,
            original_angle_degrees: original,
            snapped: false,
            distance,
        };
    }
    let snapped_angle = snap_angle(original, ANGLE_SNAP_INCREMENT);
    let r = snapped_angle.to_radians();
    AngleSnapResult {
        point: Point::new(start.x + distance * r.cos(), start.y + distance * r.sin()),
        angle_degrees: snapped_angle,
        original_angle_degrees: original,
        snapped: true,
        distance,
    }
}

/// Where the ray from `origin` at `angle_degrees` crosses grid lines, nearest to `target`.
///
/// This snaps to where the ray crosses a horizontal or vertical grid line, not to grid corners
/// (which are usually off the ray). Cells within 5 of the target are searched.
pub fn snap_ray_to_grid_lines(origin: Point, angle_degrees: f64, target: Point, grid_size: f64) -> SnapResult {
    if !valid_step(grid_size)
        || !angle_degrees.is_finite()
        || !crate::point_is_finite(origin)
        || !crate::point_is_finite(target)
    {
        return SnapResult::none(target);
    }
    let rad = angle_degrees.to_radians();
    let (sin_a, cos_a) = rad.sin_cos();

    let mut best = target;
    let mut best_d = f64::MAX;
    let mut found = false;
    let mut consider = |c: Point| {
        let d = (c.x - target.x).powi(2) + (c.y - target.y).powi(2);
        if d < best_d {
            best_d = d;
            best = c;
            found = true;
        }
    };

    let base_x = (target.x / grid_size).round();
    let base_y = (target.y / grid_size).round();
    let is_horizontal = sin_a.abs() < 0.001;
    let is_vertical = cos_a.abs() < 0.001;

    if is_horizontal {
        for dx in -5..=5 {
            let gx = (base_x + f64::from(dx)) * grid_size;
            if (gx - origin.x) * cos_a >= 0.0 || (gx - origin.x).abs() < 0.001 {
                consider(Point::new(gx, origin.y));
            }
        }
    } else if is_vertical {
        for dy in -5..=5 {
            let gy = (base_y + f64::from(dy)) * grid_size;
            if (gy - origin.y) * sin_a >= 0.0 || (gy - origin.y).abs() < 0.001 {
                consider(Point::new(origin.x, gy));
            }
        }
    } else {
        for dx in -5..=5 {
            let gx = (base_x + f64::from(dx)) * grid_size;
            let t = (gx - origin.x) / cos_a;
            if t >= 0.0 {
                consider(Point::new(gx, origin.y + t * sin_a));
            }
        }
        for dy in -5..=5 {
            let gy = (base_y + f64::from(dy)) * grid_size;
            let t = (gy - origin.y) / sin_a;
            if t >= 0.0 {
                consider(Point::new(origin.x + t * cos_a, gy));
            }
        }
    }

    if found && crate::point_is_finite(best) {
        SnapResult { point: best, snapped_x: true, snapped_y: true }
    } else {
        SnapResult::none(target)
    }
}

/// What [`snap_endpoint`] snaps a line endpoint to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EndpointSnap {
    /// Nothing: the endpoint is unchanged.
    #[default]
    None,
    /// The direction, as [`snap_line_endpoint`].
    Angle,
    /// The nearest grid corner.
    Grid,
    /// The direction first, then along that ray to the nearest grid line crossing, so the endpoint stays exactly on
    /// the angle.
    AngleAndGrid,
}

impl EndpointSnap {
    /// The mode for two switches (the angle key and the grid toggle).
    pub const fn from_switches(angle: bool, grid: bool) -> Self {
        match (angle, grid) {
            (false, false) => EndpointSnap::None,
            (true, false) => EndpointSnap::Angle,
            (false, true) => EndpointSnap::Grid,
            (true, true) => EndpointSnap::AngleAndGrid,
        }
    }
}

/// Snap a line endpoint as `mode` says.
///
/// (Called `snap_line_endpoint_isometric` in the old app, which took an unused isometric flag.)
pub fn snap_endpoint(start: Point, end: Point, mode: EndpointSnap, grid_size: f64) -> AngleSnapResult {
    let (dx, dy) = (end.x - start.x, end.y - start.y);
    let distance = dx.hypot(dy);
    if !distance.is_finite() || distance < MIN_SNAP_LENGTH {
        return AngleSnapResult::degenerate(end, distance);
    }
    let original = angle_degrees(dx, dy);

    match mode {
        EndpointSnap::None => AngleSnapResult {
            point: end,
            angle_degrees: original,
            original_angle_degrees: original,
            snapped: false,
            distance,
        },
        EndpointSnap::Grid => {
            let g = snap_to_grid(end, grid_size);
            AngleSnapResult {
                point: g.point,
                angle_degrees: original,
                original_angle_degrees: original,
                snapped: g.is_snapped(),
                distance,
            }
        }
        EndpointSnap::Angle => snap_line_endpoint(start, end, true),
        EndpointSnap::AngleAndGrid => {
            let snapped_angle = snap_angle(original, ANGLE_SNAP_INCREMENT);
            let r = snapped_angle.to_radians();
            let on_ray = Point::new(start.x + distance * r.cos(), start.y + distance * r.sin());
            let g = snap_ray_to_grid_lines(start, snapped_angle, on_ray, grid_size);
            let (point, distance) = if g.is_snapped() {
                (g.point, (g.point.x - start.x).hypot(g.point.y - start.y))
            } else {
                (on_ray, distance)
            };
            AngleSnapResult {
                point,
                angle_degrees: snapped_angle,
                original_angle_degrees: original,
                snapped: true,
                distance,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The first thirteen tests are the old app's tests, unchanged in input and expectation.

    #[test]
    fn test_snap_to_grid() {
        let result = snap_to_grid(Point::new(23.0, 47.0), 20.0);
        assert_eq!(result.point, Point::new(20.0, 40.0));
        assert!(result.snapped_x);
        assert!(result.snapped_y);
    }

    #[test]
    fn test_snap_to_grid_exact() {
        let result = snap_to_grid(Point::new(40.0, 60.0), 20.0);
        assert_eq!(result.point, Point::new(40.0, 60.0));
    }

    #[test]
    fn test_snap_to_grid_round_up() {
        let result = snap_to_grid(Point::new(31.0, 51.0), 20.0);
        assert_eq!(result.point, Point::new(40.0, 60.0));
    }

    #[test]
    fn test_snap_angle() {
        assert!((snap_angle(0.0, 15.0) - 0.0).abs() < 0.01);
        assert!((snap_angle(7.0, 15.0) - 0.0).abs() < 0.01);
        assert!((snap_angle(8.0, 15.0) - 15.0).abs() < 0.01);
        assert!((snap_angle(22.0, 15.0) - 15.0).abs() < 0.01);
        assert!((snap_angle(23.0, 15.0) - 30.0).abs() < 0.01);
        assert!((snap_angle(45.0, 15.0) - 45.0).abs() < 0.01);
        assert!((snap_angle(90.0, 15.0) - 90.0).abs() < 0.01);
        assert!((snap_angle(180.0, 15.0) - 180.0).abs() < 0.01);
        assert!((snap_angle(270.0, 15.0) - 270.0).abs() < 0.01);
        assert!((snap_angle(359.0, 15.0) - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_snap_line_endpoint_horizontal() {
        let result = snap_line_endpoint(Point::new(0.0, 0.0), Point::new(100.0, 5.0), true);
        assert!(result.snapped);
        assert!((result.angle_degrees - 0.0).abs() < 0.01);
        assert!((result.point.y - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_snap_line_endpoint_45_degrees() {
        let result = snap_line_endpoint(Point::new(0.0, 0.0), Point::new(100.0, 102.0), true);
        assert!(result.snapped);
        assert!((result.angle_degrees - 45.0).abs() < 0.01);
    }

    #[test]
    fn test_snap_line_endpoint_disabled() {
        let end = Point::new(100.0, 5.0);
        let result = snap_line_endpoint(Point::new(0.0, 0.0), end, false);
        assert!(!result.snapped);
        assert_eq!(result.point, end);
    }

    #[test]
    fn test_snap_line_preserves_distance() {
        let result = snap_line_endpoint(Point::new(0.0, 0.0), Point::new(100.0, 10.0), true);
        let original_distance = ((100.0_f64).powi(2) + (10.0_f64).powi(2)).sqrt();
        assert!((result.distance - original_distance).abs() < 0.01);
        let on = (result.point.x.powi(2) + result.point.y.powi(2)).sqrt();
        assert!((on - original_distance).abs() < 1e-9, "the snapped point keeps the length");
    }

    #[test]
    fn test_snap_ray_to_grid_lines_horizontal() {
        let result = snap_ray_to_grid_lines(Point::new(0.0, 0.0), 0.0, Point::new(55.0, 0.0), 20.0);
        assert!(result.is_snapped());
        assert!((result.point.x - 60.0).abs() < 0.01);
        assert!((result.point.y - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_snap_ray_to_grid_lines_45_degrees() {
        let result = snap_ray_to_grid_lines(Point::new(0.0, 0.0), 45.0, Point::new(50.0, 50.0), 20.0);
        assert!(result.is_snapped());
        assert!((result.point.x - result.point.y).abs() < 0.01);
    }

    #[test]
    fn test_snap_ray_to_grid_lines_30_degrees() {
        let result = snap_ray_to_grid_lines(Point::new(0.0, 0.0), 30.0, Point::new(100.0, 57.7), 20.0);
        assert!(result.is_snapped());
        let angle = (result.point.y / result.point.x).atan().to_degrees();
        assert!((angle - 30.0).abs() < 0.1);
    }

    #[test]
    fn test_snap_endpoint_both_horizontal() {
        let result = snap_endpoint(Point::new(0.0, 0.0), Point::new(100.0, 5.0), EndpointSnap::AngleAndGrid, 20.0);
        assert!(result.snapped);
        assert!(result.point.y.abs() < 0.01);
        assert!((result.point.x % 20.0).abs() < 0.01 || (result.point.x % 20.0 - 20.0).abs() < 0.01);
    }

    #[test]
    fn test_snap_endpoint_both_45_degrees() {
        let result = snap_endpoint(Point::new(0.0, 0.0), Point::new(100.0, 102.0), EndpointSnap::AngleAndGrid, 20.0);
        assert!(result.snapped);
        assert!((result.point.x - result.point.y).abs() < 0.1);
        let on_x = (result.point.x % 20.0).abs() < 0.01 || (result.point.x % 20.0 - 20.0).abs() < 0.01;
        let on_y = (result.point.y % 20.0).abs() < 0.01 || (result.point.y % 20.0 - 20.0).abs() < 0.01;
        assert!(on_x || on_y);
    }

    // New tests.

    #[test]
    fn constants_match_the_old_app() {
        assert_eq!(GRID_SIZE, 20.0);
        assert_eq!(SMART_GUIDE_THRESHOLD, 8.0);
        assert_eq!(EQUAL_SPACING_SNAP_RADIUS, 800.0);
        assert_eq!(EQUAL_SPACING_REACH, 1600.0);
        assert_eq!(ENDPOINT_SNAP_RADIUS, 800.0);
        assert_eq!(MULTI_MOVE_SNAP_RADIUS, 400.0);
        assert_eq!(ANGLE_SNAP_INCREMENT, 15.0);
        assert!((ROTATION_SNAP_STEP.to_degrees() - 15.0).abs() < 1e-12);
    }

    #[test]
    fn rounding_is_half_away_from_zero_like_the_old_code() {
        assert_eq!(snap_to_grid(Point::new(10.0, -10.0), 20.0).point, Point::new(20.0, -20.0));
        assert_eq!(snap_to_grid(Point::new(-23.0, -47.0), 20.0).point, Point::new(-20.0, -40.0));
    }

    #[test]
    fn snap_angle_wraps_and_normalises() {
        assert_eq!(snap_angle(-7.0, 15.0), 0.0);
        assert!((snap_angle(-8.0, 15.0) - 345.0).abs() < 1e-9);
        assert_eq!(snap_angle(367.0, 15.0), 0.0);
        assert!((snap_angle(37.0, 0.0) - 37.0).abs() < 1e-12, "bad step leaves the angle");
        assert_eq!(snap_angle(f64::NAN, 15.0), 0.0);
    }

    #[test]
    fn snapped_flag_is_true_across_the_wrap() {
        // 359.95 degrees rounds to 0: the old flag said "not snapped".
        let end = Point::new(100.0 * 359.95_f64.to_radians().cos(), 100.0 * 359.95_f64.to_radians().sin());
        let r = snap_line_endpoint(Point::ZERO, end, true);
        assert!(r.snapped);
        assert_eq!(r.angle_degrees, 0.0);
    }

    #[test]
    fn zero_length_line_does_not_snap() {
        let r = snap_line_endpoint(Point::new(5.0, 5.0), Point::new(5.0, 5.0), true);
        assert!(!r.snapped && r.distance == 0.0 && r.point == Point::new(5.0, 5.0));
        let r = snap_endpoint(Point::new(5.0, 5.0), Point::new(5.0, 5.0), EndpointSnap::AngleAndGrid, 20.0);
        assert!(!r.snapped);
    }

    #[test]
    fn snap_endpoint_modes() {
        let (s, e) = (Point::new(3.0, 4.0), Point::new(97.0, 41.0));
        assert_eq!(snap_endpoint(s, e, EndpointSnap::None, 20.0).point, e);
        assert_eq!(snap_endpoint(s, e, EndpointSnap::Grid, 20.0).point, Point::new(100.0, 40.0));
        assert_eq!(snap_endpoint(s, e, EndpointSnap::Angle, 20.0).point, snap_line_endpoint(s, e, true).point);
    }

    #[test]
    fn ray_to_grid_vertical_and_direction() {
        let r = snap_ray_to_grid_lines(Point::new(7.0, 0.0), 90.0, Point::new(7.0, 58.0), 20.0);
        assert!(r.is_snapped() && (r.point.x - 7.0).abs() < 1e-9 && (r.point.y - 60.0).abs() < 1e-9);
        // The ray points right: a grid line behind the origin is not a candidate.
        let r = snap_ray_to_grid_lines(Point::new(100.0, 0.0), 0.0, Point::new(70.0, 0.0), 20.0);
        assert!(r.point.x >= 100.0);
    }

    #[test]
    fn rotation_snap() {
        let a = snap_rotation(20.0_f64.to_radians(), ROTATION_SNAP_STEP);
        assert!((a.to_degrees() - 15.0).abs() < 1e-9);
        let a = snap_rotation((-3.0_f64).to_radians(), ROTATION_SNAP_STEP);
        assert_eq!(a, 0.0);
        assert!((snap_rotation(-10.0_f64.to_radians(), ROTATION_SNAP_STEP).to_degrees() - 345.0).abs() < 1e-9);
    }

    #[test]
    fn hostile_inputs_never_panic_and_never_snap() {
        let bad = [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0, -20.0, 1e300];
        for &g in &bad {
            let r = snap_to_grid(Point::new(13.0, 17.0), g);
            if !(g.is_finite() && g > 0.0) {
                assert!(!r.is_snapped() && r.point == Point::new(13.0, 17.0), "grid {g}");
            }
            for &x in &bad {
                let p = Point::new(x, g);
                let _ = (
                    snap_to_grid(p, 20.0),
                    snap_ray_to_grid_lines(p, x, p, g),
                    snap_ray_to_grid_lines(Point::ZERO, 45.0, p, 20.0),
                );
                let _ = (
                    snap_line_endpoint(Point::ZERO, p, true),
                    snap_endpoint(p, Point::ZERO, EndpointSnap::AngleAndGrid, g),
                    snap_angle(x, g),
                    snap_rotation(x, g),
                );
            }
        }
    }
}
