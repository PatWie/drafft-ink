//! Widget manager for tracking UI state of shapes.

use super::handles::{Handle, HandleKind, HandleShape};
use super::state::{EditingKind, WidgetState};
use crate::shapes::{Shape, ShapeId, ShapeTrait};
use kurbo::Point;
use std::collections::{HashMap, HashSet};

/// Manages UI state for all shapes in the document.
///
/// This separates UI concerns (selection, editing, hover) from
/// the pure shape data, making the system easier to reason about.
#[derive(Debug, Clone)]
pub struct WidgetManager {
    /// UI state for each shape.
    states: HashMap<ShapeId, WidgetState>,
    /// Currently selected shapes (subset of states with Selected/Editing).
    selected: HashSet<ShapeId>,
    /// Shape that has keyboard focus (for text editing).
    focused: Option<ShapeId>,
    /// Shape currently being hovered.
    hovered: Option<ShapeId>,
}

impl WidgetManager {
    /// Create a new widget manager.
    pub fn new() -> Self {
        Self {
            states: HashMap::new(),
            selected: HashSet::new(),
            focused: None,
            hovered: None,
        }
    }

    /// Get the state of a shape.
    pub fn state(&self, id: ShapeId) -> WidgetState {
        self.states.get(&id).cloned().unwrap_or_default()
    }

    /// Set the state of a shape.
    pub fn set_state(&mut self, id: ShapeId, state: WidgetState) {
        // Update selected set
        if state.is_selected() {
            self.selected.insert(id);
        } else {
            self.selected.remove(&id);
        }

        // Update focused
        if state.is_editing() {
            self.focused = Some(id);
        } else if self.focused == Some(id) {
            self.focused = None;
        }

        self.states.insert(id, state);
    }

    /// Check if a shape is selected.
    pub fn is_selected(&self, id: ShapeId) -> bool {
        self.selected.contains(&id)
    }

    /// Get all selected shape IDs.
    pub fn selected(&self) -> &HashSet<ShapeId> {
        &self.selected
    }

    /// Get the focused shape ID (if any).
    pub fn focused(&self) -> Option<ShapeId> {
        self.focused
    }

    /// Get the hovered shape ID (if any).
    pub fn hovered(&self) -> Option<ShapeId> {
        self.hovered
    }

    /// Set the hovered shape.
    pub fn set_hovered(&mut self, id: Option<ShapeId>) {
        // Clear old hover state
        if let Some(old_id) = self.hovered {
            if Some(old_id) != id {
                if let Some(state) = self.states.get(&old_id) {
                    if *state == WidgetState::Hovered {
                        self.states.insert(old_id, WidgetState::Normal);
                    }
                }
            }
        }

        // Set new hover state
        if let Some(new_id) = id {
            let current = self.state(new_id);
            if current == WidgetState::Normal {
                self.states.insert(new_id, WidgetState::Hovered);
            }
        }

        self.hovered = id;
    }

    /// Select a single shape (clears other selections).
    pub fn select(&mut self, id: ShapeId) {
        self.clear_selection();
        self.add_to_selection(id);
    }

    /// Add a shape to the selection.
    pub fn add_to_selection(&mut self, id: ShapeId) {
        self.set_state(id, WidgetState::Selected);
    }

    /// Remove a shape from the selection.
    pub fn deselect(&mut self, id: ShapeId) {
        if self.selected.contains(&id) {
            self.set_state(id, WidgetState::Normal);
        }
    }

    /// Clear all selections.
    pub fn clear_selection(&mut self) {
        let selected: Vec<_> = self.selected.iter().copied().collect();
        for id in selected {
            self.set_state(id, WidgetState::Normal);
        }
        self.selected.clear();
    }

    /// Enter editing mode for a shape.
    pub fn enter_editing(&mut self, id: ShapeId, kind: EditingKind) {
        // Exit any current editing
        if let Some(old_id) = self.focused {
            if old_id != id {
                self.exit_editing();
            }
        }

        self.set_state(id, WidgetState::Editing(kind));
    }

    /// Exit editing mode.
    pub fn exit_editing(&mut self) {
        if let Some(id) = self.focused {
            self.set_state(id, WidgetState::Selected);
        }
    }

    /// Check if currently in editing mode.
    pub fn is_editing(&self) -> bool {
        self.focused.is_some()
    }

    /// Check if a specific shape is being edited.
    pub fn is_editing_shape(&self, id: ShapeId) -> bool {
        self.focused == Some(id)
    }

    /// Remove state for a deleted shape.
    pub fn remove(&mut self, id: ShapeId) {
        self.states.remove(&id);
        self.selected.remove(&id);
        if self.focused == Some(id) {
            self.focused = None;
        }
        if self.hovered == Some(id) {
            self.hovered = None;
        }
    }

    /// Get handles for a shape based on its type and state.
    pub fn get_handles(&self, shape: &Shape) -> Vec<Handle> {
        let id = shape.id();
        if !self.is_selected(id) {
            return vec![];
        }

        get_shape_handles(shape)
    }
}

impl Default for WidgetManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Get the manipulation handles for a shape.
pub fn get_shape_handles(shape: &Shape) -> Vec<Handle> {
    match shape {
        Shape::Rectangle(r) => {
            let bounds = r.bounds();
            vec![
                Handle::new(HandleKind::TopLeft, Point::new(bounds.x0, bounds.y0)),
                Handle::new(HandleKind::TopRight, Point::new(bounds.x1, bounds.y0)),
                Handle::new(HandleKind::BottomLeft, Point::new(bounds.x0, bounds.y1)),
                Handle::new(HandleKind::BottomRight, Point::new(bounds.x1, bounds.y1)),
            ]
        }
        Shape::Ellipse(e) => {
            let bounds = e.bounds();
            vec![
                Handle::new(HandleKind::TopLeft, Point::new(bounds.x0, bounds.y0)),
                Handle::new(HandleKind::TopRight, Point::new(bounds.x1, bounds.y0)),
                Handle::new(HandleKind::BottomLeft, Point::new(bounds.x0, bounds.y1)),
                Handle::new(HandleKind::BottomRight, Point::new(bounds.x1, bounds.y1)),
            ]
        }
        Shape::Line(l) => {
            vec![
                Handle::new(HandleKind::Start, l.start).with_shape(HandleShape::Circle),
                Handle::new(HandleKind::End, l.end).with_shape(HandleShape::Circle),
            ]
        }
        Shape::Arrow(a) => {
            vec![
                Handle::new(HandleKind::Start, a.start).with_shape(HandleShape::Circle),
                Handle::new(HandleKind::End, a.end).with_shape(HandleShape::Circle),
            ]
        }
        Shape::Freehand(f) => {
            let bounds = f.bounds();
            vec![
                Handle::new(HandleKind::TopLeft, Point::new(bounds.x0, bounds.y0)),
                Handle::new(HandleKind::TopRight, Point::new(bounds.x1, bounds.y0)),
                Handle::new(HandleKind::BottomLeft, Point::new(bounds.x0, bounds.y1)),
                Handle::new(HandleKind::BottomRight, Point::new(bounds.x1, bounds.y1)),
            ]
        }
        Shape::Text(t) => {
            let bounds = t.bounds();
            vec![
                Handle::new(HandleKind::TopLeft, Point::new(bounds.x0, bounds.y0)),
                Handle::new(HandleKind::TopRight, Point::new(bounds.x1, bounds.y0)),
                Handle::new(HandleKind::BottomLeft, Point::new(bounds.x0, bounds.y1)),
                Handle::new(HandleKind::BottomRight, Point::new(bounds.x1, bounds.y1)),
            ]
        }
        Shape::Group(g) => {
            // Groups use bounding box corners
            let bounds = g.bounds();
            vec![
                Handle::new(HandleKind::TopLeft, Point::new(bounds.x0, bounds.y0)),
                Handle::new(HandleKind::TopRight, Point::new(bounds.x1, bounds.y0)),
                Handle::new(HandleKind::BottomLeft, Point::new(bounds.x0, bounds.y1)),
                Handle::new(HandleKind::BottomRight, Point::new(bounds.x1, bounds.y1)),
            ]
        }
        Shape::Image(img) => {
            // Images use bounding box corners for resizing
            let bounds = img.bounds();
            vec![
                Handle::new(HandleKind::TopLeft, Point::new(bounds.x0, bounds.y0)),
                Handle::new(HandleKind::TopRight, Point::new(bounds.x1, bounds.y0)),
                Handle::new(HandleKind::BottomLeft, Point::new(bounds.x0, bounds.y1)),
                Handle::new(HandleKind::BottomRight, Point::new(bounds.x1, bounds.y1)),
            ]
        }
        Shape::Math(m) => {
            let bounds = m.bounds();
            vec![
                Handle::new(HandleKind::TopLeft, Point::new(bounds.x0, bounds.y0)),
                Handle::new(HandleKind::TopRight, Point::new(bounds.x1, bounds.y0)),
                Handle::new(HandleKind::BottomLeft, Point::new(bounds.x0, bounds.y1)),
                Handle::new(HandleKind::BottomRight, Point::new(bounds.x1, bounds.y1)),
            ]
        }
    }
}

/// Hit test handles for a shape.
#[allow(dead_code)]
pub fn hit_test_handle(shape: &Shape, point: Point, tolerance: f64) -> Option<HandleKind> {
    let handles = get_shape_handles(shape);
    for handle in handles {
        let dx = point.x - handle.position.x;
        let dy = point.y - handle.position.y;
        if dx * dx + dy * dy <= tolerance * tolerance {
            return Some(handle.kind);
        }
    }
    None
}
