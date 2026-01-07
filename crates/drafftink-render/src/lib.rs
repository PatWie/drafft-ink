//! DrafftInk Render Library
//!
//! Renderer abstraction and implementations for DrafftInk.
//! The default implementation uses Vello for GPU-accelerated rendering.

mod renderer;
pub mod text_editor;

#[cfg(feature = "vello-renderer")]
mod vello_impl;

#[cfg(feature = "vello-renderer")]
pub mod rex_backend;

pub use renderer::{
    AngleSnapInfo, GridStyle, RenderContext, Renderer, RendererError, RotationInfo,
};
pub use text_editor::{TextEditResult, TextEditState, TextKey, TextModifiers};

#[cfg(feature = "vello-renderer")]
pub use vello_impl::{PngRenderResult, VelloRenderer};
