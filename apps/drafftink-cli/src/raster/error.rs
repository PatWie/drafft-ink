//! Why a picture could not be made.

use drafftink_render::ExportError;
use drafftink_scene::StoreError;

/// A picture the CPU renderer could not make.
#[derive(Debug, thiserror::Error)]
pub enum RasterError {
    /// The picture has a side that cannot be drawn, or its pixels could not be written as a PNG.
    #[error(transparent)]
    Export(#[from] ExportError),
    /// The request asks for selection marks, which only the app draws.
    #[error("selection outlines are drawn by the app only. Export without `overlay`")]
    Overlay,
    /// The document could not be read as a scene.
    #[error("the document could not be drawn: {0}")]
    Document(#[from] StoreError),
    /// The GPU failed while drawing or handing the pixels back.
    #[cfg(feature = "png-gpu")]
    #[error("the GPU could not draw the picture: {0}")]
    Gpu(#[from] super::GpuError),
}
