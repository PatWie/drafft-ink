//! Opening files: which formats the package reads.
//!
//! `drafftink_io::open` reads every format, including the old app's Loro snapshots, and for that it links the Loro
//! CRDT engine. This package keeps to the in-memory store and stays small, so it routes the formats itself and
//! leaves the snapshots out.
//!
//! | Bytes | Result |
//! |---|---|
//! | native JSON (`toJSON`) | the document |
//! | PNG or SVG exported by `toPNG` / `toSVG` | the scene inside |
//! | `.excalidraw` scene | imported |
//! | old drafft-ink JSON | migrated |
//! | gzip of one of these | unpacked, then as above |

use drafftink_io::native::{self, Format};
use drafftink_io::{FormatName, IoError, TextMeasure};
use drafftink_io::{excalidraw, legacy, png_meta, svg};
use drafftink_scene::{Document, IdSource};

use crate::error::ApiError;

/// A scene inside a picture, inside a compressed file, may be nested this deep.
const MAX_NESTING: u8 = 3;

/// Reads a document from the bytes of a file.
pub(crate) fn read_document(
    bytes: &[u8],
    ids: &mut dyn IdSource,
    measure: &dyn TextMeasure,
) -> Result<Document, ApiError> {
    read(bytes, ids, measure, 0)
}

fn read(bytes: &[u8], ids: &mut dyn IdSource, measure: &dyn TextMeasure, depth: u8) -> Result<Document, ApiError> {
    if depth > MAX_NESTING {
        return Err(ApiError::invalid("the file holds a scene inside a scene inside a scene; it is too deeply nested"));
    }
    let format = native::sniff(bytes);
    let text =
        || std::str::from_utf8(bytes).map_err(|_| IoError::parse(FormatName::Text, "the data is not UTF-8 text"));
    match format {
        Format::Native => Ok(native::load(text()?)?),
        Format::LegacyJson => Ok(legacy::migrate_json(text()?, ids, measure)?.document),
        Format::ExcalidrawScene => Ok(excalidraw::import_scene(text()?, ids)?.document),
        Format::Gzip => read(&legacy::gunzip(bytes)?, ids, measure, depth + 1),
        Format::Png | Format::Svg => {
            let scene =
                if format == Format::Png { png_meta::extract_scene(bytes)? } else { svg::extract_scene(text()?)? };
            match scene {
                Some(scene) => read(scene.as_bytes(), ids, measure, depth + 1),
                None => Err(ApiError::invalid(
                    "this picture has no scene inside it; only pictures made by toPNG or toSVG can be opened as a drawing",
                )),
            }
        }
        _ => Err(ApiError::invalid(
            "this file cannot be opened here. The package reads its own JSON, PNG and SVG exports, .excalidraw scenes and old drafft-ink JSON",
        )),
    }
}
