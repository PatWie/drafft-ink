//! Events the shell hands to the app, and what the app answers.

use std::time::Duration;

use drafftink_profile::RedrawReason;

/// The kind of device behind a pointer sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerKind {
    Mouse,
    Touch,
    Pen,
}

/// What a pointer sample reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerPhase {
    /// Moved with no button or contact. This is the hover case: it never draws a frame by itself.
    Hover,
    /// A button went down, or a finger or pen touched the surface.
    Down,
    /// Moved while a button or contact is held.
    Move,
    /// The button or contact was released.
    Up,
    /// The system cancelled the contact (touch cancel, pointer capture lost).
    Cancel,
    /// The pointer left the window.
    Leave,
}

impl PointerPhase {
    /// Whether the web shell reads the browser's coalesced points for this phase (`getCoalescedEvents`: a JS
    /// call and an array of event objects per sample). Only a move with contact draws from them (a pen stroke);
    /// a hover never uses them, and a mouse hovers 125 times a second.
    pub fn reads_coalesced(self) -> bool {
        self == Self::Move
    }
}

/// One position of a pointer with its pressure.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerPoint {
    /// Window position in egui points (logical pixels).
    pub pos: egui::Pos2,
    /// 0..=1 when the device reports pressure. `None` for a mouse and for devices that do not report it.
    pub pressure: Option<f32>,
    /// Milliseconds on the shell clock (monotonic since start).
    pub time_ms: f64,
}

/// A raw pointer event, delivered to the app before egui sees it, for every move including hover.
/// The app decides whether it needs a frame (the return value of [`crate::ShellApp::event`]).
#[derive(Clone, Debug, PartialEq)]
pub struct PointerSample {
    pub phase: PointerPhase,
    pub kind: PointerKind,
    /// Stable id of the contact. 0 for the mouse.
    pub id: u64,
    pub point: PointerPoint,
    /// Pen tilt in degrees (x, y), when reported.
    pub tilt: Option<[f32; 2]>,
    /// Which button changed, for `Down` and `Up` of a mouse (0 primary, 1 middle, 2 secondary, 3 back, 4 forward).
    pub button: Option<u8>,
    pub modifiers: egui::Modifiers,
    /// Intermediate positions since the previous sample, oldest first, excluding `point`.
    /// Filled on the web (`getCoalescedEvents`). Use them for freehand strokes.
    pub coalesced: Vec<PointerPoint>,
}

impl PointerSample {
    /// A sample with no tilt, button or coalesced points.
    pub fn new(phase: PointerPhase, kind: PointerKind, point: PointerPoint, modifiers: egui::Modifiers) -> Self {
        Self { phase, kind, id: 0, point, tilt: None, button: None, modifiers, coalesced: Vec::new() }
    }
}

/// An image from the clipboard or a dropped file, still encoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageBytes {
    /// MIME type such as `image/png`.
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// What the system clipboard holds, or what the app wants to put there.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClipboardContent {
    pub text: Option<String>,
    /// Encoded images (PNG on write). On read: PNG, JPEG, WebP, ... as the source offered them.
    pub images: Vec<ImageBytes>,
    /// An app-private payload (for example the JSON of copied elements) that survives a round trip
    /// through the system clipboard. On the web it travels as the `web application/x-drafftink`
    /// custom format when the browser allows it, and falls back to the text.
    pub custom: Option<Vec<u8>>,
}

impl ClipboardContent {
    pub fn text(text: impl Into<String>) -> Self {
        Self { text: Some(text.into()), ..Self::default() }
    }

    pub fn is_empty(&self) -> bool {
        self.text.as_deref().is_none_or(str::is_empty) && self.images.is_empty() && self.custom.is_none()
    }
}

/// A file dropped on the window, read completely.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DroppedFile {
    pub name: String,
    /// MIME type if known (always known on the web; guessed from the extension natively), else empty.
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// IME and soft-keyboard events for the canvas text editor. egui text fields get the same input as
/// `egui::Event::Ime` / `Text` automatically; these are for editors that are not egui widgets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextInput {
    /// Composition started or the preedit string changed. `cursor` is a byte range inside `text`.
    Preedit { text: String, cursor: Option<(usize, usize)> },
    /// Final text to insert (also sent for plain typing while a text input is requested).
    Commit(String),
    /// Composition ended without a commit.
    Cancel,
}

/// Wheel, trackpad scroll or pinch input with the pointer over the canvas (not over an egui area).
///
/// The shell offers it to the app before egui sees it. egui smooths a scroll over several frames and asks for a
/// repaint for each of them, so an app that moves the camera itself would draw two or three frames for one tick.
/// If the app answers [`Handled::Yes`] egui never sees the events.
#[derive(Clone, Debug, PartialEq)]
pub struct WheelInput {
    /// Pointer position in window points.
    pub pos: egui::Pos2,
    /// `egui::Event::MouseWheel` and `egui::Event::Zoom`, as egui's window layer made them.
    pub events: Vec<egui::Event>,
    /// Modifiers held when the events arrived.
    pub modifiers: egui::Modifiers,
}

/// Why the shell talks to the app. See [`crate::ShellApp::event`].
#[derive(Clone, Debug, PartialEq)]
pub enum ShellEvent {
    /// A pointer moved, pressed or released. Also hover.
    Pointer(PointerSample),
    /// The user pasted (Ctrl/Cmd+V, the browser paste event, a context-menu paste). Text is also
    /// given to a focused egui text field unless the app returns [`Handled::Yes`].
    Paste(ClipboardContent),
    /// The user pressed copy or cut. The app calls [`crate::EventCx::set_clipboard`] with the selection.
    /// With nothing selected it does nothing, and a focused egui text field handles it instead.
    CopyRequested { cut: bool },
    /// Files dropped on the window.
    FilesDropped(Vec<DroppedFile>),
    /// Files are being dragged over the window (`true`) or left it (`false`).
    FileHover(bool),
    /// The canvas for IME text (see [`TextInput`]).
    TextInput(TextInput),
    /// The window or tab became visible (`true`) or hidden (`false`). No frame is drawn while hidden.
    Visibility(bool),
    /// The window gained or lost keyboard focus.
    Focus(bool),
    /// The GPU device was lost and recreated. Drop every GPU resource the app created itself.
    /// The shell re-renders the retained Vello scene by itself.
    GpuRecreated,
    /// Another thread or task called [`crate::Waker::wake`]. Drain your queues here.
    Wake(RedrawReason),
    /// Wheel or pinch over the canvas. Answer [`Handled::Yes`] to keep it from egui.
    Wheel(WheelInput),
}

/// The answer to [`ShellEvent::Paste`]: did the app take it?
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Handled {
    /// Not taken. The shell passes pasted text to egui.
    #[default]
    No,
    Yes,
}

/// What an app needs next. The shell records a [`RedrawReason`] for every frame it draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Repaint {
    /// Nothing changed. No frame is drawn.
    #[default]
    Idle,
    /// Draw a frame as soon as possible.
    Now(RedrawReason),
    /// Draw a frame after this delay (autosave indicator, caret blink, animation tick). Tie it to pending work.
    After(Duration, RedrawReason),
}

impl Repaint {
    /// The earlier of two requests.
    pub fn min(self, other: Self) -> Self {
        match (self, other) {
            (Self::Idle, o) | (o, Self::Idle) => o,
            (Self::Now(r), _) | (_, Self::Now(r)) => Self::Now(r),
            (Self::After(a, ra), Self::After(b, rb)) => {
                if a <= b {
                    Self::After(a, ra)
                } else {
                    Self::After(b, rb)
                }
            }
        }
    }
}

/// The system cursor the app wants over the canvas. egui's own cursor icon wins over widgets.
pub use egui::CursorIcon;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repaint_min_prefers_now_then_the_shorter_delay() {
        let a = Repaint::After(Duration::from_millis(50), RedrawReason::Timer);
        let b = Repaint::After(Duration::from_millis(10), RedrawReason::Autosave);
        assert_eq!(a.min(b), b);
        assert_eq!(Repaint::Idle.min(a), a);
        assert_eq!(a.min(Repaint::Now(RedrawReason::Command)), Repaint::Now(RedrawReason::Command));
        assert_eq!(Repaint::Idle.min(Repaint::Idle), Repaint::Idle);
    }

    #[test]
    fn only_a_move_with_contact_reads_coalesced_points() {
        use PointerPhase::{Cancel, Down, Hover, Leave, Move, Up};
        assert!(Move.reads_coalesced());
        for phase in [Hover, Down, Up, Cancel, Leave] {
            assert!(!phase.reads_coalesced(), "{phase:?}");
        }
    }

    #[test]
    fn empty_clipboard() {
        assert!(ClipboardContent::default().is_empty());
        assert!(ClipboardContent::text("").is_empty());
        assert!(!ClipboardContent::text("a").is_empty());
    }
}
