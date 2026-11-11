//! The wasm entry point.

use drafftink_app::web::{WebOptions, platform};
use drafftink_app::{App, AppConfig};
use wasm_bindgen::prelude::*;

use crate::{agent, api};

/// True when the address asks for `?perf=1` (or `perf=overlay`), or the page set `window.__drafftinkPerf`.
fn perf_requested(search: &str) -> (bool, bool) {
    let flag = search.trim_start_matches('?').split('&').find_map(|p| p.strip_prefix("perf="));
    let global = web_sys::window()
        .and_then(|w| js_sys::Reflect::get(&w, &JsValue::from_str("__drafftinkPerf")).ok())
        .is_some_and(|v| v.is_truthy());
    let overlay = flag == Some("overlay");
    (global || overlay || matches!(flag, Some("1" | "true" | "on")), overlay)
}

/// A `performance.mark` for the start-up timeline (`scripts/perf_scenarios.py` reads them; the page's own marks are in
/// `window.__marks`). Costs a microsecond; a page without `performance` skips it.
fn mark(name: &str) {
    if let Some(performance) = web_sys::window().and_then(|w| w.performance()) {
        let _ = performance.mark(name);
    }
}

/// Starts Drafft.ink on the canvas with this id. Resolves when the app is running; the first frame follows
/// once the GPU is ready. If WebGPU is missing the shell shows its error page and this rejects.
#[wasm_bindgen]
pub async fn start(canvas_id: String) -> Result<(), JsValue> {
    mark("drafftink:start");
    console_error_panic_hook::set_once();
    // A second call (hot reload in development) finds the logger set; that is fine.
    let _ = console_log::init_with_level(log::Level::Info);

    let window = web_sys::window().ok_or_else(|| JsValue::from_str("this code must run in a browser page"))?;
    let search = window.location().search().unwrap_or_default();
    let (perf, overlay) = perf_requested(&search);
    drafftink_profile::set_enabled(perf);

    let options = WebOptions::detect(canvas_id.clone()).map_err(|e| JsValue::from_str(&e.to_string()))?;
    let join = options.startup_link();
    let web = platform(options).await.map_err(|e| JsValue::from_str(&e.to_string()))?;

    mark("drafftink:platform");
    api::install(&web.jobs)?;
    agent::start_from_page();

    let config = AppConfig {
        join,
        // "/ws" means the server that delivered this page.
        default_server: Some("/ws".to_owned()),
        collab_token: None,
        open: None,
        perf_overlay: overlay,
        stored_default_target: true,
    };
    #[cfg_attr(not(feature = "scenarios"), allow(unused_mut))]
    let mut app = App::new(web.platform, config);
    mark("drafftink:app");
    #[cfg(feature = "scenarios")]
    crate::scenario::install(app.scene_loader())?;
    #[cfg(not(feature = "scenarios"))]
    crate::scenario::install_unavailable()?;
    drafftink_gpu::run(app, drafftink_gpu::Options { canvas_id, ..Default::default() })
        .map_err(|e| JsValue::from_str(&e.to_string()))
}
