//! Thresholds and switches. Every number a recognizer compares against lives here, so hosts can tune them and
//! tests can state them.

use serde::{Deserialize, Serialize};

use crate::transformer::TransformerConfig;

/// What a primary-button drag means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum PointerMode {
    /// Press on something and drag: [`crate::Intent::DragStart`] after the slop. Press on empty space: a marquee.
    #[default]
    Select,
    /// The hand tool, or Space held: a drag pans the view. No drag intents.
    Hand,
    /// A drawing tool: the drag starts as soon as the pointer moves (`draw_slop_px`) and the origin is the press.
    Draw,
}

/// Gesture thresholds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputConfig {
    /// A mouse or pen must move more than this many pixels from the press before a drag starts.
    pub drag_slop_px: f64,
    /// The same for a finger, which is less precise.
    pub drag_slop_touch_px: f64,
    /// The same in [`PointerMode::Draw`].
    pub draw_slop_px: f64,
    /// Two clicks within this many milliseconds make a double click.
    pub double_click_ms: f64,
    /// ... if the second is within this many pixels of the first.
    pub double_click_dist_px: f64,
    /// Hold time for a long press, in milliseconds. `None` turns long press off.
    pub long_press_ms: Option<f64>,
    /// Long press also for a mouse (off by default: a held mouse button is usually a slow drag).
    pub long_press_mouse: bool,
    /// A middle-button drag pans.
    pub middle_button_pans: bool,
    /// A plain wheel zooms and Ctrl/Cmd+wheel pans (Figma style). The default is the reverse (browser style).
    pub wheel_zooms: bool,
    /// Zoom factor of one wheel notch (100 pixels of delta). The old app used 1.1.
    pub wheel_zoom_step: f64,
    /// A pinch changes the zoom only when the finger distance ratio differs from 1 by more than this.
    pub pinch_epsilon: f64,
    /// Fingers closer than this are not used for pinch maths.
    pub pinch_min_distance_px: f64,
    /// Pixels per line for wheel events measured in lines.
    pub wheel_line_px: f64,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            drag_slop_px: 3.0,
            drag_slop_touch_px: 8.0,
            draw_slop_px: 0.0,
            double_click_ms: 500.0,
            double_click_dist_px: 5.0,
            long_press_ms: Some(500.0),
            long_press_mouse: false,
            middle_button_pans: true,
            wheel_zooms: false,
            wheel_zoom_step: 1.1,
            pinch_epsilon: 0.001,
            pinch_min_distance_px: 4.0,
            wheel_line_px: 33.0,
        }
    }
}

/// Configuration of the whole facade.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CanvasConfig {
    /// Gesture thresholds.
    pub input: InputConfig,
    /// Reach around outlines for mouse and pen hits, in logical pixels.
    pub hit_tolerance_px: f64,
    /// Reach for a finger.
    pub hit_tolerance_touch_px: f64,
    /// Transformer handles.
    pub transformer: TransformerConfig,
    /// Most per-element selection boxes the overlay lists. A larger selection gets only its transformer frame.
    pub max_selection_boxes: usize,
    /// Most per-element boxes for one remote user's selection.
    pub max_remote_selection_boxes: usize,
}

impl Default for CanvasConfig {
    fn default() -> Self {
        Self {
            input: InputConfig::default(),
            hit_tolerance_px: 5.0,
            hit_tolerance_touch_px: 12.0,
            transformer: TransformerConfig::default(),
            max_selection_boxes: 256,
            max_remote_selection_boxes: 64,
        }
    }
}
