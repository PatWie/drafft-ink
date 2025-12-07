//! Line shape.

use super::{ShapeId, ShapeStyle, ShapeTrait};
use kurbo::{Affine, BezPath, Line as KurboLine, Point, Rect};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A line segment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Line {
    pub(crate) id: ShapeId,
    /// Start point.
    pub start: Point,
    /// End point.
    pub end: Point,
    /// Style properties.
    pub style: ShapeStyle,
}

impl Line {
    /// Create a new line.
    pub fn new(start: Point, end: Point) -> Self {
        Self {
            id: Uuid::new_v4(),
            start,
            end,
            style: ShapeStyle::default(),
        }
    }

    /// Get the length of the line.
    pub fn length(&self) -> f64 {
        let dx = self.end.x - self.start.x;
        let dy = self.end.y - self.start.y;
        (dx * dx + dy * dy).sqrt()
    }

    /// Get the midpoint of the line.
    pub fn midpoint(&self) -> Point {
        Point::new(
            (self.start.x + self.end.x) / 2.0,
            (self.start.y + self.end.y) / 2.0,
        )
    }

    /// Get as a kurbo Line.
    pub fn as_kurbo(&self) -> KurboLine {
        KurboLine::new(self.start, self.end)
    }
}

impl ShapeTrait for Line {
    fn id(&self) -> ShapeId {
        self.id
    }

    fn bounds(&self) -> Rect {
        Rect::new(
            self.start.x.min(self.end.x),
            self.start.y.min(self.end.y),
            self.start.x.max(self.end.x),
            self.start.y.max(self.end.y),
        )
    }

    fn hit_test(&self, point: Point, tolerance: f64) -> bool {
        // Distance from point to line segment
        let line_vec = kurbo::Vec2::new(self.end.x - self.start.x, self.end.y - self.start.y);
        let point_vec = kurbo::Vec2::new(point.x - self.start.x, point.y - self.start.y);

        let line_len_sq = line_vec.hypot2();
        if line_len_sq < f64::EPSILON {
            // Line is a point
            let dist = point_vec.hypot();
            return dist <= tolerance;
        }

        // Project point onto line, clamped to segment
        let t = (point_vec.dot(line_vec) / line_len_sq).clamp(0.0, 1.0);
        let projection = Point::new(self.start.x + t * line_vec.x, self.start.y + t * line_vec.y);

        let dist = ((point.x - projection.x).powi(2) + (point.y - projection.y).powi(2)).sqrt();
        dist <= tolerance + self.style.stroke_width / 2.0
    }

    fn to_path(&self) -> BezPath {
        let mut path = BezPath::new();
        path.move_to(self.start);
        path.line_to(self.end);
        path
    }

    fn style(&self) -> &ShapeStyle {
        &self.style
    }

    fn style_mut(&mut self) -> &mut ShapeStyle {
        &mut self.style
    }

    fn transform(&mut self, affine: Affine) {
        self.start = affine * self.start;
        self.end = affine * self.end;
    }

    fn clone_box(&self) -> Box<dyn ShapeTrait + Send + Sync> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_line_creation() {
        let line = Line::new(Point::new(0.0, 0.0), Point::new(100.0, 0.0));
        assert!((line.length() - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_midpoint() {
        let line = Line::new(Point::new(0.0, 0.0), Point::new(100.0, 100.0));
        let mid = line.midpoint();
        assert!((mid.x - 50.0).abs() < f64::EPSILON);
        assert!((mid.y - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_hit_test_on_line() {
        let line = Line::new(Point::new(0.0, 0.0), Point::new(100.0, 0.0));
        assert!(line.hit_test(Point::new(50.0, 0.0), 1.0));
        assert!(line.hit_test(Point::new(50.0, 2.0), 5.0));
        assert!(!line.hit_test(Point::new(50.0, 20.0), 5.0));
    }

    #[test]
    fn test_hit_test_endpoints() {
        let line = Line::new(Point::new(0.0, 0.0), Point::new(100.0, 0.0));
        assert!(line.hit_test(Point::new(0.0, 0.0), 1.0));
        assert!(line.hit_test(Point::new(100.0, 0.0), 1.0));
    }

    #[test]
    fn test_bounds() {
        let line = Line::new(Point::new(10.0, 20.0), Point::new(50.0, 80.0));
        let bounds = line.bounds();
        assert!((bounds.x0 - 10.0).abs() < f64::EPSILON);
        assert!((bounds.y0 - 20.0).abs() < f64::EPSILON);
        assert!((bounds.x1 - 50.0).abs() < f64::EPSILON);
        assert!((bounds.y1 - 80.0).abs() < f64::EPSILON);
    }
}
