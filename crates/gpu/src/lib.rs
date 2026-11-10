//! drafftink-gpu (L6): the wgpu device and surface, the Vello + egui compositor, and the winit shell
//! for native and web. See `docs/shell.md`.
// `Instant` here is `web_time::Instant`, which works in the browser; clippy.toml bans std's version
// because it panics on wasm32, and it cannot tell the two apart on native targets.
#![allow(clippy::disallowed_methods)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod app;
pub mod compositor;
pub mod events;
pub mod gpu;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod host;
#[cfg(not(target_arch = "wasm32"))]
mod native;
mod repaint;
#[cfg(target_arch = "wasm32")]
pub mod web;

pub use app::*;
pub use compositor::*;
pub use events::*;
pub use gpu::*;
pub use shell::{Options, ShellError, frames_drawn, run, vello_runs};

/// Redraw reasons are part of the contract with the app.
pub use drafftink_profile::RedrawReason;
pub mod shell;
