//! drafftink-gpu (L6): the winit shell for native and web, and the two drawing back ends under it: Vello + egui on wgpu
//! (feature `wgpu`, the default) and `vello_gpu` + egui_glow on WebGL2 for browsers without WebGPU (feature `webgl`).
//! See `docs/shell.md` and `docs/web-app.md`.
// `Instant` here is `web_time::Instant`, which works in the browser; clippy.toml bans std's version
// because it panics on wasm32, and it cannot tell the two apart on native targets.
#![allow(clippy::disallowed_methods)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

#[cfg(all(not(target_arch = "wasm32"), not(feature = "wgpu")))]
compile_error!("drafftink-gpu needs the `wgpu` feature on native targets; `webgl` is the browser fallback only");

pub mod app;
mod backend;
#[cfg(feature = "wgpu")]
pub mod compositor;
pub mod error;
mod cursor;
pub mod events;
pub mod frame;
#[cfg(feature = "webgl")]
pub mod gl;
#[cfg(feature = "wgpu")]
pub mod gpu;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod host;
#[cfg(not(target_arch = "wasm32"))]
mod native;
mod repaint;
mod settle;
#[cfg(target_arch = "wasm32")]
pub mod web;
#[cfg(feature = "wgpu")]
mod wgpu_backend;

pub use app::*;
pub use backend::{BackendKind, Detail};
#[cfg(feature = "wgpu")]
pub use compositor::*;
pub use error::*;
pub use events::*;
pub use frame::*;
#[cfg(feature = "wgpu")]
pub use gpu::*;
pub use shell::{Options, ShellError, frames_drawn, run, vello_runs};

/// Redraw reasons are part of the contract with the app.
pub use drafftink_profile::RedrawReason;
pub mod shell;
