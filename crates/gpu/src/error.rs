//! Errors shared by the drawing back ends.

use thiserror::Error;

/// Why the GPU could not be set up or a frame could not be drawn.
#[derive(Debug, Error)]
pub enum GpuError {
    #[error("no GPU adapter is available ({0}). Drafft.ink needs Vulkan, Metal, DirectX 12 or WebGPU")]
    NoAdapter(String),
    #[error("the GPU refused to start ({0})")]
    Device(String),
    #[error("could not create a drawing surface for the window ({0})")]
    Surface(String),
    #[error("the vello renderer could not be created ({0})")]
    Vello(String),
    #[error("the surface supports no usable pixel format")]
    NoFormat,
    #[error("rendering failed ({0})")]
    Render(String),
    #[error("reading back the image failed ({0})")]
    Readback(String),
    /// The WebGL2 fallback could not start or draw.
    #[error("WebGL2 could not be used ({0})")]
    WebGl(String),
}

/// Why the device is gone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LostInfo {
    pub destroyed: bool,
    pub message: String,
}
