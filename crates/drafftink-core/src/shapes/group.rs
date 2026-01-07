//! Group shape for combining multiple shapes.

use super::{Shape, ShapeId, ShapeStyle, ShapeTrait};
use kurbo::{Affine, BezPath, Point, Rect};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A group of shapes that can be manipulated as a single unit.
/// Groups can contain other groups, enabling nested hierarchies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub(crate) id: ShapeId,
    /// Child shapes in this group.
    pub children: Vec<Shape>,
    /// Style properties (not directly used, but kept for consistency).
    style: ShapeStyle,
}

impl Group {
    /// Create a new group from a list of shapes.
    pub fn new(children: Vec<Shape>) -> Self {
        Self {
            id: Uuid::new_v4(),
            children,
            style: ShapeStyle::default(),
        }
    }

    /// Reconstruct a group with a specific ID (for CRDT/storage).
    pub(crate) fn reconstruct(id: ShapeId, children: Vec<Shape>) -> Self {
        Self {
            id,
            children,
            style: ShapeStyle::default(),
        }
    }

    /// Get the children of this group.
    pub fn children(&self) -> &[Shape] {
        &self.children
    }

    /// Get mutable access to children.
    pub fn children_mut(&mut self) -> &mut Vec<Shape> {
        &mut self.children
    }

    /// Dissolve this group and return its children.
    pub fn ungroup(self) -> Vec<Shape> {
        self.children
    }

    /// Get all shape IDs in this group (including nested groups).
    pub fn all_shape_ids(&self) -> Vec<ShapeId> {
        let mut ids = vec![self.id];
        for child in &self.children {
            if let Shape::Group(group) = child {
                ids.extend(group.all_shape_ids());
            } else {
                ids.push(child.id());
            }
        }
        ids
    }

    /// Find a shape by ID within this group (including nested groups).
    pub fn find_shape(&self, id: ShapeId) -> Option<&Shape> {
        for child in &self.children {
            if child.id() == id {
                return Some(child);
            }
            if let Shape::Group(group) = child {
                if let Some(found) = group.find_shape(id) {
                    return Some(found);
                }
            }
        }
        None
    }

    /// Find a mutable shape by ID within this group (including nested groups).
    pub fn find_shape_mut(&mut self, id: ShapeId) -> Option<&mut Shape> {
        for child in &mut self.children {
            if child.id() == id {
                return Some(child);
            }
            if let Shape::Group(group) = child {
                if let Some(found) = group.find_shape_mut(id) {
                    return Some(found);
                }
            }
        }
        None
    }
}

impl ShapeTrait for Group {
    fn id(&self) -> ShapeId {
        self.id
    }

    fn bounds(&self) -> Rect {
        if self.children.is_empty() {
            return Rect::ZERO;
        }

        let mut bounds = self.children[0].bounds();
        for child in &self.children[1..] {
            bounds = bounds.union(child.bounds());
        }
        bounds
    }

    fn hit_test(&self, point: Point, tolerance: f64) -> bool {
        // Hit if any child is hit
        for child in &self.children {
            if child.hit_test(point, tolerance) {
                return true;
            }
        }
        false
    }

    fn to_path(&self) -> BezPath {
        // Combine all children's paths
        let mut path = BezPath::new();
        for child in &self.children {
            path.extend(child.to_path());
        }
        path
    }

    fn style(&self) -> &ShapeStyle {
        // Return the group's style (not very meaningful for groups)
        &self.style
    }

    fn style_mut(&mut self) -> &mut ShapeStyle {
        &mut self.style
    }

    fn transform(&mut self, affine: Affine) {
        // Transform all children
        for child in &mut self.children {
            child.transform(affine);
        }
    }

    fn clone_box(&self) -> Box<dyn ShapeTrait + Send + Sync> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shapes::Rectangle;

    #[test]
    fn test_group_creation() {
        let rect1 = Rectangle::new(Point::new(0.0, 0.0), 100.0, 50.0);
        let rect2 = Rectangle::new(Point::new(200.0, 200.0), 50.0, 100.0);

        let group = Group::new(vec![Shape::Rectangle(rect1), Shape::Rectangle(rect2)]);

        assert_eq!(group.children().len(), 2);
    }

    #[test]
    fn test_group_bounds() {
        let rect1 = Rectangle::new(Point::new(0.0, 0.0), 100.0, 50.0);
        let rect2 = Rectangle::new(Point::new(200.0, 200.0), 50.0, 100.0);

        let group = Group::new(vec![Shape::Rectangle(rect1), Shape::Rectangle(rect2)]);
        let bounds = group.bounds();

        assert!((bounds.x0 - 0.0).abs() < f64::EPSILON);
        assert!((bounds.y0 - 0.0).abs() < f64::EPSILON);
        assert!((bounds.x1 - 250.0).abs() < f64::EPSILON);
        assert!((bounds.y1 - 300.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_group_hit_test() {
        let rect1 = Rectangle::new(Point::new(0.0, 0.0), 100.0, 50.0);
        let rect2 = Rectangle::new(Point::new(200.0, 200.0), 50.0, 100.0);

        let group = Group::new(vec![Shape::Rectangle(rect1), Shape::Rectangle(rect2)]);

        // Hit test on first child
        assert!(group.hit_test(Point::new(50.0, 25.0), 0.0));
        // Hit test on second child
        assert!(group.hit_test(Point::new(225.0, 250.0), 0.0));
        // Hit test in empty space between children
        assert!(!group.hit_test(Point::new(150.0, 100.0), 0.0));
    }

    #[test]
    fn test_nested_groups() {
        let rect1 = Rectangle::new(Point::new(0.0, 0.0), 100.0, 50.0);
        let rect2 = Rectangle::new(Point::new(200.0, 200.0), 50.0, 100.0);

        let inner_group = Group::new(vec![Shape::Rectangle(rect1)]);
        let outer_group = Group::new(vec![Shape::Group(inner_group), Shape::Rectangle(rect2)]);

        // Should be able to hit test through nested groups
        assert!(outer_group.hit_test(Point::new(50.0, 25.0), 0.0));
    }

    #[test]
    fn test_ungroup() {
        let rect1 = Rectangle::new(Point::new(0.0, 0.0), 100.0, 50.0);
        let rect2 = Rectangle::new(Point::new(200.0, 200.0), 50.0, 100.0);

        let group = Group::new(vec![Shape::Rectangle(rect1), Shape::Rectangle(rect2)]);
        let children = group.ungroup();

        assert_eq!(children.len(), 2);
    }
}
