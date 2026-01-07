//! Snap functionality for aligning points to grid and shapes.

use kurbo::Point;

/// Grid size for snapping (matches the visual grid).
pub const GRID_SIZE: f64 = 20.0;

/// Snap mode for aligning shapes to grid or other elements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SnapMode {
    /// No snapping.
    #[default]
    None,
    /// Snap to grid intersections.
    Grid,
    /// Snap to other shape edges/corners.
    Shapes,
    /// Snap to both grid and shapes.
    All,
}

impl SnapMode {
    /// Cycle to the next snap mode.
    pub fn next(self) -> Self {
        match self {
            SnapMode::None => SnapMode::Grid,
            SnapMode::Grid => SnapMode::Shapes,
            SnapMode::Shapes => SnapMode::All,
            SnapMode::All => SnapMode::None,
        }
    }

    /// Check if grid snapping is enabled.
    pub fn snaps_to_grid(self) -> bool {
        matches!(self, SnapMode::Grid | SnapMode::All)
    }

    /// Check if shape snapping is enabled.
    pub fn snaps_to_shapes(self) -> bool {
        matches!(self, SnapMode::Shapes | SnapMode::All)
    }

    /// Check if any snapping is enabled.
    pub fn is_enabled(self) -> bool {
        self != SnapMode::None
    }
}

/// Result of a snap operation.
#[derive(Debug, Clone, Copy)]
pub struct SnapResult {
    /// The snapped point.
    pub point: Point,
    /// Whether the X coordinate was snapped.
    pub snapped_x: bool,
    /// Whether the Y coordinate was snapped.
    pub snapped_y: bool,
}

impl SnapResult {
    /// Create a result with no snapping.
    pub fn none(point: Point) -> Self {
        Self {
            point,
            snapped_x: false,
            snapped_y: false,
        }
    }

    /// Check if any snapping occurred.
    pub fn is_snapped(&self) -> bool {
        self.snapped_x || self.snapped_y
    }
}

/// Angle snap increment in degrees.
pub const ANGLE_SNAP_INCREMENT: f64 = 15.0;

/// Result of an angle snap operation.
#[derive(Debug, Clone, Copy)]
pub struct AngleSnapResult {
    /// The snapped endpoint.
    pub point: Point,
    /// The snapped angle in degrees (0-360).
    pub angle_degrees: f64,
    /// The original (unsnapped) angle in degrees.
    pub original_angle_degrees: f64,
    /// Whether angle snapping occurred.
    pub snapped: bool,
    /// Distance from start point (preserved from original).
    pub distance: f64,
}

impl AngleSnapResult {
    /// Create a result with no snapping.
    pub fn none(point: Point, start: Point) -> Self {
        let dx = point.x - start.x;
        let dy = point.y - start.y;
        let angle = dy.atan2(dx).to_degrees();
        // Normalize to 0-360
        let angle_normalized = if angle < 0.0 { angle + 360.0 } else { angle };
        let distance = (dx * dx + dy * dy).sqrt();

        Self {
            point,
            angle_degrees: angle_normalized,
            original_angle_degrees: angle_normalized,
            snapped: false,
            distance,
        }
    }
}

/// Snap an angle to the nearest increment.
/// Returns the snapped angle in degrees (0-360).
pub fn snap_angle(angle_degrees: f64, increment: f64) -> f64 {
    let snapped = (angle_degrees / increment).round() * increment;
    // Normalize to 0-360
    if snapped < 0.0 {
        snapped + 360.0
    } else if snapped >= 360.0 {
        snapped - 360.0
    } else {
        snapped
    }
}

/// Snap a line endpoint to angle increments from a start point.
/// This snaps the angle while preserving the distance from start.
pub fn snap_line_endpoint(start: Point, end: Point, angle_snap_enabled: bool) -> AngleSnapResult {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let distance = (dx * dx + dy * dy).sqrt();

    // Handle zero-length case
    if distance < 0.001 {
        return AngleSnapResult {
            point: end,
            angle_degrees: 0.0,
            original_angle_degrees: 0.0,
            snapped: false,
            distance: 0.0,
        };
    }

    let original_angle = dy.atan2(dx).to_degrees();
    // Normalize to 0-360
    let original_angle_normalized = if original_angle < 0.0 {
        original_angle + 360.0
    } else {
        original_angle
    };

    if !angle_snap_enabled {
        return AngleSnapResult {
            point: end,
            angle_degrees: original_angle_normalized,
            original_angle_degrees: original_angle_normalized,
            snapped: false,
            distance,
        };
    }

    // Snap to nearest 15° increment
    let snapped_angle = snap_angle(original_angle_normalized, ANGLE_SNAP_INCREMENT);
    let snapped_radians = snapped_angle.to_radians();

    // Calculate new endpoint at snapped angle with same distance
    let snapped_point = Point::new(
        start.x + distance * snapped_radians.cos(),
        start.y + distance * snapped_radians.sin(),
    );

    // Check if we actually snapped (angle changed meaningfully)
    let angle_diff = (snapped_angle - original_angle_normalized).abs();
    let did_snap = angle_diff > 0.1 && angle_diff < 359.9;

    AngleSnapResult {
        point: snapped_point,
        angle_degrees: snapped_angle,
        original_angle_degrees: original_angle_normalized,
        snapped: did_snap || angle_diff < 0.1, // Consider it snapped if very close to snap angle
        distance,
    }
}

/// Snap a point to the nearest grid intersection.
pub fn snap_to_grid(point: Point, grid_size: f64) -> SnapResult {
    let snapped_x = (point.x / grid_size).round() * grid_size;
    let snapped_y = (point.y / grid_size).round() * grid_size;

    SnapResult {
        point: Point::new(snapped_x, snapped_y),
        snapped_x: true,
        snapped_y: true,
    }
}

/// Snap a point based on the current snap mode (without shape data).
/// For shape snapping, use `snap_point_with_shapes` instead.
pub fn snap_point(point: Point, mode: SnapMode, grid_size: f64) -> SnapResult {
    match mode {
        SnapMode::None => SnapResult::none(point),
        SnapMode::Grid => snap_to_grid(point, grid_size),
        SnapMode::Shapes => SnapResult::none(point), // Need shapes, use snap_point_with_shapes
        SnapMode::All => snap_to_grid(point, grid_size), // Grid only without shapes
    }
}

/// Distance threshold for shape snapping (in world units).
pub const SHAPE_SNAP_THRESHOLD: f64 = 10.0;

/// A point that can be snapped to on a shape.
#[derive(Debug, Clone, Copy)]
pub struct SnapTarget {
    /// The snap point location.
    pub point: Point,
    /// Type of snap target for visual feedback.
    pub kind: SnapTargetKind,
}

/// Type of snap target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapTargetKind {
    /// Corner/vertex of a shape.
    Corner,
    /// Midpoint of an edge.
    Midpoint,
    /// Center of a shape.
    Center,
    /// Point on an edge.
    Edge,
}

/// Collect snap targets from a shape's bounds.
pub fn get_snap_targets_from_bounds(bounds: kurbo::Rect) -> Vec<SnapTarget> {
    vec![
        // Corners
        SnapTarget {
            point: Point::new(bounds.x0, bounds.y0),
            kind: SnapTargetKind::Corner,
        },
        SnapTarget {
            point: Point::new(bounds.x1, bounds.y0),
            kind: SnapTargetKind::Corner,
        },
        SnapTarget {
            point: Point::new(bounds.x1, bounds.y1),
            kind: SnapTargetKind::Corner,
        },
        SnapTarget {
            point: Point::new(bounds.x0, bounds.y1),
            kind: SnapTargetKind::Corner,
        },
        // Edge midpoints
        SnapTarget {
            point: Point::new((bounds.x0 + bounds.x1) / 2.0, bounds.y0),
            kind: SnapTargetKind::Midpoint,
        },
        SnapTarget {
            point: Point::new(bounds.x1, (bounds.y0 + bounds.y1) / 2.0),
            kind: SnapTargetKind::Midpoint,
        },
        SnapTarget {
            point: Point::new((bounds.x0 + bounds.x1) / 2.0, bounds.y1),
            kind: SnapTargetKind::Midpoint,
        },
        SnapTarget {
            point: Point::new(bounds.x0, (bounds.y0 + bounds.y1) / 2.0),
            kind: SnapTargetKind::Midpoint,
        },
        // Center
        SnapTarget {
            point: Point::new((bounds.x0 + bounds.x1) / 2.0, (bounds.y0 + bounds.y1) / 2.0),
            kind: SnapTargetKind::Center,
        },
    ]
}

/// Collect snap targets from line endpoints.
pub fn get_snap_targets_from_line(start: Point, end: Point) -> Vec<SnapTarget> {
    vec![
        SnapTarget {
            point: start,
            kind: SnapTargetKind::Corner,
        },
        SnapTarget {
            point: end,
            kind: SnapTargetKind::Corner,
        },
        SnapTarget {
            point: Point::new((start.x + end.x) / 2.0, (start.y + end.y) / 2.0),
            kind: SnapTargetKind::Midpoint,
        },
    ]
}

/// Snap a point to the nearest shape snap target.
pub fn snap_to_shapes(point: Point, targets: &[SnapTarget], threshold: f64) -> SnapResult {
    let mut best_target: Option<&SnapTarget> = None;
    let mut best_dist_sq = threshold * threshold;

    for target in targets {
        let dx = point.x - target.point.x;
        let dy = point.y - target.point.y;
        let dist_sq = dx * dx + dy * dy;

        if dist_sq < best_dist_sq {
            best_dist_sq = dist_sq;
            best_target = Some(target);
        }
    }

    if let Some(target) = best_target {
        SnapResult {
            point: target.point,
            snapped_x: true,
            snapped_y: true,
        }
    } else {
        SnapResult::none(point)
    }
}

/// Snap a point based on the current snap mode with shape targets.
pub fn snap_point_with_shapes(
    point: Point,
    mode: SnapMode,
    grid_size: f64,
    shape_targets: &[SnapTarget],
) -> SnapResult {
    match mode {
        SnapMode::None => SnapResult::none(point),
        SnapMode::Grid => snap_to_grid(point, grid_size),
        SnapMode::Shapes => snap_to_shapes(point, shape_targets, SHAPE_SNAP_THRESHOLD),
        SnapMode::All => {
            // Try shape snap first (higher priority), then grid
            let shape_result = snap_to_shapes(point, shape_targets, SHAPE_SNAP_THRESHOLD);
            if shape_result.is_snapped() {
                shape_result
            } else {
                snap_to_grid(point, grid_size)
            }
        }
    }
}

/// Find where a ray from origin at a given angle intersects with grid lines.
/// Returns the nearest intersection point to the target point.
///
/// This snaps to where the angle ray crosses horizontal or vertical grid lines,
/// NOT to grid corners (which may not be on the ray).
///
/// `origin` - The start point of the ray
/// `angle_degrees` - The angle of the ray in degrees
/// `target_point` - The point we're trying to snap (used to find nearest intersection)
/// `grid_size` - The grid cell size
pub fn snap_ray_to_grid_lines(
    origin: Point,
    angle_degrees: f64,
    target_point: Point,
    grid_size: f64,
) -> SnapResult {
    let angle_rad = angle_degrees.to_radians();
    let cos_a = angle_rad.cos();
    let sin_a = angle_rad.sin();

    let mut best_point = target_point;
    let mut best_dist_to_target = f64::MAX;
    let mut found = false;

    // Handle special cases for axis-aligned angles
    let is_horizontal = sin_a.abs() < 0.001;
    let is_vertical = cos_a.abs() < 0.001;

    if is_horizontal {
        // Horizontal line: snap to vertical grid lines
        let y = origin.y;
        // Find vertical grid lines near target
        let base_grid_x = (target_point.x / grid_size).round() as i32;
        for dx in -5..=5 {
            let grid_x = (base_grid_x + dx) as f64 * grid_size;
            // Check direction matches
            if (grid_x - origin.x) * cos_a >= 0.0 || (grid_x - origin.x).abs() < 0.001 {
                let candidate = Point::new(grid_x, y);
                let dist =
                    (candidate.x - target_point.x).powi(2) + (candidate.y - target_point.y).powi(2);
                if dist < best_dist_to_target {
                    best_dist_to_target = dist;
                    best_point = candidate;
                    found = true;
                }
            }
        }
    } else if is_vertical {
        // Vertical line: snap to horizontal grid lines
        let x = origin.x;
        let base_grid_y = (target_point.y / grid_size).round() as i32;
        for dy in -5..=5 {
            let grid_y = (base_grid_y + dy) as f64 * grid_size;
            // Check direction matches
            if (grid_y - origin.y) * sin_a >= 0.0 || (grid_y - origin.y).abs() < 0.001 {
                let candidate = Point::new(x, grid_y);
                let dist =
                    (candidate.x - target_point.x).powi(2) + (candidate.y - target_point.y).powi(2);
                if dist < best_dist_to_target {
                    best_dist_to_target = dist;
                    best_point = candidate;
                    found = true;
                }
            }
        }
    } else {
        // General case: find intersections with both horizontal and vertical grid lines

        // Intersections with vertical grid lines (x = n * grid_size)
        let base_grid_x = (target_point.x / grid_size).round() as i32;
        for dx in -5..=5 {
            let grid_x = (base_grid_x + dx) as f64 * grid_size;
            // Ray: x = origin.x + t * cos_a, y = origin.y + t * sin_a
            // At grid_x: t = (grid_x - origin.x) / cos_a
            let t = (grid_x - origin.x) / cos_a;
            if t >= 0.0 {
                // t must be positive (in direction of ray)
                let y = origin.y + t * sin_a;
                let candidate = Point::new(grid_x, y);
                let dist =
                    (candidate.x - target_point.x).powi(2) + (candidate.y - target_point.y).powi(2);
                if dist < best_dist_to_target {
                    best_dist_to_target = dist;
                    best_point = candidate;
                    found = true;
                }
            }
        }

        // Intersections with horizontal grid lines (y = n * grid_size)
        let base_grid_y = (target_point.y / grid_size).round() as i32;
        for dy in -5..=5 {
            let grid_y = (base_grid_y + dy) as f64 * grid_size;
            // At grid_y: t = (grid_y - origin.y) / sin_a
            let t = (grid_y - origin.y) / sin_a;
            if t >= 0.0 {
                let x = origin.x + t * cos_a;
                let candidate = Point::new(x, grid_y);
                let dist =
                    (candidate.x - target_point.x).powi(2) + (candidate.y - target_point.y).powi(2);
                if dist < best_dist_to_target {
                    best_dist_to_target = dist;
                    best_point = candidate;
                    found = true;
                }
            }
        }
    }

    if found {
        SnapResult {
            point: best_point,
            snapped_x: true,
            snapped_y: true,
        }
    } else {
        SnapResult::none(target_point)
    }
}

/// Snap a line endpoint considering both angle snapping and grid snapping.
///
/// When BOTH angle_snap AND grid_snap are enabled:
///   - Snaps to grid LINE intersections (where the angle ray crosses horizontal/vertical grid lines)
///   - This keeps the endpoint EXACTLY on the snapped angle ray
///
/// When only angle_snap is enabled:
///   - Snaps angle to 15° increments, preserves distance
///
/// When only grid_snap is enabled:
///   - Snaps to nearest grid corner
///
/// The `_use_isometric_grid` parameter is kept for API compatibility but is no longer used.
/// The behavior is now: angle_snap + grid_snap = snap to grid LINE intersections on the angle ray.
pub fn snap_line_endpoint_isometric(
    start: Point,
    end: Point,
    angle_snap_enabled: bool,
    grid_snap_enabled: bool,
    _use_isometric_grid: bool,
    grid_size: f64,
) -> AngleSnapResult {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let distance = (dx * dx + dy * dy).sqrt();

    // Handle zero-length case
    if distance < 0.001 {
        return AngleSnapResult {
            point: end,
            angle_degrees: 0.0,
            original_angle_degrees: 0.0,
            snapped: false,
            distance: 0.0,
        };
    }

    let original_angle = dy.atan2(dx).to_degrees();
    let original_angle_normalized = if original_angle < 0.0 {
        original_angle + 360.0
    } else {
        original_angle
    };

    // When BOTH angle snap AND grid snap are enabled:
    // Snap to grid LINE intersections (not corners) - this keeps the point ON the angle ray
    if angle_snap_enabled && grid_snap_enabled {
        // First snap to angle
        let snapped_angle = snap_angle(original_angle_normalized, ANGLE_SNAP_INCREMENT);

        // Calculate point on the snapped angle line at the original distance
        let snapped_radians = snapped_angle.to_radians();
        let angle_snapped_point = Point::new(
            start.x + distance * snapped_radians.cos(),
            start.y + distance * snapped_radians.sin(),
        );

        // Find where this angle ray intersects with grid LINES (not corners)
        let grid_snap =
            snap_ray_to_grid_lines(start, snapped_angle, angle_snapped_point, grid_size);

        if grid_snap.is_snapped() {
            // Recalculate distance for the grid-snapped point
            let new_dx = grid_snap.point.x - start.x;
            let new_dy = grid_snap.point.y - start.y;
            let new_distance = (new_dx * new_dx + new_dy * new_dy).sqrt();

            return AngleSnapResult {
                point: grid_snap.point,
                angle_degrees: snapped_angle,
                original_angle_degrees: original_angle_normalized,
                snapped: true,
                distance: new_distance,
            };
        }

        // If no grid line intersection found, just use angle snapping
        return AngleSnapResult {
            point: angle_snapped_point,
            angle_degrees: snapped_angle,
            original_angle_degrees: original_angle_normalized,
            snapped: true,
            distance,
        };
    }

    // Only grid snap (no angle snap): snap to grid corners
    if !angle_snap_enabled {
        if grid_snap_enabled {
            let grid_snap = snap_to_grid(end, grid_size);
            return AngleSnapResult {
                point: grid_snap.point,
                angle_degrees: original_angle_normalized,
                original_angle_degrees: original_angle_normalized,
                snapped: grid_snap.is_snapped(),
                distance,
            };
        }
        return AngleSnapResult {
            point: end,
            angle_degrees: original_angle_normalized,
            original_angle_degrees: original_angle_normalized,
            snapped: false,
            distance,
        };
    }

    // Only angle snap (no grid snap): snap angle, preserve distance
    let snapped_angle = snap_angle(original_angle_normalized, ANGLE_SNAP_INCREMENT);
    let snapped_radians = snapped_angle.to_radians();

    // Calculate new endpoint at snapped angle with same distance
    let snapped_point = Point::new(
        start.x + distance * snapped_radians.cos(),
        start.y + distance * snapped_radians.sin(),
    );

    let angle_diff = (snapped_angle - original_angle_normalized).abs();
    let did_snap = angle_diff > 0.1 && angle_diff < 359.9;

    AngleSnapResult {
        point: snapped_point,
        angle_degrees: snapped_angle,
        original_angle_degrees: original_angle_normalized,
        snapped: did_snap || angle_diff < 0.1,
        distance,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_snap_mode_cycle() {
        assert_eq!(SnapMode::None.next(), SnapMode::Grid);
        assert_eq!(SnapMode::Grid.next(), SnapMode::Shapes);
        assert_eq!(SnapMode::Shapes.next(), SnapMode::All);
        assert_eq!(SnapMode::All.next(), SnapMode::None);
    }

    #[test]
    fn test_snap_mode_flags() {
        assert!(!SnapMode::None.snaps_to_grid());
        assert!(SnapMode::Grid.snaps_to_grid());
        assert!(!SnapMode::Shapes.snaps_to_grid());
        assert!(SnapMode::All.snaps_to_grid());

        assert!(!SnapMode::None.snaps_to_shapes());
        assert!(!SnapMode::Grid.snaps_to_shapes());
        assert!(SnapMode::Shapes.snaps_to_shapes());
        assert!(SnapMode::All.snaps_to_shapes());
    }

    #[test]
    fn test_snap_angle() {
        // Test snapping to 15° increments
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
        let start = Point::new(0.0, 0.0);
        let end = Point::new(100.0, 5.0); // Slightly off horizontal
        let result = snap_line_endpoint(start, end, true);

        // Should snap to 0° (horizontal)
        assert!(result.snapped);
        assert!((result.angle_degrees - 0.0).abs() < 0.01);
        assert!((result.point.y - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_snap_line_endpoint_45_degrees() {
        let start = Point::new(0.0, 0.0);
        let end = Point::new(100.0, 102.0); // Slightly off 45°
        let result = snap_line_endpoint(start, end, true);

        // Should snap to 45°
        assert!(result.snapped);
        assert!((result.angle_degrees - 45.0).abs() < 0.01);
    }

    #[test]
    fn test_snap_line_endpoint_disabled() {
        let start = Point::new(0.0, 0.0);
        let end = Point::new(100.0, 5.0);
        let result = snap_line_endpoint(start, end, false);

        // Should not snap
        assert!(!result.snapped);
        assert_eq!(result.point, end);
    }

    #[test]
    fn test_snap_line_preserves_distance() {
        let start = Point::new(0.0, 0.0);
        let end = Point::new(100.0, 10.0);
        let result = snap_line_endpoint(start, end, true);

        // Distance should be preserved
        let original_distance = ((100.0_f64).powi(2) + (10.0_f64).powi(2)).sqrt();
        assert!((result.distance - original_distance).abs() < 0.01);
    }

    #[test]
    fn test_snap_ray_to_grid_lines_horizontal() {
        // Ray from origin at 0° (horizontal)
        let origin = Point::new(0.0, 0.0);
        let target = Point::new(55.0, 0.0);

        // Should snap to nearest vertical grid line intersection
        let result = snap_ray_to_grid_lines(origin, 0.0, target, 20.0);
        assert!(result.is_snapped());
        assert!((result.point.x - 60.0).abs() < 0.01); // Nearest grid line at x=60
        assert!((result.point.y - 0.0).abs() < 0.01); // Y stays at 0
    }

    #[test]
    fn test_snap_ray_to_grid_lines_45_degrees() {
        // Ray from origin at 45°
        let origin = Point::new(0.0, 0.0);
        let target = Point::new(50.0, 50.0);

        // At 45°, the ray crosses grid lines at various points
        // e.g., crosses x=20 at (20, 20), x=40 at (40, 40), etc.
        // and crosses y=20 at (20, 20), y=40 at (40, 40), etc.
        let result = snap_ray_to_grid_lines(origin, 45.0, target, 20.0);
        assert!(result.is_snapped());
        // Should snap to (40, 40) or (60, 60) - nearest grid line intersection
        // At 45°, x should equal y
        assert!((result.point.x - result.point.y).abs() < 0.01);
    }

    #[test]
    fn test_snap_ray_to_grid_lines_30_degrees() {
        // Ray from origin at 30°
        let origin = Point::new(0.0, 0.0);
        let target = Point::new(100.0, 57.7); // approximately 100 * tan(30°)

        let result = snap_ray_to_grid_lines(origin, 30.0, target, 20.0);
        assert!(result.is_snapped());
        // Point should be on the 30° ray
        let angle = (result.point.y / result.point.x).atan().to_degrees();
        assert!((angle - 30.0).abs() < 0.1);
    }

    #[test]
    fn test_snap_line_endpoint_isometric() {
        let start = Point::new(0.0, 0.0);
        let end = Point::new(100.0, 5.0); // Slightly off horizontal

        // With isometric grid, should snap to grid LINE (not corner) on 0° ray
        let result = snap_line_endpoint_isometric(start, end, true, true, true, 20.0);

        // Should be snapped
        assert!(result.snapped);
        // Y should be 0 (on horizontal line from origin)
        assert!((result.point.y).abs() < 0.01);
        // X should be on a grid line (multiple of 20)
        assert!(
            (result.point.x % 20.0).abs() < 0.01 || (result.point.x % 20.0 - 20.0).abs() < 0.01
        );
    }

    #[test]
    fn test_snap_line_endpoint_isometric_45_degrees() {
        let start = Point::new(0.0, 0.0);
        let end = Point::new(100.0, 102.0); // Slightly off 45°

        // With isometric grid, should snap to grid LINE on 45° ray
        let result = snap_line_endpoint_isometric(start, end, true, true, true, 20.0);

        // Should be snapped
        assert!(result.snapped);
        // Should be on a 45° angle (x ≈ y since from origin)
        assert!((result.point.x - result.point.y).abs() < 0.1);
        // Either x or y should be on a grid line
        let on_x_grid =
            (result.point.x % 20.0).abs() < 0.01 || (result.point.x % 20.0 - 20.0).abs() < 0.01;
        let on_y_grid =
            (result.point.y % 20.0).abs() < 0.01 || (result.point.y % 20.0 - 20.0).abs() < 0.01;
        assert!(on_x_grid || on_y_grid);
    }
}
