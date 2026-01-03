//! Canvas document and state management.

use crate::camera::Camera;
use crate::shapes::{Group, Shape, ShapeId, ShapeTrait};
use crate::tools::{ToolKind, ToolManager};
use crate::widget::{WidgetManager, WidgetState, EditingKind};
use kurbo::{Point, Rect};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Maximum number of undo states to keep.
const MAX_UNDO_HISTORY: usize = 50;

/// A snapshot of document state for undo/redo.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DocumentSnapshot {
    /// All shapes in the snapshot.
    shapes: HashMap<ShapeId, Shape>,
    /// Z-order of shapes.
    z_order: Vec<ShapeId>,
}

/// A canvas document containing all shapes and state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanvasDocument {
    /// Unique document identifier.
    pub id: String,
    /// Document name.
    pub name: String,
    /// All shapes in the document, keyed by ID.
    pub shapes: HashMap<ShapeId, Shape>,
    /// Z-order of shapes (back to front).
    pub z_order: Vec<ShapeId>,
    /// Undo history stack.
    #[serde(skip)]
    undo_stack: Vec<DocumentSnapshot>,
    /// Redo history stack.
    #[serde(skip)]
    redo_stack: Vec<DocumentSnapshot>,
}

impl Default for CanvasDocument {
    fn default() -> Self {
        Self::new()
    }
}

impl CanvasDocument {
    /// Create a new empty document.
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: "Untitled".to_string(),
            shapes: HashMap::new(),
            z_order: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    /// Take a snapshot of the current document state for undo.
    fn snapshot(&self) -> DocumentSnapshot {
        DocumentSnapshot {
            shapes: self.shapes.clone(),
            z_order: self.z_order.clone(),
        }
    }

    /// Push current state to undo stack (call before making changes).
    pub fn push_undo(&mut self) {
        let snapshot = self.snapshot();
        self.undo_stack.push(snapshot);
        
        // Clear redo stack when new changes are made
        self.redo_stack.clear();
        
        // Limit undo history size
        if self.undo_stack.len() > MAX_UNDO_HISTORY {
            self.undo_stack.remove(0);
        }
    }

    /// Undo the last change.
    /// Returns true if undo was performed, false if nothing to undo.
    pub fn undo(&mut self) -> bool {
        if let Some(snapshot) = self.undo_stack.pop() {
            // Save current state to redo stack
            let current = self.snapshot();
            self.redo_stack.push(current);
            
            // Restore the snapshot
            self.shapes = snapshot.shapes;
            self.z_order = snapshot.z_order;
            
            true
        } else {
            false
        }
    }

    /// Redo the last undone change.
    /// Returns true if redo was performed, false if nothing to redo.
    pub fn redo(&mut self) -> bool {
        if let Some(snapshot) = self.redo_stack.pop() {
            // Save current state to undo stack
            let current = self.snapshot();
            self.undo_stack.push(current);
            
            // Restore the snapshot
            self.shapes = snapshot.shapes;
            self.z_order = snapshot.z_order;
            
            true
        } else {
            false
        }
    }

    /// Check if undo is available.
    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    /// Check if redo is available.
    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Add a shape to the document.
    pub fn add_shape(&mut self, shape: Shape) {
        let id = shape.id();
        self.z_order.push(id);
        self.shapes.insert(id, shape);
    }

    /// Remove a shape from the document.
    pub fn remove_shape(&mut self, id: ShapeId) -> Option<Shape> {
        self.z_order.retain(|&shape_id| shape_id != id);
        self.shapes.remove(&id)
    }

    /// Clear all shapes from the document.
    pub fn clear(&mut self) {
        self.shapes.clear();
        self.z_order.clear();
    }

    /// Get a shape by ID.
    pub fn get_shape(&self, id: ShapeId) -> Option<&Shape> {
        self.shapes.get(&id)
    }

    /// Get a mutable reference to a shape by ID.
    pub fn get_shape_mut(&mut self, id: ShapeId) -> Option<&mut Shape> {
        self.shapes.get_mut(&id)
    }

    /// Get shapes in z-order (back to front).
    pub fn shapes_ordered(&self) -> impl Iterator<Item = &Shape> {
        self.z_order.iter().filter_map(|id| self.shapes.get(id))
    }

    /// Bring a shape to the front (topmost).
    pub fn bring_to_front(&mut self, id: ShapeId) {
        self.z_order.retain(|&shape_id| shape_id != id);
        self.z_order.push(id);
    }

    /// Send a shape to the back (bottommost).
    pub fn send_to_back(&mut self, id: ShapeId) {
        self.z_order.retain(|&shape_id| shape_id != id);
        self.z_order.insert(0, id);
    }

    /// Move a shape one layer forward (towards front).
    /// Returns true if the shape was moved, false if already at front.
    pub fn bring_forward(&mut self, id: ShapeId) -> bool {
        if let Some(pos) = self.z_order.iter().position(|&shape_id| shape_id == id) {
            if pos < self.z_order.len() - 1 {
                self.z_order.swap(pos, pos + 1);
                return true;
            }
        }
        false
    }

    /// Move a shape one layer backward (towards back).
    /// Returns true if the shape was moved, false if already at back.
    pub fn send_backward(&mut self, id: ShapeId) -> bool {
        if let Some(pos) = self.z_order.iter().position(|&shape_id| shape_id == id) {
            if pos > 0 {
                self.z_order.swap(pos, pos - 1);
                return true;
            }
        }
        false
    }

    /// Get the bounding box of all shapes.
    pub fn bounds(&self) -> Option<Rect> {
        let mut result: Option<Rect> = None;
        for shape in self.shapes.values() {
            let bounds = shape.bounds();
            result = Some(match result {
                Some(r) => r.union(bounds),
                None => bounds,
            });
        }
        result
    }

    /// Find shapes at a point (in world coordinates).
    pub fn shapes_at_point(&self, point: Point, tolerance: f64) -> Vec<ShapeId> {
        // Return in reverse z-order (front to back) for selection priority
        self.z_order
            .iter()
            .rev()
            .filter_map(|&id| {
                self.shapes
                    .get(&id)
                    .filter(|s| s.hit_test(point, tolerance))
                    .map(|_| id)
            })
            .collect()
    }

    /// Find shapes that intersect or are contained within a rectangle.
    pub fn shapes_in_rect(&self, rect: Rect) -> Vec<ShapeId> {
        self.z_order
            .iter()
            .filter_map(|&id| {
                self.shapes
                    .get(&id)
                    .filter(|s| {
                        let bounds = s.bounds();
                        // Check if shape bounds intersect with selection rect
                        rect.intersect(bounds).area() > 0.0
                    })
                    .map(|_| id)
            })
            .collect()
    }

    /// Check if the document is empty.
    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }

    /// Get the number of shapes.
    pub fn len(&self) -> usize {
        self.shapes.len()
    }

    /// Serialize the document to JSON.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Deserialize a document from JSON.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Import from Excalidraw JSON format.
    pub fn from_excalidraw(json: &str) -> Result<Self, String> {
        use crate::shapes::{Arrow, Ellipse, Freehand, Line, PathStyle, Rectangle, ShapeStyle, Sloppiness, Text, FillPattern};
        
        let data: serde_json::Value = serde_json::from_str(json)
            .map_err(|e| format!("Invalid JSON: {}", e))?;
        
        let elements = data.get("elements")
            .and_then(|e| e.as_array())
            .ok_or("Missing 'elements' array")?;
        
        let mut doc = Self::new();
        
        for elem in elements {
            // Skip deleted elements
            if elem.get("isDeleted").and_then(|v| v.as_bool()).unwrap_or(false) {
                continue;
            }
            
            let elem_type = elem.get("type").and_then(|t| t.as_str()).unwrap_or("");
            let x = elem.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let y = elem.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            
            // Parse colors
            let stroke_color = parse_excalidraw_color(
                elem.get("strokeColor").and_then(|v| v.as_str()).unwrap_or("#000000")
            );
            let bg_color = elem.get("backgroundColor").and_then(|v| v.as_str()).unwrap_or("transparent");
            let fill_color = if bg_color == "transparent" {
                None
            } else {
                Some(parse_excalidraw_color(bg_color))
            };
            
            let stroke_width = elem.get("strokeWidth").and_then(|v| v.as_f64()).unwrap_or(2.0);
            let roughness = elem.get("roughness").and_then(|v| v.as_i64()).unwrap_or(1);
            let sloppiness = match roughness {
                0 => Sloppiness::Architect,
                1 => Sloppiness::Artist,
                _ => Sloppiness::Cartoonist,
            };
            
            let style = ShapeStyle {
                stroke_color,
                stroke_width,
                fill_color,
                fill_pattern: FillPattern::default(),
                sloppiness,
                seed: elem.get("seed").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            };
            
            let shape: Option<Shape> = match elem_type {
                "rectangle" | "diamond" => {
                    let width = elem.get("width").and_then(|v| v.as_f64()).unwrap_or(100.0);
                    let height = elem.get("height").and_then(|v| v.as_f64()).unwrap_or(100.0);
                    let mut rect = Rectangle::new(Point::new(x, y), width, height);
                    rect.style = style;
                    // Handle roundness
                    if elem.get("roundness").is_some() {
                        rect.corner_radius = Rectangle::DEFAULT_ADAPTIVE_RADIUS.min(width / 4.0).min(height / 4.0);
                    }
                    // Diamond is rendered as rotated rectangle (TODO: proper diamond shape)
                    Some(Shape::Rectangle(rect))
                }
                "ellipse" => {
                    let width = elem.get("width").and_then(|v| v.as_f64()).unwrap_or(100.0);
                    let height = elem.get("height").and_then(|v| v.as_f64()).unwrap_or(100.0);
                    let center = Point::new(x + width / 2.0, y + height / 2.0);
                    let mut ellipse = Ellipse::new(center, width / 2.0, height / 2.0);
                    ellipse.style = style;
                    Some(Shape::Ellipse(ellipse))
                }
                "freedraw" => {
                    let points = elem.get("points").and_then(|p| p.as_array());
                    if let Some(pts) = points {
                        let freehand_points: Vec<Point> = pts.iter()
                            .filter_map(|p| p.as_array())
                            .filter_map(|arr| {
                                let px = arr.first().and_then(|v| v.as_f64())?;
                                let py = arr.get(1).and_then(|v| v.as_f64())?;
                                Some(Point::new(x + px, y + py))
                            })
                            .collect();
                        if !freehand_points.is_empty() {
                            let mut freehand = Freehand::from_points(freehand_points);
                            freehand.style = style;
                            Some(Shape::Freehand(freehand))
                        } else { None }
                    } else { None }
                }
                "line" => {
                    let points = elem.get("points").and_then(|p| p.as_array());
                    if let Some(pts) = points {
                        let line_points: Vec<Point> = pts.iter()
                            .filter_map(|p| p.as_array())
                            .filter_map(|arr| {
                                let px = arr.first().and_then(|v| v.as_f64())?;
                                let py = arr.get(1).and_then(|v| v.as_f64())?;
                                Some(Point::new(x + px, y + py))
                            })
                            .collect();
                        if line_points.len() >= 2 {
                            let path_style = if elem.get("roundness").map(|r| !r.is_null()).unwrap_or(false) {
                                PathStyle::Flowing
                            } else {
                                PathStyle::Direct
                            };
                            let mut line = Line::from_points(line_points, path_style);
                            line.style = style;
                            Some(Shape::Line(line))
                        } else { None }
                    } else { None }
                }
                "arrow" => {
                    let points = elem.get("points").and_then(|p| p.as_array());
                    if let Some(pts) = points {
                        let arrow_points: Vec<Point> = pts.iter()
                            .filter_map(|p| p.as_array())
                            .filter_map(|arr| {
                                let px = arr.first().and_then(|v| v.as_f64())?;
                                let py = arr.get(1).and_then(|v| v.as_f64())?;
                                Some(Point::new(x + px, y + py))
                            })
                            .collect();
                        if arrow_points.len() >= 2 {
                            let elbowed = elem.get("elbowed").and_then(|v| v.as_bool()).unwrap_or(false);
                            let has_roundness = elem.get("roundness").map(|r| !r.is_null()).unwrap_or(false);
                            let path_style = if elbowed {
                                PathStyle::Angular
                            } else if has_roundness {
                                PathStyle::Flowing
                            } else {
                                PathStyle::Direct
                            };
                            let mut arrow = Arrow::from_points(arrow_points, path_style);
                            arrow.style = style;
                            Some(Shape::Arrow(arrow))
                        } else { None }
                    } else { None }
                }
                "text" => {
                    let content = elem.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let font_size = elem.get("fontSize").and_then(|v| v.as_f64()).unwrap_or(20.0);
                    let mut text = Text::new(Point::new(x, y), content);
                    text.font_size = font_size;
                    text.style = style;
                    Some(Shape::Text(text))
                }
                _ => None,
            };
            
            if let Some(s) = shape {
                doc.add_shape(s);
            }
        }
        
        Ok(doc)
    }

    /// Export selected shapes to a new document.
    pub fn export_selection(&self, selection: &[ShapeId]) -> Self {
        let mut doc = Self::new();
        for &id in selection {
            if let Some(shape) = self.shapes.get(&id) {
                doc.add_shape(shape.clone());
            }
        }
        doc
    }

    /// Group the given shapes into a single group.
    /// Returns the new group's ID, or None if less than 2 shapes were provided.
    /// The group will be placed at the position of the frontmost shape in z-order.
    pub fn group_shapes(&mut self, shape_ids: &[ShapeId]) -> Option<ShapeId> {
        if shape_ids.len() < 2 {
            return None;
        }
        
        // Collect the shapes to group (in z-order)
        let mut shapes_to_group: Vec<(usize, Shape)> = Vec::new();
        for (idx, &zid) in self.z_order.iter().enumerate() {
            if shape_ids.contains(&zid) {
                if let Some(shape) = self.shapes.get(&zid) {
                    shapes_to_group.push((idx, shape.clone()));
                }
            }
        }
        
        if shapes_to_group.len() < 2 {
            return None;
        }
        
        // Find the highest z-order position (where the group will go)
        let max_z_idx = shapes_to_group.iter().map(|(idx, _)| *idx).max().unwrap();
        
        // Extract shapes in their z-order
        let children: Vec<Shape> = shapes_to_group.into_iter().map(|(_, s)| s).collect();
        
        // Create the group
        let group = Group::new(children);
        let group_id = group.id();
        
        // Remove the original shapes from the document
        for &id in shape_ids {
            self.shapes.remove(&id);
            self.z_order.retain(|&zid| zid != id);
        }
        
        // Add the group at the position of the frontmost shape
        self.shapes.insert(group_id, Shape::Group(group));
        // Insert at the max position (adjusted for removed items)
        let insert_pos = max_z_idx.saturating_sub(shape_ids.len() - 1).min(self.z_order.len());
        self.z_order.insert(insert_pos, group_id);
        
        Some(group_id)
    }

    /// Ungroup a group shape, returning its children to the document.
    /// Returns the IDs of the ungrouped children, or None if the shape is not a group.
    pub fn ungroup_shape(&mut self, group_id: ShapeId) -> Option<Vec<ShapeId>> {
        // Check if the shape is a group
        let group = match self.shapes.get(&group_id) {
            Some(Shape::Group(g)) => g.clone(),
            _ => return None,
        };
        
        // Find the group's position in z-order
        let z_pos = self.z_order.iter().position(|&id| id == group_id)?;
        
        // Remove the group
        self.shapes.remove(&group_id);
        self.z_order.retain(|&id| id != group_id);
        
        // Add children back to the document at the group's position
        let children = group.ungroup();
        let child_ids: Vec<ShapeId> = children.iter().map(|s| s.id()).collect();
        
        for (i, child) in children.into_iter().enumerate() {
            let child_id = child.id();
            self.shapes.insert(child_id, child);
            // Insert at the original position, maintaining child order
            self.z_order.insert(z_pos + i, child_id);
        }
        
        Some(child_ids)
    }

}

/// Runtime canvas state (not persisted).
#[derive(Debug, Clone)]
pub struct Canvas {
    /// The document being edited.
    pub document: CanvasDocument,
    /// Camera for view transform.
    pub camera: Camera,
    /// Tool manager.
    pub tool_manager: ToolManager,
    /// Currently selected shape IDs.
    /// NOTE: Prefer using widget_manager for selection state; this is kept for backward compatibility.
    pub selection: Vec<ShapeId>,
    /// Viewport size.
    pub viewport_size: kurbo::Size,
    /// Widget manager for UI state (selection, hover, editing).
    pub widgets: WidgetManager,
}

impl Default for Canvas {
    fn default() -> Self {
        Self::new()
    }
}

impl Canvas {
    /// Create a new canvas with an empty document.
    pub fn new() -> Self {
        Self {
            document: CanvasDocument::new(),
            camera: Camera::new(),
            tool_manager: ToolManager::new(),
            selection: Vec::new(),
            viewport_size: kurbo::Size::new(800.0, 600.0),
            widgets: WidgetManager::new(),
        }
    }

    /// Create a canvas with an existing document.
    pub fn with_document(document: CanvasDocument) -> Self {
        Self {
            document,
            camera: Camera::new(),
            tool_manager: ToolManager::new(),
            selection: Vec::new(),
            viewport_size: kurbo::Size::new(800.0, 600.0),
            widgets: WidgetManager::new(),
        }
    }

    /// Set the viewport size.
    pub fn set_viewport_size(&mut self, width: f64, height: f64) {
        self.viewport_size = kurbo::Size::new(width, height);
    }

    /// Select a shape (clears previous selection).
    pub fn select(&mut self, id: ShapeId) {
        self.clear_selection();
        self.add_to_selection(id);
    }

    /// Add to selection.
    pub fn add_to_selection(&mut self, id: ShapeId) {
        if !self.selection.contains(&id) {
            self.selection.push(id);
        }
        self.widgets.add_to_selection(id);
    }

    /// Clear selection.
    pub fn clear_selection(&mut self) {
        self.selection.clear();
        self.widgets.clear_selection();
    }

    /// Select all shapes.
    pub fn select_all(&mut self) {
        self.clear_selection();
        for &id in &self.document.z_order {
            self.selection.push(id);
            self.widgets.add_to_selection(id);
        }
    }

    /// Check if a shape is selected.
    pub fn is_selected(&self, id: ShapeId) -> bool {
        self.widgets.is_selected(id)
    }

    /// Enter text editing mode for a shape.
    pub fn enter_text_editing(&mut self, id: ShapeId) {
        self.widgets.enter_editing(id, EditingKind::Text);
    }

    /// Exit text editing mode.
    pub fn exit_text_editing(&mut self) {
        self.widgets.exit_editing();
    }

    /// Get the shape currently being edited (if any).
    pub fn editing_shape(&self) -> Option<ShapeId> {
        self.widgets.focused()
    }

    /// Check if a shape is being edited.
    pub fn is_editing(&self, id: ShapeId) -> bool {
        self.widgets.is_editing_shape(id)
    }

    /// Get widget state for a shape.
    pub fn widget_state(&self, id: ShapeId) -> WidgetState {
        self.widgets.state(id)
    }

    /// Set the current tool.
    pub fn set_tool(&mut self, tool: ToolKind) {
        self.tool_manager.set_tool(tool);
    }

    /// Fit the view to show all shapes.
    pub fn fit_to_content(&mut self) {
        if let Some(bounds) = self.document.bounds() {
            self.camera.fit_to_bounds(bounds, self.viewport_size, 50.0);
        }
    }

    /// Delete selected shapes.
    pub fn delete_selected(&mut self) {
        for id in self.selection.drain(..).collect::<Vec<_>>() {
            self.document.remove_shape(id);
            self.widgets.remove(id);
        }
    }

    /// Flip selected shapes horizontally (mirror around vertical axis).
    pub fn flip_selected_horizontal(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        
        // Get combined bounds of all selected shapes
        let mut combined_bounds: Option<kurbo::Rect> = None;
        for &id in &self.selection {
            if let Some(shape) = self.document.get_shape(id) {
                let bounds = shape.bounds();
                combined_bounds = Some(match combined_bounds {
                    Some(cb) => cb.union(bounds),
                    None => bounds,
                });
            }
        }
        
        let Some(bounds) = combined_bounds else { return };
        let center_x = bounds.center().x;
        
        // Flip each selected shape around the combined center
        for &id in &self.selection {
            if let Some(shape) = self.document.get_shape_mut(id) {
                // Create flip transform: translate to origin, scale -1 on x, translate back
                let flip = kurbo::Affine::translate(kurbo::Vec2::new(center_x, 0.0))
                    * kurbo::Affine::scale_non_uniform(-1.0, 1.0)
                    * kurbo::Affine::translate(kurbo::Vec2::new(-center_x, 0.0));
                shape.transform(flip);
            }
        }
    }

    /// Flip selected shapes vertically (mirror around horizontal axis).
    pub fn flip_selected_vertical(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        
        // Get combined bounds of all selected shapes
        let mut combined_bounds: Option<kurbo::Rect> = None;
        for &id in &self.selection {
            if let Some(shape) = self.document.get_shape(id) {
                let bounds = shape.bounds();
                combined_bounds = Some(match combined_bounds {
                    Some(cb) => cb.union(bounds),
                    None => bounds,
                });
            }
        }
        
        let Some(bounds) = combined_bounds else { return };
        let center_y = bounds.center().y;
        
        // Flip each selected shape around the combined center
        for &id in &self.selection {
            if let Some(shape) = self.document.get_shape_mut(id) {
                // Create flip transform: translate to origin, scale -1 on y, translate back
                let flip = kurbo::Affine::translate(kurbo::Vec2::new(0.0, center_y))
                    * kurbo::Affine::scale_non_uniform(1.0, -1.0)
                    * kurbo::Affine::translate(kurbo::Vec2::new(0.0, -center_y));
                shape.transform(flip);
            }
        }
    }

    /// Remove a shape from the canvas.
    pub fn remove_shape(&mut self, id: ShapeId) {
        self.selection.retain(|&s| s != id);
        self.document.remove_shape(id);
        self.widgets.remove(id);
    }

    /// Group the currently selected shapes.
    /// Returns the new group's ID, or None if less than 2 shapes were selected.
    pub fn group_selected(&mut self) -> Option<ShapeId> {
        if self.selection.len() < 2 {
            return None;
        }
        
        let shape_ids: Vec<ShapeId> = self.selection.clone();
        
        // Push undo before grouping
        self.document.push_undo();
        
        if let Some(group_id) = self.document.group_shapes(&shape_ids) {
            // Clear old selection and widgets
            for &id in &shape_ids {
                self.widgets.remove(id);
            }
            
            // Select the new group
            self.selection.clear();
            self.selection.push(group_id);
            self.widgets.add_to_selection(group_id);
            
            Some(group_id)
        } else {
            None
        }
    }

    /// Ungroup the currently selected groups.
    /// Returns the IDs of all ungrouped children.
    pub fn ungroup_selected(&mut self) -> Vec<ShapeId> {
        let mut all_children = Vec::new();
        
        // Find groups in selection
        let groups: Vec<ShapeId> = self.selection
            .iter()
            .filter(|&&id| {
                self.document.get_shape(id)
                    .map(|s| s.is_group())
                    .unwrap_or(false)
            })
            .copied()
            .collect();
        
        if groups.is_empty() {
            return all_children;
        }
        
        // Push undo before ungrouping
        self.document.push_undo();
        
        // Ungroup each group
        for group_id in groups {
            self.widgets.remove(group_id);
            self.selection.retain(|&id| id != group_id);
            
            if let Some(children) = self.document.ungroup_shape(group_id) {
                // Add children to selection
                for &child_id in &children {
                    if !self.selection.contains(&child_id) {
                        self.selection.push(child_id);
                    }
                    self.widgets.add_to_selection(child_id);
                }
                all_children.extend(children);
            }
        }
        
        all_children
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shapes::{Rectangle, ShapeTrait};

    #[test]
    fn test_document_creation() {
        let doc = CanvasDocument::new();
        assert!(doc.is_empty());
    }

    #[test]
    fn test_add_shape() {
        let mut doc = CanvasDocument::new();
        let rect = Rectangle::new(Point::new(0.0, 0.0), 100.0, 100.0);
        let id = rect.id();

        doc.add_shape(Shape::Rectangle(rect));
        assert_eq!(doc.len(), 1);
        assert!(doc.get_shape(id).is_some());
    }

    #[test]
    fn test_remove_shape() {
        let mut doc = CanvasDocument::new();
        let rect = Rectangle::new(Point::new(0.0, 0.0), 100.0, 100.0);
        let id = rect.id();

        doc.add_shape(Shape::Rectangle(rect));
        let removed = doc.remove_shape(id);

        assert!(removed.is_some());
        assert!(doc.is_empty());
    }

    #[test]
    fn test_z_order() {
        let mut doc = CanvasDocument::new();
        let rect1 = Rectangle::new(Point::new(0.0, 0.0), 100.0, 100.0);
        let rect2 = Rectangle::new(Point::new(50.0, 50.0), 100.0, 100.0);
        let id1 = rect1.id();
        let id2 = rect2.id();

        doc.add_shape(Shape::Rectangle(rect1));
        doc.add_shape(Shape::Rectangle(rect2));

        assert_eq!(doc.z_order, vec![id1, id2]);

        doc.bring_to_front(id1);
        assert_eq!(doc.z_order, vec![id2, id1]);

        doc.send_to_back(id1);
        assert_eq!(doc.z_order, vec![id1, id2]);
    }

    #[test]
    fn test_shapes_at_point() {
        let mut doc = CanvasDocument::new();
        let rect1 = Rectangle::new(Point::new(0.0, 0.0), 100.0, 100.0);
        let rect2 = Rectangle::new(Point::new(50.0, 50.0), 100.0, 100.0);
        let id1 = rect1.id();
        let id2 = rect2.id();

        doc.add_shape(Shape::Rectangle(rect1));
        doc.add_shape(Shape::Rectangle(rect2));

        // Point in both shapes
        let hits = doc.shapes_at_point(Point::new(75.0, 75.0), 0.0);
        assert_eq!(hits.len(), 2);
        // Front shape should be first
        assert_eq!(hits[0], id2);
        assert_eq!(hits[1], id1);

        // Point only in rect1
        let hits = doc.shapes_at_point(Point::new(25.0, 25.0), 0.0);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0], id1);
    }

    #[test]
    fn test_canvas_selection() {
        let mut canvas = Canvas::new();
        let rect = Rectangle::new(Point::new(0.0, 0.0), 100.0, 100.0);
        let id = rect.id();

        canvas.document.add_shape(Shape::Rectangle(rect));

        assert!(!canvas.is_selected(id));
        canvas.select(id);
        assert!(canvas.is_selected(id));
        canvas.clear_selection();
        assert!(!canvas.is_selected(id));
    }

    #[test]
    fn test_delete_selected() {
        let mut canvas = Canvas::new();
        let rect = Rectangle::new(Point::new(0.0, 0.0), 100.0, 100.0);
        let id = rect.id();

        canvas.document.add_shape(Shape::Rectangle(rect));
        canvas.select(id);
        canvas.delete_selected();

        assert!(canvas.document.is_empty());
        assert!(canvas.selection.is_empty());
    }

    #[test]
    fn test_undo_add_shape() {
        let mut doc = CanvasDocument::new();
        
        // Add a shape with undo point
        doc.push_undo();
        let rect = Rectangle::new(Point::new(0.0, 0.0), 100.0, 100.0);
        let id = rect.id();
        doc.add_shape(Shape::Rectangle(rect));
        
        assert_eq!(doc.len(), 1);
        assert!(doc.can_undo());
        
        // Undo should remove the shape
        assert!(doc.undo());
        assert!(doc.is_empty());
        assert!(doc.can_redo());
        
        // Redo should restore the shape
        assert!(doc.redo());
        assert_eq!(doc.len(), 1);
        assert!(doc.get_shape(id).is_some());
    }

    #[test]
    fn test_undo_remove_shape() {
        let mut doc = CanvasDocument::new();
        let rect = Rectangle::new(Point::new(0.0, 0.0), 100.0, 100.0);
        let id = rect.id();
        doc.add_shape(Shape::Rectangle(rect));
        
        // Remove with undo point
        doc.push_undo();
        doc.remove_shape(id);
        
        assert!(doc.is_empty());
        
        // Undo should restore the shape
        assert!(doc.undo());
        assert_eq!(doc.len(), 1);
        assert!(doc.get_shape(id).is_some());
    }

    #[test]
    fn test_undo_clears_redo() {
        let mut doc = CanvasDocument::new();
        
        // Add first shape
        doc.push_undo();
        let rect1 = Rectangle::new(Point::new(0.0, 0.0), 100.0, 100.0);
        doc.add_shape(Shape::Rectangle(rect1));
        
        // Undo
        assert!(doc.undo());
        assert!(doc.can_redo());
        
        // Add new shape (should clear redo)
        doc.push_undo();
        let rect2 = Rectangle::new(Point::new(50.0, 50.0), 100.0, 100.0);
        doc.add_shape(Shape::Rectangle(rect2));
        
        // Redo should not be available
        assert!(!doc.can_redo());
    }

    #[test]
    fn test_undo_empty_stack() {
        let mut doc = CanvasDocument::new();
        
        // Undo on empty stack should return false
        assert!(!doc.can_undo());
        assert!(!doc.undo());
        
        // Redo on empty stack should return false
        assert!(!doc.can_redo());
        assert!(!doc.redo());
    }
}

/// Parse Excalidraw color string to SerializableColor.
fn parse_excalidraw_color(color: &str) -> crate::shapes::SerializableColor {
    use crate::shapes::SerializableColor;
    
    if color == "transparent" {
        return SerializableColor::transparent();
    }
    
    // Handle hex colors (#rgb, #rrggbb, #rrggbbaa)
    if let Some(hex) = color.strip_prefix('#') {
        let hex = hex.trim();
        match hex.len() {
            3 => {
                // #rgb -> #rrggbb
                let r = u8::from_str_radix(&hex[0..1], 16).unwrap_or(0) * 17;
                let g = u8::from_str_radix(&hex[1..2], 16).unwrap_or(0) * 17;
                let b = u8::from_str_radix(&hex[2..3], 16).unwrap_or(0) * 17;
                return SerializableColor::new(r, g, b, 255);
            }
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
                let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
                let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
                return SerializableColor::new(r, g, b, 255);
            }
            8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
                let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
                let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
                let a = u8::from_str_radix(&hex[6..8], 16).unwrap_or(255);
                return SerializableColor::new(r, g, b, a);
            }
            _ => {}
        }
    }
    
    // Default to black
    SerializableColor::black()
}
