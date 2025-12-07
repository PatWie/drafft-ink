//! Input state management for mouse/touch/keyboard events.

use kurbo::{Point, Vec2};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

// Use web_time for WASM compatibility
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;
#[cfg(target_arch = "wasm32")]
use web_time::Instant;

/// Mouse button identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Modifier keys state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

/// Pointer event type for unified mouse/touch handling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PointerEvent {
    Down {
        position: Point,
        button: MouseButton,
    },
    Up {
        position: Point,
        button: MouseButton,
    },
    Move {
        position: Point,
    },
    Scroll {
        position: Point,
        delta: Vec2,
    },
}

/// Keyboard event type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum KeyEvent {
    Pressed(String),
    Released(String),
}

/// Double-click detection constants.
const DOUBLE_CLICK_TIME_MS: u128 = 500;
const DOUBLE_CLICK_DISTANCE: f64 = 5.0;

/// Tracks the current input state across frames.
#[derive(Debug, Clone)]
pub struct InputState {
    /// Current pointer position in screen coordinates.
    pub pointer_position: Point,
    /// Previous pointer position for delta calculations.
    pub previous_pointer_position: Point,
    /// Currently pressed mouse buttons.
    pressed_buttons: HashSet<MouseButton>,
    /// Buttons that were just pressed this frame.
    just_pressed_buttons: HashSet<MouseButton>,
    /// Buttons that were just released this frame.
    just_released_buttons: HashSet<MouseButton>,
    /// Current modifier keys state.
    pub modifiers: Modifiers,
    /// Accumulated scroll delta since last frame.
    pub scroll_delta: Vec2,
    /// Currently pressed keys.
    pressed_keys: HashSet<String>,
    /// Keys that were just pressed this frame.
    just_pressed_keys: HashSet<String>,
    /// Whether the pointer is currently dragging.
    pub is_dragging: bool,
    /// Start position of current drag operation.
    pub drag_start: Option<Point>,
    /// Last click time for double-click detection.
    last_click_time: Option<Instant>,
    /// Last click position for double-click detection.
    last_click_position: Option<Point>,
    /// Whether a double-click was detected this frame.
    double_click_detected: bool,
}

impl Default for InputState {
    fn default() -> Self {
        Self {
            pointer_position: Point::ZERO,
            previous_pointer_position: Point::ZERO,
            pressed_buttons: HashSet::new(),
            just_pressed_buttons: HashSet::new(),
            just_released_buttons: HashSet::new(),
            modifiers: Modifiers::default(),
            scroll_delta: Vec2::ZERO,
            pressed_keys: HashSet::new(),
            just_pressed_keys: HashSet::new(),
            is_dragging: false,
            drag_start: None,
            last_click_time: None,
            last_click_position: None,
            double_click_detected: false,
        }
    }
}

impl InputState {
    /// Create a new input state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Call at the start of each frame to reset per-frame state.
    pub fn begin_frame(&mut self) {
        self.just_pressed_buttons.clear();
        self.just_released_buttons.clear();
        self.just_pressed_keys.clear();
        self.scroll_delta = Vec2::ZERO;
        self.previous_pointer_position = self.pointer_position;
        self.double_click_detected = false;
    }

    /// Process a pointer event.
    pub fn handle_pointer_event(&mut self, event: PointerEvent) {
        match event {
            PointerEvent::Down { position, button } => {
                self.pointer_position = position;
                if self.pressed_buttons.insert(button) {
                    self.just_pressed_buttons.insert(button);
                }

                // Double-click detection for left button
                if button == MouseButton::Left {
                    let now = Instant::now();
                    if let (Some(last_time), Some(last_pos)) =
                        (self.last_click_time, self.last_click_position)
                    {
                        let elapsed = now.duration_since(last_time).as_millis();
                        let distance = ((position.x - last_pos.x).powi(2)
                            + (position.y - last_pos.y).powi(2))
                        .sqrt();

                        if elapsed < DOUBLE_CLICK_TIME_MS && distance < DOUBLE_CLICK_DISTANCE {
                            self.double_click_detected = true;
                            // Reset to prevent triple-click being detected as another double-click
                            self.last_click_time = None;
                            self.last_click_position = None;
                        } else {
                            self.last_click_time = Some(now);
                            self.last_click_position = Some(position);
                        }
                    } else {
                        self.last_click_time = Some(now);
                        self.last_click_position = Some(position);
                    }

                    if !self.is_dragging {
                        self.is_dragging = true;
                        self.drag_start = Some(position);
                    }
                }
            }
            PointerEvent::Up { position, button } => {
                self.pointer_position = position;
                if self.pressed_buttons.remove(&button) {
                    self.just_released_buttons.insert(button);
                }
                if button == MouseButton::Left {
                    self.is_dragging = false;
                    self.drag_start = None;
                }
            }
            PointerEvent::Move { position } => {
                self.pointer_position = position;
            }
            PointerEvent::Scroll { position, delta } => {
                self.pointer_position = position;
                self.scroll_delta += delta;
            }
        }
    }

    /// Process a key event.
    pub fn handle_key_event(&mut self, event: KeyEvent) {
        match event {
            KeyEvent::Pressed(key) => {
                if self.pressed_keys.insert(key.clone()) {
                    self.just_pressed_keys.insert(key);
                }
            }
            KeyEvent::Released(key) => {
                self.pressed_keys.remove(&key);
            }
        }
    }

    /// Update modifier keys state.
    pub fn set_modifiers(&mut self, modifiers: Modifiers) {
        self.modifiers = modifiers;
    }

    /// Check if a button is currently pressed.
    pub fn is_button_pressed(&self, button: MouseButton) -> bool {
        self.pressed_buttons.contains(&button)
    }

    /// Check if a button was just pressed this frame.
    pub fn is_button_just_pressed(&self, button: MouseButton) -> bool {
        self.just_pressed_buttons.contains(&button)
    }

    /// Check if a button was just released this frame.
    pub fn is_button_just_released(&self, button: MouseButton) -> bool {
        self.just_released_buttons.contains(&button)
    }

    /// Check if a key is currently pressed.
    pub fn is_key_pressed(&self, key: &str) -> bool {
        self.pressed_keys.contains(key)
    }

    /// Check if a key was just pressed this frame.
    pub fn is_key_just_pressed(&self, key: &str) -> bool {
        self.just_pressed_keys.contains(key)
    }

    /// Check if a double-click was detected this frame.
    pub fn is_double_click(&self) -> bool {
        self.double_click_detected
    }

    /// Get the pointer movement delta since last frame.
    pub fn pointer_delta(&self) -> Vec2 {
        Vec2::new(
            self.pointer_position.x - self.previous_pointer_position.x,
            self.pointer_position.y - self.previous_pointer_position.y,
        )
    }

    /// Get the drag delta from start position, if dragging.
    pub fn drag_delta(&self) -> Option<Vec2> {
        self.drag_start.map(|start| {
            Vec2::new(
                self.pointer_position.x - start.x,
                self.pointer_position.y - start.y,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_button_press() {
        let mut input = InputState::new();

        input.handle_pointer_event(PointerEvent::Down {
            position: Point::new(100.0, 100.0),
            button: MouseButton::Left,
        });

        assert!(input.is_button_pressed(MouseButton::Left));
        assert!(input.is_button_just_pressed(MouseButton::Left));
        assert!(!input.is_button_pressed(MouseButton::Right));
    }

    #[test]
    fn test_button_release() {
        let mut input = InputState::new();

        input.handle_pointer_event(PointerEvent::Down {
            position: Point::new(100.0, 100.0),
            button: MouseButton::Left,
        });
        input.handle_pointer_event(PointerEvent::Up {
            position: Point::new(100.0, 100.0),
            button: MouseButton::Left,
        });

        assert!(!input.is_button_pressed(MouseButton::Left));
        assert!(input.is_button_just_released(MouseButton::Left));
    }

    #[test]
    fn test_begin_frame_clears_just_pressed() {
        let mut input = InputState::new();

        input.handle_pointer_event(PointerEvent::Down {
            position: Point::new(100.0, 100.0),
            button: MouseButton::Left,
        });

        assert!(input.is_button_just_pressed(MouseButton::Left));

        input.begin_frame();

        assert!(!input.is_button_just_pressed(MouseButton::Left));
        assert!(input.is_button_pressed(MouseButton::Left)); // Still pressed
    }

    #[test]
    fn test_drag_tracking() {
        let mut input = InputState::new();

        input.handle_pointer_event(PointerEvent::Down {
            position: Point::new(100.0, 100.0),
            button: MouseButton::Left,
        });

        assert!(input.is_dragging);
        assert_eq!(input.drag_start, Some(Point::new(100.0, 100.0)));

        input.handle_pointer_event(PointerEvent::Move {
            position: Point::new(150.0, 120.0),
        });

        let delta = input.drag_delta().unwrap();
        assert!((delta.x - 50.0).abs() < f64::EPSILON);
        assert!((delta.y - 20.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_scroll() {
        let mut input = InputState::new();

        input.handle_pointer_event(PointerEvent::Scroll {
            position: Point::new(100.0, 100.0),
            delta: Vec2::new(0.0, 10.0),
        });

        assert!((input.scroll_delta.y - 10.0).abs() < f64::EPSILON);

        input.begin_frame();
        assert!((input.scroll_delta.y).abs() < f64::EPSILON);
    }

    #[test]
    fn test_key_press() {
        let mut input = InputState::new();

        input.handle_key_event(KeyEvent::Pressed("a".to_string()));

        assert!(input.is_key_pressed("a"));
        assert!(input.is_key_just_pressed("a"));

        input.begin_frame();

        assert!(input.is_key_pressed("a"));
        assert!(!input.is_key_just_pressed("a"));
    }

    #[test]
    fn test_double_click_detection() {
        let mut input = InputState::new();
        let pos = Point::new(100.0, 100.0);

        // First click
        input.handle_pointer_event(PointerEvent::Down {
            position: pos,
            button: MouseButton::Left,
        });
        assert!(!input.is_double_click()); // First click is not a double-click

        input.handle_pointer_event(PointerEvent::Up {
            position: pos,
            button: MouseButton::Left,
        });
        input.begin_frame();

        // Second click shortly after - should be detected as double-click
        input.handle_pointer_event(PointerEvent::Down {
            position: pos,
            button: MouseButton::Left,
        });
        assert!(input.is_double_click()); // Second click IS a double-click

        input.begin_frame();
        assert!(!input.is_double_click()); // Cleared after frame
    }

    #[test]
    fn test_double_click_too_far() {
        let mut input = InputState::new();

        // First click
        input.handle_pointer_event(PointerEvent::Down {
            position: Point::new(100.0, 100.0),
            button: MouseButton::Left,
        });
        input.handle_pointer_event(PointerEvent::Up {
            position: Point::new(100.0, 100.0),
            button: MouseButton::Left,
        });
        input.begin_frame();

        // Second click too far away
        input.handle_pointer_event(PointerEvent::Down {
            position: Point::new(200.0, 200.0), // Far from first click
            button: MouseButton::Left,
        });
        assert!(!input.is_double_click()); // Too far - not a double-click
    }
}
