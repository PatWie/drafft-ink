//! Rectangle shape.

use super::{ShapeId, ShapeStyle, ShapeTrait};
use kurbo::{Affine, BezPath, Point, Rect, RoundedRect, Shape as KurboShape};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A rectangle shape with optional rounded corners.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rectangle {
    pub(crate) id: ShapeId,
    /// Top-left corner position.
    pub position: Point,
    /// Width of the rectangle.
    pub width: f64,
    /// Height of the rectangle.
    pub height: f64,
    /// Corner radius (0 = sharp corners).
    pub corner_radius: f64,
    /// Rotation angle in radians (around center).
    #[serde(default)]
    pub rotation: f64,
    /// Style properties.
    pub style: ShapeStyle,
}

impl Rectangle {
    /// Default adaptive corner radius in pixels.
    /// This fixed radius keeps visual appearance consistent across different element sizes.
    pub const DEFAULT_ADAPTIVE_RADIUS: f64 = 32.0;

    /// Default proportional radius (25% of largest side).
    /// Used for legacy elements, linear elements, and diamonds.
    pub const DEFAULT_PROPORTIONAL_RADIUS: f64 = 0.25;

    /// Create a new rectangle.
    pub fn new(position: Point, width: f64, height: f64) -> Self {
        Self {
            id: Uuid::new_v4(),
            position,
            width,
            height,
            corner_radius: 0.0,
            rotation: 0.0,
            style: ShapeStyle::default(),
        }
    }

    /// Reconstruct a rectangle with a specific ID (for CRDT/storage).
    pub(crate) fn reconstruct(
        id: ShapeId,
        position: Point,
        width: f64,
        height: f64,
        corner_radius: f64,
        rotation: f64,
        style: ShapeStyle,
    ) -> Self {
        Self {
            id,
            position,
            width,
            height,
            corner_radius,
            rotation,
            style,
        }
    }

    /// Create a rectangle from two corner points.
    pub fn from_corners(p1: Point, p2: Point) -> Self {
        let min_x = p1.x.min(p2.x);
        let min_y = p1.y.min(p2.y);
        let width = (p2.x - p1.x).abs();
        let height = (p2.y - p1.y).abs();

        Self::new(Point::new(min_x, min_y), width, height)
    }

    /// Get the rectangle as a kurbo Rect.
    pub fn as_rect(&self) -> Rect {
        Rect::new(
            self.position.x,
            self.position.y,
            self.position.x + self.width,
            self.position.y + self.height,
        )
    }
}

impl ShapeTrait for Rectangle {
    fn id(&self) -> ShapeId {
        self.id
    }

    fn bounds(&self) -> Rect {
        self.as_rect()
    }

    fn hit_test(&self, point: Point, tolerance: f64) -> bool {
        let rect = self.as_rect().inflate(tolerance, tolerance);
        rect.contains(point)
    }

    fn to_path(&self) -> BezPath {
        if self.corner_radius > 0.0 {
            let rounded = RoundedRect::from_rect(self.as_rect(), self.corner_radius);
            rounded.to_path(0.1)
        } else {
            self.as_rect().to_path(0.1)
        }
    }

    fn style(&self) -> &ShapeStyle {
        &self.style
    }

    fn style_mut(&mut self) -> &mut ShapeStyle {
        &mut self.style
    }

    fn transform(&mut self, affine: Affine) {
        self.position = affine * self.position;
        // Note: This is a simplified transform that doesn't handle rotation/skew
        let scale = affine.as_coeffs();
        self.width *= scale[0].abs();
        self.height *= scale[3].abs();
    }

    fn clone_box(&self) -> Box<dyn ShapeTrait + Send + Sync> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rectangle_creation() {
        let rect = Rectangle::new(Point::new(10.0, 20.0), 100.0, 50.0);
        assert!((rect.position.x - 10.0).abs() < f64::EPSILON);
        assert!((rect.position.y - 20.0).abs() < f64::EPSILON);
        assert!((rect.width - 100.0).abs() < f64::EPSILON);
        assert!((rect.height - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_rectangle_from_corners() {
        let rect = Rectangle::from_corners(Point::new(100.0, 100.0), Point::new(50.0, 50.0));
        assert!((rect.position.x - 50.0).abs() < f64::EPSILON);
        assert!((rect.position.y - 50.0).abs() < f64::EPSILON);
        assert!((rect.width - 50.0).abs() < f64::EPSILON);
        assert!((rect.height - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_hit_test() {
        let rect = Rectangle::new(Point::new(0.0, 0.0), 100.0, 100.0);
        assert!(rect.hit_test(Point::new(50.0, 50.0), 0.0));
        assert!(!rect.hit_test(Point::new(150.0, 50.0), 0.0));
        assert!(rect.hit_test(Point::new(105.0, 50.0), 10.0)); // Within tolerance
    }

    #[test]
    fn test_bounds() {
        let rect = Rectangle::new(Point::new(10.0, 20.0), 100.0, 50.0);
        let bounds = rect.bounds();
        assert!((bounds.x0 - 10.0).abs() < f64::EPSILON);
        assert!((bounds.y0 - 20.0).abs() < f64::EPSILON);
        assert!((bounds.x1 - 110.0).abs() < f64::EPSILON);
        assert!((bounds.y1 - 70.0).abs() < f64::EPSILON);
    }
}
