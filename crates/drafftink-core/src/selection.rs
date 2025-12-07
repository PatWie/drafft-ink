//! Selection and manipulation handle system.

use crate::shapes::{Shape, ShapeId, ShapeTrait};
use kurbo::{Point, Rect};
use serde::{Deserialize, Serialize};

/// Handle size in screen pixels.
pub const HANDLE_SIZE: f64 = 8.0;
/// Handle hit tolerance in screen pixels.
pub const HANDLE_HIT_TOLERANCE: f64 = 12.0;

/// Type of selection handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HandleKind {
    /// Endpoint handle for lines/arrows (index 0 = start, 1 = end).
    Endpoint(usize),
    /// Corner handle for rectangles/ellipses.
    Corner(Corner),
    /// Edge midpoint handle for rectangles/ellipses (for resizing).
    Edge(Edge),
}

/// Corner positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// Edge positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Edge {
    Top,
    Right,
    Bottom,
    Left,
}

/// A selection handle with its position and type.
#[derive(Debug, Clone, Copy)]
pub struct Handle {
    /// Position in world coordinates.
    pub position: Point,
    /// Handle type.
    pub kind: HandleKind,
}

impl Handle {
    /// Create a new handle.
    pub fn new(position: Point, kind: HandleKind) -> Self {
        Self { position, kind }
    }

    /// Check if a point (in world coordinates) hits this handle.
    /// `tolerance` should be adjusted for camera zoom.
    pub fn hit_test(&self, point: Point, tolerance: f64) -> bool {
        let dx = point.x - self.position.x;
        let dy = point.y - self.position.y;
        let dist_sq = dx * dx + dy * dy;
        dist_sq <= tolerance * tolerance
    }
}

/// Get the selection handles for a shape.
pub fn get_handles(shape: &Shape) -> Vec<Handle> {
    match shape {
        Shape::Line(line) => vec![
            Handle::new(line.start, HandleKind::Endpoint(0)),
            Handle::new(line.end, HandleKind::Endpoint(1)),
        ],
        Shape::Arrow(arrow) => vec![
            Handle::new(arrow.start, HandleKind::Endpoint(0)),
            Handle::new(arrow.end, HandleKind::Endpoint(1)),
        ],
        Shape::Rectangle(_) | Shape::Ellipse(_) => {
            let bounds = shape.bounds();
            corner_handles(bounds)
        }
        Shape::Freehand(_) => {
            // Freehand uses bounding box corners
            let bounds = shape.bounds();
            corner_handles(bounds)
        }
        Shape::Text(_) => {
            // Text uses bounding box corners
            let bounds = shape.bounds();
            corner_handles(bounds)
        }
        Shape::Group(_) => {
            // Groups use bounding box corners (resize not supported, only move)
            let bounds = shape.bounds();
            corner_handles(bounds)
        }
        Shape::Image(_) => {
            // Images use bounding box corners for resizing
            let bounds = shape.bounds();
            corner_handles(bounds)
        }
    }
}

/// Generate corner handles for a bounding rectangle.
fn corner_handles(bounds: Rect) -> Vec<Handle> {
    vec![
        Handle::new(
            Point::new(bounds.x0, bounds.y0),
            HandleKind::Corner(Corner::TopLeft),
        ),
        Handle::new(
            Point::new(bounds.x1, bounds.y0),
            HandleKind::Corner(Corner::TopRight),
        ),
        Handle::new(
            Point::new(bounds.x0, bounds.y1),
            HandleKind::Corner(Corner::BottomLeft),
        ),
        Handle::new(
            Point::new(bounds.x1, bounds.y1),
            HandleKind::Corner(Corner::BottomRight),
        ),
    ]
}

/// Find which handle (if any) is hit at the given point.
/// Returns the handle kind if hit.
pub fn hit_test_handles(shape: &Shape, point: Point, tolerance: f64) -> Option<HandleKind> {
    let handles = get_handles(shape);
    for handle in handles {
        if handle.hit_test(point, tolerance) {
            return Some(handle.kind);
        }
    }
    None
}

/// State of an active manipulation operation (single shape with handle).
#[derive(Debug, Clone)]
pub struct ManipulationState {
    /// The shape being manipulated.
    pub shape_id: ShapeId,
    /// The handle being dragged (None = moving the whole shape).
    pub handle: Option<HandleKind>,
    /// Starting point of the drag.
    pub start_point: Point,
    /// Current point of the drag.
    pub current_point: Point,
    /// Original shape state for preview/undo.
    pub original_shape: Shape,
}

/// State for moving multiple shapes at once.
#[derive(Debug, Clone)]
pub struct MultiMoveState {
    /// Starting point of the drag.
    pub start_point: Point,
    /// Current point of the drag.
    pub current_point: Point,
    /// Original shapes state for preview/undo (shape_id -> original shape).
    pub original_shapes: std::collections::HashMap<ShapeId, Shape>,
    /// Whether this is an alt-drag duplicate operation.
    pub is_duplicate: bool,
    /// IDs of duplicated shapes (only set if is_duplicate is true).
    pub duplicated_ids: Vec<ShapeId>,
}

impl ManipulationState {
    /// Create a new manipulation state.
    pub fn new(
        shape_id: ShapeId,
        handle: Option<HandleKind>,
        start_point: Point,
        original_shape: Shape,
    ) -> Self {
        Self {
            shape_id,
            handle,
            start_point,
            current_point: start_point,
            original_shape,
        }
    }

    /// Get the drag delta.
    pub fn delta(&self) -> kurbo::Vec2 {
        kurbo::Vec2::new(
            self.current_point.x - self.start_point.x,
            self.current_point.y - self.start_point.y,
        )
    }
}

impl MultiMoveState {
    /// Create a new multi-move state.
    pub fn new(
        start_point: Point,
        original_shapes: std::collections::HashMap<ShapeId, Shape>,
    ) -> Self {
        Self {
            start_point,
            current_point: start_point,
            original_shapes,
            is_duplicate: false,
            duplicated_ids: Vec::new(),
        }
    }

    /// Create a new multi-move state for duplication (Alt+drag).
    pub fn new_duplicate(
        start_point: Point,
        original_shapes: std::collections::HashMap<ShapeId, Shape>,
    ) -> Self {
        Self {
            start_point,
            current_point: start_point,
            original_shapes,
            is_duplicate: true,
            duplicated_ids: Vec::new(),
        }
    }

    /// Get the drag delta.
    pub fn delta(&self) -> kurbo::Vec2 {
        kurbo::Vec2::new(
            self.current_point.x - self.start_point.x,
            self.current_point.y - self.start_point.y,
        )
    }

    /// Get the shape IDs being moved.
    pub fn shape_ids(&self) -> Vec<ShapeId> {
        self.original_shapes.keys().copied().collect()
    }
}

/// Get the position of the manipulation target (handle or shape center).
/// This is used for snapping - we snap the target position, not the cursor.
pub fn get_manipulation_target_position(shape: &Shape, handle: Option<HandleKind>) -> Point {
    match handle {
        None => {
            // Moving entire shape - use top-left of bounding box as reference
            let bounds = shape.bounds();
            Point::new(bounds.x0, bounds.y0)
        }
        Some(HandleKind::Endpoint(idx)) => match shape {
            Shape::Line(line) => {
                if idx == 0 {
                    line.start
                } else {
                    line.end
                }
            }
            Shape::Arrow(arrow) => {
                if idx == 0 {
                    arrow.start
                } else {
                    arrow.end
                }
            }
            _ => shape.bounds().center(),
        },
        Some(HandleKind::Corner(corner)) => {
            let bounds = shape.bounds();
            match corner {
                Corner::TopLeft => Point::new(bounds.x0, bounds.y0),
                Corner::TopRight => Point::new(bounds.x1, bounds.y0),
                Corner::BottomLeft => Point::new(bounds.x0, bounds.y1),
                Corner::BottomRight => Point::new(bounds.x1, bounds.y1),
            }
        }
        Some(HandleKind::Edge(edge)) => {
            let bounds = shape.bounds();
            match edge {
                Edge::Top => Point::new(bounds.center().x, bounds.y0),
                Edge::Right => Point::new(bounds.x1, bounds.center().y),
                Edge::Bottom => Point::new(bounds.center().x, bounds.y1),
                Edge::Left => Point::new(bounds.x0, bounds.center().y),
            }
        }
    }
}

/// Apply a handle manipulation to a shape.
/// Returns the modified shape.
pub fn apply_manipulation(shape: &Shape, handle: Option<HandleKind>, delta: kurbo::Vec2) -> Shape {
    let mut shape = shape.clone();

    match handle {
        None => {
            // Move the entire shape
            let translation = kurbo::Affine::translate(delta);
            shape.transform(translation);
        }
        Some(HandleKind::Endpoint(idx)) => {
            // Move an endpoint (for lines/arrows)
            match &mut shape {
                Shape::Line(line) => {
                    if idx == 0 {
                        line.start.x += delta.x;
                        line.start.y += delta.y;
                    } else {
                        line.end.x += delta.x;
                        line.end.y += delta.y;
                    }
                }
                Shape::Arrow(arrow) => {
                    if idx == 0 {
                        arrow.start.x += delta.x;
                        arrow.start.y += delta.y;
                    } else {
                        arrow.end.x += delta.x;
                        arrow.end.y += delta.y;
                    }
                }
                _ => {}
            }
        }
        Some(HandleKind::Corner(corner)) => {
            // Resize from a corner
            match &mut shape {
                Shape::Rectangle(rect) => {
                    apply_corner_resize_rect(rect, corner, delta);
                }
                Shape::Ellipse(ellipse) => {
                    apply_corner_resize_ellipse(ellipse, corner, delta);
                }
                _ => {}
            }
        }
        Some(HandleKind::Edge(_)) => {
            // Edge resize not implemented yet
        }
    }

    shape
}

/// Apply corner resize to a rectangle.
fn apply_corner_resize_rect(
    rect: &mut crate::shapes::Rectangle,
    corner: Corner,
    delta: kurbo::Vec2,
) {
    let bounds = rect.bounds();
    let (new_x0, new_y0, new_x1, new_y1) = match corner {
        Corner::TopLeft => (
            bounds.x0 + delta.x,
            bounds.y0 + delta.y,
            bounds.x1,
            bounds.y1,
        ),
        Corner::TopRight => (
            bounds.x0,
            bounds.y0 + delta.y,
            bounds.x1 + delta.x,
            bounds.y1,
        ),
        Corner::BottomLeft => (
            bounds.x0 + delta.x,
            bounds.y0,
            bounds.x1,
            bounds.y1 + delta.y,
        ),
        Corner::BottomRight => (
            bounds.x0,
            bounds.y0,
            bounds.x1 + delta.x,
            bounds.y1 + delta.y,
        ),
    };

    // Ensure min size and handle flipping
    let (x0, x1) = if new_x0 < new_x1 {
        (new_x0, new_x1)
    } else {
        (new_x1, new_x0)
    };
    let (y0, y1) = if new_y0 < new_y1 {
        (new_y0, new_y1)
    } else {
        (new_y1, new_y0)
    };

    rect.position = Point::new(x0, y0);
    rect.width = (x1 - x0).max(1.0);
    rect.height = (y1 - y0).max(1.0);
}

/// Apply corner resize to an ellipse.
fn apply_corner_resize_ellipse(
    ellipse: &mut crate::shapes::Ellipse,
    corner: Corner,
    delta: kurbo::Vec2,
) {
    let bounds = ellipse.bounds();
    let (new_x0, new_y0, new_x1, new_y1) = match corner {
        Corner::TopLeft => (
            bounds.x0 + delta.x,
            bounds.y0 + delta.y,
            bounds.x1,
            bounds.y1,
        ),
        Corner::TopRight => (
            bounds.x0,
            bounds.y0 + delta.y,
            bounds.x1 + delta.x,
            bounds.y1,
        ),
        Corner::BottomLeft => (
            bounds.x0 + delta.x,
            bounds.y0,
            bounds.x1,
            bounds.y1 + delta.y,
        ),
        Corner::BottomRight => (
            bounds.x0,
            bounds.y0,
            bounds.x1 + delta.x,
            bounds.y1 + delta.y,
        ),
    };

    // Ensure min size and handle flipping
    let (x0, x1) = if new_x0 < new_x1 {
        (new_x0, new_x1)
    } else {
        (new_x1, new_x0)
    };
    let (y0, y1) = if new_y0 < new_y1 {
        (new_y0, new_y1)
    } else {
        (new_y1, new_y0)
    };

    let width = (x1 - x0).max(1.0);
    let height = (y1 - y0).max(1.0);

    ellipse.center = Point::new(x0 + width / 2.0, y0 + height / 2.0);
    ellipse.radius_x = width / 2.0;
    ellipse.radius_y = height / 2.0;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shapes::{Line, Rectangle};

    #[test]
    fn test_line_handles() {
        let line = Line::new(Point::new(0.0, 0.0), Point::new(100.0, 100.0));
        let handles = get_handles(&Shape::Line(line));

        assert_eq!(handles.len(), 2);
        assert!(matches!(handles[0].kind, HandleKind::Endpoint(0)));
        assert!(matches!(handles[1].kind, HandleKind::Endpoint(1)));
    }

    #[test]
    fn test_rectangle_handles() {
        let rect = Rectangle::new(Point::new(0.0, 0.0), 100.0, 50.0);
        let handles = get_handles(&Shape::Rectangle(rect));

        assert_eq!(handles.len(), 4);
        assert!(matches!(
            handles[0].kind,
            HandleKind::Corner(Corner::TopLeft)
        ));
    }

    #[test]
    fn test_handle_hit_test() {
        let handle = Handle::new(Point::new(50.0, 50.0), HandleKind::Endpoint(0));

        assert!(handle.hit_test(Point::new(50.0, 50.0), 10.0));
        assert!(handle.hit_test(Point::new(55.0, 55.0), 10.0));
        assert!(!handle.hit_test(Point::new(70.0, 70.0), 10.0));
    }

    #[test]
    fn test_apply_endpoint_manipulation() {
        let line = Line::new(Point::new(0.0, 0.0), Point::new(100.0, 100.0));
        let shape = Shape::Line(line);

        let result = apply_manipulation(
            &shape,
            Some(HandleKind::Endpoint(1)),
            kurbo::Vec2::new(10.0, 20.0),
        );

        if let Shape::Line(line) = result {
            assert!((line.end.x - 110.0).abs() < f64::EPSILON);
            assert!((line.end.y - 120.0).abs() < f64::EPSILON);
        } else {
            panic!("Expected Line shape");
        }
    }

    #[test]
    fn test_apply_corner_manipulation() {
        let rect = Rectangle::new(Point::new(0.0, 0.0), 100.0, 100.0);
        let shape = Shape::Rectangle(rect);

        let result = apply_manipulation(
            &shape,
            Some(HandleKind::Corner(Corner::BottomRight)),
            kurbo::Vec2::new(50.0, 50.0),
        );

        if let Shape::Rectangle(rect) = result {
            assert!((rect.width - 150.0).abs() < f64::EPSILON);
            assert!((rect.height - 150.0).abs() < f64::EPSILON);
        } else {
            panic!("Expected Rectangle shape");
        }
    }
}
