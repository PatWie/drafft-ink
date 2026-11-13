//! Excalidraw files: `.excalidraw` scenes, the Excalidraw clipboard, and `.excalidrawlib` libraries, in both
//! directions. Pure: text in, [`Document`] out, no file access.
//!
//! Every input is hostile. Wrong types fall back to a default and add a warning to the [`ImportReport`]; numbers are
//! clamped to the scene limits; a document this module returns always passes [`Document::validate`] without errors.
//! Deep JSON stops at serde_json's recursion limit (128 levels) with a parse error, not a stack overflow.
//!
//! # Import: decisions
//!
//! * **Ids** are kept. A raw id with characters outside `A-Z a-z 0-9 _ -` has them replaced by `_` and is cut to 64
//!   bytes. A missing, empty, duplicate or colliding id gets a new one from the caller's [`IdSource`]. Every
//!   reference (`frameId`, `containerId`, binding `elementId`) is resolved by the id as written in the file, so a
//!   renamed element is still found.
//! * **Deleted elements** (`isDeleted: true`) are skipped, not kept as tombstones: Excalidraw does not write them to
//!   files, and a tombstone would leave dangling references. References to them are dropped. `boundElements` is
//!   ignored; the relation lives on the arrow (`startBinding`/`endBinding`) and on the text (`containerId`).
//! * **Groups**: `groupIds` become `group_ids` (innermost first, same as Excalidraw). A group with a single member
//!   is not a group: its id is removed. Nesting that contradicts itself (two members disagree about the groups
//!   around a group) keeps the first member's view and drops the contradicting ids; stacks deeper than 16 lose
//!   their innermost ids. Both add a warning.
//! * **Frames**: `frame` and `magicframe` become `Frame` (a magic frame loses its AI role). `frameId` becomes
//!   `frame_id`; a frame's own `frameId` and `angle` are dropped. A frame is moved above its last child in z-order,
//!   the order the model expects (Excalidraw files already have it that way).
//! * **Bound text**: `containerId` becomes `container_id` for rectangle, diamond, ellipse and arrow containers;
//!   anything else makes the text free. When two texts claim one container the first keeps it. A text is moved
//!   above its container if it was below. `originalText` is preferred over `text` for the content.
//! * **Arrow bindings**: `{elementId, fixedPoint, mode}` becomes a [`Binding`](drafftink_scene::Binding). Mode
//!   `skip` means no binding. A binding to a missing, deleted or unbindable element, or to the arrow itself, is
//!   dropped with a warning. Old files without `mode`: a given `fixedPoint` is kept, and the mode is `inside` when
//!   the point lies strictly inside the box (never for elbow arrows), `orbit` otherwise. Without `fixedPoint` it
//!   is worked out from where the arrow end lies in the target's rotated box. The old `focus` and `gap` are ignored.
//! * **Opacity** is `opacity / 100`. A value of 1 is one percent (the old importer treated it as fully opaque).
//! * **Corner radius** follows Excalidraw: rounded rectangles use the adaptive rule (a quarter of the shorter side,
//!   at most 32, or the `value` given), diamonds a quarter of the shorter side. Lines and arrows with `roundness`
//!   become `PathType::Round`.
//! * **Linear elements** keep Excalidraw's pivot: it rotates about the centre of the points' bounding box, which is
//!   the model's pivot too, so `angle` and points carry over without moving anything. A line with `polygon: true`
//!   becomes `closed` and drops the repeated last point. Arrows or lines with fewer than two points, and strokes
//!   with none, are skipped.
//! * **Images** need their `files` entry: base64 data URLs only, at most [`MAX_FILE_BYTES`](drafftink_scene::limits),
//!   media type `image/*`. Pictures are stored once per SHA-256. The pixel size comes from the PNG, JPEG, GIF or WebP
//!   header (SVG: Excalidraw's `crop` record, else the box). An image without usable picture data is skipped.
//! * **Not imported**: `embeddable`, `iframe`, `selection` (warning for the first two), `version`, `versionNonce`,
//!   `updated`, `created`, `lastCommittedPoint`, `boundElements`, `baseFontSize`, appState except the background.
//! * **Z-order**: a valid fractional `index` is kept when it increases; missing, tied or inverted keys are filled in
//!   from array order.
//!
//! # Export: decisions
//!
//! * Deleted elements are not written. Version fields are `version: 1`, `updated: 0` and a `versionNonce` that is a
//!   hash of the id, so the same document always gives the same file.
//! * `boundElements` is derived on every container and on both ends of every binding.
//! * Things Excalidraw cannot hold go into `customData.drafftink`, only when the plain fields would not read back
//!   the same (see `notes`): the fifth sloppiness level, the dots, dashed and zig-zag-line fills, an exact corner
//!   radius, fonts without an Excalidraw twin (serif, pixel, light and heavy weights), `head_size`, per-character
//!   colour runs, element names, brush, highlighter and closed freehand strokes, a frame that does not clip.
//! * Arrowheads: Excalidraw has no head size field and shrinks a head on a short end segment by its own rule. The
//!   file keeps the real points and heads, and `head_size` is the size the head is asked for, never the shrunk one;
//!   drafftink shrinks it again by its own rule (`drafftink_sketch::fit_head_size`) when the file comes back.
//! * **Math** has no Excalidraw type. It is written as a text element holding the LaTeX source and marked in the
//!   note; the importer turns it back into math. Excalidraw shows the raw source.
//! * Text is written unwrapped (`text` and `originalText` both hold the content); Excalidraw wraps it again to the
//!   box when it loads a fixed-width or bound text.
//! * Images are written with their picture in `files`. An image without its picture is left out with a warning.
//!   A library cannot hold pictures, so image elements are left out of `.excalidrawlib` with a warning.
//! * Drafftink to Excalidraw to drafftink is lossless for what the model stores, except `version` and tombstones.
//!   Excalidraw to drafftink to Excalidraw drops what import drops (above) and rounds nothing else.
//!   Numbers can differ in the last bit because points move between the two coordinate frames.
//! * The clipboard accepts both `excalidraw/clipboard` and `excalidraw-api/clipboard` (the type Excalidraw writes
//!   when an embedding app copies; Excalidraw pastes both). The old importer accepted only the first.
//! * A library item becomes a document; names are `None` when the file gives none or a blank one (the old importer
//!   invented "Item N"). In the grid, an item of two or more elements gets one new outermost group; a single
//!   element gets none (a group needs two members). The old importer always wrapped.

use drafftink_scene::{Color, Document, IdSource};
use serde_json::Value;

use crate::limits::MAX_INPUT_BYTES;
use crate::{Expected, FormatName, ImportReport, Imported, IoError};

mod clipboard;
mod export;
mod image;
mod import;
mod library;
mod map;
mod notes;
mod read;

pub use clipboard::export_clipboard;
pub use library::{GRID_CELL_PADDING, GRID_MAX_COLUMNS};

/// An Excalidraw library: a list of small documents.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Library {
    /// The items, in file order.
    pub items: Vec<LibraryItem>,
}

/// One library item.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryItem {
    /// The name, when the file has one.
    pub name: Option<String>,
    /// The item's elements. Elements that were grouped in the file share group ids here.
    pub document: Document,
}

fn check_size(len: usize, max: usize) -> Result<(), IoError> {
    if len > max {
        return Err(IoError::Limit { what: "input", max: format!("{max} bytes") });
    }
    Ok(())
}

fn parse_json(text: &str, format: FormatName) -> Result<Value, IoError> {
    check_size(text.len(), MAX_INPUT_BYTES)?;
    // serde_json refuses nesting deeper than 128 levels, so hostile input cannot overflow the stack.
    serde_json::from_str(text).map_err(|e| IoError::parse(format, e.to_string()))
}

const SCENE_TYPES: [&str; 3] = ["excalidraw", "excalidraw/clipboard", "excalidraw-api/clipboard"];
const CLIPBOARD_TYPES: [&str; 2] = ["excalidraw/clipboard", "excalidraw-api/clipboard"];

fn convert_scene(
    obj: &serde_json::Map<String, Value>,
    ids: &mut dyn IdSource,
    format: FormatName,
) -> Result<Imported, IoError> {
    let Some(elements) = obj.get("elements").and_then(Value::as_array) else {
        return Err(IoError::invalid(format, "there is no `elements` list"));
    };
    let mut report = ImportReport::default();
    let files = obj.get("files").and_then(Value::as_object);
    let conv = import::convert(import::Source { elements, files, prefix: "" }, ids, &mut report)?;
    let mut document = Document { elements: conv.elements, files: conv.files, ..Document::new() };
    if let Some(bg) = obj.get("appState").and_then(|a| a.get("viewBackgroundColor")).and_then(Value::as_str) {
        match Color::parse(bg.trim()) {
            Ok(c) => document.background = c,
            Err(_) => read::warn(&mut report, || "the background colour is not readable; white is used".to_owned()),
        }
    }
    let check = document.validate();
    if check.has_errors() {
        return Err(IoError::Document(drafftink_scene::DocumentError::Invalid(check)));
    }
    Ok(Imported { document, report })
}

/// Reads a `.excalidraw` scene.
///
/// Errors: not JSON ([`IoError::Parse`]); not an object, or another Excalidraw file type such as a library
/// ([`IoError::WrongFormat`]); no `elements` list ([`IoError::Invalid`]); input over [`MAX_INPUT_BYTES`]
/// ([`IoError::Limit`]). A scene whose elements are all unsupported is not an error: the document is empty and the
/// report says why.
pub fn import_scene(text: &str, ids: &mut dyn IdSource) -> Result<Imported, IoError> {
    let v = parse_json(text, FormatName::Excalidraw)?;
    let Some(obj) = v.as_object() else {
        return Err(IoError::WrongFormat {
            expected: Expected::ExcalidrawScene,
            hint: "the top level of an Excalidraw file is a JSON object".to_owned(),
        });
    };
    match obj.get("type").and_then(Value::as_str) {
        None => {}
        Some(t) if SCENE_TYPES.contains(&t) => {}
        Some("excalidrawlib") => {
            return Err(IoError::WrongFormat {
                expected: Expected::ExcalidrawScene,
                hint: "this is an Excalidraw library; open it as a library".to_owned(),
            });
        }
        Some(other) => {
            return Err(IoError::WrongFormat {
                expected: Expected::ExcalidrawScene,
                hint: format!(
                    "its \"type\" is {:?}, and a scene says \"excalidraw\"",
                    other.chars().take(40).collect::<String>()
                ),
            });
        }
    }
    convert_scene(obj, ids, FormatName::Excalidraw)
}

/// Reads the JSON Excalidraw puts on the clipboard (`excalidraw/clipboard`, or `excalidraw-api/clipboard`).
///
/// The elements keep their file ids; the caller gives them new ones when it pastes. Pictures in `files` come along.
/// A clipboard with nothing importable is an error ([`IoError::Invalid`]) so a paste handler can fall back to
/// other formats.
pub fn import_clipboard(text: &str, ids: &mut dyn IdSource) -> Result<Imported, IoError> {
    let v = parse_json(text, FormatName::ExcalidrawClipboard)?;
    let ty = v.get("type").and_then(Value::as_str);
    let Some(obj) = v.as_object().filter(|_| ty.is_some_and(|t| CLIPBOARD_TYPES.contains(&t))) else {
        let hint = match ty {
            Some("excalidraw") => "this is a saved Excalidraw scene, not clipboard data; open it as a file".to_owned(),
            _ => "Excalidraw clipboard data says \"type\": \"excalidraw/clipboard\"".to_owned(),
        };
        return Err(IoError::WrongFormat { expected: Expected::ExcalidrawClipboard, hint });
    };
    let imported = convert_scene(obj, ids, FormatName::ExcalidrawClipboard)?;
    if imported.document.elements.is_empty() {
        return Err(IoError::invalid(FormatName::ExcalidrawClipboard, "it holds no elements this app can paste"));
    }
    Ok(imported)
}

/// Reads an `.excalidrawlib` (version 1: `library`, a list of element lists; version 2: `libraryItems`).
///
/// Items with nothing importable are skipped with a warning. Rejects anything whose `type` is not `excalidrawlib`.
pub fn import_library(text: &str, ids: &mut dyn IdSource) -> Result<(Library, ImportReport), IoError> {
    library::import(text, ids)
}

/// Lays a library out as one document: uniform cells of the largest item plus 24 units of padding on every side, at
/// most 10 columns, items centred in their cells. Each item of two or more elements gets one new group id above its
/// own. Element and group ids that clash between items are replaced.
pub fn library_to_grid_document(lib: &Library, ids: &mut dyn IdSource) -> Document {
    library::grid(lib, ids)
}

/// Writes a document as `.excalidraw` JSON. See [`export_scene_with_report`].
pub fn export_scene(doc: &Document) -> Result<String, IoError> {
    export_scene_with_report(doc).map(|(text, _)| text)
}

/// Writes a document as `.excalidraw` JSON and lists what could not be written (an image without its picture).
///
/// Refuses a document that breaks a model rule ([`IoError::Document`]). Values Excalidraw cannot hold are kept in
/// `customData.drafftink`, so they are not warnings.
pub fn export_scene_with_report(doc: &Document) -> Result<(String, ImportReport), IoError> {
    let check = doc.validate();
    if check.has_errors() {
        return Err(IoError::Document(drafftink_scene::DocumentError::Invalid(check)));
    }
    let mut report = ImportReport::default();
    let out = export::export_elements(&doc.elements, &doc.files, true, &mut report);
    let root = serde_json::json!({
        "type": "excalidraw",
        "version": 2,
        "source": export::SOURCE,
        "elements": out.elements,
        "appState": {"gridSize": null, "viewBackgroundColor": doc.background.to_hex()},
        "files": out.files,
    });
    let text =
        serde_json::to_string_pretty(&root).map_err(|e| IoError::invalid(FormatName::Excalidraw, e.to_string()))?;
    Ok((text, report))
}

/// Writes a library as `.excalidrawlib` version 2. See [`export_library_with_report`].
pub fn export_library(lib: &Library) -> Result<String, IoError> {
    export_library_with_report(lib).map(|(text, _)| text)
}

/// Writes a library and lists what was left out (image elements: a library holds no pictures).
pub fn export_library_with_report(lib: &Library) -> Result<(String, ImportReport), IoError> {
    library::export(lib)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_size_check_refuses_exactly_past_the_limit() {
        assert!(check_size(10, 10).is_ok());
        let e = check_size(11, 10).unwrap_err();
        assert!(matches!(e, IoError::Limit { what: "input", .. }));
        assert!(e.to_string().contains("10 bytes"));
    }
}
