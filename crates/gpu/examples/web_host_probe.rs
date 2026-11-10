//! Browser probe for the web host (`crates/gpu/src/web`). It installs the host on `<canvas id=canvas>`
//! and records every host event as a JSON string in `window.__events`. `scripts/web_host_test.py`
//! drives it with real browser input and asserts the recorded events.
//!
//! Build: see `scripts/web_host_test.py` (cargo build --example web_host_probe --target wasm32-unknown-unknown,
//! then `wasm-bindgen --target web`). On native targets this example does nothing.

#[cfg(not(target_arch = "wasm32"))]
fn main() {}

#[cfg(target_arch = "wasm32")]
fn main() {
    probe::start();
}

#[cfg(target_arch = "wasm32")]
mod probe {
    use std::cell::RefCell;

    use drafftink_gpu::web::probe::ProbeHost;
    use wasm_bindgen::prelude::*;

    thread_local! {
        static HOST: RefCell<Option<ProbeHost>> = const { RefCell::new(None) };
    }

    fn with_host(f: impl FnOnce(&ProbeHost)) {
        HOST.with(|h| {
            if let Ok(h) = h.try_borrow()
                && let Some(host) = h.as_ref()
            {
                f(host);
            }
        });
    }

    fn fail(message: &str) {
        web_sys::console::error_1(&JsValue::from_str(message));
        if let Some(window) = web_sys::window() {
            let _ = js_sys::Reflect::set(&window, &JsValue::from_str("__probe_error"), &JsValue::from_str(message));
        }
    }

    pub fn start() {
        console_error_panic_hook::set_once();
        let _ = console_log::init_with_level(log::Level::Debug);
        let Some(window) = web_sys::window() else { return fail("no window") };
        let events = js_sys::Array::new();
        let _ = js_sys::Reflect::set(&window, &JsValue::from_str("__events"), &events);
        let canvas = match drafftink_gpu::web::probe::lookup_canvas("canvas") {
            Ok(canvas) => canvas,
            Err(e) => return fail(&e),
        };
        let hook_window = window.clone();
        let callback = Closure::<dyn FnMut(JsValue)>::new(move |json: JsValue| {
            events.push(&json);
            // Tests set `window.__hook` to act inside an event (for example write the clipboard
            // synchronously while a copy event is being dispatched).
            if let Ok(hook) = js_sys::Reflect::get(&hook_window, &JsValue::from_str("__hook"))
                && let Some(hook) = hook.dyn_ref::<js_sys::Function>()
            {
                let _ = hook.call1(&JsValue::NULL, &json);
            }
        });
        let function: js_sys::Function = callback.as_ref().unchecked_ref::<js_sys::Function>().clone();
        callback.forget(); // lives as long as the page
        match ProbeHost::install(&canvas, function) {
            Ok(host) => HOST.with(|h| *h.borrow_mut() = Some(host)),
            Err(e) => return fail(&e),
        }
        let _ = js_sys::Reflect::set(&window, &JsValue::from_str("__probe_ready"), &JsValue::TRUE);
    }

    #[wasm_bindgen]
    pub fn probe_write_clipboard(text: &str) {
        with_host(|h| h.write_clipboard(Some(text.to_owned()), None, None));
    }

    /// Text + PNG bytes + custom payload.
    #[wasm_bindgen]
    pub fn probe_write_clipboard_rich(text: &str, png: Vec<u8>, custom: &str) {
        with_host(|h| h.write_clipboard(Some(text.to_owned()), Some(png), Some(custom.as_bytes().to_vec())));
    }

    #[wasm_bindgen]
    pub fn probe_set_text_input(x: f32, y: f32, w: f32, h: f32) {
        with_host(|host| host.set_text_input(Some((x, y, w, h))));
    }

    #[wasm_bindgen]
    pub fn probe_clear_text_input() {
        with_host(|host| host.set_text_input(None));
    }

    #[wasm_bindgen]
    pub fn probe_read_clipboard() {
        with_host(ProbeHost::read_clipboard);
    }

    #[wasm_bindgen]
    pub fn probe_open_url(url: &str, new_tab: bool) {
        with_host(|h| h.open_url(url, new_tab));
    }

    /// Is the hidden textarea focused?
    #[wasm_bindgen]
    pub fn probe_text_input_focused() -> bool {
        HOST.with(|h| h.try_borrow().is_ok_and(|h| h.as_ref().is_some_and(ProbeHost::text_input_focused)))
    }

    #[wasm_bindgen]
    pub fn probe_webgpu_available() -> bool {
        drafftink_gpu::web::webgpu_available()
    }

    #[wasm_bindgen]
    pub fn probe_show_error_page(title: &str, details: &str) {
        drafftink_gpu::web::show_error_page(title, details);
    }

    /// Drop the host: all listeners and the textarea must go away.
    #[wasm_bindgen]
    pub fn probe_uninstall() {
        HOST.with(|h| {
            if let Ok(mut h) = h.try_borrow_mut()
                && let Some(host) = h.as_mut()
            {
                host.uninstall();
            }
        });
    }
}
