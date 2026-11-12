//! Why a picture could not be made.

/// A picture the CPU renderer could not make.
#[derive(Debug, thiserror::Error)]
pub enum RasterError {
    /// The picture has a side the renderer cannot draw.
    #[error("the picture would be {width} by {height} pixels, and the command line draws at most {max} on a side")]
    Size {
        /// Requested width.
        width: u32,
        /// Requested height.
        height: u32,
        /// Longest side that is drawn.
        max: u32,
    },
    /// The request asks for selection marks, which only the app draws.
    #[error("selection outlines are drawn by the app only. Export without `overlay`")]
    Overlay,
    /// The document could not be read as a scene.
    #[error("the document could not be drawn: {0}")]
    Document(String),
    /// The GPU failed while drawing or handing the pixels back.
    #[error("the GPU could not draw the picture: {0}")]
    Gpu(String),
    /// The pixels could not be written as a PNG.
    #[error("could not encode the PNG: {0}")]
    Encode(#[from] png::EncodingError),
}
