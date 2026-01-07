//! Ellipse shape.

use super::{ShapeId, ShapeStyle, ShapeTrait};
use kurbo::{Affine, BezPath, Ellipse as KurboEllipse, Point, Rect, Shape as KurboShape};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// An ellipse shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ellipse {
    pub(crate) id: ShapeId,
    /// Center point.
    pub center: Point,
    /// Horizontal radius.
    pub radius_x: f64,
    /// Vertical radius.
    pub radius_y: f64,
    /// Rotation angle in radians (around center).
    #[serde(default)]
    pub rotation: f64,
    /// Style properties.
    pub style: ShapeStyle,
}

impl Ellipse {
    /// Create a new ellipse.
    pub fn new(center: Point, radius_x: f64, radius_y: f64) -> Self {
        Self {
            id: Uuid::new_v4(),
            center,
            radius_x,
            radius_y,
            rotation: 0.0,
            style: ShapeStyle::default(),
        }
    }

    /// Reconstruct an ellipse with a specific ID (for CRDT/storage).
    pub(crate) fn reconstruct(
        id: ShapeId,
        center: Point,
        radius_x: f64,
        radius_y: f64,
        rotation: f64,
        style: ShapeStyle,
    ) -> Self {
        Self {
            id,
            center,
            radius_x,
            radius_y,
            rotation,
            style,
        }
    }

    /// Create a circle.
    pub fn circle(center: Point, radius: f64) -> Self {
        Self::new(center, radius, radius)
    }

    /// Create an ellipse from a bounding rectangle.
    pub fn from_rect(rect: Rect) -> Self {
        Self::new(rect.center(), rect.width() / 2.0, rect.height() / 2.0)
    }

    /// Get as a kurbo Ellipse.
    pub fn as_kurbo(&self) -> KurboEllipse {
        KurboEllipse::new(self.center, (self.radius_x, self.radius_y), 0.0)
    }
}

impl ShapeTrait for Ellipse {
    fn id(&self) -> ShapeId {
        self.id
    }

    fn bounds(&self) -> Rect {
        Rect::new(
            self.center.x - self.radius_x,
            self.center.y - self.radius_y,
            self.center.x + self.radius_x,
            self.center.y + self.radius_y,
        )
    }

    fn hit_test(&self, point: Point, tolerance: f64) -> bool {
        let dx = (point.x - self.center.x) / (self.radius_x + tolerance);
        let dy = (point.y - self.center.y) / (self.radius_y + tolerance);
        dx * dx + dy * dy <= 1.0
    }

    fn to_path(&self) -> BezPath {
        self.as_kurbo().to_path(0.1)
    }

    fn style(&self) -> &ShapeStyle {
        &self.style
    }

    fn style_mut(&mut self) -> &mut ShapeStyle {
        &mut self.style
    }

    fn transform(&mut self, affine: Affine) {
        self.center = affine * self.center;
        let scale = affine.as_coeffs();
        self.radius_x *= scale[0].abs();
        self.radius_y *= scale[3].abs();
    }

    fn clone_box(&self) -> Box<dyn ShapeTrait + Send + Sync> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ellipse_creation() {
        let ellipse = Ellipse::new(Point::new(50.0, 50.0), 30.0, 20.0);
        assert!((ellipse.center.x - 50.0).abs() < f64::EPSILON);
        assert!((ellipse.radius_x - 30.0).abs() < f64::EPSILON);
        assert!((ellipse.radius_y - 20.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_circle() {
        let circle = Ellipse::circle(Point::new(0.0, 0.0), 10.0);
        assert!((circle.radius_x - circle.radius_y).abs() < f64::EPSILON);
    }

    #[test]
    fn test_hit_test_center() {
        let ellipse = Ellipse::new(Point::new(50.0, 50.0), 30.0, 20.0);
        assert!(ellipse.hit_test(Point::new(50.0, 50.0), 0.0));
    }

    #[test]
    fn test_hit_test_edge() {
        let circle = Ellipse::circle(Point::new(0.0, 0.0), 10.0);
        assert!(circle.hit_test(Point::new(10.0, 0.0), 0.0));
        assert!(!circle.hit_test(Point::new(15.0, 0.0), 0.0));
    }

    #[test]
    fn test_bounds() {
        let ellipse = Ellipse::new(Point::new(50.0, 50.0), 30.0, 20.0);
        let bounds = ellipse.bounds();
        assert!((bounds.x0 - 20.0).abs() < f64::EPSILON);
        assert!((bounds.y0 - 30.0).abs() < f64::EPSILON);
        assert!((bounds.x1 - 80.0).abs() < f64::EPSILON);
        assert!((bounds.y1 - 70.0).abs() < f64::EPSILON);
    }
}
