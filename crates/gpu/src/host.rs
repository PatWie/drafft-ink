//! What a platform host (the web glue in `web/`) sends to the shell, and the monotonic clock
//! both use for event time stamps.

use std::rc::Rc;
use std::sync::OnceLock;

use web_time::Instant;

use crate::events::ShellEvent;

/// An event from the platform host. The shell turns it into egui input and/or app events.
#[derive(Clone, Debug)]
pub(crate) enum HostEvent {
    /// Input only egui needs: `Event::Key` from the hidden textarea, `Event::Text`, `Event::Ime`.
    /// The shell pushes it into egui's raw input and draws a frame (reason `ui`).
    Egui(egui::Event),
    /// An event for the app (`Pointer`, `Paste`, `CopyRequested`, `FilesDropped`, `FileHover`,
    /// `TextInput`, `Visibility`, `Focus`). The shell hands it to `ShellApp::event`; the answer decides
    /// the frame. `Paste` text that the app does not take goes to egui as `Event::Paste`;
    /// `CopyRequested` that the app does not answer with a clipboard write goes to egui as `Event::Copy` or `Cut`.
    App(ShellEvent),
}

/// Where a host delivers events. Cheap to clone. Calls may come from DOM callbacks or from
/// futures spawned with `spawn_local`; delivery wakes the event loop.
#[derive(Clone)]
pub(crate) struct HostSink {
    send: Rc<dyn Fn(HostEvent)>,
}

impl HostSink {
    pub(crate) fn new(send: impl Fn(HostEvent) + 'static) -> Self {
        Self { send: Rc::new(send) }
    }

    pub(crate) fn send(&self, event: HostEvent) {
        (self.send)(event);
    }

    pub(crate) fn app(&self, event: ShellEvent) {
        self.send(HostEvent::App(event));
    }

    pub(crate) fn egui(&self, event: egui::Event) {
        self.send(HostEvent::Egui(event));
    }
}

/// Milliseconds on a monotonic clock that starts at the first call. Works on the web.
pub(crate) fn now_ms() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}
