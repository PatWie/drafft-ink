//! Loro document schema and operations.

use super::convert::{shape_from_loro, shape_to_loro};
use crate::shapes::Shape;
use loro::{
    ExportMode, LoroDoc, LoroList, LoroMap, LoroResult, LoroValue, UndoManager, ValueOrContainer,
};

/// Key for the shapes map in the document.
pub const SHAPES_KEY: &str = "shapes";
/// Key for the z-order list in the document.
pub const Z_ORDER_KEY: &str = "z_order";
/// Key for the document name.
pub const NAME_KEY: &str = "name";

/// A CRDT-backed document for collaborative editing.
///
/// This wraps a `LoroDoc` and provides a high-level API for working with shapes.
/// It also includes an `UndoManager` for local undo/redo operations.
pub struct CrdtDocument {
    /// The underlying Loro document.
    doc: LoroDoc,
    /// Undo manager for local undo/redo.
    undo_manager: UndoManager,
}

impl CrdtDocument {
    /// Create a new empty CRDT document.
    pub fn new() -> Self {
        let doc = LoroDoc::new();
        let mut undo_manager = UndoManager::new(&doc);
        // Set reasonable defaults for undo
        undo_manager.set_max_undo_steps(100);
        undo_manager.set_merge_interval(300); // Merge edits within 300ms
        Self { doc, undo_manager }
    }

    /// Create a CRDT document from a snapshot.
    pub fn from_snapshot(bytes: &[u8]) -> LoroResult<Self> {
        let doc = LoroDoc::new();
        doc.import(bytes)?;
        let mut undo_manager = UndoManager::new(&doc);
        undo_manager.set_max_undo_steps(100);
        undo_manager.set_merge_interval(300);
        Ok(Self { doc, undo_manager })
    }

    /// Get the underlying LoroDoc.
    pub fn loro_doc(&self) -> &LoroDoc {
        &self.doc
    }

    /// Get a mutable reference to the underlying LoroDoc.
    pub fn loro_doc_mut(&mut self) -> &mut LoroDoc {
        &mut self.doc
    }

    /// Get the shapes map.
    fn shapes_map(&self) -> LoroMap {
        self.doc.get_map(SHAPES_KEY)
    }

    /// Get the z-order list.
    fn z_order_list(&self) -> LoroList {
        self.doc.get_list(Z_ORDER_KEY)
    }

    /// Get the number of shapes in the document.
    pub fn shape_count(&self) -> usize {
        self.shapes_map().len()
    }

    /// Get the z-order as a list of shape IDs.
    pub fn z_order(&self) -> Vec<String> {
        let list = self.z_order_list();
        let mut result = Vec::with_capacity(list.len());
        for i in 0..list.len() {
            if let Some(ValueOrContainer::Value(LoroValue::String(id))) = list.get(i) {
                result.push(id.to_string());
            }
        }
        result
    }

    /// Add a shape to the document.
    pub fn add_shape(&mut self, shape: &Shape) -> LoroResult<()> {
        let id = shape.id().to_string();
        let shapes = self.shapes_map();
        let z_order = self.z_order_list();

        // Create a new map for the shape
        let shape_map = shapes.insert_container(&id, LoroMap::new())?;

        // Convert shape to Loro values
        shape_to_loro(shape, &shape_map)?;

        // Add to z-order
        z_order.push(LoroValue::String(id.into()))?;

        // Commit the transaction
        self.doc.commit();

        Ok(())
    }

    /// Remove a shape from the document.
    pub fn remove_shape(&mut self, id: &str) -> LoroResult<()> {
        let shapes = self.shapes_map();
        let z_order = self.z_order_list();

        // Remove from shapes map
        shapes.delete(id)?;

        // Remove from z-order
        for i in 0..z_order.len() {
            if let Some(ValueOrContainer::Value(LoroValue::String(s))) = z_order.get(i) {
                if s.as_ref() == id {
                    z_order.delete(i, 1)?;
                    break;
                }
            }
        }

        self.doc.commit();
        Ok(())
    }

    /// Get a shape by ID.
    pub fn get_shape(&self, id: &str) -> Option<Shape> {
        let shapes = self.shapes_map();

        // Get the deep value and extract the shape map
        let shapes_value = shapes.get_deep_value();
        if let LoroValue::Map(map) = shapes_value {
            if let Some(LoroValue::Map(shape_map)) = map.get(id) {
                return shape_from_loro(shape_map);
            }
        }

        None
    }

    /// Update a shape in the document.
    pub fn update_shape(&mut self, shape: &Shape) -> LoroResult<()> {
        let id = shape.id().to_string();
        let shapes = self.shapes_map();

        // Remove the old shape data and add new
        shapes.delete(&id)?;
        let shape_map = shapes.insert_container(&id, LoroMap::new())?;
        shape_to_loro(shape, &shape_map)?;

        self.doc.commit();
        Ok(())
    }

    /// Get all shapes in z-order.
    pub fn shapes_ordered(&self) -> Vec<Shape> {
        let z_order = self.z_order();
        let mut shapes = Vec::with_capacity(z_order.len());

        for id in z_order {
            if let Some(shape) = self.get_shape(&id) {
                shapes.push(shape);
            }
        }

        shapes
    }

    /// Bring a shape to the front (top of z-order).
    pub fn bring_to_front(&mut self, id: &str) -> LoroResult<()> {
        let z_order = self.z_order_list();

        // Find and remove from current position
        for i in 0..z_order.len() {
            if let Some(ValueOrContainer::Value(LoroValue::String(s))) = z_order.get(i) {
                if s.as_ref() == id {
                    z_order.delete(i, 1)?;
                    break;
                }
            }
        }

        // Add to end (front)
        z_order.push(LoroValue::String(id.to_string().into()))?;

        self.doc.commit();
        Ok(())
    }

    /// Send a shape to the back (bottom of z-order).
    pub fn send_to_back(&mut self, id: &str) -> LoroResult<()> {
        let z_order = self.z_order_list();

        // Find and remove from current position
        for i in 0..z_order.len() {
            if let Some(ValueOrContainer::Value(LoroValue::String(s))) = z_order.get(i) {
                if s.as_ref() == id {
                    z_order.delete(i, 1)?;
                    break;
                }
            }
        }

        // Insert at beginning (back)
        z_order.insert(0, LoroValue::String(id.to_string().into()))?;

        self.doc.commit();
        Ok(())
    }

    /// Move a shape one layer forward (towards front).
    /// Returns true if the shape was moved, false if already at front.
    pub fn bring_forward(&mut self, id: &str) -> LoroResult<bool> {
        let z_order = self.z_order_list();
        let len = z_order.len();

        // Find current position
        let mut pos = None;
        for i in 0..len {
            if let Some(ValueOrContainer::Value(LoroValue::String(s))) = z_order.get(i) {
                if s.as_ref() == id {
                    pos = Some(i);
                    break;
                }
            }
        }

        if let Some(i) = pos {
            if i < len - 1 {
                // Get the item at the next position
                let next_id = if let Some(ValueOrContainer::Value(LoroValue::String(s))) =
                    z_order.get(i + 1)
                {
                    s.to_string()
                } else {
                    return Ok(false);
                };

                // Swap by removing and reinserting
                z_order.delete(i, 2)?;
                z_order.insert(i, LoroValue::String(id.to_string().into()))?;
                z_order.insert(i, LoroValue::String(next_id.into()))?;

                self.doc.commit();
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Move a shape one layer backward (towards back).
    /// Returns true if the shape was moved, false if already at back.
    pub fn send_backward(&mut self, id: &str) -> LoroResult<bool> {
        let z_order = self.z_order_list();

        // Find current position
        let mut pos = None;
        for i in 0..z_order.len() {
            if let Some(ValueOrContainer::Value(LoroValue::String(s))) = z_order.get(i) {
                if s.as_ref() == id {
                    pos = Some(i);
                    break;
                }
            }
        }

        if let Some(i) = pos {
            if i > 0 {
                // Get the item at the previous position
                let prev_id = if let Some(ValueOrContainer::Value(LoroValue::String(s))) =
                    z_order.get(i - 1)
                {
                    s.to_string()
                } else {
                    return Ok(false);
                };

                // Swap by removing and reinserting
                z_order.delete(i - 1, 2)?;
                z_order.insert(i - 1, LoroValue::String(id.to_string().into()))?;
                z_order.insert(i - 1, LoroValue::String(prev_id.into()))?;

                self.doc.commit();
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Export the document as a snapshot (full state).
    pub fn export_snapshot(&self) -> Vec<u8> {
        self.doc.export(ExportMode::Snapshot).unwrap_or_default()
    }

    /// Export incremental updates since a version.
    pub fn export_updates(&self, since: &loro::VersionVector) -> Vec<u8> {
        self.doc
            .export(ExportMode::updates(since))
            .unwrap_or_default()
    }

    /// Import updates from another document.
    pub fn import(&mut self, bytes: &[u8]) -> LoroResult<()> {
        self.doc.import(bytes)?;
        Ok(())
    }

    /// Get the current version vector.
    pub fn version(&self) -> loro::VersionVector {
        self.doc.oplog_vv()
    }

    /// Get the document name.
    pub fn name(&self) -> String {
        let text = self.doc.get_text(NAME_KEY);
        text.to_string()
    }

    /// Set the document name.
    pub fn set_name(&mut self, name: &str) -> LoroResult<()> {
        let text = self.doc.get_text(NAME_KEY);
        // Clear existing text
        let len = text.len_unicode();
        if len > 0 {
            text.delete(0, len)?;
        }
        text.insert(0, name)?;
        self.doc.commit();
        Ok(())
    }

    /// Clear all shapes from the document.
    pub fn clear(&mut self) -> LoroResult<()> {
        // Clear z_order
        let z_order = self.z_order_list();
        let len = z_order.len();
        if len > 0 {
            z_order.delete(0, len)?;
        }

        // Clear shapes - need to delete each key
        let shapes = self.shapes_map();
        let keys: Vec<String> = {
            let value = shapes.get_deep_value();
            if let LoroValue::Map(map) = value {
                map.keys().cloned().collect()
            } else {
                vec![]
            }
        };

        for key in keys {
            shapes.delete(&key)?;
        }

        self.doc.commit();
        Ok(())
    }

    // --- Undo/Redo API ---

    /// Undo the last change made by this peer.
    /// Returns true if undo was performed, false if nothing to undo.
    pub fn undo(&mut self) -> bool {
        self.undo_manager.undo().unwrap_or(false)
    }

    /// Redo the last undone change.
    /// Returns true if redo was performed, false if nothing to redo.
    pub fn redo(&mut self) -> bool {
        self.undo_manager.redo().unwrap_or(false)
    }

    /// Check if undo is available.
    pub fn can_undo(&self) -> bool {
        self.undo_manager.can_undo()
    }

    /// Check if redo is available.
    pub fn can_redo(&self) -> bool {
        self.undo_manager.can_redo()
    }

    /// Get the number of available undo steps.
    pub fn undo_count(&self) -> usize {
        self.undo_manager.undo_count()
    }

    /// Get the number of available redo steps.
    pub fn redo_count(&self) -> usize {
        self.undo_manager.redo_count()
    }

    /// Record a new checkpoint for undo grouping.
    /// All changes between checkpoints are grouped into a single undo step.
    pub fn record_checkpoint(&mut self) {
        let _ = self.undo_manager.record_new_checkpoint();
    }

    /// Start a new undo group. All changes until `end_undo_group` will be undone together.
    pub fn start_undo_group(&mut self) {
        let _ = self.undo_manager.group_start();
    }

    /// End the current undo group.
    pub fn end_undo_group(&mut self) {
        self.undo_manager.group_end();
    }

    /// Clear undo/redo history.
    pub fn clear_undo_history(&self) {
        self.undo_manager.clear();
    }
}

impl Default for CrdtDocument {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for CrdtDocument {
    fn clone(&self) -> Self {
        // Note: Clone creates a new document with fresh undo history
        let bytes = self.export_snapshot();
        Self::from_snapshot(&bytes).unwrap_or_else(|_| Self::new())
    }
}
