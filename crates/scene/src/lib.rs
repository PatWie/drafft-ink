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

pub mod color;
pub mod derived;
pub mod document;
pub mod element;
pub mod geom;
pub mod id;
pub mod limits;
pub mod memory;
pub mod palette;
pub mod store;
pub mod style;
pub mod validate;
pub mod words;
pub mod zindex;

#[cfg(any(test, feature = "conformance"))]
pub mod conformance;

pub use color::Color;
pub use derived::DerivedIndex;
pub use document::{DocMeta, Document, DocumentError, FileEntry, SCHEMA_VERSION, json_schema};
pub use element::{
    BindMode, Binding, Brush, Crop, Element, ElementKind, FixedSegment, FrameData, FreehandData, Head, ImageData, Kind,
    LinearData, MathData, ParseKindError, PathType, PressureMode, TextData, TextRun, Transform,
};
pub use id::{ElementId, FileId, GroupId, IdError, IdSource, RandomIds, Rng64, SeqIds, SplitMix64};
pub use memory::MemoryStore;
pub use store::{
    Change, ChangeKind, ChangeSet, Fields, SceneRead, SceneStore, SceneTx, StoreError, SubscriptionId, load_document,
    snapshot,
};
pub use style::{
    FillPattern, FontFamily, FontWeight, Sloppiness, StrokeStyle, Style, TextAlign, TextStyle, VerticalAlign,
};
pub use validate::{
    DocError, ElementError, Issue, Report, Severity, element_errors, is_image_media_type, is_valid_name,
    validate_document, validate_element, validate_scene,
};
pub use zindex::{IndexError, ZIndex};
