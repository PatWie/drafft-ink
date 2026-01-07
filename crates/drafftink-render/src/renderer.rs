//! Renderer trait abstraction.

use drafftink_core::canvas::Canvas;
use drafftink_core::shapes::Shape;
use drafftink_core::snap::SnapTarget;
use kurbo::{Affine, Rect, Size};
use peniko::Color;
use thiserror::Error;

/// Renderer errors.
#[derive(Debug, Error)]
pub enum RendererError {
    #[error("Initialization failed: {0}")]
    InitFailed(String),
    #[error("Render failed: {0}")]
    RenderFailed(String),
    #[error("Surface error: {0}")]
    Surface(String),
}

/// Result type for renderer operations.
#[allow(dead_code)]
pub type RenderResult<T> = Result<T, RendererError>;

/// Grid display style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GridStyle {
    /// No grid (plain white background).
    None,
    /// Full grid lines.
    #[default]
    Lines,
    /// Horizontal lines only.
    HorizontalLines,
    /// Only corner crosses (+).
    CrossPlus,
    /// Only corner dots (.).
    Dots,
}

impl GridStyle {
    /// Cycle to the next grid style.
    pub fn next(self) -> Self {
        match self {
            GridStyle::None => GridStyle::Lines,
            GridStyle::Lines => GridStyle::HorizontalLines,
            GridStyle::HorizontalLines => GridStyle::CrossPlus,
            GridStyle::CrossPlus => GridStyle::Dots,
            GridStyle::Dots => GridStyle::None,
        }
    }

    /// Get display name for this grid style.
    pub fn name(self) -> &'static str {
        match self {
            GridStyle::None => "None",
            GridStyle::Lines => "Grid",
            GridStyle::HorizontalLines => "Lines",
            GridStyle::CrossPlus => "Crosses",
            GridStyle::Dots => "Dots",
        }
    }
}

/// Angle snap visualization info.
#[derive(Debug, Clone, Copy)]
pub struct AngleSnapInfo {
    /// Start point of the line being drawn.
    pub start_point: kurbo::Point,
    /// Current end point (snapped).
    pub end_point: kurbo::Point,
    /// Snapped angle in degrees.
    pub angle_degrees: f64,
    /// Whether angle snapping is active.
    pub is_snapped: bool,
}

/// Rotation visualization info (for rotation helper lines).
#[derive(Debug, Clone, Copy)]
pub struct RotationInfo {
    /// Center of the shape being rotated.
    pub center: kurbo::Point,
    /// Current rotation angle in radians.
    pub angle: f64,
    /// Whether snapping to 15° increments.
    pub snapped: bool,
}

/// Context for a single render frame.
pub struct RenderContext<'a> {
    /// The canvas to render.
    pub canvas: &'a Canvas,
    /// Viewport size in physical pixels.
    pub viewport_size: Size,
    /// Device pixel ratio (for HiDPI).
    pub scale_factor: f64,
    /// Background color.
    pub background_color: Color,
    /// Grid display style.
    pub grid_style: GridStyle,
    /// Selection highlight color.
    pub selection_color: Color,
    /// Selection rectangle (marquee) in world coordinates.
    pub selection_rect: Option<Rect>,
    /// Shape ID currently being edited (skip rendering in build_scene).
    pub editing_shape_id: Option<drafftink_core::shapes::ShapeId>,
    /// Snap point for rendering guides (in world coordinates).
    pub snap_point: Option<kurbo::Point>,
    /// Angle snap visualization info (for lines/arrows).
    pub angle_snap_info: Option<AngleSnapInfo>,
    /// Nearby snap targets to highlight (when shape snapping is enabled).
    pub nearby_snap_targets: Vec<SnapTarget>,
    /// Rotation visualization info (for rotation helper lines).
    pub rotation_info: Option<RotationInfo>,
    /// Eraser cursor position and radius.
    pub eraser_cursor: Option<(kurbo::Point, f64)>,
    /// Laser pointer position and trail.
    pub laser_pointer: Option<(kurbo::Point, Vec<(kurbo::Point, f64)>)>,
}

impl<'a> RenderContext<'a> {
    /// Create a new render context.
    pub fn new(canvas: &'a Canvas, viewport_size: Size) -> Self {
        Self {
            canvas,
            viewport_size,
            scale_factor: 1.0,
            background_color: Color::from_rgba8(250, 250, 250, 255),
            grid_style: GridStyle::Lines,
            selection_color: Color::from_rgba8(59, 130, 246, 255), // Blue
            selection_rect: None,
            editing_shape_id: None,
            snap_point: None,
            angle_snap_info: None,
            nearby_snap_targets: Vec::new(),
            rotation_info: None,
            eraser_cursor: None,
            laser_pointer: None,
        }
    }

    /// Set the scale factor for HiDPI.
    pub fn with_scale_factor(mut self, scale_factor: f64) -> Self {
        self.scale_factor = scale_factor;
        self
    }

    /// Set the background color.
    pub fn with_background(mut self, color: Color) -> Self {
        self.background_color = color;
        self
    }

    /// Set the grid style.
    pub fn with_grid(mut self, style: GridStyle) -> Self {
        self.grid_style = style;
        self
    }

    /// Set the selection rectangle.
    pub fn with_selection_rect(mut self, rect: Option<Rect>) -> Self {
        self.selection_rect = rect;
        self
    }

    /// Set the shape ID being edited (will be skipped in build_scene).
    pub fn with_editing_shape(mut self, shape_id: Option<drafftink_core::shapes::ShapeId>) -> Self {
        self.editing_shape_id = shape_id;
        self
    }

    /// Set the snap point for rendering guides.
    pub fn with_snap_point(mut self, point: Option<kurbo::Point>) -> Self {
        self.snap_point = point;
        self
    }

    /// Set the angle snap info for rendering angle guides.
    pub fn with_angle_snap(mut self, info: Option<AngleSnapInfo>) -> Self {
        self.angle_snap_info = info;
        self
    }

    /// Set nearby snap targets to highlight.
    pub fn with_snap_targets(mut self, targets: Vec<SnapTarget>) -> Self {
        self.nearby_snap_targets = targets;
        self
    }

    /// Set rotation info for rendering rotation helper lines.
    pub fn with_rotation_info(mut self, info: Option<RotationInfo>) -> Self {
        self.rotation_info = info;
        self
    }

    /// Set eraser cursor position and radius.
    pub fn with_eraser_cursor(mut self, cursor: Option<(kurbo::Point, f64)>) -> Self {
        self.eraser_cursor = cursor;
        self
    }

    /// Set laser pointer position and trail.
    pub fn with_laser_pointer(
        mut self,
        pointer: Option<(kurbo::Point, Vec<(kurbo::Point, f64)>)>,
    ) -> Self {
        self.laser_pointer = pointer;
        self
    }
}

/// Trait for rendering backends.
///
/// Implementations can use Vello, wgpu directly, or other rendering engines.
pub trait Renderer: Send + Sync {
    /// Build the scene/command buffer for a frame.
    ///
    /// This method is called once per frame and should prepare all drawing commands.
    fn build_scene(&mut self, ctx: &RenderContext);

    /// Get the background color (for clearing).
    fn background_color(&self, ctx: &RenderContext) -> Color {
        ctx.background_color
    }
}

/// Helper trait for shape rendering (used internally by renderers).
#[allow(dead_code)]
pub trait ShapeRenderer {
    /// Render a shape with the given transform and style.
    fn render_shape(&mut self, shape: &Shape, transform: Affine, selected: bool);

    /// Render a grid pattern.
    fn render_grid(&mut self, viewport: Rect, transform: Affine, grid_size: f64);

    /// Render selection handles for a shape.
    fn render_selection_handles(&mut self, bounds: Rect, transform: Affine);
}
