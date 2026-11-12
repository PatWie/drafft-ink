//! The WebGL2 back end for browsers without WebGPU (Cargo feature `webgl`).
//!
//! * `drafftink_render::replay` reads a `vello::Scene` back into immediate-mode calls. Pure, and tested natively.
//! * `canvas`: those calls on `vello_gpu`'s scene.
//! * `renderer` (wasm only): `vello_gpu`'s WebGL2 renderer on a canvas, with the pictures in its atlas.
//! * `backend` (wasm only): the shell's [`Backend`](crate::backend::Backend): scene texture, egui_glow, context loss.
//! * `offscreen` (wasm only): PNG export without WebGPU.

#[cfg(target_arch = "wasm32")]
mod backend;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod canvas;
#[cfg(target_arch = "wasm32")]
mod offscreen;
#[cfg(target_arch = "wasm32")]
mod renderer;

#[cfg(target_arch = "wasm32")]
pub(crate) use backend::GlBackend;
pub use drafftink_render::replay::ReplayStats;
#[cfg(target_arch = "wasm32")]
pub use offscreen::GlOffscreen;
