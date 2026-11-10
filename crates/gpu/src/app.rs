//! The contract between the shell and an app.

use std::sync::Arc;

use drafftink_profile::RedrawReason;
use vello::Scene;
use vello::peniko::Color;
use winit::event_loop::EventLoopProxy;

use crate::events::{ClipboardContent, CursorIcon, Handled, Repaint, ShellEvent};
use crate::gpu::Gpu;
use crate::shell::UserEvent;

/// Size and scale of the window surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewInfo {
    /// Surface size in physical pixels.
    pub size_px: [u32; 2],
    /// Physical pixels per egui point (devicePixelRatio on the web).
    pub scale: f32,
}

impl ViewInfo {
    /// Size in egui points.
    pub fn size_points(&self) -> egui::Vec2 {
        let s = self.scale.max(f32::EPSILON);
        egui::vec2(self.size_px[0] as f32 / s, self.size_px[1] as f32 / s)
    }
}

/// Wakes the event loop from any thread or task with a reason. Cloneable, `Send + Sync`.
///
/// The loop answers with [`ShellEvent::Wake`] and then a frame. Use it for network messages, finished
/// autosaves and image decodes. Do not call it for work that changes nothing visible.
#[derive(Clone)]
pub struct Waker {
    pub(crate) proxy: Arc<EventLoopProxy<UserEvent>>,
}

impl Waker {
    /// Wake the loop. Returns `false` when the loop has already exited.
    pub fn wake(&self, reason: RedrawReason) -> bool {
        self.proxy.send_event(UserEvent::Wake(reason)).is_ok()
    }
}

impl std::fmt::Debug for Waker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Waker")
    }
}

/// Things an app asks the shell to do outside of drawing. Applied after the callback returns.
#[derive(Default)]
pub(crate) struct Outbox {
    pub clipboard: Option<ClipboardContent>,
    pub cursor: Option<CursorIcon>,
    pub open_url: Option<(String, bool)>,
    pub lose_device: bool,
}

/// Passed to [`ShellApp::setup`].
pub struct SetupCx<'a> {
    /// A fresh egui context. Set fonts and style here. The shell calls `setup` again if it has to
    /// recreate the context after a lost GPU device, so do not keep the context.
    pub ctx: &'a egui::Context,
    pub gpu: &'a Gpu,
    pub waker: Waker,
    pub view: ViewInfo,
}

/// Passed to [`ShellApp::event`].
pub struct EventCx<'a> {
    pub ctx: &'a egui::Context,
    pub gpu: &'a Gpu,
    pub waker: &'a Waker,
    pub view: ViewInfo,
    pub(crate) out: &'a mut Outbox,
}

impl EventCx<'_> {
    /// Put content on the system clipboard (text, PNG images, a private payload).
    /// On the web this is the async Clipboard API and needs a user gesture; call it while handling
    /// [`ShellEvent::CopyRequested`] or a key or pointer event.
    pub fn set_clipboard(&mut self, content: ClipboardContent) {
        self.out.clipboard = Some(content);
    }

    /// Change the cursor now, without a frame. Hover feedback on the canvas uses this.
    pub fn set_cursor(&mut self, icon: CursorIcon) {
        self.out.cursor = Some(icon);
    }

    /// Open a URL in the system browser (a new tab when `new_tab` is set; native always uses the browser).
    pub fn open_url(&mut self, url: impl Into<String>, new_tab: bool) {
        self.out.open_url = Some((url.into(), new_tab));
    }

    /// Test hook: destroy the GPU device so the recovery path runs on the next frame. Not for the app.
    pub fn debug_lose_device(&mut self) {
        self.out.lose_device = true;
    }
}

/// Passed to [`ShellApp::frame`].
pub struct FrameCx<'a> {
    pub gpu: &'a Gpu,
    pub waker: &'a Waker,
    pub view: ViewInfo,
    /// Why this frame is drawn (the first reason of the pending ones).
    pub reason: RedrawReason,
    /// The surface size or scale changed since the previous frame.
    pub resized: bool,
    pub(crate) scene: &'a mut Scene,
    pub(crate) scene_dirty: &'a mut bool,
    pub(crate) scene_base: &'a mut Color,
    pub(crate) out: &'a mut Outbox,
}

impl FrameCx<'_> {
    /// The Vello scene for this frame, reset and ready to fill. Calling this marks the scene dirty:
    /// Vello runs for this frame. Do not call it when the canvas content did not change; the shell
    /// then composites the texture of the last Vello frame again, which costs no Vello work.
    /// `base` is the clear colour under the scene.
    pub fn scene_mut(&mut self, base: Color) -> &mut Scene {
        self.scene.reset();
        *self.scene_dirty = true;
        *self.scene_base = base;
        self.scene
    }

    /// See [`EventCx::set_clipboard`].
    pub fn set_clipboard(&mut self, content: ClipboardContent) {
        self.out.clipboard = Some(content);
    }

    /// See [`EventCx::open_url`].
    pub fn open_url(&mut self, url: impl Into<String>, new_tab: bool) {
        self.out.open_url = Some((url.into(), new_tab));
    }

    /// See [`EventCx::debug_lose_device`].
    pub fn debug_lose_device(&mut self) {
        self.out.lose_device = true;
    }
}

/// What a frame reports back.
#[derive(Clone, Debug, Default)]
pub struct FrameOutput {
    /// What the app needs next. egui's own repaint requests are honoured as well.
    pub repaint: Repaint,
    /// Where the canvas is, in egui points. Pointer moves inside it that no egui area covers are
    /// "hover only": they are sent to the app as [`ShellEvent::Pointer`] and draw no frame unless the
    /// app asks. `None` means the whole window.
    pub canvas_rect: Option<egui::Rect>,
    /// The caret rectangle (points) when the app has a text editor that is not an egui widget.
    /// The shell turns the IME and the soft keyboard on for it. `None`: egui decides.
    pub text_input: Option<egui::Rect>,
    /// Cursor over the canvas, used when egui has no opinion.
    pub cursor: Option<CursorIcon>,
}

/// An app that runs in the shell. All methods run on the UI thread.
pub trait ShellApp: 'static {
    /// A new egui context exists (at start, and again after a GPU loss).
    fn setup(&mut self, _cx: &mut SetupCx<'_>) {}

    /// Something happened outside egui's own widgets. The return value says whether a frame is
    /// needed. A hover move that changes nothing returns [`Repaint::Idle`]. The default does nothing.
    fn event(&mut self, _cx: &mut EventCx<'_>, _event: ShellEvent) -> (Repaint, Handled) {
        (Repaint::Idle, Handled::No)
    }

    /// Build the frame. The shell calls this only when a frame is due. Build the egui UI into `ui` and,
    /// when the canvas changed, the Vello scene through [`FrameCx::scene_mut`].
    fn frame(&mut self, ui: &mut egui::Ui, cx: &mut FrameCx<'_>) -> FrameOutput;
}
