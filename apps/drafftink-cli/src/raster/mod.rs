//! PNG export without a GPU (Cargo feature `png`).
//!
//! The app draws a picture by encoding the document into a `vello::Scene` ([`compose`]) and letting Vello's compute
//! shaders rasterise it. This tool has no GPU, so it reads the same scene back with [`drafftink_render::replay`] and
//! rasterises it with `vello_cpu`, which is pure Rust and needs no system library. The scene, and with it every
//! stroke, hatch, glyph and picture, is the one the app draws; only the rasteriser differs, so the pixels agree up to
//! anti-aliasing at the edges.
//!
//! [`PngRenderer`] is the `Renderer` port of a command line session: `doc.export` with format `png` goes through it.
//! With the `png-gpu` feature it draws on a GPU when the machine has one (a real one, or a software Vulkan driver) and
//! on the CPU otherwise.

mod canvas;
mod compose;
mod error;
#[cfg(feature = "png-gpu")]
mod gpu;
mod renderer;

pub use error::RasterError;
pub use renderer::PngRenderer;
