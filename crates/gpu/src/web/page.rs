//! Page-level helpers: finding the canvas, WebGPU detection, the error page and opening links.

use wasm_bindgen::prelude::*;
use web_sys::HtmlCanvasElement;

use super::dom;

/// Find the canvas with this element id.
pub(crate) fn lookup_canvas(id: &str) -> Result<HtmlCanvasElement, String> {
    let document = dom::document()?;
    let element = document.get_element_by_id(id).ok_or_else(|| format!("no element with id `{id}` in the page"))?;
    element.dyn_into::<HtmlCanvasElement>().map_err(|_| format!("element `{id}` is not a <canvas>"))
}

/// `navigator.gpu` exists. It says the browser has WebGPU; whether an adapter is available is only known
/// after `requestAdapter`.
pub fn webgpu_available() -> bool {
    let Ok(window) = dom::window() else { return false };
    js_sys::Reflect::get(&window.navigator(), &JsValue::from_str("gpu"))
        .is_ok_and(|gpu| !gpu.is_undefined() && !gpu.is_null())
}

/// The primary pointer of the device is imprecise: a finger or a stylus. A mouse or a trackpad is `fine`; an old
/// browser without media queries answers `false`.
pub(crate) fn coarse_pointer() -> bool {
    dom::window()
        .ok()
        .and_then(|window| window.match_media("(pointer: coarse)").ok().flatten())
        .is_some_and(|query| query.matches())
}

const ERROR_PAGE_CSS: &str = "\
:root { color-scheme: light dark; }\
body { margin: 0; font: 16px/1.5 system-ui, -apple-system, 'Segoe UI', Roboto, sans-serif; }\
main { max-width: 40rem; margin: 0 auto; padding: 2rem 1.25rem; }\
h1 { font-size: 1.6rem; margin: 0 0 1rem; }\
h2 { font-size: 1.1rem; margin: 1.75rem 0 0.5rem; }\
pre { white-space: pre-wrap; overflow-wrap: anywhere; padding: 0.75rem; border: 1px solid currentcolor; border-radius: 4px; font-size: 0.85rem; }";

/// Replace the page body with a plain error page: the title, why the app cannot start, which browsers
/// work, and the technical details. Self-contained (no network, no scripts). Text goes in through
/// `textContent`, so `details` can hold anything.
pub fn show_error_page(title: &str, details: &str) {
    if let Err(e) = build_error_page(title, details) {
        // Last resort: the page stays as it is.
        log::error!("could not show the error page ({e}): {title}: {details}");
    }
}

fn add(parent: &web_sys::Element, child: web_sys::Element) -> Result<(), String> {
    parent.append_child(&child).map(|_| ()).map_err(|e| dom::js_error(&e))
}

fn build_error_page(title: &str, details: &str) -> Result<(), String> {
    let document = dom::document()?;
    let body = document.body().ok_or("document has no body")?;
    let fail = |e: JsValue| dom::js_error(&e);
    let make = |tag: &str, text: &str| -> Result<web_sys::Element, String> {
        let element = document.create_element(tag).map_err(fail)?;
        element.set_text_content(Some(text));
        Ok(element)
    };

    body.set_text_content(None);
    document.set_title(&format!("{title} - Drafft.ink"));

    let main = make("main", "")?;
    main.set_attribute("role", "main").map_err(fail)?;
    let style = make("style", ERROR_PAGE_CSS)?;
    add(&main, make("h1", title)?)?;
    add(
        &main,
        make(
            "p",
            "Drafft.ink draws with WebGPU, or with WebGL2 when WebGPU is missing. This browser or device provided neither, or they are turned off.",
        )?,
    )?;
    add(&main, make("h2", "Browsers that work")?)?;
    let list = make("ul", "")?;
    for line in [
        "Chrome or Edge 113 or newer. On Linux and on some phones this needs a recent version and a supported graphics driver.",
        "Firefox 141 or newer on Windows, 145 or newer on macOS.",
        "Safari 26 or newer on macOS, iOS and iPadOS.",
        "If your browser is new enough, update it and your graphics drivers, and check that hardware acceleration is on in its settings.",
    ] {
        add(&list, make("li", line)?)?;
    }
    add(&main, list)?;
    add(&main, make("h2", "Technical details")?)?;
    add(&main, make("pre", details)?)?;

    add(&body, style)?;
    add(&body, main)?;
    Ok(())
}

/// Open a link. Only `http:`, `https:` and `mailto:` are allowed; anything else (`javascript:`, `data:`,
/// `file:`, ...) is logged and ignored.
pub(crate) fn open_url(url: &str, new_tab: bool) {
    let Ok(parsed) = web_sys::Url::new(url.trim()) else {
        log::warn!("open_url: not an absolute URL, ignored: {url:?}");
        return;
    };
    let protocol = parsed.protocol();
    if !matches!(protocol.as_str(), "http:" | "https:" | "mailto:") {
        log::warn!("open_url: scheme {protocol:?} is not allowed, ignored");
        return;
    }
    let href = parsed.href();
    let Ok(window) = dom::window() else { return };
    if new_tab && protocol != "mailto:" {
        if let Err(e) = window.open_with_url_and_target_and_features(&href, "_blank", "noopener,noreferrer") {
            log::warn!("open_url: window.open failed: {}", dom::js_error(&e));
        }
    } else if let Err(e) = window.location().assign(&href) {
        log::warn!("open_url: navigation failed: {}", dom::js_error(&e));
    }
}
