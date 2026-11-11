//! drafftink-scene (L1): the pure-data element model every other crate builds on.
//!
//! * [`Element`] and its kinds, [`Style`], [`ZIndex`] (fractional z-order), ids and colours: plain data with serde
//!   and JSON Schema. See `docs/model.md` for what every field means.
//! * [`validate_element`], [`validate_document`], [`validate_scene`]: the invariants, as functions with typed errors.
//! * [`SceneStore`]: read access, transactions with change sets, notifications, derived indexes. [`MemoryStore`] is
//!   the in-memory implementation; the `conformance` feature exposes the tests every store must pass.
//! * [`Document`]: the versioned native JSON format.
//!
//! No clock, no global randomness, no I/O: ids and seeds come from an injected [`IdSource`].

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
#![warn(missing_docs)]

mod color;
mod derived;
mod document;
mod element;
mod id;
pub mod limits;
mod memory;
pub mod palette;
mod points;
#[cfg(feature = "scenegen")]
pub mod scenegen;
mod store;
mod style;
mod validate;
pub mod words;
mod zindex;

#[cfg(any(test, feature = "conformance"))]
pub mod conformance;

pub use color::Color;
pub use derived::DerivedIndex;
pub use document::{DocMeta, Document, DocumentError, FORMAT_NAME, FileEntry, SCHEMA_VERSION, json_schema};
pub use element::{
    BindMode, Binding, Brush, Crop, Element, ElementKind, FixedSegment, FrameData, FreehandData, Head, ImageData, Kind,
    LinearData, LinearKind, MathData, ParseKindError, PathType, PressureMode, RefitError, TextData, TextRun, Transform,
};
pub use id::{ElementId, FileId, GroupId, IdError, IdSource, MAX_ID_LEN, RandomIds, Rng64, SeqIds, SplitMix64};
pub use memory::{MemoryStore, MemoryTx};
pub use store::{
    Change, ChangeKind, ChangeSet, Fields, FileRejection, SceneRead, SceneStore, SceneTx, StoreError, SubscriptionId,
    load_document, snapshot,
};
pub use style::{
    FillPattern, FontFamily, FontWeight, InlineMath, Sloppiness, StrokeStyle, Style, TextAlign, TextStyle,
    VerticalAlign,
};
pub use validate::{
    DocError, ElementError, Issue, Report, Severity, element_errors, is_image_media_type, is_valid_name,
    validate_document, validate_element, validate_scene, validate_transform,
};
pub use zindex::{IndexError, MAX_INDEX_LEN, ZIndex};
