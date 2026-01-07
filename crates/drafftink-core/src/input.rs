//! Input state management using winit_input_helper.

use kurbo::{Point, Vec2};
use winit::event::{DeviceEvent, MouseButton, Touch, TouchPhase, WindowEvent};
use winit::keyboard::KeyCode;
use winit_input_helper::WinitInputHelper;

// Use web_time for WASM compatibility
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;
#[cfg(target_arch = "wasm32")]
use web_time::Instant;

/// Double-click detection constants.
const DOUBLE_CLICK_TIME_MS: u128 = 500;
const DOUBLE_CLICK_DISTANCE: f64 = 5.0;

/// Touch state for a single finger.
#[derive(Debug, Clone, Copy)]
pub struct TouchState {
    pub id: u64,
    pub position: Point,
    pub phase: TouchPhase,
}

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
    /// Active touch points (up to 2 for pinch-zoom).
    touches: [Option<TouchState>; 2],
    /// Previous distance between two fingers (for pinch zoom).
    pinch_distance: Option<f64>,
    /// Previous center between two fingers (for pan during pinch).
    pinch_center: Option<Point>,
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
            touches: [None, None],
            pinch_distance: None,
            pinch_center: None,
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

            if let (Some(last_time), Some(last_pos)) =
                (self.last_click_time, self.last_click_position)
            {
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

    // --- Touch ---

    /// Process a touch event. Returns (pan_delta, zoom_delta, zoom_center) if gesture detected.
    pub fn process_touch(&mut self, touch: &Touch) -> Option<(Vec2, f64, Point)> {
        let pos = Point::new(touch.location.x, touch.location.y);
        let state = TouchState {
            id: touch.id,
            position: pos,
            phase: touch.phase,
        };

        match touch.phase {
            TouchPhase::Started => {
                // Find empty slot
                if self.touches[0].is_none() {
                    self.touches[0] = Some(state);
                } else if self.touches[1].is_none() {
                    self.touches[1] = Some(state);
                    // Initialize pinch state
                    if let (Some(t0), Some(t1)) = (self.touches[0], self.touches[1]) {
                        self.pinch_distance = Some(t0.position.distance(t1.position));
                        self.pinch_center = Some(Point::new(
                            (t0.position.x + t1.position.x) / 2.0,
                            (t0.position.y + t1.position.y) / 2.0,
                        ));
                    }
                }
                None
            }
            TouchPhase::Moved => {
                // Update touch position
                for slot in &mut self.touches {
                    if let Some(t) = slot {
                        if t.id == touch.id {
                            t.position = pos;
                            t.phase = touch.phase;
                        }
                    }
                }

                // Calculate gesture
                if let (Some(t0), Some(t1)) = (self.touches[0], self.touches[1]) {
                    // Two-finger gesture: pinch zoom + pan
                    let new_dist = t0.position.distance(t1.position);
                    let new_center = Point::new(
                        (t0.position.x + t1.position.x) / 2.0,
                        (t0.position.y + t1.position.y) / 2.0,
                    );

                    let zoom_delta = if let Some(old_dist) = self.pinch_distance {
                        if old_dist > 0.0 {
                            new_dist / old_dist
                        } else {
                            1.0
                        }
                    } else {
                        1.0
                    };

                    let pan_delta = if let Some(old_center) = self.pinch_center {
                        Vec2::new(new_center.x - old_center.x, new_center.y - old_center.y)
                    } else {
                        Vec2::ZERO
                    };

                    self.pinch_distance = Some(new_dist);
                    self.pinch_center = Some(new_center);

                    Some((pan_delta, zoom_delta, new_center))
                } else {
                    None
                }
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                // Remove touch
                for slot in &mut self.touches {
                    if let Some(t) = slot {
                        if t.id == touch.id {
                            *slot = None;
                        }
                    }
                }
                self.pinch_distance = None;
                self.pinch_center = None;
                None
            }
        }
    }

    /// Get the primary touch position (first finger).
    pub fn primary_touch(&self) -> Option<Point> {
        self.touches[0].map(|t| t.position)
    }

    /// Get number of active touches.
    pub fn touch_count(&self) -> usize {
        self.touches.iter().filter(|t| t.is_some()).count()
    }

    /// Check if single touch is active (for drawing).
    pub fn is_single_touch(&self) -> bool {
        self.touches[0].is_some() && self.touches[1].is_none()
    }

    /// Check if touch just started (first finger down).
    pub fn touch_just_started(&self) -> bool {
        self.touches[0]
            .map(|t| t.phase == TouchPhase::Started)
            .unwrap_or(false)
    }

    /// Check if touch just ended.
    pub fn touch_just_ended(&self) -> bool {
        self.touches[0].is_none() && self.touches[1].is_none()
    }
}
