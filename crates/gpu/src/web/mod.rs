//! Web host: everything the browser needs that winit 0.30 does not provide. See docs/shell.md.
//!
//! winit already handles the canvas size (ResizeObserver, device pixel ratio), plain keys, wheel and
//! the cursor. This host adds what eframe used to do on the web, and what the app needs on top:
//!
//! * [`text_agent`]: hidden `<textarea>` for IME, composition and soft keyboards, and the key events
//!   winit cannot see while the textarea has focus;
//! * [`clipboard`]: `paste`/`copy`/`cut` events, async Clipboard API write and read, images;
//! * [`files`]: file drop;
//! * [`pointer`]: pen, touch and mouse samples with pressure, tilt and coalesced points;
//! * [`page`]: canvas lookup, WebGPU check, the error page, `open_url`;
//! * this file: canvas setup, the `preventDefault` policy, visibility and focus.
//!
//! The shell sets `prevent_default(false)` on the winit window (winit's `preventDefault` on every key
//! event would block the browser's `paste` event, F5 and the devtools keys). This host then cancels only the
//! browser actions the app wants for itself, see [`keys::should_prevent_default`].
#![cfg(target_arch = "wasm32")]

mod clipboard;
mod dom;
mod files;
mod keys;
mod page;
mod pointer;
#[doc(hidden)]
pub mod probe;
mod text_agent;

use web_sys::{EventTarget, HtmlCanvasElement, KeyboardEvent};

use dom::{ListenOptions, Listener};
pub(crate) use page::lookup_canvas;
pub use page::{show_error_page, webgpu_available};

use crate::events::{ClipboardContent, ShellEvent};
use crate::host::HostSink;

/// Listeners and elements that live as long as the app. Dropping the host removes every listener it
/// registered and the hidden textarea.
pub(crate) struct WebHost {
    text_agent: text_agent::TextAgent,
    clipboard: clipboard::Clipboard,
    /// Held so they are removed on drop.
    _listeners: Vec<Listener>,
}

impl WebHost {
    /// Prepare the canvas and register every listener. Call once, after the window exists.
    pub(crate) fn install(canvas: &HtmlCanvasElement, sink: HostSink) -> Result<Self, String> {
        prepare_canvas(canvas)?;
        let is_mac = dom::detect_mac();
        let text_agent = text_agent::TextAgent::attach(canvas, sink.clone(), is_mac)?;
        let core = text_agent.core();
        let clipboard = clipboard::Clipboard::install(sink.clone(), core.textarea.clone())?;

        let mut listeners = pointer::install(canvas, &sink, is_mac, core)?;
        listeners.extend(files::install(canvas, &sink)?);
        listeners.extend(canvas_listeners(canvas, is_mac)?);
        listeners.extend(page_listeners(&sink)?);
        Ok(Self { text_agent, clipboard, _listeners: listeners })
    }

    /// Put text, a PNG image and/or the app payload on the system clipboard (async Clipboard API;
    /// inside a `copy`/`cut` event plain text goes through the event). Never panics: a refusal is
    /// logged.
    pub(crate) fn write_clipboard(&self, content: ClipboardContent) {
        self.clipboard.write(content);
    }

    /// Read the system clipboard now (for a "Paste" button) and send the result as `ShellEvent::Paste`.
    /// Needs a user gesture and, in most browsers, the user's permission.
    pub(crate) fn read_clipboard(&self) {
        self.clipboard.read();
    }

    /// Show or hide the hidden textarea (IME and soft keyboard) at a caret rectangle in CSS pixels
    /// relative to the canvas. `Some` focuses the textarea the first time (the soft keyboard appears),
    /// later calls only move it. `None` blurs it (the keyboard hides) and focuses the canvas again.
    pub(crate) fn set_text_input(&self, caret_css: Option<egui::Rect>) {
        self.text_agent.set(caret_css);
    }

    /// The textarea has keyboard focus. While it does, winit sees a canvas blur and reports the window as
    /// unfocused; the host sends `egui::Event::WindowFocused(true)` for the textarea, and the shell should
    /// ignore winit's `Focused(false)` in this state if both events can arrive out of order.
    pub(crate) fn text_input_focused(&self) -> bool {
        self.text_agent.has_focus()
    }

    /// The primary input of the device is a finger or a stylus (`(pointer: coarse)`, true on phones and tablets).
    /// The app starts in its touch layout then, before the first touch.
    pub(crate) fn coarse_pointer(&self) -> bool {
        page::coarse_pointer()
    }

    /// Open `http:`, `https:` or `mailto:` links; anything else is logged and ignored.
    pub(crate) fn open_url(&self, url: &str, new_tab: bool) {
        page::open_url(url, new_tab);
    }
}

/// Canvas attributes and styles that the browser needs for pointer, touch and keyboard input.
fn prepare_canvas(canvas: &HtmlCanvasElement) -> Result<(), String> {
    let fail = |e: wasm_bindgen::JsValue| dom::js_error(&e);
    // A tab stop so the canvas can take keyboard focus (winit sets it too).
    if canvas.get_attribute("tabindex").is_none() {
        canvas.set_attribute("tabindex", "0").map_err(fail)?;
    }
    let style = canvas.style();
    for (name, value) in [
        // The page must not scroll or zoom under a finger or pen; the app handles those gestures.
        ("touch-action", "none"),
        ("outline", "none"),
        ("user-select", "none"),
        ("-webkit-user-select", "none"),
        ("-webkit-touch-callout", "none"),
        ("-webkit-tap-highlight-color", "transparent"),
        // No back/forward swipe or pull-to-refresh from a drag on the canvas.
        ("overscroll-behavior", "none"),
    ] {
        style.set_property(name, value).map_err(fail)?;
    }
    Ok(())
}

/// Listeners on the canvas for the keyboard `preventDefault` policy and the page gestures that would
/// otherwise zoom or scroll the page.
fn canvas_listeners(canvas: &HtmlCanvasElement, is_mac: bool) -> Result<Vec<Listener>, String> {
    let target: &EventTarget = canvas.as_ref();
    let mut out = Vec::new();
    out.push(Listener::new(target, "keydown", ListenOptions::default(), move |e: KeyboardEvent| {
        let key = e.key();
        if keys::should_prevent_default(&keys::facts(is_mac, &key, &e, false)) {
            e.prevent_default();
        }
    })?);
    // Ctrl+wheel (also a trackpad pinch) would zoom the page; a plain wheel would scroll or navigate.
    // Needs a non-passive listener to be allowed to cancel.
    let active = ListenOptions { capture: false, passive: Some(false) };
    for name in ["wheel", "gesturestart", "gesturechange", "gestureend"] {
        out.push(Listener::new(target, name, active, |e: web_sys::Event| e.prevent_default())?);
    }
    // Safari on iOS and iPadOS: a touch that is not cancelled can still scroll or zoom the page, or open the
    // text-selection and Scribble callouts, even with `touch-action: none` (older versions honour only part of
    // it). The pointer events carry the input; the touch events are only cancelled, which does not stop the
    // pointer events. Safari makes `touchstart` and `touchmove` listeners on the document passive by default, so
    // these sit on the canvas and say `passive: false`.
    for name in ["touchstart", "touchmove"] {
        out.push(Listener::new(target, name, active, |e: web_sys::Event| {
            if e.cancelable() {
                e.prevent_default();
            }
        })?);
    }
    Ok(out)
}

/// Page visibility and window focus.
fn page_listeners(sink: &HostSink) -> Result<Vec<Listener>, String> {
    let document = dom::document()?;
    let window = dom::window()?;
    let opts = ListenOptions::default();
    let mut out = Vec::new();

    let s = sink.clone();
    let doc = document.clone();
    out.push(Listener::new(document.as_ref(), "visibilitychange", opts, move |_: web_sys::Event| {
        s.app(ShellEvent::Visibility(!doc.hidden()));
    })?);
    let s = sink.clone();
    out.push(Listener::new(window.as_ref(), "focus", opts, move |_: web_sys::Event| s.app(ShellEvent::Focus(true)))?);
    let s = sink.clone();
    out.push(Listener::new(window.as_ref(), "blur", opts, move |_: web_sys::Event| s.app(ShellEvent::Focus(false)))?);
    Ok(out)
}
