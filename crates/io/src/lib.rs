//! drafftink-io (L4): every file format, pure.
//!
//! Bytes (or text) in, a scene [`Document`](drafftink_scene::Document) or plain data out. There is no OS file
//! access here: the engine passes bytes through its ports. Every parser treats its input as hostile: it returns a
//! typed [`IoError`], never panics, and keeps memory bounded by the limits in [`limits`].
//!
//! | Module | Format |
//! |---|---|
//! | [`native`] | the versioned native JSON, plus format sniffing ([`native::sniff`]) |
//! | [`legacy`] | old drafft-ink JSON documents and old Loro snapshots, migrated to the native model |
//! | [`excalidraw`] | `.excalidraw`, `.excalidrawlib` (v1, v2) and the Excalidraw clipboard, import and export |
//! | [`mermaid`] | Mermaid flowchart and sequence diagram parsing |
//! | [`svg`] | SVG export (with the scene embedded for re-import) |
//! | [`png_meta`] | scene JSON inside a PNG text chunk, writer and reader |
//! | [`facts`] | recomputes the facts of the legacy fixtures and compares them (`cargo xtask fixtures --check`) |

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod error;
pub mod excalidraw;
pub mod facts;
pub mod legacy;
pub mod limits;
pub mod measure;
pub mod mermaid;
pub mod native;
pub mod png_meta;
pub mod svg;

pub use error::{ImportReport, Imported, IoError};
pub use measure::{EstimateMeasure, MathBox, TextMeasure};
