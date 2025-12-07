//! Arrow shape.

use super::{ShapeId, ShapeStyle, ShapeTrait};
use kurbo::{Affine, BezPath, Point, Rect, Vec2};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// An arrow shape (line with arrowhead).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Arrow {
    pub(crate) id: ShapeId,
    /// Start point.
    pub start: Point,
    /// End point (where the arrowhead points).
    pub end: Point,
    /// Size of the arrowhead.
    pub head_size: f64,
    /// Style properties.
    pub style: ShapeStyle,
}

impl Arrow {
    /// Create a new arrow.
    pub fn new(start: Point, end: Point) -> Self {
        Self {
            id: Uuid::new_v4(),
            start,
            end,
            head_size: 15.0,
            style: ShapeStyle::default(),
        }
    }

    /// Get the direction vector (normalized).
    pub fn direction(&self) -> Vec2 {
        let dx = self.end.x - self.start.x;
        let dy = self.end.y - self.start.y;
        let len = (dx * dx + dy * dy).sqrt();
        if len < f64::EPSILON {
            Vec2::new(1.0, 0.0)
        } else {
            Vec2::new(dx / len, dy / len)
        }
    }

    /// Get the length of the arrow shaft.
    pub fn length(&self) -> f64 {
        let dx = self.end.x - self.start.x;
        let dy = self.end.y - self.start.y;
        (dx * dx + dy * dy).sqrt()
    }
}

impl ShapeTrait for Arrow {
    fn id(&self) -> ShapeId {
        self.id
    }

    fn bounds(&self) -> Rect {
        // Include arrowhead in bounds
        let dir = self.direction();
        let perp = Vec2::new(-dir.y, dir.x);

        let head_back = Point::new(
            self.end.x - dir.x * self.head_size,
            self.end.y - dir.y * self.head_size,
        );
        let head_left = Point::new(
            head_back.x + perp.x * self.head_size * 0.5,
            head_back.y + perp.y * self.head_size * 0.5,
        );
        let head_right = Point::new(
            head_back.x - perp.x * self.head_size * 0.5,
            head_back.y - perp.y * self.head_size * 0.5,
        );

        let min_x = self
            .start
            .x
            .min(self.end.x)
            .min(head_left.x)
            .min(head_right.x);
        let min_y = self
            .start
            .y
            .min(self.end.y)
            .min(head_left.y)
            .min(head_right.y);
        let max_x = self
            .start
            .x
            .max(self.end.x)
            .max(head_left.x)
            .max(head_right.x);
        let max_y = self
            .start
            .y
            .max(self.end.y)
            .max(head_left.y)
            .max(head_right.y);

        Rect::new(min_x, min_y, max_x, max_y)
    }

    fn hit_test(&self, point: Point, tolerance: f64) -> bool {
        // Check line segment
        let line_vec = Vec2::new(self.end.x - self.start.x, self.end.y - self.start.y);
        let point_vec = Vec2::new(point.x - self.start.x, point.y - self.start.y);

        let line_len_sq = line_vec.hypot2();
        if line_len_sq > f64::EPSILON {
            let t = (point_vec.dot(line_vec) / line_len_sq).clamp(0.0, 1.0);
            let projection =
                Point::new(self.start.x + t * line_vec.x, self.start.y + t * line_vec.y);
            let dist = ((point.x - projection.x).powi(2) + (point.y - projection.y).powi(2)).sqrt();
            if dist <= tolerance + self.style.stroke_width / 2.0 {
                return true;
            }
        }

        // Check arrowhead triangle
        let dir = self.direction();
        let perp = Vec2::new(-dir.y, dir.x);
        let head_back = Point::new(
            self.end.x - dir.x * self.head_size,
            self.end.y - dir.y * self.head_size,
        );
        let head_left = Point::new(
            head_back.x + perp.x * self.head_size * 0.5,
            head_back.y + perp.y * self.head_size * 0.5,
        );
        let head_right = Point::new(
            head_back.x - perp.x * self.head_size * 0.5,
            head_back.y - perp.y * self.head_size * 0.5,
        );

        // Point in triangle test
        fn sign(p1: Point, p2: Point, p3: Point) -> f64 {
            (p1.x - p3.x) * (p2.y - p3.y) - (p2.x - p3.x) * (p1.y - p3.y)
        }

        let d1 = sign(point, self.end, head_left);
        let d2 = sign(point, head_left, head_right);
        let d3 = sign(point, head_right, self.end);

        let has_neg = (d1 < 0.0) || (d2 < 0.0) || (d3 < 0.0);
        let has_pos = (d1 > 0.0) || (d2 > 0.0) || (d3 > 0.0);

        !(has_neg && has_pos)
    }

    fn to_path(&self) -> BezPath {
        let mut path = BezPath::new();

        // Shaft
        path.move_to(self.start);
        path.line_to(self.end);

        // Arrowhead
        let dir = self.direction();
        let perp = Vec2::new(-dir.y, dir.x);

        let head_back = Point::new(
            self.end.x - dir.x * self.head_size,
            self.end.y - dir.y * self.head_size,
        );
        let head_left = Point::new(
            head_back.x + perp.x * self.head_size * 0.5,
            head_back.y + perp.y * self.head_size * 0.5,
        );
        let head_right = Point::new(
            head_back.x - perp.x * self.head_size * 0.5,
            head_back.y - perp.y * self.head_size * 0.5,
        );

        path.move_to(self.end);
        path.line_to(head_left);
        path.move_to(self.end);
        path.line_to(head_right);

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
        // Scale head size based on transform
        let scale = affine.as_coeffs();
        self.head_size *= (scale[0].abs() + scale[3].abs()) / 2.0;
    }

    fn clone_box(&self) -> Box<dyn ShapeTrait + Send + Sync> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arrow_creation() {
        let arrow = Arrow::new(Point::new(0.0, 0.0), Point::new(100.0, 0.0));
        assert!((arrow.length() - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_direction() {
        let arrow = Arrow::new(Point::new(0.0, 0.0), Point::new(100.0, 0.0));
        let dir = arrow.direction();
        assert!((dir.x - 1.0).abs() < f64::EPSILON);
        assert!(dir.y.abs() < f64::EPSILON);
    }

    #[test]
    fn test_hit_test_shaft() {
        let arrow = Arrow::new(Point::new(0.0, 0.0), Point::new(100.0, 0.0));
        assert!(arrow.hit_test(Point::new(50.0, 0.0), 5.0));
    }

    #[test]
    fn test_hit_test_head() {
        let arrow = Arrow::new(Point::new(0.0, 0.0), Point::new(100.0, 0.0));
        assert!(arrow.hit_test(Point::new(100.0, 0.0), 1.0));
    }
}
