//! Native demo: `cargo run -p drafftink-gpu --example demo`.
//! Environment: `DEMO_TICK_MS=100` makes the app request a repaint every 100 ms (tests the timer path).
//! The web build is `examples/demo_web.rs` (see scripts/build_web_demo.sh).
#![cfg_attr(target_arch = "wasm32", allow(unused))]

#[path = "common/demo_app.rs"]
mod demo_app;

#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), drafftink_gpu::ShellError> {
    env_logger::init();
    drafftink_profile::set_enabled(true);
    let mut app = demo_app::Demo::new();
    app.tick_ms = std::env::var("DEMO_TICK_MS").ok().and_then(|v| v.parse().ok());
    let result =
        drafftink_gpu::run(app, drafftink_gpu::Options { title: "drafftink-gpu demo".into(), ..Default::default() });
    // The idle check reads this line.
    eprintln!("frames={} vello_runs={}", drafftink_gpu::frames_drawn(), drafftink_gpu::vello_runs());
    result
}

#[cfg(target_arch = "wasm32")]
fn main() {}
