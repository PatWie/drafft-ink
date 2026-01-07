//! CRDT integration using Loro for collaborative editing.
//!
//! This module provides the bridge between DrafftInk's shape model and Loro's CRDT document.
//!
//! # Schema
//!
//! The Loro document has the following structure:
//! ```text
//! LoroDoc
//! ├── "name": LoroText (document name)
//! ├── "shapes": LoroMap<ShapeId, LoroMap> (shape data)
//! └── "z_order": LoroList<String> (shape IDs in z-order)
//! ```
//!
//! Each shape in "shapes" is a LoroMap with:
//! - "type": String ("rectangle", "ellipse", "line", "arrow", "freehand", "text")
//! - "id": String (UUID)
//! - Type-specific fields (position, dimensions, points, etc.)
//! - Style fields (stroke_color, stroke_width, fill_color, sloppiness)

mod convert;
mod schema;

pub use convert::{shape_from_loro, shape_to_loro};
pub use schema::{CrdtDocument, NAME_KEY, SHAPES_KEY, Z_ORDER_KEY};

// Re-export Loro types that may be useful for collaboration
pub use loro::{ExportMode, VersionVector};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shapes::{FillPattern, Rectangle, SerializableColor, Shape, ShapeStyle, Sloppiness};
    use kurbo::Point;

    #[test]
    fn test_crdt_document_creation() {
        let doc = CrdtDocument::new();
        assert!(doc.shape_count() == 0);
    }

    #[test]
    fn test_add_shape_to_crdt() {
        let mut doc = CrdtDocument::new();
        let rect = Rectangle::new(Point::new(100.0, 200.0), 50.0, 30.0);
        let shape = Shape::Rectangle(rect);
        let id = shape.id();

        doc.add_shape(&shape).expect("Failed to add shape");

        assert_eq!(doc.shape_count(), 1);
        assert!(doc.z_order().contains(&id.to_string()));
    }

    #[test]
    fn test_roundtrip_rectangle() {
        let mut doc = CrdtDocument::new();

        let mut rect = Rectangle::new(Point::new(100.0, 200.0), 150.0, 80.0);
        rect.corner_radius = 16.0;
        rect.style = ShapeStyle {
            stroke_color: SerializableColor::new(255, 0, 0, 255),
            stroke_width: 3.0,
            fill_color: Some(SerializableColor::new(0, 255, 0, 128)),
            fill_pattern: FillPattern::default(),
            sloppiness: Sloppiness::Artist,
            seed: 12345, // Fixed seed for testing
        };
        let original = Shape::Rectangle(rect);
        let id = original.id();

        doc.add_shape(&original).expect("Failed to add shape");

        let recovered = doc.get_shape(&id.to_string()).expect("Shape not found");

        // Verify the shape was recovered correctly
        match recovered {
            Shape::Rectangle(r) => {
                assert!((r.position.x - 100.0).abs() < 0.001);
                assert!((r.position.y - 200.0).abs() < 0.001);
                assert!((r.width - 150.0).abs() < 0.001);
                assert!((r.height - 80.0).abs() < 0.001);
                assert!((r.corner_radius - 16.0).abs() < 0.001);
                assert_eq!(r.style.stroke_color.r, 255);
                assert_eq!(r.style.stroke_color.g, 0);
                assert_eq!(r.style.stroke_width as i32, 3);
                assert!(r.style.fill_color.is_some());
                assert_eq!(r.style.sloppiness, Sloppiness::Artist);
            }
            _ => panic!("Expected Rectangle, got different shape type"),
        }
    }

    #[test]
    fn test_remove_shape() {
        let mut doc = CrdtDocument::new();
        let rect = Rectangle::new(Point::new(0.0, 0.0), 100.0, 100.0);
        let shape = Shape::Rectangle(rect);
        let id = shape.id().to_string();

        doc.add_shape(&shape).expect("Failed to add shape");
        assert_eq!(doc.shape_count(), 1);

        doc.remove_shape(&id).expect("Failed to remove shape");
        assert_eq!(doc.shape_count(), 0);
        assert!(!doc.z_order().contains(&id));
    }

    #[test]
    fn test_z_order_manipulation() {
        let mut doc = CrdtDocument::new();

        let rect1 = Shape::Rectangle(Rectangle::new(Point::new(0.0, 0.0), 50.0, 50.0));
        let rect2 = Shape::Rectangle(Rectangle::new(Point::new(25.0, 25.0), 50.0, 50.0));
        let id1 = rect1.id().to_string();
        let id2 = rect2.id().to_string();

        doc.add_shape(&rect1).expect("Failed to add shape 1");
        doc.add_shape(&rect2).expect("Failed to add shape 2");

        let order = doc.z_order();
        assert_eq!(order.len(), 2);
        assert_eq!(order[0], id1);
        assert_eq!(order[1], id2);

        // Bring first shape to front
        doc.bring_to_front(&id1).expect("Failed to bring to front");
        let order = doc.z_order();
        assert_eq!(order[0], id2);
        assert_eq!(order[1], id1);

        // Send it back
        doc.send_to_back(&id1).expect("Failed to send to back");
        let order = doc.z_order();
        assert_eq!(order[0], id1);
        assert_eq!(order[1], id2);
    }

    #[test]
    fn test_export_import() {
        let mut doc = CrdtDocument::new();
        let rect = Shape::Rectangle(Rectangle::new(Point::new(10.0, 20.0), 100.0, 50.0));
        doc.add_shape(&rect).expect("Failed to add shape");

        // Export to bytes
        let bytes = doc.export_snapshot();

        // Import into new document
        let doc2 = CrdtDocument::from_snapshot(&bytes).expect("Failed to import");

        assert_eq!(doc2.shape_count(), 1);
    }

    #[test]
    fn test_crdt_undo_add_shape() {
        let mut doc = CrdtDocument::new();

        // Add a shape
        let rect = Rectangle::new(Point::new(100.0, 200.0), 50.0, 30.0);
        let shape = Shape::Rectangle(rect);
        doc.add_shape(&shape).expect("Failed to add shape");

        assert_eq!(doc.shape_count(), 1);
        assert!(doc.can_undo());

        // Undo should remove the shape
        assert!(doc.undo());
        assert_eq!(doc.shape_count(), 0);
        assert!(doc.can_redo());

        // Redo should restore it
        assert!(doc.redo());
        assert_eq!(doc.shape_count(), 1);
    }

    #[test]
    fn test_crdt_undo_remove_shape() {
        let mut doc = CrdtDocument::new();

        // Add a shape
        let rect = Rectangle::new(Point::new(0.0, 0.0), 100.0, 100.0);
        let shape = Shape::Rectangle(rect);
        let id = shape.id().to_string();
        doc.add_shape(&shape).expect("Failed to add shape");

        // Clear undo history so we only track the remove
        doc.clear_undo_history();

        // Remove the shape
        doc.remove_shape(&id).expect("Failed to remove shape");
        assert_eq!(doc.shape_count(), 0);

        // Undo should restore it
        assert!(doc.undo());
        assert_eq!(doc.shape_count(), 1);
    }

    #[test]
    fn test_crdt_undo_count() {
        let mut doc = CrdtDocument::new();

        // Add multiple shapes
        for i in 0..5 {
            let rect = Rectangle::new(Point::new(i as f64 * 10.0, 0.0), 50.0, 50.0);
            doc.add_shape(&Shape::Rectangle(rect))
                .expect("Failed to add shape");
        }

        // Should have 5 undo steps (or possibly merged if within merge interval)
        assert!(doc.undo_count() > 0);
        assert_eq!(doc.redo_count(), 0);
    }
}
