//! Event handling for tool interactions.

use kurbo::{Point, Rect};
use drafftink_core::canvas::Canvas;
use drafftink_core::input::InputState;
use drafftink_core::selection::{
    apply_manipulation, apply_rotation, get_handles, get_manipulation_target_position, hit_test_handles, hit_test_boundary, ManipulationState, MultiMoveState, HANDLE_HIT_TOLERANCE,
};
use drafftink_core::shapes::{Freehand, Shape, ShapeId, ShapeStyle, ShapeTrait, Text};
use drafftink_core::selection::{Corner, HandleKind};
use drafftink_core::snap::{snap_line_endpoint_isometric, snap_point_with_shapes, get_snap_targets_from_bounds, get_snap_targets_from_line, AngleSnapResult, SnapMode, SnapResult, SnapTarget, GRID_SIZE};
use drafftink_core::tools::ToolKind;

/// Collect all snap targets from shapes in the canvas.
/// Optionally excludes a specific shape (e.g., the one being manipulated).
fn collect_snap_targets(canvas: &Canvas, exclude_shape_id: Option<ShapeId>) -> Vec<SnapTarget> {
    let exclude_ids: Vec<ShapeId> = exclude_shape_id.into_iter().collect();
    collect_snap_targets_excluding(canvas, &exclude_ids)
}

/// Collect all snap targets from shapes in the canvas, excluding multiple shapes.
fn collect_snap_targets_excluding(canvas: &Canvas, exclude_ids: &[ShapeId]) -> Vec<SnapTarget> {
    let mut targets = Vec::new();
    
    for shape in canvas.document.shapes_ordered() {
        // Skip excluded shapes
        if exclude_ids.contains(&shape.id()) {
            continue;
        }
        
        // Get snap targets based on shape type
        match shape {
            Shape::Line(line) => {
                targets.extend(get_snap_targets_from_line(line.start, line.end));
            }
            Shape::Arrow(arrow) => {
                targets.extend(get_snap_targets_from_line(arrow.start, arrow.end));
            }
            _ => {
                // For other shapes, use bounds
                targets.extend(get_snap_targets_from_bounds(shape.bounds()));
            }
        }
    }
    
    targets
}

/// Get the other endpoint of a line/arrow given the handle being manipulated.
/// Returns the start point if manipulating the end, and vice versa.
fn get_line_other_endpoint(shape: &Shape, handle: Option<HandleKind>) -> Point {
    match shape {
        Shape::Line(line) => {
            match handle {
                Some(HandleKind::Endpoint(0)) => line.end,   // Manipulating start -> return end
                Some(HandleKind::Endpoint(1)) => line.start, // Manipulating end -> return start
                _ => line.start, // Default to start
            }
        }
        Shape::Arrow(arrow) => {
            match handle {
                Some(HandleKind::Endpoint(0)) => arrow.end,   // Manipulating start -> return end
                Some(HandleKind::Endpoint(1)) => arrow.start, // Manipulating end -> return start
                _ => arrow.start, // Default to start
            }
        }
        _ => Point::ZERO, // Not a line/arrow
    }
}

/// Selection rectangle state for marquee selection.
#[derive(Debug, Clone)]
pub struct SelectionRect {
    /// Starting point in world coordinates.
    pub start: Point,
    /// Current point in world coordinates.
    pub current: Point,
}

impl SelectionRect {
    /// Get the selection rectangle as a Rect.
    pub fn to_rect(&self) -> Rect {
        Rect::new(
            self.start.x.min(self.current.x),
            self.start.y.min(self.current.y),
            self.start.x.max(self.current.x),
            self.start.y.max(self.current.y),
        )
    }
}

/// Handles high-level events and translates them to canvas operations.
pub struct EventHandler {
    /// Current manipulation state (when dragging a handle on a single shape).
    manipulation: Option<ManipulationState>,
    /// Current multi-move state (when moving multiple selected shapes).
    multi_move: Option<MultiMoveState>,
    /// Selection rectangle for marquee selection.
    selection_rect: Option<SelectionRect>,
    /// Shape ID being edited (for text editing).
    pub editing_text: Option<ShapeId>,
    /// Original top-left handle position when editing started.
    pub text_edit_anchor: Option<Point>,
    /// Original size (width, height) when editing started.
    text_edit_size: Option<(f64, f64)>,
    /// Last snap result (for rendering snap guides).
    pub last_snap: Option<SnapResult>,
    /// Last angle snap result (for rendering angle guides).
    pub last_angle_snap: Option<AngleSnapResult>,
    /// Start point for line/arrow drawing (for angle snap visualization).
    pub line_start_point: Option<Point>,
    /// Current snap targets from nearby shapes (for rendering snap indicators).
    pub current_snap_targets: Vec<SnapTarget>,
    /// Current rotation angle during rotation drag (for helper line rendering).
    pub rotation_state: Option<RotationState>,
    /// Eraser path points for current stroke.
    eraser_points: Vec<Point>,
    /// Eraser radius for hit detection.
    pub eraser_radius: f64,
    /// Laser pointer position (for rendering).
    pub laser_position: Option<Point>,
    /// Laser pointer trail for fading effect.
    pub laser_trail: Vec<(Point, f64)>,
}

/// State for rotation drag operation.
#[derive(Debug, Clone)]
pub struct RotationState {
    /// Center of the shape being rotated.
    pub center: Point,
    /// Current rotation angle in radians.
    pub angle: f64,
    /// Whether snapping to 15° increments.
    pub snapped: bool,
}

impl EventHandler {
    /// Create a new event handler.
    pub fn new() -> Self {
        Self {
            manipulation: None,
            multi_move: None,
            selection_rect: None,
            editing_text: None,
            text_edit_anchor: None,
            text_edit_size: None,
            last_snap: None,
            last_angle_snap: None,
            line_start_point: None,
            current_snap_targets: Vec::new(),
            rotation_state: None,
            eraser_points: Vec::new(),
            eraser_radius: 10.0,
            laser_position: None,
            laser_trail: Vec::new(),
        }
    }

    /// Check if a manipulation is in progress.
    pub fn is_manipulating(&self) -> bool {
        self.manipulation.is_some() || self.multi_move.is_some()
    }

    /// Check if a selection rectangle is active.
    pub fn is_selecting(&self) -> bool {
        self.selection_rect.is_some()
    }

    /// Cancel any ongoing operation.
    pub fn cancel(&mut self, canvas: &mut Canvas) {
        // Restore original shapes if manipulating
        if let Some(manip) = self.manipulation.take() {
            if let Some(shape) = canvas.document.get_shape_mut(manip.shape_id) {
                *shape = manip.original_shape;
            }
        }
        if let Some(mm) = self.multi_move.take() {
            for (id, original) in mm.original_shapes {
                if let Some(shape) = canvas.document.get_shape_mut(id) {
                    *shape = original;
                }
            }
        }
        self.selection_rect = None;
        self.last_snap = None;
        self.last_angle_snap = None;
        self.rotation_state = None;
        canvas.tool_manager.cancel();
    }

    /// Get the current selection rectangle (for rendering).
    pub fn selection_rect(&self) -> Option<&SelectionRect> {
        self.selection_rect.as_ref()
    }

    /// Get the current manipulation state.
    #[allow(dead_code)]
    pub fn manipulation(&self) -> Option<&ManipulationState> {
        self.manipulation.as_ref()
    }

    /// Update snap targets based on current snap mode.
    /// Should be called each frame when shape snapping is enabled.
    pub fn update_snap_targets(&mut self, canvas: &Canvas, snap_mode: SnapMode) {
        if snap_mode.snaps_to_shapes() {
            // Collect all snap targets, excluding any shapes being manipulated/moved
            let exclude_ids: Vec<ShapeId> = if let Some(m) = &self.manipulation {
                vec![m.shape_id]
            } else if let Some(mm) = &self.multi_move {
                mm.shape_ids()
            } else {
                vec![]
            };
            self.current_snap_targets = collect_snap_targets_excluding(canvas, &exclude_ids);
        } else {
            self.current_snap_targets.clear();
        }
    }

    /// Enter text editing mode for a shape.
    /// This updates both the local state and the Canvas's WidgetManager.
    /// Stores the top-left handle position and size to preserve anchor after editing.
    pub fn enter_text_edit(&mut self, canvas: &mut Canvas, id: ShapeId) {
        if let Some(shape) = canvas.document.get_shape(id) {
            // Store the actual top-left handle position
            self.text_edit_anchor = get_handles(shape).into_iter()
                .find(|h| matches!(h.kind, HandleKind::Corner(Corner::TopLeft)))
                .map(|h| h.position);
            // Store the current bounds size
            let bounds = shape.bounds();
            self.text_edit_size = Some((bounds.width(), bounds.height()));
        }
        self.editing_text = Some(id);
        canvas.enter_text_editing(id);
    }

    /// Exit text editing mode.
    /// This updates both the local state and the Canvas's WidgetManager.
    /// If the text is empty, the shape is deleted.
    /// Preserves top-left handle position using current size.
    pub fn exit_text_edit(&mut self, canvas: &mut Canvas) {
        if let Some(id) = self.editing_text {
            let should_delete = canvas.document.get_shape(id)
                .map(|shape| {
                    if let Shape::Text(text) = shape {
                        text.content.trim().is_empty()
                    } else {
                        false
                    }
                })
                .unwrap_or(false);
            
            if should_delete {
                canvas.remove_shape(id);
            } else if let Some(anchor) = self.text_edit_anchor {
                // Update position to keep anchor fixed with current size
                if let Some(Shape::Text(text)) = canvas.document.get_shape_mut(id) {
                    let bounds = text.bounds();
                    let half_w = bounds.width() / 2.0;
                    let half_h = bounds.height() / 2.0;
                    let rotation = text.rotation;
                    
                    if rotation.abs() > 0.001 {
                        let cos_r = rotation.cos();
                        let sin_r = rotation.sin();
                        let rot_x = -half_w * cos_r + half_h * sin_r;
                        let rot_y = -half_w * sin_r - half_h * cos_r;
                        text.position = Point::new(anchor.x - half_w - rot_x, anchor.y - half_h - rot_y);
                    } else {
                        text.position = anchor;
                    }
                }
            }
        }
        
        self.editing_text = None;
        self.text_edit_anchor = None;
        self.text_edit_size = None;
        canvas.exit_text_editing();
    }

    /// Check if currently editing text.
    #[allow(dead_code)]
    pub fn is_editing_text(&self) -> bool {
        self.editing_text.is_some()
    }

    /// Determine the cursor type based on hover position.
    /// Returns: None = default, Some(None) = move, Some(Some(handle)) = resize with direction
    pub fn get_cursor_for_position(&self, canvas: &Canvas, world_point: Point) -> Option<Option<HandleKind>> {
        let handle_tolerance = HANDLE_HIT_TOLERANCE / canvas.camera.zoom;
        let boundary_tolerance = 8.0 / canvas.camera.zoom;
        
        match canvas.tool_manager.current_tool {
            ToolKind::Text => {
                let hits = canvas.document.shapes_at_point(world_point, 5.0 / canvas.camera.zoom);
                if let Some(&id) = hits.first() {
                    if let Some(shape @ Shape::Text(_)) = canvas.document.get_shape(id) {
                        if let Some(handle) = hit_test_handles(shape, world_point, handle_tolerance) {
                            return Some(Some(handle));
                        }
                        if hit_test_boundary(shape, world_point, boundary_tolerance) {
                            return Some(None); // move
                        }
                    }
                }
            }
            ToolKind::Select => {
                for &shape_id in &canvas.selection {
                    if let Some(shape) = canvas.document.get_shape(shape_id) {
                        if let Some(handle) = hit_test_handles(shape, world_point, handle_tolerance) {
                            return Some(Some(handle));
                        }
                    }
                }
                let hits = canvas.document.shapes_at_point(world_point, 5.0 / canvas.camera.zoom);
                if !hits.is_empty() {
                    return Some(None); // move
                }
            }
            _ => {}
        }
        None
    }

    /// Handle a press event (mouse down).
    /// `snap_mode` controls whether the start point should snap to grid.
    pub fn handle_press(&mut self, canvas: &mut Canvas, world_point: Point, input: &InputState, snap_mode: SnapMode) {
        // If we're editing text and click elsewhere, stop editing
        if self.editing_text.is_some() {
            let hits = canvas.document.shapes_at_point(world_point, 5.0 / canvas.camera.zoom);
            let clicked_on_editing = hits.first().map(|&id| Some(id) == self.editing_text).unwrap_or(false);
            if !clicked_on_editing {
                self.exit_text_edit(canvas);
            } else {
                // Clicked on the text we're editing - stay in edit mode
                return;
            }
        }
        
        match canvas.tool_manager.current_tool {
            ToolKind::Text => {
                // Text tool: check if clicking on existing text
                let hits = canvas.document.shapes_at_point(world_point, 5.0 / canvas.camera.zoom);
                if let Some(&id) = hits.first() {
                    if let Some(shape @ Shape::Text(_)) = canvas.document.get_shape(id) {
                        let boundary_tolerance = 8.0 / canvas.camera.zoom;
                        let handle_tolerance = HANDLE_HIT_TOLERANCE / canvas.camera.zoom;
                        
                        // Check for handle hit first (for scaling)
                        if let Some(handle_kind) = hit_test_handles(shape, world_point, handle_tolerance) {
                            // Start handle manipulation (scale)
                            self.manipulation = Some(ManipulationState::new(
                                id,
                                Some(handle_kind),
                                world_point,
                                shape.clone(),
                            ));
                            canvas.clear_selection();
                            canvas.select(id);
                            return;
                        }
                        
                        // Check for boundary hit (for dragging)
                        if hit_test_boundary(shape, world_point, boundary_tolerance) {
                            // Start move operation
                            let mut original_shapes = std::collections::HashMap::new();
                            original_shapes.insert(id, shape.clone());
                            self.multi_move = Some(MultiMoveState::new(world_point, original_shapes));
                            canvas.clear_selection();
                            canvas.select(id);
                            return;
                        }
                        
                        // Click inside text - enter edit mode
                        self.enter_text_edit(canvas, id);
                        canvas.clear_selection();
                        canvas.select(id);
                        return;
                    }
                }
                // If not clicking on text, will create new text on release
            }
            ToolKind::Select => {
                // Check for double-click on text shape to enter edit mode
                if input.is_double_click() {
                    let hits = canvas.document.shapes_at_point(world_point, 5.0 / canvas.camera.zoom);
                    if let Some(&id) = hits.first() {
                        if let Some(Shape::Text(_)) = canvas.document.get_shape(id) {
                            // Double-click on text - enter edit mode
                            self.enter_text_edit(canvas, id);
                            canvas.clear_selection();
                            canvas.select(id);
                            return;
                        }
                    }
                    
                    // Check for double-click on rotation handle to reset rotation
                    let handle_tolerance = HANDLE_HIT_TOLERANCE / canvas.camera.zoom;
                    for &shape_id in &canvas.selection {
                        if let Some(shape) = canvas.document.get_shape(shape_id) {
                            if let Some(HandleKind::Rotate) = hit_test_handles(shape, world_point, handle_tolerance) {
                                // Double-click on rotation handle - reset to 0°
                                canvas.document.push_undo();
                                if let Some(shape) = canvas.document.get_shape_mut(shape_id) {
                                    shape.set_rotation(0.0);
                                }
                                return;
                            }
                        }
                    }
                }
                
                // First, check if we clicked on a handle of a selected shape
                let handle_tolerance = HANDLE_HIT_TOLERANCE / canvas.camera.zoom;
                
                for &shape_id in &canvas.selection {
                    if let Some(shape) = canvas.document.get_shape(shape_id) {
                        if let Some(handle_kind) = hit_test_handles(shape, world_point, handle_tolerance) {
                            // Start handle manipulation
                            self.manipulation = Some(ManipulationState::new(
                                shape_id,
                                Some(handle_kind),
                                world_point,
                                shape.clone(),
                            ));
                            return;
                        }
                    }
                }

                // Check for shape hit (for selection or move)
                let hits = canvas.document.shapes_at_point(world_point, 5.0 / canvas.camera.zoom);
                if let Some(&id) = hits.first() {
                    if input.shift() {
                        // Add to/toggle selection
                        if canvas.is_selected(id) {
                            canvas.selection.retain(|&s| s != id);
                        } else {
                            canvas.add_to_selection(id);
                        }
                    } else {
                        // If clicking on an already selected shape, start moving it
                        // Otherwise, replace selection
                        if !canvas.is_selected(id) {
                            canvas.clear_selection();
                            canvas.select(id);
                        }
                        
                        // Start move - use MultiMoveState for all selected shapes
                        let mut original_shapes = std::collections::HashMap::new();
                        for &shape_id in &canvas.selection {
                            if let Some(shape) = canvas.document.get_shape(shape_id) {
                                original_shapes.insert(shape_id, shape.clone());
                            }
                        }
                        
                        if !original_shapes.is_empty() {
                            // Alt/Option + drag = duplicate
                            if input.alt() {
                                let mut mm = MultiMoveState::new_duplicate(world_point, original_shapes);
                                // Create duplicates immediately with new IDs
                                for shape in mm.original_shapes.values() {
                                    let mut new_shape = shape.clone();
                                    new_shape.regenerate_id();
                                    let new_id = new_shape.id();
                                    mm.duplicated_ids.push(new_id);
                                    canvas.document.add_shape(new_shape);
                                }
                                // Update selection to the duplicates
                                canvas.clear_selection();
                                for &new_id in &mm.duplicated_ids {
                                    canvas.add_to_selection(new_id);
                                }
                                self.multi_move = Some(mm);
                            } else {
                                self.multi_move = Some(MultiMoveState::new(world_point, original_shapes));
                            }
                        }
                    }
                } else {
                    // Clicked on empty space - start selection rectangle
                    if !input.shift() {
                        canvas.clear_selection();
                    }
                    self.selection_rect = Some(SelectionRect {
                        start: world_point,
                        current: world_point,
                    });
                }
            }
            ToolKind::Pan => {
                // Pan is handled in mouse move
            }
            ToolKind::Freehand | ToolKind::Highlighter => {
                // Freehand/Highlighter doesn't snap - would be too jerky
                canvas.tool_manager.begin(world_point);
            }
            ToolKind::Eraser => {
                // Start eraser stroke
                self.eraser_points.clear();
                self.eraser_points.push(world_point);
            }
            ToolKind::LaserPointer => {
                // Laser pointer just updates position
                self.laser_position = Some(world_point);
                self.laser_trail.push((world_point, 1.0));
            }
            ToolKind::Line | ToolKind::Arrow => {
                // Start line/arrow drawing - snap start point if enabled
                // Store start point for angle snapping visualization
                let start_point = if snap_mode.is_enabled() {
                    let targets = collect_snap_targets(canvas, None);
                    let snap_result = snap_point_with_shapes(world_point, snap_mode, GRID_SIZE, &targets);
                    if snap_result.is_snapped() {
                        self.last_snap = Some(snap_result);
                    }
                    snap_result.point
                } else {
                    world_point
                };
                self.line_start_point = Some(start_point);
                canvas.tool_manager.begin(start_point);
            }
            _ => {
                // Start shape drawing - snap start point if enabled
                let start_point = if snap_mode.is_enabled() {
                    let targets = collect_snap_targets(canvas, None);
                    let snap_result = snap_point_with_shapes(world_point, snap_mode, GRID_SIZE, &targets);
                    if snap_result.is_snapped() {
                        self.last_snap = Some(snap_result);
                    }
                    snap_result.point
                } else {
                    world_point
                };
                self.line_start_point = None; // Clear for non-line tools
                canvas.tool_manager.begin(start_point);
            }
        }
    }

    /// Handle a release event (mouse up).
    /// `snap_mode` controls whether the end point should snap to grid.
    /// `angle_snap_enabled` enables 15° angle snapping for lines/arrows.
    pub fn handle_release(&mut self, canvas: &mut Canvas, world_point: Point, input: &InputState, current_style: &ShapeStyle, snap_mode: SnapMode, angle_snap_enabled: bool) {
        // Clear rotation state
        self.rotation_state = None;
        
        // If we were manipulating a single shape (handle resize), finalize it
        if let Some(manip) = self.manipulation.take() {
            // Check if this was a rotation operation
            if matches!(manip.handle, Some(HandleKind::Rotate)) {
                // Rotation was already applied during drag, just need to push undo
                // Get the current rotation from the shape
                if let Some(shape) = canvas.document.get_shape(manip.shape_id) {
                    let current_rotation = shape.rotation();
                    let original_rotation = manip.original_shape.rotation();
                    
                    // Only push undo if rotation actually changed
                    if (current_rotation - original_rotation).abs() > 0.001 {
                        // Restore original, push undo, then re-apply current rotation
                        if let Some(shape) = canvas.document.get_shape_mut(manip.shape_id) {
                            *shape = manip.original_shape.clone();
                        }
                        canvas.document.push_undo();
                        if let Some(shape) = canvas.document.get_shape_mut(manip.shape_id) {
                            shape.set_rotation(current_rotation);
                        }
                    }
                }
                return;
            }
            
            // Use manip.current_point which was already snapped during drag,
            // NOT the raw world_point which would cause a jump on release
            let delta = manip.delta();
            
            // Check if shape actually changed (delta is non-zero)
            if delta.x.abs() > 0.1 || delta.y.abs() > 0.1 {
                // Push undo state before finalizing (restore original, then re-apply)
                // First restore the original shape
                if let Some(shape) = canvas.document.get_shape_mut(manip.shape_id) {
                    *shape = manip.original_shape.clone();
                }
                // Now push undo and apply the final change
                canvas.document.push_undo();
                let new_shape = apply_manipulation(&manip.original_shape, manip.handle, delta, input.shift());
                if let Some(shape) = canvas.document.get_shape_mut(manip.shape_id) {
                    *shape = new_shape;
                }
            }
            return;
        }

        // If we were moving multiple shapes, finalize them
        if let Some(mm) = self.multi_move.take() {
            let delta = mm.delta();
            
            if mm.is_duplicate {
                // Duplicate mode: shapes were already created, just need to finalize their positions
                // Check if actually moved
                if delta.x.abs() > 0.1 || delta.y.abs() > 0.1 {
                    // Push undo for the duplicate operation
                    canvas.document.push_undo();
                    let translation = kurbo::Affine::translate(delta);
                    for (idx, &dup_id) in mm.duplicated_ids.iter().enumerate() {
                        let original_shape = mm.original_shapes.values().nth(idx);
                        if let Some(orig) = original_shape {
                            let mut new_shape = orig.clone();
                            new_shape.transform(translation);
                            if let Some(shape) = canvas.document.get_shape_mut(dup_id) {
                                *shape = new_shape;
                            }
                        }
                    }
                } else {
                    // No movement - remove the duplicates (cancelled)
                    for &dup_id in &mm.duplicated_ids {
                        canvas.document.remove_shape(dup_id);
                    }
                    // Restore original selection
                    canvas.clear_selection();
                    for &id in mm.original_shapes.keys() {
                        canvas.add_to_selection(id);
                    }
                }
            } else {
                // Normal move mode
                // Check if shapes actually moved (delta is non-zero)
                if delta.x.abs() > 0.1 || delta.y.abs() > 0.1 {
                    // First restore all original shapes
                    for (shape_id, original_shape) in &mm.original_shapes {
                        if let Some(shape) = canvas.document.get_shape_mut(*shape_id) {
                            *shape = original_shape.clone();
                        }
                    }
                    
                    // Now push undo and apply the final changes
                    canvas.document.push_undo();
                    let translation = kurbo::Affine::translate(delta);
                    for (shape_id, original_shape) in &mm.original_shapes {
                        let mut new_shape = original_shape.clone();
                        new_shape.transform(translation);
                        if let Some(shape) = canvas.document.get_shape_mut(*shape_id) {
                            *shape = new_shape;
                        }
                    }
                }
            }
            return;
        }

        // If we were doing marquee selection, finalize it
        if let Some(sel_rect) = self.selection_rect.take() {
            let rect = sel_rect.to_rect();
            // Only select if rectangle has meaningful size
            if rect.width() > 2.0 && rect.height() > 2.0 {
                let shapes_in_rect = canvas.document.shapes_in_rect(rect);
                if !input.shift() {
                    canvas.clear_selection();
                }
                for id in shapes_in_rect {
                    canvas.add_to_selection(id);
                }
            }
            return;
        }

        match canvas.tool_manager.current_tool {
            ToolKind::Select | ToolKind::Pan => {
                // Nothing to do
            }
            ToolKind::Freehand => {
                let points = canvas.tool_manager.freehand_points();
                if points.len() >= 2 {
                    let mut freehand = Freehand::from_points(points.to_vec());
                    freehand.simplify(2.0); // Simplify the path
                    freehand.style = current_style.clone(); // Apply current style
                    canvas.document.push_undo();
                    canvas
                        .document
                        .add_shape(Shape::Freehand(freehand));
                }
                canvas.tool_manager.cancel();
            }
            ToolKind::Highlighter => {
                let points = canvas.tool_manager.freehand_points();
                if points.len() >= 2 {
                    let mut freehand = Freehand::from_points(points.to_vec());
                    freehand.simplify(2.0);
                    // Highlighter: wider stroke, semi-transparent
                    freehand.style = current_style.clone();
                    freehand.style.stroke_width = current_style.stroke_width.max(12.0);
                    freehand.style.stroke_color.a = 128; // 50% opacity
                    canvas.document.push_undo();
                    canvas.document.add_shape(Shape::Freehand(freehand));
                }
                canvas.tool_manager.cancel();
            }
            ToolKind::Eraser => {
                // Finalize eraser stroke - erase shapes that intersect
                if !self.eraser_points.is_empty() {
                    self.apply_eraser(canvas);
                }
                self.eraser_points.clear();
            }
            ToolKind::LaserPointer => {
                // Laser pointer doesn't create anything, just clear position
                // Trail will fade out over time
            }
            ToolKind::Text => {
                // If we're already editing text, don't create new text
                if self.editing_text.is_some() {
                    return;
                }
                
                // Text tool: create text at click position with empty content
                let mut text = Text::new(world_point, String::new());
                text.style = current_style.clone();
                let shape = Shape::Text(text);
                let shape_id = shape.id();
                canvas.document.push_undo();
                canvas.document.add_shape(shape);
                // Enter edit mode for the new text
                canvas.clear_selection();
                canvas.add_to_selection(shape_id);
                self.enter_text_edit(canvas, shape_id);
                canvas.tool_manager.cancel();
            }
            ToolKind::Line | ToolKind::Arrow => {
                // Complete line/arrow - use shape snapping first, then angle/grid snapping
                let end_point = if let Some(start) = self.line_start_point {
                    // First try shape snapping (highest priority when enabled)
                    if snap_mode.snaps_to_shapes() {
                        let targets = collect_snap_targets(canvas, None);
                        let shape_snap = snap_point_with_shapes(world_point, SnapMode::Shapes, GRID_SIZE, &targets);
                        if shape_snap.is_snapped() {
                            shape_snap.point
                        } else {
                            // Fall back to angle/grid snapping
                            let angle_result = snap_line_endpoint_isometric(
                                start,
                                world_point,
                                angle_snap_enabled,
                                snap_mode.snaps_to_grid(),
                                false,
                                GRID_SIZE,
                            );
                            angle_result.point
                        }
                    } else {
                        // Only angle/grid snapping
                        let angle_result = snap_line_endpoint_isometric(
                            start,
                            world_point,
                            angle_snap_enabled,
                            snap_mode.snaps_to_grid(),
                            false,
                            GRID_SIZE,
                        );
                        angle_result.point
                    }
                } else if snap_mode.is_enabled() {
                    let targets = collect_snap_targets(canvas, None);
                    snap_point_with_shapes(world_point, snap_mode, GRID_SIZE, &targets).point
                } else {
                    world_point
                };
                
                // Clear line start point
                self.line_start_point = None;
                
                if let Some(mut shape) = canvas.tool_manager.end(end_point) {
                    // Only add if shape has meaningful size
                    let bounds = shape.bounds();
                    if bounds.width() > 1.0 || bounds.height() > 1.0 {
                        // Apply current style to the new shape
                        *shape.style_mut() = current_style.clone();
                        canvas.document.push_undo();
                        canvas.document.add_shape(shape);
                    }
                }
            }
            _ => {
                // Complete other shapes (Rectangle, Ellipse) - snap end point if enabled
                let end_point = if snap_mode.is_enabled() {
                    let targets = collect_snap_targets(canvas, None);
                    snap_point_with_shapes(world_point, snap_mode, GRID_SIZE, &targets).point
                } else {
                    world_point
                };
                
                if let Some(mut shape) = canvas.tool_manager.end(end_point) {
                    // Only add if shape has meaningful size
                    let bounds = shape.bounds();
                    if bounds.width() > 1.0 || bounds.height() > 1.0 {
                        // Apply current style to the new shape
                        *shape.style_mut() = current_style.clone();
                        canvas.document.push_undo();
                        canvas.document.add_shape(shape);
                    }
                }
            }
        }
    }

    /// Handle a move event while dragging.
    /// `snap_mode` controls whether points should snap to grid.
    /// `angle_snap_enabled` enables 15° angle snapping for lines/arrows.
    pub fn handle_drag(&mut self, canvas: &mut Canvas, world_point: Point, input: &InputState, snap_mode: SnapMode, angle_snap_enabled: bool) {
        // Clear previous snap
        self.last_snap = None;
        self.last_angle_snap = None;
        
        // If we're manipulating a shape, update it
        if let Some(manip) = &mut self.manipulation {
            // Check if this is a rotation handle
            if matches!(manip.handle, Some(HandleKind::Rotate)) {
                // Handle rotation - Shift key snaps to 15° increments
                let snap_to_15deg = input.shift();
                let center = manip.original_shape.bounds().center();
                
                // Apply rotation to the shape
                if let Some(shape) = canvas.document.get_shape_mut(manip.shape_id) {
                    let angle = apply_rotation(shape, world_point, snap_to_15deg);
                    
                    // Update rotation state for helper line rendering
                    self.rotation_state = Some(RotationState {
                        center,
                        angle,
                        snapped: snap_to_15deg,
                    });
                }
                
                // Update current_point for delta calculation on release
                manip.current_point = world_point;
                return;
            }
            
            // Check if we're manipulating a line or arrow endpoint
            let is_line_or_arrow = matches!(manip.original_shape, Shape::Line(_) | Shape::Arrow(_));
            
            // Calculate raw delta from cursor movement
            let raw_delta = kurbo::Vec2::new(
                world_point.x - manip.start_point.x,
                world_point.y - manip.start_point.y,
            );
            
            // Get the original handle/shape position that's being manipulated
            let original_position = get_manipulation_target_position(&manip.original_shape, manip.handle);
            
            // Calculate where the handle/shape would end up
            let target_position = Point::new(
                original_position.x + raw_delta.x,
                original_position.y + raw_delta.y,
            );
            
            // For line/arrow endpoint manipulation, try shape snapping first (highest priority)
            // then fall back to angle/grid snapping
            let snap_result = if is_line_or_arrow && manip.handle.is_some() {
                // Get the other endpoint as the origin for polar snapping
                let other_endpoint = get_line_other_endpoint(&manip.original_shape, manip.handle);
                
                // First try shape snapping (highest priority when enabled)
                if snap_mode.snaps_to_shapes() {
                    let targets = collect_snap_targets(canvas, Some(manip.shape_id));
                    let shape_snap = snap_point_with_shapes(target_position, SnapMode::Shapes, GRID_SIZE, &targets);
                    if shape_snap.is_snapped() {
                        self.last_snap = Some(shape_snap);
                        self.last_angle_snap = None;
                        self.line_start_point = None;
                        shape_snap
                    } else if angle_snap_enabled {
                        // Fall back to angle/grid snapping
                        let angle_result = snap_line_endpoint_isometric(
                            other_endpoint,
                            target_position,
                            angle_snap_enabled,
                            snap_mode.snaps_to_grid(),
                            false,
                            GRID_SIZE,
                        );
                        
                        if angle_result.snapped {
                            self.last_angle_snap = Some(angle_result);
                            self.line_start_point = Some(other_endpoint);
                        }
                        
                        SnapResult {
                            point: angle_result.point,
                            snapped_x: angle_result.snapped,
                            snapped_y: angle_result.snapped,
                        }
                    } else {
                        // Just grid snapping
                        let targets = collect_snap_targets(canvas, Some(manip.shape_id));
                        snap_point_with_shapes(target_position, snap_mode, GRID_SIZE, &targets)
                    }
                } else if angle_snap_enabled {
                    // Only angle/grid snapping (no shape snap)
                    let angle_result = snap_line_endpoint_isometric(
                        other_endpoint,
                        target_position,
                        angle_snap_enabled,
                        snap_mode.snaps_to_grid(),
                        false,
                        GRID_SIZE,
                    );
                    
                    if angle_result.snapped {
                        self.last_angle_snap = Some(angle_result);
                        self.line_start_point = Some(other_endpoint);
                    }
                    
                    SnapResult {
                        point: angle_result.point,
                        snapped_x: angle_result.snapped,
                        snapped_y: angle_result.snapped,
                    }
                } else {
                    // Just grid snapping for endpoints
                    let targets = collect_snap_targets(canvas, Some(manip.shape_id));
                    snap_point_with_shapes(target_position, snap_mode, GRID_SIZE, &targets)
                }
            } else {
                // Regular snapping for non-line shapes (grid and/or shapes)
                let targets = collect_snap_targets(canvas, Some(manip.shape_id));
                snap_point_with_shapes(target_position, snap_mode, GRID_SIZE, &targets)
            };
            
            // Store snap result for visual guides (show where handle snaps to)
            if snap_result.is_snapped() && self.last_angle_snap.is_none() {
                self.last_snap = Some(snap_result);
            }
            
            // Calculate adjusted delta so that original + adjusted_delta = snapped_position
            let adjusted_delta = kurbo::Vec2::new(
                snap_result.point.x - original_position.x,
                snap_result.point.y - original_position.y,
            );
            
            // Update current_point to reflect the adjusted delta
            manip.current_point = Point::new(
                manip.start_point.x + adjusted_delta.x,
                manip.start_point.y + adjusted_delta.y,
            );
            
            // Apply manipulation preview to the shape
            let new_shape = apply_manipulation(&manip.original_shape, manip.handle, adjusted_delta, input.shift());
            if let Some(shape) = canvas.document.get_shape_mut(manip.shape_id) {
                *shape = new_shape;
            }
            return;
        }

        // If we're moving multiple shapes, update all of them
        if let Some(mm) = &mut self.multi_move {
            // Calculate raw delta from cursor movement
            let raw_delta = kurbo::Vec2::new(
                world_point.x - mm.start_point.x,
                world_point.y - mm.start_point.y,
            );
            
            // For multi-move, use the first shape's top-left as reference for snapping
            let reference_shape = mm.original_shapes.values().next();
            let snap_result = if let Some(ref_shape) = reference_shape {
                let original_position = ref_shape.bounds().origin();
                let target_position = Point::new(
                    original_position.x + raw_delta.x,
                    original_position.y + raw_delta.y,
                );
                
                let exclude_ids: Vec<ShapeId> = mm.original_shapes.keys().copied().collect();
                let targets = collect_snap_targets_excluding(canvas, &exclude_ids);
                let snap_result = snap_point_with_shapes(target_position, snap_mode, GRID_SIZE, &targets);
                
                if snap_result.is_snapped() {
                    self.last_snap = Some(snap_result);
                }
                
                // Calculate adjusted delta based on snapped position
                kurbo::Vec2::new(
                    snap_result.point.x - original_position.x,
                    snap_result.point.y - original_position.y,
                )
            } else {
                raw_delta
            };
            
            // Update current_point
            mm.current_point = Point::new(
                mm.start_point.x + snap_result.x,
                mm.start_point.y + snap_result.y,
            );
            
            // Apply movement to shapes
            let translation = kurbo::Affine::translate(snap_result);
            if mm.is_duplicate {
                // For duplicate, move the duplicated shapes (originals stay in place)
                for (idx, &dup_id) in mm.duplicated_ids.iter().enumerate() {
                    // Get the corresponding original shape
                    let original_shape = mm.original_shapes.values().nth(idx);
                    if let (Some(orig), Some(shape)) = (original_shape, canvas.document.get_shape_mut(dup_id)) {
                        let mut new_shape = orig.clone();
                        new_shape.transform(translation);
                        *shape = new_shape;
                    }
                }
            } else {
                // Normal move - move the original shapes
                for (shape_id, original_shape) in &mm.original_shapes {
                    let mut new_shape = original_shape.clone();
                    new_shape.transform(translation);
                    if let Some(shape) = canvas.document.get_shape_mut(*shape_id) {
                        *shape = new_shape;
                    }
                }
            }
            return;
        }

        // If we're doing marquee selection, update the rectangle (no snapping for selection)
        if let Some(sel_rect) = &mut self.selection_rect {
            sel_rect.current = world_point;
            return;
        }

        // Handle eraser tool
        if canvas.tool_manager.current_tool == ToolKind::Eraser && !self.eraser_points.is_empty() {
            self.eraser_points.push(world_point);
            return;
        }

        // Handle laser pointer tool
        if canvas.tool_manager.current_tool == ToolKind::LaserPointer {
            self.laser_position = Some(world_point);
            self.laser_trail.push((world_point, 1.0));
            // Keep trail limited
            if self.laser_trail.len() > 50 {
                self.laser_trail.remove(0);
            }
            return;
        }

        // Update tool manager (handles freehand point accumulation internally)
        if canvas.tool_manager.is_active() {
            let tool = canvas.tool_manager.current_tool;
            
            // Special handling for Line and Arrow tools with angle snapping
            if matches!(tool, ToolKind::Line | ToolKind::Arrow) {
                if let Some(start) = self.line_start_point {
                    // First try shape snapping (highest priority when enabled)
                    if snap_mode.snaps_to_shapes() {
                        let targets = collect_snap_targets(canvas, None);
                        let shape_snap = snap_point_with_shapes(world_point, SnapMode::Shapes, GRID_SIZE, &targets);
                        if shape_snap.is_snapped() {
                            self.last_snap = Some(shape_snap);
                            self.last_angle_snap = None;
                            canvas.tool_manager.update(shape_snap.point);
                            return;
                        }
                    }
                    
                    // Use angle-aware snapping (grid + angle)
                    let angle_result = snap_line_endpoint_isometric(
                        start,
                        world_point,
                        angle_snap_enabled,
                        snap_mode.snaps_to_grid(),
                        false, // unused parameter kept for API compatibility
                        GRID_SIZE,
                    );
                    
                    // Store for visualization
                    if angle_result.snapped {
                        self.last_angle_snap = Some(angle_result);
                    }
                    
                    canvas.tool_manager.update(angle_result.point);
                    return;
                }
            }
            
            // Apply snapping for other shape creation tools (except freehand/highlighter)
            let point = if snap_mode.is_enabled() && !matches!(tool, ToolKind::Freehand | ToolKind::Highlighter) {
                let targets = collect_snap_targets(canvas, None);
                let snap_result = snap_point_with_shapes(world_point, snap_mode, GRID_SIZE, &targets);
                if snap_result.is_snapped() {
                    self.last_snap = Some(snap_result);
                }
                snap_result.point
            } else {
                world_point
            };
            canvas.tool_manager.update(point);
        }
    }
    
    /// Clear the last snap result (call when dragging ends).
    pub fn clear_snap(&mut self) {
        self.last_snap = None;
        self.last_angle_snap = None;
        self.line_start_point = None;
    }

    /// Apply eraser to shapes that intersect with the eraser path.
    fn apply_eraser(&mut self, canvas: &mut Canvas) {
        if self.eraser_points.len() < 2 {
            return;
        }

        let radius = self.eraser_radius;
        let mut shapes_to_remove: Vec<ShapeId> = Vec::new();
        let mut shapes_to_add: Vec<Shape> = Vec::new();

        // Check each freehand shape for intersection with eraser path
        for shape in canvas.document.shapes_ordered() {
            if let Shape::Freehand(freehand) = shape {
                let result = self.erase_from_freehand(freehand, radius);
                match result {
                    EraseResult::Unchanged => {}
                    EraseResult::Remove => {
                        shapes_to_remove.push(freehand.id());
                    }
                    EraseResult::Split(new_shapes) => {
                        shapes_to_remove.push(freehand.id());
                        shapes_to_add.extend(new_shapes);
                    }
                }
            }
        }

        // Apply changes
        if !shapes_to_remove.is_empty() || !shapes_to_add.is_empty() {
            canvas.document.push_undo();
            for id in shapes_to_remove {
                canvas.document.remove_shape(id);
            }
            for shape in shapes_to_add {
                canvas.document.add_shape(shape);
            }
        }
    }

    /// Check if a freehand shape intersects with the eraser path and return the result.
    fn erase_from_freehand(&self, freehand: &Freehand, radius: f64) -> EraseResult {
        if freehand.points.len() < 2 {
            return EraseResult::Unchanged;
        }

        // Find segments that are NOT erased (i.e., not within radius of eraser path)
        let mut segments: Vec<Vec<Point>> = Vec::new();
        let mut current_segment: Vec<Point> = Vec::new();

        for point in &freehand.points {
            let is_erased = self.point_near_eraser_path(*point, radius);
            
            if is_erased {
                // End current segment if it has points
                if current_segment.len() >= 2 {
                    segments.push(std::mem::take(&mut current_segment));
                } else {
                    current_segment.clear();
                }
            } else {
                current_segment.push(*point);
            }
        }

        // Don't forget the last segment
        if current_segment.len() >= 2 {
            segments.push(current_segment);
        }

        // Determine result
        if segments.is_empty() {
            EraseResult::Remove
        } else if segments.len() == 1 && segments[0].len() == freehand.points.len() {
            EraseResult::Unchanged
        } else {
            // Create new freehand shapes from remaining segments
            let new_shapes: Vec<Shape> = segments
                .into_iter()
                .map(|points| {
                    let mut new_freehand = Freehand::from_points(points);
                    new_freehand.style = freehand.style.clone();
                    Shape::Freehand(new_freehand)
                })
                .collect();
            EraseResult::Split(new_shapes)
        }
    }

    /// Check if a point is within radius of the eraser path.
    fn point_near_eraser_path(&self, point: Point, radius: f64) -> bool {
        for window in self.eraser_points.windows(2) {
            let start = window[0];
            let end = window[1];
            
            // Distance from point to line segment
            let line_vec = kurbo::Vec2::new(end.x - start.x, end.y - start.y);
            let point_vec = kurbo::Vec2::new(point.x - start.x, point.y - start.y);
            
            let line_len_sq = line_vec.hypot2();
            if line_len_sq < f64::EPSILON {
                // Segment is a point
                let dist = ((point.x - start.x).powi(2) + (point.y - start.y).powi(2)).sqrt();
                if dist <= radius {
                    return true;
                }
                continue;
            }
            
            let t = (point_vec.dot(line_vec) / line_len_sq).clamp(0.0, 1.0);
            let projection = Point::new(start.x + t * line_vec.x, start.y + t * line_vec.y);
            let dist = ((point.x - projection.x).powi(2) + (point.y - projection.y).powi(2)).sqrt();
            
            if dist <= radius {
                return true;
            }
        }
        false
    }

    /// Update laser trail (fade out old points). Call each frame.
    pub fn update_laser_trail(&mut self, dt: f64) {
        // Fade out trail points
        for (_, alpha) in &mut self.laser_trail {
            *alpha -= dt * 3.0; // Fade over ~0.33 seconds
        }
        // Remove fully faded points
        self.laser_trail.retain(|(_, alpha)| *alpha > 0.0);
    }

    /// Get the current eraser path for rendering.
    pub fn eraser_path(&self) -> &[Point] {
        &self.eraser_points
    }
}

/// Result of erasing from a freehand shape.
enum EraseResult {
    /// Shape was not affected.
    Unchanged,
    /// Shape should be completely removed.
    Remove,
    /// Shape should be split into new shapes.
    Split(Vec<Shape>),
}

impl Default for EventHandler {
    fn default() -> Self {
        Self::new()
    }
}
