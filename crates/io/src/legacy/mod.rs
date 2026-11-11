//! Migration of old drafft-ink files (commit `8ce40ab`, `docs/formats.md`) to the native model.
//!
//! * [`migrate_json`]: the old `CanvasDocument` JSON (no version field; detected structurally).
//! * [`migrate_gzip`]: the same, gzip-compressed (the large fixtures).
//! * [`migrate_snapshot`]: an old Loro snapshot from the collaboration code.
//! * [`migrate_clipboard`]: the old internal clipboard, a JSON array of shapes.
//!
//! Every function takes an [`IdSource`] (for shapes without a valid id and for missing seeds) and a [`TextMeasure`]
//! (old files do not store the size of text and formulas). Old ids that are valid and unique are kept.
//! Loading is lenient where the old loader failed the whole file: a shape that cannot be read is skipped and
//! listed in the [`ImportReport`].
//!
//! What changes (`docs/model.md` section 10 has the field table):
//! * Groups become `group_ids` stacks; a group needs two non-empty members to get an id.
//! * A group's rotation is baked into its members (their boxes move about the group's old pivot, their angles grow);
//!   lines and strokes move their points. The old app drew exactly that.
//! * Shapes missing from `z_order` were never drawn and are dropped.
//! * A `Line` or `Arrow` with fewer than three points loses `closed`. An `Angular` path stays two points (the old
//!   router ran at draw time); the edit crate routes it.

mod build;
mod crdt;
mod json;
mod model;

use std::io::Read;

use drafftink_scene::IdSource;
use serde_json::Value;

use crate::error::{Expected, FormatName, ImportReport, Imported, IoError};
use crate::limits::{MAX_INFLATED_BYTES, MAX_INPUT_BYTES};
use crate::measure::TextMeasure;

/// Deepest group nesting the readers follow.
pub(crate) const MAX_NEST: usize = 64;

/// True when `text` is JSON with the structure of an old document: an object with `shapes` and `z_order`.
pub fn is_legacy_json(text: &str) -> bool {
    serde_json::from_str::<Value>(text).is_ok_and(|v| json::looks_like_old_document(&v))
}

/// Migrates old document JSON.
pub fn migrate_json(text: &str, ids: &mut dyn IdSource, measure: &dyn TextMeasure) -> Result<Imported, IoError> {
    if text.len() > MAX_INPUT_BYTES {
        return Err(IoError::Limit { what: "document", max: format!("{MAX_INPUT_BYTES} bytes") });
    }
    let value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| IoError::parse(FormatName::LegacyDocument, e.to_string()))?;
    let mut report = ImportReport::default();
    let old = json::read_document(&value, &mut report)?;
    build::build(&old, ids, measure, report)
}

/// Migrates gzip-compressed old document JSON. Inflation is capped at [`MAX_INFLATED_BYTES`].
pub fn migrate_gzip(bytes: &[u8], ids: &mut dyn IdSource, measure: &dyn TextMeasure) -> Result<Imported, IoError> {
    let text = gunzip(bytes)?;
    let text = String::from_utf8(text)
        .map_err(|_| IoError::parse(FormatName::LegacyDocument, "the decompressed data is not UTF-8 text"))?;
    migrate_json(&text, ids, measure)
}

/// Inflates a gzip stream, refusing to produce more than [`MAX_INFLATED_BYTES`].
pub fn gunzip(bytes: &[u8]) -> Result<Vec<u8>, IoError> {
    gunzip_limited(bytes, MAX_INFLATED_BYTES)
}

/// [`gunzip`] with an explicit output limit.
pub fn gunzip_limited(bytes: &[u8], max: usize) -> Result<Vec<u8>, IoError> {
    let mut out = Vec::new();
    let limit = u64::try_from(max).unwrap_or(u64::MAX);
    flate2::read::GzDecoder::new(bytes)
        .take(limit.saturating_add(1))
        .read_to_end(&mut out)
        .map_err(|e| IoError::parse(FormatName::Gzip, e.to_string()))?;
    if out.len() > max {
        return Err(IoError::Limit { what: "decompressed data", max: format!("{max} bytes") });
    }
    Ok(out)
}

/// Migrates an old Loro snapshot.
pub fn migrate_snapshot(bytes: &[u8], ids: &mut dyn IdSource, measure: &dyn TextMeasure) -> Result<Imported, IoError> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(IoError::Limit { what: "snapshot", max: format!("{MAX_INPUT_BYTES} bytes") });
    }
    let mut report = ImportReport::default();
    let old = crdt::read_snapshot(bytes, &mut report)?;
    build::build(&old, ids, measure, report)
}

/// Migrates the old internal clipboard (a JSON array of externally tagged shapes) into a document.
pub fn migrate_clipboard(text: &str, ids: &mut dyn IdSource, measure: &dyn TextMeasure) -> Result<Imported, IoError> {
    if text.len() > MAX_INPUT_BYTES {
        return Err(IoError::Limit { what: "clipboard", max: format!("{MAX_INPUT_BYTES} bytes") });
    }
    let value: Value =
        serde_json::from_str(text).map_err(|e| IoError::parse(FormatName::LegacyClipboard, e.to_string()))?;
    if !value.is_array() {
        return Err(IoError::WrongFormat {
            expected: Expected::LegacyShapeList,
            hint: "the clipboard holds a JSON array of shapes".into(),
        });
    }
    let mut report = ImportReport::default();
    let shapes = json::read_shape_list(&value, &mut report);
    build::build(&model::OldDoc { name: None, shapes, undrawn: 0 }, ids, measure, report)
}

/// The bounds the old code gave an arrow: its points plus the two barbs of the head. Used by the facts check.
pub(crate) fn arrow_bounds(points: &[kurbo::Point], head_size: f64) -> kurbo::Rect {
    build::arrow_rect(points, head_size)
}
