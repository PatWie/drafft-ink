//! Web demo. Built as a cdylib for wasm32 only; `scripts/build_web_demo.sh` runs wasm-bindgen on it.
#![cfg(target_arch = "wasm32")]

#[path = "common/demo_app.rs"]
mod demo_app;

use wasm_bindgen::prelude::*;

/// Start the demo on the canvas with this id. Returns at once; GPU setup finishes asynchronously.
/// `tick_ms` > 0 makes the app request a repaint every that many ms (tests the timer path).
#[wasm_bindgen]
pub fn start(canvas_id: String, tick_ms: u32) -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Info);
    drafftink_profile::set_enabled(true);
    let mut app = demo_app::Demo::new();
    app.tick_ms = (tick_ms > 0).then_some(u64::from(tick_ms));
    drafftink_gpu::run(app, drafftink_gpu::Options { canvas_id, ..Default::default() })
        .map_err(|e| JsValue::from_str(&e.to_string()))
}

/// `[frames_drawn, vello_runs]` for the test harness.
#[wasm_bindgen]
pub fn stats() -> Vec<f64> {
    vec![drafftink_gpu::frames_drawn() as f64, drafftink_gpu::vello_runs() as f64]
}
