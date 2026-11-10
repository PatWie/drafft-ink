//! Small DOM helpers shared by the web host modules: listener handles that remove themselves,
//! JS error text, focus without scrolling, modifier conversion and promise helpers.

use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{AddEventListenerOptions, EventTarget};

pub(super) fn window() -> Result<web_sys::Window, String> {
    web_sys::window().ok_or_else(|| "no window object".to_owned())
}

pub(super) fn document() -> Result<web_sys::Document, String> {
    window()?.document().ok_or_else(|| "no document object".to_owned())
}

/// Readable text for a thrown JS value (a `DOMException`, an `Error`, a string, anything else).
pub(super) fn js_error(value: &JsValue) -> String {
    if let Some(text) = value.as_string() {
        return text;
    }
    let field = |name: &str| js_sys::Reflect::get(value, &JsValue::from_str(name)).ok().and_then(|v| v.as_string());
    match (field("name"), field("message")) {
        (Some(name), Some(message)) => format!("{name}: {message}"),
        (None, Some(message)) => message,
        _ => format!("{value:?}"),
    }
}

/// How a listener is registered.
#[derive(Clone, Copy, Default)]
pub(super) struct ListenOptions {
    pub capture: bool,
    /// `Some(false)` makes the listener able to call `preventDefault` on wheel and touch events.
    pub passive: Option<bool>,
}

/// A registered DOM listener. Dropping it removes the listener again.
pub(super) struct Listener {
    target: EventTarget,
    name: &'static str,
    capture: bool,
    closure: Closure<dyn FnMut(web_sys::Event)>,
}

impl Listener {
    /// Register `handler` for `name` on `target`. Events that are not an `E` (for example a plain
    /// `Event` named `paste`) are ignored.
    pub(super) fn new<E: JsCast + 'static>(
        target: &EventTarget,
        name: &'static str,
        options: ListenOptions,
        mut handler: impl FnMut(E) + 'static,
    ) -> Result<Self, String> {
        let closure = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
            if let Ok(event) = event.dyn_into::<E>() {
                handler(event);
            }
        });
        let init = AddEventListenerOptions::new();
        init.set_capture(options.capture);
        if let Some(passive) = options.passive {
            init.set_passive(passive);
        }
        target
            .add_event_listener_with_callback_and_add_event_listener_options(
                name,
                closure.as_ref().unchecked_ref(),
                &init,
            )
            .map_err(|e| format!("add_event_listener({name}): {}", js_error(&e)))?;
        Ok(Self { target: target.clone(), name, capture: options.capture, closure })
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        if let Err(e) = self.target.remove_event_listener_with_callback_and_bool(
            self.name,
            self.closure.as_ref().unchecked_ref(),
            self.capture,
        ) {
            log::warn!("remove_event_listener({}): {}", self.name, js_error(&e));
        }
    }
}

/// Focus an element without letting the browser scroll it into view.
pub(super) fn focus_without_scroll(element: &web_sys::HtmlElement) {
    let options = web_sys::FocusOptions::new();
    options.set_prevent_scroll(true);
    if let Err(e) = element.focus_with_options(&options) {
        log::warn!("focus failed: {}", js_error(&e));
    }
}

/// Is `element` the document's active element?
pub(super) fn is_active(element: &web_sys::Element) -> bool {
    document().ok().and_then(|d| d.active_element()).is_some_and(|active| active == *element)
}

/// Apple platforms (macOS, iOS, iPadOS) use Cmd for shortcuts.
pub(super) fn detect_mac() -> bool {
    let Ok(window) = window() else { return false };
    let agent = window.navigator().user_agent().unwrap_or_default();
    ["Mac", "iPhone", "iPad", "iPod"].iter().any(|needle| agent.contains(needle))
}

/// egui modifiers from the four DOM modifier flags. `command` is Cmd on Apple platforms, Ctrl elsewhere,
/// the same rule egui-winit uses natively.
pub(super) fn modifiers(is_mac: bool, alt: bool, ctrl: bool, shift: bool, meta: bool) -> egui::Modifiers {
    egui::Modifiers { alt, ctrl, shift, mac_cmd: is_mac && meta, command: if is_mac { meta } else { ctrl } }
}

pub(super) fn modifiers_from_mouse(is_mac: bool, e: &web_sys::MouseEvent) -> egui::Modifiers {
    modifiers(is_mac, e.alt_key(), e.ctrl_key(), e.shift_key(), e.meta_key())
}

pub(super) fn modifiers_from_key(is_mac: bool, e: &web_sys::KeyboardEvent) -> egui::Modifiers {
    modifiers(is_mac, e.alt_key(), e.ctrl_key(), e.shift_key(), e.meta_key())
}

/// `object[name](...args)`.
pub(super) fn call_method(object: &JsValue, name: &str, args: &[JsValue]) -> Result<JsValue, JsValue> {
    let function: js_sys::Function = js_sys::Reflect::get(object, &JsValue::from_str(name))?
        .dyn_into()
        .map_err(|_| JsValue::from_str(&format!("{name} is not a function")))?;
    let list = js_sys::Array::new();
    for arg in args {
        list.push(arg);
    }
    function.apply(object, &list)
}

/// Await a JS value that is a promise (or a plain value).
pub(super) async fn await_value(value: JsValue) -> Result<JsValue, JsValue> {
    JsFuture::from(js_sys::Promise::resolve(&value)).await
}

/// `navigator.clipboard`, or `None` in an insecure context or an old browser.
pub(super) fn navigator_clipboard() -> Option<JsValue> {
    let navigator = window().ok()?.navigator();
    let clipboard = js_sys::Reflect::get(&navigator, &JsValue::from_str("clipboard")).ok()?;
    (!clipboard.is_undefined() && !clipboard.is_null()).then_some(clipboard)
}

pub(super) fn has_property(object: &JsValue, name: &str) -> bool {
    js_sys::Reflect::has(object, &JsValue::from_str(name)).unwrap_or(false)
}

/// Largest single file or clipboard image the host reads into memory.
pub(super) const MAX_FILE_BYTES: f64 = 256.0 * 1024.0 * 1024.0;

/// All bytes of a `Blob` or `File`. `Err` for a blob over [`MAX_FILE_BYTES`] or when the read fails.
pub(super) async fn read_blob_bytes(blob: &JsValue) -> Result<Vec<u8>, String> {
    let size = js_sys::Reflect::get(blob, &JsValue::from_str("size")).ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
    if size > MAX_FILE_BYTES {
        return Err(format!("{size} bytes is over the {MAX_FILE_BYTES} byte limit"));
    }
    let promise = call_method(blob, "arrayBuffer", &[]).map_err(|e| js_error(&e))?;
    let buffer = await_value(promise).await.map_err(|e| js_error(&e))?;
    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
}

/// The text of a `Blob`.
pub(super) async fn read_blob_text(blob: &JsValue) -> Result<String, String> {
    let promise = call_method(blob, "text", &[]).map_err(|e| js_error(&e))?;
    let text = await_value(promise).await.map_err(|e| js_error(&e))?;
    text.as_string().ok_or_else(|| "blob.text() did not give a string".to_owned())
}

/// A Blob with `type` made from bytes.
pub(super) fn blob_from_bytes(bytes: &[u8], mime: &str) -> Result<web_sys::Blob, String> {
    let parts = js_sys::Array::new();
    parts.push(&js_sys::Uint8Array::from(bytes));
    let options = web_sys::BlobPropertyBag::new();
    options.set_type(mime);
    web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options).map_err(|e| js_error(&e))
}
