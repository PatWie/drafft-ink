//! Platform-neutral input events.
//!
//! The shell (a browser page, a winit window, a test script) translates its own events into these and feeds them to
//! [`crate::Canvas::handle_input`] or to a [`crate::Recognizer`] directly. Positions are logical screen pixels
//! relative to the canvas. Time is a number of milliseconds the caller supplies with every event; the library
//! never reads a clock, so a recorded event stream replays identically.

use drafftink_geom::kurbo::{Point, Vec2};
use serde::{Deserialize, Serialize};

/// Identifies one pointer (a mouse, a pen, one finger) for as long as it is down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PointerId(pub u64);

/// The kind of device behind a pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PointerKind {
    /// A mouse or trackpad cursor. Hovers.
    Mouse,
    /// A stylus. Hovers while in range, reports pressure and tilt.
    Pen,
    /// A finger. Never hovers; two fingers make a view gesture.
    Touch,
}

/// What happened to the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PointerPhase {
    /// A button went down, the pen touched, or a finger landed.
    Down,
    /// The pointer moved (with or without buttons).
    Move,
    /// A button went up, the pen lifted, or a finger left.
    Up,
    /// The system cancelled the pointer (palm rejection, focus loss). Ends a gesture without a click.
    Cancel,
}

/// A mouse button. Touch and pen contact count as [`Button::Primary`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Button {
    /// No button changed (a plain move).
    None,
    /// Left button, touch contact, pen tip.
    Primary,
    /// Right button, pen barrel.
    Secondary,
    /// Wheel button.
    Middle,
    /// Any other button.
    Other,
}

/// Which buttons are held, as a bit set with the DOM `buttons` layout (1 primary, 2 secondary, 4 middle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Buttons(pub u8);

impl Buttons {
    /// No button held.
    pub const NONE: Buttons = Buttons(0);
    /// Primary held.
    pub const PRIMARY: Buttons = Buttons(1);
    /// Secondary held.
    pub const SECONDARY: Buttons = Buttons(2);
    /// Middle held.
    pub const MIDDLE: Buttons = Buttons(4);

    /// True when no button is held.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// True when every bit of `other` is held.
    pub fn contains(self, other: Buttons) -> bool {
        self.0 & other.0 == other.0 && other.0 != 0
    }
}

/// Keyboard modifiers at the time of an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Modifiers {
    /// Shift.
    pub shift: bool,
    /// Control.
    pub ctrl: bool,
    /// Alt or Option.
    pub alt: bool,
    /// Command (macOS) or the Windows key.
    pub meta: bool,
}

impl Modifiers {
    /// No modifier.
    pub const NONE: Modifiers = Modifiers { shift: false, ctrl: false, alt: false, meta: false };

    /// Ctrl or Cmd: the "command" modifier of the platform.
    pub fn command(self) -> bool {
        self.ctrl || self.meta
    }
}

/// Pen tilt in degrees, each in `-90..=90`.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Tilt {
    /// Tilt towards the right (positive) or left of the screen.
    pub x: f32,
    /// Tilt towards the bottom (positive) or top of the screen.
    pub y: f32,
}

/// One pointer event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PointerEvent {
    /// The pointer.
    pub id: PointerId,
    /// The device kind.
    pub kind: PointerKind,
    /// What happened.
    pub phase: PointerPhase,
    /// Position in logical pixels relative to the canvas.
    pub pos: Point,
    /// The button that changed for `Down` and `Up`; `None` for moves.
    pub button: Button,
    /// Buttons held after this event.
    pub buttons: Buttons,
    /// Keyboard modifiers.
    pub modifiers: Modifiers,
    /// Pressure `0..=1`. 0.5 for devices that cannot tell.
    pub pressure: f32,
    /// Pen tilt.
    pub tilt: Tilt,
    /// Milliseconds on the caller's clock. Must not run backwards.
    pub time_ms: f64,
}

impl PointerEvent {
    /// A mouse event at a position. Down/Up carry the primary button; use the `with_*` methods to change that.
    pub fn mouse(phase: PointerPhase, x: f64, y: f64, time_ms: f64) -> Self {
        Self::new(PointerId(0), PointerKind::Mouse, phase, Point::new(x, y), time_ms)
    }

    /// A touch event for finger `id`.
    pub fn touch(id: u64, phase: PointerPhase, x: f64, y: f64, time_ms: f64) -> Self {
        Self::new(PointerId(id + 1), PointerKind::Touch, phase, Point::new(x, y), time_ms)
    }

    /// A pen event.
    pub fn pen(phase: PointerPhase, x: f64, y: f64, pressure: f32, time_ms: f64) -> Self {
        let mut e = Self::new(PointerId(100), PointerKind::Pen, phase, Point::new(x, y), time_ms);
        e.pressure = pressure;
        e
    }

    /// An event for any pointer. The button is derived from the phase: primary for down and up, none otherwise.
    pub fn new(id: PointerId, kind: PointerKind, phase: PointerPhase, pos: Point, time_ms: f64) -> Self {
        let (button, buttons) = match phase {
            PointerPhase::Down => (Button::Primary, Buttons::PRIMARY),
            PointerPhase::Move => (Button::None, Buttons::NONE),
            PointerPhase::Up | PointerPhase::Cancel => (Button::Primary, Buttons::NONE),
        };
        Self {
            id,
            kind,
            phase,
            pos,
            button,
            buttons,
            modifiers: Modifiers::NONE,
            pressure: 0.5,
            tilt: Tilt::default(),
            time_ms,
        }
    }

    /// Sets the changed button and the held buttons.
    #[must_use]
    pub fn with_button(mut self, button: Button, held: Buttons) -> Self {
        self.button = button;
        self.buttons = held;
        self
    }

    /// Sets the modifiers.
    #[must_use]
    pub fn with_modifiers(mut self, modifiers: Modifiers) -> Self {
        self.modifiers = modifiers;
        self
    }

    /// A move with the primary button held (a drag sample).
    #[must_use]
    pub fn held(mut self) -> Self {
        self.buttons = Buttons::PRIMARY;
        self
    }

    pub(crate) fn is_finite(&self) -> bool {
        self.pos.x.is_finite() && self.pos.y.is_finite() && self.time_ms.is_finite()
    }
}

/// The unit of a wheel delta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WheelUnit {
    /// Pixels (trackpads, most browsers).
    Pixel,
    /// Lines of text (some mouse wheels).
    Line,
    /// Pages.
    Page,
}

/// A wheel or trackpad scroll event. A pinch on a trackpad arrives as a wheel event with Ctrl set in browsers; a
/// native pinch event should be sent as a wheel event with `ctrl` and a `delta.y` of `-scale_change * 100`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WheelEvent {
    /// Pointer position.
    pub pos: Point,
    /// Scroll amount, positive y scrolls down.
    pub delta: Vec2,
    /// Unit of `delta`.
    pub unit: WheelUnit,
    /// Keyboard modifiers.
    pub modifiers: Modifiers,
    /// Milliseconds on the caller's clock.
    pub time_ms: f64,
}

/// Anything the recognizer consumes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum InputEvent {
    /// A pointer event.
    Pointer(PointerEvent),
    /// A wheel or trackpad event.
    Wheel(WheelEvent),
    /// The pointer left the canvas without a button held. Clears the hover.
    PointerLeave {
        /// The pointer.
        id: PointerId,
    },
    /// Abort the running gesture: Escape, window blur, a release over a panel.
    CancelAll,
}

impl From<PointerEvent> for InputEvent {
    fn from(e: PointerEvent) -> Self {
        InputEvent::Pointer(e)
    }
}

impl From<WheelEvent> for InputEvent {
    fn from(e: WheelEvent) -> Self {
        InputEvent::Wheel(e)
    }
}
