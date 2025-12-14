//! Input state management using winit_input_helper.

use kurbo::{Point, Vec2};
use winit::event::{DeviceEvent, MouseButton, WindowEvent};
use winit::keyboard::KeyCode;
use winit_input_helper::WinitInputHelper;

// Use web_time for WASM compatibility
#[cfg(target_arch = "wasm32")]
use web_time::Instant;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;

/// Double-click detection constants.
const DOUBLE_CLICK_TIME_MS: u128 = 500;
const DOUBLE_CLICK_DISTANCE: f64 = 5.0;

/// Tracks the current input state across frames using WinitInputHelper.
pub struct InputState {
    helper: WinitInputHelper,
    /// Last click time for double-click detection.
    last_click_time: Option<Instant>,
    /// Last click position for double-click detection.
    last_click_position: Option<Point>,
    /// Whether a double-click was detected this frame.
    double_click_detected: bool,
    /// Whether the pointer is currently dragging.
    pub is_dragging: bool,
    /// Start position of current drag operation.
    pub drag_start: Option<Point>,
}

impl Default for InputState {
    fn default() -> Self {
        Self::new()
    }
}

impl InputState {
    pub fn new() -> Self {
        Self {
            helper: WinitInputHelper::new(),
            last_click_time: None,
            last_click_position: None,
            double_click_detected: false,
            is_dragging: false,
            drag_start: None,
        }
    }

    /// Call at the start of each frame.
    pub fn step(&mut self) {
        self.helper.step();
        self.double_click_detected = false;
    }

    /// Call at the end of each frame.
    pub fn end_step(&mut self) {
        self.helper.end_step();
    }

    /// Process a window event. Returns true on redraw request.
    pub fn process_window_event(&mut self, event: &WindowEvent) -> bool {
        let result = self.helper.process_window_event(event);
        
        // Handle double-click and drag detection
        if self.mouse_just_pressed(MouseButton::Left) {
            let current_pos = self.mouse_position();
            let now = Instant::now();

            if let (Some(last_time), Some(last_pos)) = (self.last_click_time, self.last_click_position) {
                let elapsed = now.duration_since(last_time).as_millis();
                let distance = current_pos.distance(last_pos);

                if elapsed < DOUBLE_CLICK_TIME_MS && distance < DOUBLE_CLICK_DISTANCE {
                    self.double_click_detected = true;
                    self.last_click_time = None;
                } else {
                    self.last_click_time = Some(now);
                    self.last_click_position = Some(current_pos);
                }
            } else {
                self.last_click_time = Some(now);
                self.last_click_position = Some(current_pos);
            }

            if !self.is_dragging {
                self.is_dragging = true;
                self.drag_start = Some(current_pos);
            }
        }

        if self.mouse_just_released(MouseButton::Left) {
            self.is_dragging = false;
            self.drag_start = None;
        }

        result
    }

    /// Process a device event.
    pub fn process_device_event(&mut self, event: &DeviceEvent) {
        self.helper.process_device_event(event);
    }

    // --- Mouse / Pointer ---

    pub fn mouse_position(&self) -> Point {
        let (x, y) = self.helper.cursor().unwrap_or((0.0, 0.0));
        Point::new(x as f64, y as f64)
    }

    pub fn is_button_pressed(&self, button: MouseButton) -> bool {
        self.helper.mouse_held(button)
    }

    pub fn mouse_just_pressed(&self, button: MouseButton) -> bool {
        self.helper.mouse_pressed(button)
    }

    pub fn mouse_just_released(&self, button: MouseButton) -> bool {
        self.helper.mouse_released(button)
    }

    pub fn scroll_delta(&self) -> Vec2 {
        let (dx, dy) = self.helper.scroll_diff();
        Vec2::new(dx as f64, dy as f64)
    }

    pub fn cursor_diff(&self) -> Vec2 {
        let (dx, dy) = self.helper.cursor_diff();
        Vec2::new(dx as f64, dy as f64)
    }

    // --- Keyboard ---

    pub fn is_key_pressed(&self, key: KeyCode) -> bool {
        self.helper.key_held(key)
    }

    pub fn is_key_just_pressed(&self, key: KeyCode) -> bool {
        self.helper.key_pressed(key)
    }

    pub fn is_key_just_released(&self, key: KeyCode) -> bool {
        self.helper.key_released(key)
    }

    // --- Modifiers ---

    pub fn shift(&self) -> bool {
        self.helper.held_shift()
    }

    pub fn ctrl(&self) -> bool {
        self.helper.held_control()
    }

    pub fn alt(&self) -> bool {
        self.helper.held_alt()
    }

    // --- Custom logic ---

    pub fn is_double_click(&self) -> bool {
        self.double_click_detected
    }

    pub fn drag_delta(&self) -> Option<Vec2> {
        self.drag_start.map(|start| {
            let pos = self.mouse_position();
            Vec2::new(pos.x - start.x, pos.y - start.y)
        })
    }

    pub fn close_requested(&self) -> bool {
        self.helper.close_requested()
    }
}
