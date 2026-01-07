//! Freehand drawing shape.

use super::{ShapeId, ShapeStyle, ShapeTrait};
use kurbo::{Affine, BezPath, Point, Rect};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A freehand drawing (series of points with optional pressure).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Freehand {
    pub(crate) id: ShapeId,
    /// Points in the freehand path.
    pub points: Vec<Point>,
    /// Pressure values per point (0.0-1.0). If empty, uniform pressure is assumed.
    #[serde(default)]
    pub pressures: Vec<f64>,
    /// Style properties.
    pub style: ShapeStyle,
}

impl Freehand {
    /// Create a new empty freehand shape.
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4(),
            points: Vec::new(),
            pressures: Vec::new(),
            style: ShapeStyle::default(),
        }
    }

    /// Create from existing points.
    pub fn from_points(points: Vec<Point>) -> Self {
        Self {
            id: Uuid::new_v4(),
            points,
            pressures: Vec::new(),
            style: ShapeStyle::default(),
        }
    }

    /// Create from points with pressure values.
    pub fn from_points_with_pressure(points: Vec<Point>, pressures: Vec<f64>) -> Self {
        Self {
            id: Uuid::new_v4(),
            points,
            pressures,
            style: ShapeStyle::default(),
        }
    }

    /// Reconstruct a freehand with a specific ID (for CRDT/storage).
    pub(crate) fn reconstruct(id: ShapeId, points: Vec<Point>, style: ShapeStyle) -> Self {
        Self {
            id,
            points,
            pressures: Vec::new(),
            style,
        }
    }

    /// Add a point to the path.
    pub fn add_point(&mut self, point: Point) {
        self.points.push(point);
    }

    /// Add a point with pressure value.
    pub fn add_point_with_pressure(&mut self, point: Point, pressure: f64) {
        self.points.push(point);
        self.pressures.push(pressure.clamp(0.0, 1.0));
    }

    /// Get pressure at index (returns 1.0 if no pressure data).
    pub fn pressure_at(&self, index: usize) -> f64 {
        self.pressures.get(index).copied().unwrap_or(1.0)
    }

    /// Check if this freehand has pressure data.
    pub fn has_pressure(&self) -> bool {
        !self.pressures.is_empty()
    }

    /// Get the number of points.
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// Check if the path is empty.
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Simplify the path by removing redundant points.
    pub fn simplify(&mut self, tolerance: f64) {
        if self.points.len() < 3 {
            return;
        }

        // Ramer-Douglas-Peucker algorithm with pressure preservation
        let (new_points, new_pressures) =
            rdp_simplify_with_pressure(&self.points, &self.pressures, tolerance);
        self.points = new_points;
        self.pressures = new_pressures;
    }
}

impl Default for Freehand {
    fn default() -> Self {
        Self::new()
    }
}

/// Ramer-Douglas-Peucker line simplification with pressure preservation.
fn rdp_simplify_with_pressure(
    points: &[Point],
    pressures: &[f64],
    tolerance: f64,
) -> (Vec<Point>, Vec<f64>) {
    if points.len() < 3 {
        let p = if pressures.len() == points.len() {
            pressures.to_vec()
        } else {
            vec![1.0; points.len()]
        };
        return (points.to_vec(), p);
    }

    // Find point with maximum distance from line between first and last
    let first = points[0];
    let last = points[points.len() - 1];

    let mut max_dist = 0.0;
    let mut max_index = 0;

    for (i, point) in points.iter().enumerate().skip(1).take(points.len() - 2) {
        let dist = perpendicular_distance(*point, first, last);
        if dist > max_dist {
            max_dist = dist;
            max_index = i;
        }
    }

    if max_dist > tolerance {
        // Recursively simplify
        let left_pressures = if pressures.len() > max_index {
            &pressures[..=max_index]
        } else {
            &[]
        };
        let right_pressures = if pressures.len() > max_index {
            &pressures[max_index..]
        } else {
            &[]
        };

        let (mut left_pts, mut left_press) =
            rdp_simplify_with_pressure(&points[..=max_index], left_pressures, tolerance);
        let (right_pts, right_press) =
            rdp_simplify_with_pressure(&points[max_index..], right_pressures, tolerance);

        // Combine, removing duplicate point at junction
        left_pts.pop();
        left_press.pop();
        left_pts.extend(right_pts);
        left_press.extend(right_press);
        (left_pts, left_press)
    } else {
        // All points between first and last can be removed
        let first_pressure = pressures.first().copied().unwrap_or(1.0);
        let last_pressure = pressures.last().copied().unwrap_or(1.0);
        (vec![first, last], vec![first_pressure, last_pressure])
    }
}

/// Ramer-Douglas-Peucker line simplification (legacy, no pressure).
fn rdp_simplify(points: &[Point], tolerance: f64) -> Vec<Point> {
    rdp_simplify_with_pressure(points, &[], tolerance).0
}

/// Calculate perpendicular distance from point to line.
fn perpendicular_distance(point: Point, line_start: Point, line_end: Point) -> f64 {
    let dx = line_end.x - line_start.x;
    let dy = line_end.y - line_start.y;

    let line_len_sq = dx * dx + dy * dy;
    if line_len_sq < f64::EPSILON {
        // Line is a point
        let px = point.x - line_start.x;
        let py = point.y - line_start.y;
        return (px * px + py * py).sqrt();
    }

    // Area of triangle * 2 / base = height
    let area2 = ((point.x - line_start.x) * dy - (point.y - line_start.y) * dx).abs();
    area2 / line_len_sq.sqrt()
}

impl ShapeTrait for Freehand {
    fn id(&self) -> ShapeId {
        self.id
    }

    fn bounds(&self) -> Rect {
        if self.points.is_empty() {
            return Rect::ZERO;
        }

        let mut min_x = f64::MAX;
        let mut min_y = f64::MAX;
        let mut max_x = f64::MIN;
        let mut max_y = f64::MIN;

        for point in &self.points {
            min_x = min_x.min(point.x);
            min_y = min_y.min(point.y);
            max_x = max_x.max(point.x);
            max_y = max_y.max(point.y);
        }

        Rect::new(min_x, min_y, max_x, max_y)
    }

    fn hit_test(&self, point: Point, tolerance: f64) -> bool {
        if self.points.len() < 2 {
            if let Some(p) = self.points.first() {
                let dx = point.x - p.x;
                let dy = point.y - p.y;
                return (dx * dx + dy * dy).sqrt() <= tolerance;
            }
            return false;
        }

        // Check distance to each line segment
        for window in self.points.windows(2) {
            let start = window[0];
            let end = window[1];

            let line_vec = kurbo::Vec2::new(end.x - start.x, end.y - start.y);
            let point_vec = kurbo::Vec2::new(point.x - start.x, point.y - start.y);

            let line_len_sq = line_vec.hypot2();
            if line_len_sq < f64::EPSILON {
                continue;
            }

            let t = (point_vec.dot(line_vec) / line_len_sq).clamp(0.0, 1.0);
            let projection = Point::new(start.x + t * line_vec.x, start.y + t * line_vec.y);

            let dist = ((point.x - projection.x).powi(2) + (point.y - projection.y).powi(2)).sqrt();
            if dist <= tolerance + self.style.stroke_width / 2.0 {
                return true;
            }
        }

        false
    }

    fn to_path(&self) -> BezPath {
        let mut path = BezPath::new();

        if self.points.is_empty() {
            return path;
        }

        path.move_to(self.points[0]);
        for point in self.points.iter().skip(1) {
            path.line_to(*point);
        }

        path
    }

    fn style(&self) -> &ShapeStyle {
        &self.style
    }

    fn style_mut(&mut self) -> &mut ShapeStyle {
        &mut self.style
    }

    fn transform(&mut self, affine: Affine) {
        for point in &mut self.points {
            *point = affine * *point;
        }
    }

    fn clone_box(&self) -> Box<dyn ShapeTrait + Send + Sync> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_freehand_creation() {
        let freehand = Freehand::new();
        assert!(freehand.is_empty());
    }

    #[test]
    fn test_add_points() {
        let mut freehand = Freehand::new();
        freehand.add_point(Point::new(0.0, 0.0));
        freehand.add_point(Point::new(10.0, 10.0));
        assert_eq!(freehand.len(), 2);
    }

    #[test]
    fn test_bounds() {
        let freehand = Freehand::from_points(vec![
            Point::new(0.0, 0.0),
            Point::new(100.0, 50.0),
            Point::new(50.0, 100.0),
        ]);

        let bounds = freehand.bounds();
        assert!((bounds.x0).abs() < f64::EPSILON);
        assert!((bounds.y0).abs() < f64::EPSILON);
        assert!((bounds.x1 - 100.0).abs() < f64::EPSILON);
        assert!((bounds.y1 - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_simplify() {
        let mut freehand = Freehand::from_points(vec![
            Point::new(0.0, 0.0),
            Point::new(1.0, 0.1), // Almost on line
            Point::new(2.0, 0.0),
            Point::new(3.0, 0.1), // Almost on line
            Point::new(4.0, 0.0),
        ]);

        freehand.simplify(0.5);
        assert!(freehand.len() < 5);
    }

    #[test]
    fn test_hit_test() {
        let freehand = Freehand::from_points(vec![Point::new(0.0, 0.0), Point::new(100.0, 0.0)]);

        assert!(freehand.hit_test(Point::new(50.0, 0.0), 5.0));
        assert!(!freehand.hit_test(Point::new(50.0, 20.0), 5.0));
    }
}
