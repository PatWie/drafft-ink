//! One entry point for "the user opened (or pasted, or dropped) these bytes".
//!
//! [`open`] sniffs the bytes ([`crate::native::sniff`]) and routes them to the right reader. A PNG or an SVG that
//! carries a scene opens as that scene. The result says what the bytes were, so the caller can choose between
//! replacing the document, adding a library tab and importing a diagram.

use drafftink_scene::IdSource;

use crate::error::{ImportReport, Imported, IoError};
use crate::excalidraw::{self, Library};
use crate::legacy;
use crate::measure::TextMeasure;
use crate::mermaid::{self, Diagram};
use crate::native::{self, Format};
use crate::{png_meta, svg};

/// What [`open`] made of the bytes.
#[derive(Debug, Clone, PartialEq)]
pub enum Opened {
    /// A document: native, old drafft-ink, Excalidraw scene or clipboard, or a PNG/SVG with an embedded scene.
    Document {
        /// The document and its caveats.
        imported: Imported,
        /// What the bytes were.
        source: Format,
    },
    /// An Excalidraw library.
    Library {
        /// The items.
        library: Library,
        /// Caveats.
        report: ImportReport,
    },
    /// A Mermaid diagram, still to be laid out by the edit crate.
    Diagram(Diagram),
}

/// Opens bytes of any supported format. Never panics; unknown or empty input is an [`IoError::WrongFormat`] whose
/// hint says what is supported.
pub fn open(bytes: &[u8], ids: &mut dyn IdSource, measure: &dyn TextMeasure) -> Result<Opened, IoError> {
    open_at(bytes, ids, measure, 0)
}

const MAX_NESTING: u8 = 3;

fn open_at(bytes: &[u8], ids: &mut dyn IdSource, measure: &dyn TextMeasure, depth: u8) -> Result<Opened, IoError> {
    let format = native::sniff(bytes);
    let text = || std::str::from_utf8(bytes).map_err(|_| IoError::parse("text", "the data is not UTF-8 text"));
    let doc = |imported: Imported| Opened::Document { imported, source: format };
    match format {
        Format::Native => {
            let document = native::load(text()?)?;
            Ok(doc(Imported { document, report: ImportReport::default() }))
        }
        Format::LegacyJson => Ok(doc(legacy::migrate_json(text()?, ids, measure)?)),
        Format::LegacyClipboard => Ok(doc(legacy::migrate_clipboard(text()?, ids, measure)?)),
        Format::LegacySnapshot => Ok(doc(legacy::migrate_snapshot(bytes, ids, measure)?)),
        Format::Gzip => {
            if depth >= MAX_NESTING {
                return Err(IoError::invalid("gzip", "compressed data inside compressed data, too deep"));
            }
            open_at(&legacy::gunzip(bytes)?, ids, measure, depth + 1)
        }
        Format::ExcalidrawScene => Ok(doc(excalidraw::import_scene(text()?, ids)?)),
        Format::ExcalidrawClipboard => Ok(doc(excalidraw::import_clipboard(text()?, ids)?)),
        Format::ExcalidrawLibrary => {
            let (library, report) = excalidraw::import_library(text()?, ids)?;
            Ok(Opened::Library { library, report })
        }
        Format::Mermaid => Ok(Opened::Diagram(mermaid::parse(text()?)?)),
        Format::Png | Format::Svg => {
            let scene = if format == Format::Png {
                png_meta::extract_scene(bytes)?
            } else {
                svg::extract_scene(text()?)?
            };
            let Some(scene) = scene else {
                return Err(IoError::WrongFormat {
                    expected: "a drafft.ink document",
                    hint: "this picture has no scene inside it; insert it as an image instead".into(),
                });
            };
            if depth >= MAX_NESTING {
                return Err(IoError::invalid("image", "a scene inside a scene inside a scene, too deep"));
            }
            match open_at(scene.as_bytes(), ids, measure, depth + 1)? {
                Opened::Document { imported, .. } => Ok(doc(imported)),
                other => Ok(other),
            }
        }
        Format::Unknown => Err(IoError::WrongFormat {
            expected: "a supported file",
            hint: "drafft.ink opens its own .json files, old drafft-ink files, .excalidraw, .excalidrawlib, Mermaid text, and PNG or SVG files exported from drafft.ink".into(),
        }),
    }
}
