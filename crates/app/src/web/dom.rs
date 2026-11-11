//! Small DOM helpers for the web platform.

use wasm_bindgen::prelude::*;

use super::WebError;

/// The window.
pub(crate) fn window() -> Result<web_sys::Window, WebError> {
    web_sys::window().ok_or(WebError::NoWindow)
}

/// The document.
pub(crate) fn document() -> Result<web_sys::Document, WebError> {
    window()?.document().ok_or(WebError::NoWindow)
}

/// Readable text for a thrown JS value (a `DOMException`, an `Error`, a string, anything else).
pub(crate) fn js_error(value: &JsValue) -> String {
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

/// Runs `f` once after `ms` milliseconds.
pub(crate) fn after(ms: i32, f: impl FnOnce() + 'static) {
    let Some(window) = web_sys::window() else { return };
    let callback = Closure::once_into_js(f);
    // A refused timer only means the cleanup does not happen; nothing depends on it.
    let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(callback.unchecked_ref(), ms);
}

/// Shows a short message at the bottom of the page. It goes away on a click or after `seconds`.
pub(crate) fn show_notice(text: &str, seconds: i32) {
    let Ok(document) = document() else { return };
    let Some(body) = document.body() else { return };
    let Ok(div) = document.create_element("div") else { return };
    div.set_text_content(Some(text));
    let style = "position:fixed;left:50%;bottom:16px;transform:translateX(-50%);max-width:min(90vw,560px);\
                 padding:10px 14px;border-radius:8px;background:#2b2b33;color:#f2f2f5;border:1px solid #55555f;\
                 font:14px/1.4 system-ui,sans-serif;z-index:10;cursor:pointer;box-shadow:0 4px 16px #0006";
    let _ = div.set_attribute("style", style);
    let _ = div.set_attribute("role", "status");
    if body.append_child(&div).is_err() {
        return;
    }
    let on_click = {
        let div = div.clone();
        Closure::once_into_js(move || div.remove())
    };
    let _ = div.add_event_listener_with_callback("click", on_click.unchecked_ref());
    after(seconds.saturating_mul(1000), move || div.remove());
}
