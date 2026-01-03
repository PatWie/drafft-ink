//! Tool system for the whiteboard.

use crate::shapes::{Shape, ShapeStyle};
use kurbo::Point;
use serde::{Deserialize, Serialize};

/// Generate a random seed for new tool interactions.
/// Uses a simple counter + hash approach that works on all platforms including WASM.
fn generate_tool_seed() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    
    static SEED_COUNTER: AtomicU32 = AtomicU32::new(1);
    
    let counter = SEED_COUNTER.fetch_add(1, Ordering::Relaxed);
    
    // Mix the counter with constants for better distribution (splitmix32-like)
    let mut x = counter.wrapping_mul(0x9E3779B9);
    x ^= x >> 16;
    x = x.wrapping_mul(0x85EBCA6B);
    x ^= x >> 13;
    x = x.wrapping_mul(0xC2B2AE35);
    x ^= x >> 16;
    x
}

/// Available tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ToolKind {
    #[default]
    Select,
    Pan,
    Rectangle,
    Ellipse,
    Line,
    Arrow,
    Freehand,
    Highlighter,
    Eraser,
    Text,
    LaserPointer,
}

/// State of a tool interaction.
#[derive(Debug, Clone)]
#[derive(Default)]
pub enum ToolState {
    /// Tool is idle, waiting for interaction.
    #[default]
    Idle,
    /// Tool is actively being used (e.g., drawing a shape).
    Active {
        /// Starting point of the interaction.
        start: Point,
        /// Current point of the interaction.
        current: Point,
        /// Preview shape being drawn.
        preview: Option<Shape>,
        /// Seed for hand-drawn effect (generated once at start, stable during drawing).
        seed: u32,
    },
}


/// Manages the current tool and its state.
#[derive(Debug, Clone, Default)]
pub struct ToolManager {
    /// Currently selected tool.
    pub current_tool: ToolKind,
    /// Current state of the tool.
    pub state: ToolState,
    /// Accumulated points for freehand drawing.
    freehand_points: Vec<Point>,
    /// Accumulated pressure values for freehand drawing.
    freehand_pressures: Vec<f64>,
    /// Last point timestamp for velocity calculation.
    last_point_time: Option<std::time::Instant>,
    /// Last point position for velocity calculation.
    last_point_pos: Option<Point>,
    /// Current style to apply to new shapes.
    pub current_style: ShapeStyle,
    /// Corner radius for new rectangles (0 = sharp corners).
    pub corner_radius: f64,
    /// Calligraphy mode for freehand (MSD smoothing).
    pub calligraphy_mode: bool,
    /// Pressure simulation mode (varies width based on speed).
    pub pressure_simulation: bool,
    /// MSD brush position (mass position).
    msd_pos: Point,
    /// MSD brush velocity.
    msd_vel: Point,
}

impl ToolManager {
    /// Create a new tool manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the current tool.
    pub fn set_tool(&mut self, tool: ToolKind) {
        self.current_tool = tool;
        self.state = ToolState::Idle;
    }

    /// Begin a tool interaction.
    pub fn begin(&mut self, point: Point) {
        // Clear and start freehand points if using freehand tool
        if self.current_tool == ToolKind::Freehand || self.current_tool == ToolKind::Highlighter {
            self.freehand_points.clear();
            self.freehand_pressures.clear();
            self.freehand_points.push(point);
            self.freehand_pressures.push(1.0); // Start with full pressure
            self.last_point_time = Some(std::time::Instant::now());
            self.last_point_pos = Some(point);
            // Initialize MSD state
            self.msd_pos = point;
            self.msd_vel = Point::ZERO;
        }
        
        self.state = ToolState::Active {
            start: point,
            current: point,
            preview: None,
            seed: generate_tool_seed(),
        };
    }

    /// Update the current interaction.
    pub fn update(&mut self, point: Point) {
        if let ToolState::Active { current, .. } = &mut self.state {
            *current = point;
            
            // Accumulate points for freehand
            if self.current_tool == ToolKind::Freehand || self.current_tool == ToolKind::Highlighter {
                if self.calligraphy_mode {
                    // MSD simulation: brush follows mouse with inertia
                    const M: f64 = 0.5;   // Mass
                    const K: f64 = 120.0; // Stiffness
                    const C: f64 = 15.49; // Critical damping: 2 * sqrt(M * K) ≈ 2 * sqrt(60)
                    const DT: f64 = 1.0 / 60.0; // ~60fps
                    
                    // Force = spring + damping
                    let dx = point.x - self.msd_pos.x;
                    let dy = point.y - self.msd_pos.y;
                    let ax = (K * dx - C * self.msd_vel.x) / M;
                    let ay = (K * dy - C * self.msd_vel.y) / M;
                    
                    // Integrate
                    self.msd_vel.x += ax * DT;
                    self.msd_vel.y += ay * DT;
                    self.msd_pos.x += self.msd_vel.x * DT;
                    self.msd_pos.y += self.msd_vel.y * DT;
                    
                    // Only add point if moved enough
                    if let Some(last) = self.freehand_points.last() {
                        let dist = ((self.msd_pos.x - last.x).powi(2) + (self.msd_pos.y - last.y).powi(2)).sqrt();
                        if dist > 2.0 {
                            let pressure = self.compute_pressure(self.msd_pos);
                            self.freehand_points.push(self.msd_pos);
                            self.freehand_pressures.push(pressure);
                        }
                    }
                } else {
                    let pressure = self.compute_pressure(point);
                    self.freehand_points.push(point);
                    self.freehand_pressures.push(pressure);
                }
            }
        }
    }

    /// Compute pressure based on drawing velocity.
    /// Fast movement = low pressure (thin line), slow movement = high pressure (thick line).
    fn compute_pressure(&mut self, point: Point) -> f64 {
        if !self.pressure_simulation {
            return 1.0;
        }

        let now = std::time::Instant::now();
        let pressure = if let (Some(last_time), Some(last_pos)) = (self.last_point_time, self.last_point_pos) {
            let dt = now.duration_since(last_time).as_secs_f64();
            if dt > 0.0 {
                let dist = ((point.x - last_pos.x).powi(2) + (point.y - last_pos.y).powi(2)).sqrt();
                let velocity = dist / dt;
                
                // Map velocity to pressure: slow = high pressure, fast = low pressure
                // Typical drawing velocity range: 0-2000 pixels/second
                // Use exponential decay for natural feel
                let normalized_velocity = (velocity / 800.0).min(3.0);
                let pressure = (-normalized_velocity).exp();
                pressure.clamp(0.2, 1.0) // Keep minimum pressure at 20%
            } else {
                1.0
            }
        } else {
            1.0
        };

        self.last_point_time = Some(now);
        self.last_point_pos = Some(point);
        pressure
    }

    /// End the current interaction and return any created shape.
    pub fn end(&mut self, point: Point) -> Option<Shape> {
        if let ToolState::Active { start, seed, .. } = &self.state {
            let start = *start;
            let seed = *seed;
            let shape = self.create_shape_with_seed(start, point, seed);
            self.state = ToolState::Idle;
            self.freehand_points.clear();
            shape
        } else {
            None
        }
    }

    /// Cancel the current interaction.
    pub fn cancel(&mut self) {
        self.state = ToolState::Idle;
        self.freehand_points.clear();
        self.freehand_pressures.clear();
        self.last_point_time = None;
        self.last_point_pos = None;
    }

    /// Check if a tool interaction is active.
    pub fn is_active(&self) -> bool {
        matches!(self.state, ToolState::Active { .. })
    }

    /// Get the preview shape for the current interaction.
    pub fn preview_shape(&self) -> Option<Shape> {
        if let ToolState::Active { start, current, seed, .. } = &self.state {
            // For freehand/highlighter, use the accumulated points
            if self.current_tool == ToolKind::Freehand || self.current_tool == ToolKind::Highlighter {
                return self.create_freehand_preview(*seed);
            }
            self.create_shape_with_seed(*start, *current, *seed)
        } else {
            None
        }
    }

    /// Create a freehand preview from accumulated points.
    fn create_freehand_preview(&self, seed: u32) -> Option<Shape> {
        use crate::shapes::Freehand;
        
        if self.freehand_points.len() >= 2 {
            let mut freehand = if self.pressure_simulation && !self.freehand_pressures.is_empty() {
                Freehand::from_points_with_pressure(
                    self.freehand_points.clone(),
                    self.freehand_pressures.clone(),
                )
            } else {
                Freehand::from_points(self.freehand_points.clone())
            };
            freehand.style = self.current_style.clone();
            freehand.style.seed = seed;
            Some(Shape::Freehand(freehand))
        } else {
            None
        }
    }

    /// Get the accumulated freehand points (for final shape creation).
    pub fn freehand_points(&self) -> &[Point] {
        &self.freehand_points
    }

    /// Get the accumulated freehand pressure values.
    pub fn freehand_pressures(&self) -> &[f64] {
        &self.freehand_pressures
    }

    /// Create a shape from start and end points with a specific seed.
    fn create_shape_with_seed(&self, start: Point, end: Point, seed: u32) -> Option<Shape> {
        use crate::shapes::{Arrow, Ellipse, Line, Rectangle, Text};

        let mut shape = match self.current_tool {
            ToolKind::Rectangle => {
                let mut rect = Rectangle::from_corners(start, end);
                rect.corner_radius = self.corner_radius;
                Some(Shape::Rectangle(rect))
            }
            ToolKind::Ellipse => {
                let rect = kurbo::Rect::new(
                    start.x.min(end.x),
                    start.y.min(end.y),
                    start.x.max(end.x),
                    start.y.max(end.y),
                );
                Some(Shape::Ellipse(Ellipse::from_rect(rect)))
            }
            ToolKind::Line => Some(Shape::Line(Line::new(start, end))),
            ToolKind::Arrow => Some(Shape::Arrow(Arrow::new(start, end))),
            ToolKind::Freehand | ToolKind::Highlighter => {
                // Use accumulated points for freehand/highlighter
                return self.create_freehand_preview(seed);
            }
            ToolKind::Text => {
                // Text is created at the click position with empty content
                Some(Shape::Text(Text::new(start, String::new())))
            }
            ToolKind::Select | ToolKind::Pan | ToolKind::Eraser | ToolKind::LaserPointer => None,
        };
        
        // Apply current style to the shape with the stable seed
        if let Some(ref mut s) = shape {
            let mut style = self.current_style.clone();
            style.seed = seed;
            *s.style_mut() = style;
        }
        
        shape
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_selection() {
        let mut tm = ToolManager::new();
        assert_eq!(tm.current_tool, ToolKind::Select);

        tm.set_tool(ToolKind::Rectangle);
        assert_eq!(tm.current_tool, ToolKind::Rectangle);
    }

    #[test]
    fn test_tool_interaction() {
        let mut tm = ToolManager::new();
        tm.set_tool(ToolKind::Rectangle);

        assert!(!tm.is_active());

        tm.begin(Point::new(0.0, 0.0));
        assert!(tm.is_active());

        tm.update(Point::new(50.0, 50.0));

        let preview = tm.preview_shape();
        assert!(preview.is_some());

        let shape = tm.end(Point::new(100.0, 100.0));
        assert!(shape.is_some());
        assert!(!tm.is_active());
    }

    #[test]
    fn test_cancel_interaction() {
        let mut tm = ToolManager::new();
        tm.set_tool(ToolKind::Rectangle);

        tm.begin(Point::new(0.0, 0.0));
        assert!(tm.is_active());

        tm.cancel();
        assert!(!tm.is_active());
    }

    #[test]
    fn test_select_tool_no_shape() {
        let mut tm = ToolManager::new();
        tm.set_tool(ToolKind::Select);

        tm.begin(Point::new(0.0, 0.0));
        let shape = tm.end(Point::new(100.0, 100.0));
        assert!(shape.is_none());
    }
}
