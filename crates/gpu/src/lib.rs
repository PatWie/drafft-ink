//! drafftink-gpu (L6): the wgpu device and surface, the Vello + egui compositor, and the winit shell
//! for native and web. See `docs/shell.md`.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod events;
mod host;
#[cfg(target_arch = "wasm32")]
pub mod web;

pub use events::*;
