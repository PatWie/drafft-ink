//! Web host: everything the browser needs that winit 0.30 does not provide. Owned by the web-glue task.
//! See docs/shell.md for the design.
#![cfg(target_arch = "wasm32")]

use web_sys::HtmlCanvasElement;

use crate::events::ClipboardContent;
use crate::host::HostSink;

/// Find the canvas with this element id.
pub(crate) fn lookup_canvas(_id: &str) -> Result<HtmlCanvasElement, String> {
    Err("not implemented".into())
}

/// `navigator.gpu` exists.
pub fn webgpu_available() -> bool {
    false
}

/// Replace the page body with a plain error page (title, details, what to try).
pub fn show_error_page(_title: &str, _details: &str) {}

/// Listeners and elements that live as long as the app.
pub(crate) struct WebHost;

impl WebHost {
    pub(crate) fn install(_canvas: &HtmlCanvasElement, _sink: HostSink) -> Result<Self, String> {
        Ok(Self)
    }

    /// Async Clipboard API write of text and PNG images.
    pub(crate) fn write_clipboard(&self, _content: ClipboardContent) {}

    /// Show or hide the hidden textarea (IME and soft keyboard) at a caret rectangle in CSS pixels
    /// relative to the canvas.
    pub(crate) fn set_text_input(&self, _caret_css: Option<egui::Rect>) {}

    pub(crate) fn open_url(&self, _url: &str, _new_tab: bool) {}
}
