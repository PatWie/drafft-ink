//! What goes on and comes off the clipboard: the drafft.ink payload, and a classifier for pasted text.
//!
//! # The payload
//! A copy of elements is a JSON object, the **envelope**:
//!
//! ```json
//! { "format": "drafftink-clipboard", "version": 1,
//!   "document": { ...native document with only the copied elements and their pictures... },
//!   "text": "{\"type\":\"excalidraw/clipboard\", ...}" }
//! ```
//!
//! `document` is the native format, so one reader serves files and the clipboard. `text` is optional: the same
//! selection as Excalidraw clipboard JSON, for the `text/plain` flavour, so that a copy pastes into Excalidraw. A
//! shell puts the whole object under the custom media type [`MIME`] and `text` under `text/plain`.
//!
//! # Pasted text
//! [`classify_text`] decides what some text is: a drafft.ink envelope, Excalidraw clipboard JSON, a Mermaid diagram
//! of a supported type, or plain text. The decision sniffs the first characters and parses only the one candidate.

use drafftink_scene::{Document, IdSource};
use serde_json::{Value, json};

use crate::error::{ImportReport, Imported, IoError};
use crate::excalidraw;
use crate::mermaid::{self, Diagram};
use crate::native::{self, Format};

/// The value of the `format` key of the envelope.
pub const FORMAT: &str = "drafftink-clipboard";
/// The envelope version this build writes and reads.
pub const VERSION: u32 = 1;
/// The media type a shell stores the envelope under.
pub const MIME: &str = "application/vnd.drafftink.clipboard+json";

/// A packed copy.
#[derive(Debug, Clone, PartialEq)]
pub struct Packed {
    /// The envelope.
    pub payload: Value,
    /// What the Excalidraw flavour could not hold.
    pub report: ImportReport,
}

/// Packs the live elements of `doc` (and their pictures) into the envelope. With `excalidraw`, the Excalidraw
/// flavour is added as `text`; if it cannot be written the envelope has no `text` and the report says why.
pub fn pack(doc: &Document, excalidraw: bool) -> Result<Packed, IoError> {
    let document = serde_json::to_value(doc).map_err(|e| IoError::invalid("clipboard", e.to_string()))?;
    let mut payload = json!({ "format": FORMAT, "version": VERSION, "document": document });
    let mut report = ImportReport::default();
    if excalidraw {
        match excalidraw::export_clipboard(doc) {
            Ok((text, r)) => {
                report = r;
                if let Some(o) = payload.as_object_mut() {
                    o.insert("text".to_owned(), Value::String(text));
                }
            }
            Err(e) => report.warn(format!("the copy has no Excalidraw flavour: {e}")),
        }
    }
    Ok(Packed { payload, report })
}

/// True when `v` is a drafft.ink envelope (by its `format` key; the rest is checked by [`unpack`]).
pub fn is_envelope(v: &Value) -> bool {
    v.get("format").and_then(Value::as_str) == Some(FORMAT)
}

/// Reads an envelope. Refuses a newer version, a missing or damaged document, and a document that breaks a rule.
pub fn unpack(v: &Value) -> Result<Document, IoError> {
    if !is_envelope(v) {
        return Err(IoError::WrongFormat {
            expected: "drafft.ink clipboard data",
            hint: format!("it says \"format\": \"{FORMAT}\""),
        });
    }
    let version = v.get("version").and_then(Value::as_u64).unwrap_or(0);
    if version == 0 || version > u64::from(VERSION) {
        return Err(IoError::invalid(
            "clipboard",
            format!("its version is {version}, and this build reads version {VERSION}"),
        ));
    }
    let Some(doc) = v.get("document") else {
        return Err(IoError::invalid("clipboard", "it has no `document`"));
    };
    if let Some(found) =
        doc.get("version").and_then(Value::as_u64).filter(|n| *n > u64::from(drafftink_scene::document::SCHEMA_VERSION))
    {
        return Err(IoError::Document(drafftink_scene::DocumentError::NewerVersion {
            found: u32::try_from(found).unwrap_or(u32::MAX),
            max: drafftink_scene::document::SCHEMA_VERSION,
        }));
    }
    let document: Document =
        serde_json::from_value(doc.clone()).map_err(|e| IoError::parse("clipboard", e.to_string()))?;
    let report = document.validate();
    if report.has_errors() {
        return Err(IoError::Document(drafftink_scene::DocumentError::Invalid(report)));
    }
    Ok(document)
}

/// What pasted text is.
#[derive(Debug, Clone, PartialEq)]
pub enum TextKind {
    /// A drafft.ink envelope written as text, or Excalidraw clipboard JSON.
    Elements(Imported),
    /// A Mermaid diagram of a supported type.
    Mermaid(Diagram),
    /// Anything else. `note` says why text that looked like a diagram was not taken as one.
    Plain {
        /// Why Mermaid-looking text stayed text.
        note: Option<String>,
    },
}

/// Classifies pasted text. Excalidraw clipboard JSON that cannot be read is an error (pasting its JSON as text
/// would help nobody); text that merely looks like Mermaid but fails to parse is [`TextKind::Plain`] with a note.
pub fn classify_text(text: &str, ids: &mut dyn IdSource) -> Result<TextKind, IoError> {
    match native::sniff(text.as_bytes()) {
        Format::ExcalidrawClipboard => excalidraw::import_clipboard(text, ids).map(TextKind::Elements),
        Format::Mermaid => match mermaid::parse(text) {
            Ok(d) => Ok(TextKind::Mermaid(d)),
            Err(e) => Ok(TextKind::Plain { note: Some(e.to_string()) }),
        },
        _ => {
            // A drafft.ink envelope pasted as text: it sniffs as unknown JSON.
            let t = text.trim_start();
            if t.starts_with('{')
                && t.contains(FORMAT)
                && let Ok(v) = serde_json::from_str::<Value>(text)
                && is_envelope(&v)
            {
                let document = unpack(&v)?;
                return Ok(TextKind::Elements(Imported { document, report: ImportReport::default() }));
            }
            Ok(TextKind::Plain { note: None })
        }
    }
}

#[cfg(test)]
mod tests {
    use drafftink_scene::{Element, ElementKind, SeqIds, ZIndex};

    use super::*;

    fn doc() -> Document {
        let z = ZIndex::between(None, None).unwrap();
        let mut d = Document::new();
        d.elements.push(
            Element::new(drafftink_scene::ElementId::new("a").unwrap(), ElementKind::Rectangle, z)
                .with_bounds(0.0, 0.0, 10.0, 10.0),
        );
        d
    }

    #[test]
    fn pack_and_unpack_round_trip_with_the_excalidraw_flavour() {
        let packed = pack(&doc(), true).unwrap();
        assert!(is_envelope(&packed.payload));
        let text = packed.payload["text"].as_str().unwrap();
        assert!(text.contains("excalidraw/clipboard"));
        assert_eq!(unpack(&packed.payload).unwrap(), doc());
        assert!(pack(&doc(), false).unwrap().payload.get("text").is_none());
    }

    #[test]
    fn unpack_refuses_foreign_newer_and_broken_envelopes() {
        assert!(unpack(&json!({"type": "excalidraw/clipboard"})).is_err());
        assert!(unpack(&json!({"format": FORMAT, "version": 2, "document": {}})).is_err());
        assert!(unpack(&json!({"format": FORMAT, "version": 1})).is_err());
        assert!(unpack(&json!({"format": FORMAT, "version": 1, "document": {"elements": 5}})).is_err());
        assert!(unpack(&json!({"format": FORMAT, "version": 1, "document": {"version": 99}})).is_err());
    }

    #[test]
    fn text_is_classified() {
        let mut ids = SeqIds::new();
        assert!(matches!(classify_text("graph TD\nA-->B", &mut ids).unwrap(), TextKind::Mermaid(_)));
        assert!(matches!(classify_text("hello world", &mut ids).unwrap(), TextKind::Plain { note: None }));
        assert!(matches!(classify_text("graph TD\n", &mut ids).unwrap(), TextKind::Plain { note: Some(_) }));
        let xl = r#"{"type":"excalidraw/clipboard","elements":[{"type":"rectangle","id":"x","x":0,"y":0,"width":10,"height":10}]}"#;
        assert!(matches!(classify_text(xl, &mut ids).unwrap(), TextKind::Elements(_)));
        let empty = r#"{"type":"excalidraw/clipboard","elements":[]}"#;
        assert!(classify_text(empty, &mut ids).is_err());
        let env = serde_json::to_string(&pack(&doc(), false).unwrap().payload).unwrap();
        assert!(matches!(classify_text(&env, &mut ids).unwrap(), TextKind::Elements(_)));
    }
}
