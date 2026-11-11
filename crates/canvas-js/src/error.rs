//! The one error type of the bindings. Every message says what to do next, because it reaches a JS developer as
//! the text of a thrown `Error`.

use drafftink_edit::group::GroupError;
use drafftink_edit::lifecycle::LifecycleError;
use drafftink_edit::linear::LinearError;
use drafftink_edit::transform::TransformError;
use drafftink_edit::zorder::ZOrderError;
use drafftink_gpu::GpuError;
use drafftink_io::IoError;
use drafftink_scene::{DocumentError, ElementId, IdError, StoreError};

/// Why a call on the stage was refused. Nothing changes when a call fails.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// An argument is not usable. The text says which and what to pass instead.
    #[error("{0}")]
    Invalid(String),
    /// No live element has this id.
    #[error("no element has the id {0}; use an id that add() returned or that you gave to add()")]
    NotFound(ElementId),
    /// The selector text is not one of the forms the stage knows.
    #[error(transparent)]
    Selector(#[from] drafftink_canvas::SelectorError),
    /// An id is not made of letters, digits, `_` and `-`.
    #[error(transparent)]
    Id(#[from] IdError),
    /// The in-memory store refused a write (an invalid element, a duplicate id).
    #[error(transparent)]
    Store(#[from] StoreError),
    /// A move, resize or rotate was refused.
    #[error(transparent)]
    Transform(#[from] TransformError),
    /// Grouping or ungrouping was refused.
    #[error(transparent)]
    Group(#[from] GroupError),
    /// A z-order change was refused.
    #[error(transparent)]
    ZOrder(#[from] ZOrderError),
    /// A delete or lock was refused.
    #[error(transparent)]
    Lifecycle(#[from] LifecycleError),
    /// A point edit on a line or arrow was refused.
    #[error(transparent)]
    Linear(#[from] LinearError),
    /// The JSON does not have the shape model's layout.
    #[error("the value is not valid for the shape model: {0}")]
    Json(#[from] serde_json::Error),
    /// A document could not be read or written.
    #[error(transparent)]
    Document(#[from] DocumentError),
    /// A file could not be opened or an export failed.
    #[error(transparent)]
    Io(#[from] IoError),
    /// The GPU could not draw.
    #[error(transparent)]
    Gpu(#[from] GpuError),
    /// A PNG could not be encoded.
    #[error("PNG encoding failed: {0}")]
    Png(#[from] png::EncodingError),
}

impl ApiError {
    /// An [`ApiError::Invalid`] from text.
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
}
