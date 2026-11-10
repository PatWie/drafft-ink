//! The native format: load and save through the scene's versioned JSON, plus format sniffing.
//!
//! The format itself (`format`, `version`, strict fields, validation) belongs to `drafftink_scene::Document`.
//! This module adds what a file open needs: [`sniff`] tells which format some bytes are, so the engine can route
//! them without trusting a file extension.

use drafftink_scene::Document;
use serde::Deserialize;
use serde::de::IgnoredAny;

use crate::error::IoError;
use crate::limits::MAX_INPUT_BYTES;

/// A file format this crate reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    /// Native drafft.ink JSON (`"format": "drafftink"`).
    Native,
    /// Old drafft-ink document JSON (`shapes` and `z_order`, no version).
    LegacyJson,
    /// Old drafft-ink internal clipboard: a JSON array of shapes.
    LegacyClipboard,
    /// Gzip data (the large legacy fixtures). The content is not looked at.
    Gzip,
    /// Old Loro snapshot from the collaboration code.
    LegacySnapshot,
    /// Excalidraw scene (`"type": "excalidraw"`).
    ExcalidrawScene,
    /// Excalidraw library (`"type": "excalidrawlib"`).
    ExcalidrawLibrary,
    /// Excalidraw clipboard (`"type": "excalidraw/clipboard"`).
    ExcalidrawClipboard,
    /// Mermaid diagram text.
    Mermaid,
    /// PNG image.
    Png,
    /// SVG image.
    Svg,
    /// None of the above.
    Unknown,
}

#[derive(Deserialize)]
struct Probe {
    format: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    shapes: Option<IgnoredAny>,
    z_order: Option<IgnoredAny>,
}

/// Tells what `bytes` are. Looks at magic bytes first, then at the top-level keys of JSON, then at the first words
/// of text. It parses JSON without building a tree, so it is cheap on large documents. Never panics.
pub fn sniff(bytes: &[u8]) -> Format {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return Format::Gzip;
    }
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        return Format::Png;
    }
    if bytes.starts_with(b"loro") {
        return Format::LegacySnapshot;
    }
    let Ok(text) = std::str::from_utf8(bytes.get(..bytes.len().min(MAX_INPUT_BYTES)).unwrap_or(bytes)) else {
        return Format::Unknown;
    };
    let t = text.trim_start_matches('\u{feff}').trim_start();
    match t.chars().next() {
        Some('{') => sniff_json_object(t),
        Some('[') => sniff_json_array(t),
        Some('<') => {
            let head: String = t.chars().take(4096).collect();
            if head.contains("<svg") { Format::Svg } else { Format::Unknown }
        }
        Some(_) => {
            if looks_like_mermaid(t) {
                Format::Mermaid
            } else {
                Format::Unknown
            }
        }
        None => Format::Unknown,
    }
}

fn sniff_json_object(t: &str) -> Format {
    let Ok(p) = serde_json::from_str::<Probe>(t) else { return Format::Unknown };
    if p.format.as_deref() == Some(drafftink_scene::document::FORMAT_NAME) {
        return Format::Native;
    }
    match p.kind.as_deref() {
        Some("excalidraw") => return Format::ExcalidrawScene,
        Some("excalidrawlib") => return Format::ExcalidrawLibrary,
        Some("excalidraw/clipboard") | Some("excalidraw/clipboard_with_api") => return Format::ExcalidrawClipboard,
        _ => {}
    }
    if p.shapes.is_some() && p.z_order.is_some() { Format::LegacyJson } else { Format::Unknown }
}

fn sniff_json_array(t: &str) -> Format {
    // The old clipboard: `[{"Rectangle": {...}}, ...]`.
    #[derive(Deserialize)]
    struct Item(std::collections::BTreeMap<String, IgnoredAny>);
    let Ok(items) = serde_json::from_str::<Vec<Item>>(t) else { return Format::Unknown };
    let known = ["Rectangle", "Ellipse", "Line", "Arrow", "Freehand", "Text", "Group", "Image", "Math"];
    let all =
        !items.is_empty() && items.iter().all(|i| i.0.len() == 1 && i.0.keys().all(|k| known.contains(&k.as_str())));
    if all { Format::LegacyClipboard } else { Format::Unknown }
}

/// Reads a native document.
pub fn load(text: &str) -> Result<Document, IoError> {
    if text.len() > MAX_INPUT_BYTES {
        return Err(IoError::Limit { what: "document", max: format!("{MAX_INPUT_BYTES} bytes") });
    }
    Ok(Document::from_json(text.trim_start_matches('\u{feff}'))?)
}

/// Writes a native document as compact JSON. Refuses a document that breaks a rule.
pub fn save(doc: &Document) -> Result<String, IoError> {
    Ok(doc.to_json()?)
}

/// Writes a native document as indented JSON.
pub fn save_pretty(doc: &Document) -> Result<String, IoError> {
    Ok(doc.to_json_pretty()?)
}

/// The first meaningful line of the text starts a diagram type this crate imports.
fn looks_like_mermaid(text: &str) -> bool {
    let mut in_fence = false;
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with("%%") {
            continue;
        }
        if l.starts_with("```") && !in_fence {
            in_fence = true;
            continue;
        }
        if l == "---" {
            continue;
        }
        return l.starts_with("graph") || l.starts_with("flowchart") || l.starts_with("sequenceDiagram");
    }
    false
}

/// A copy of `doc` holding only the elements in `keep` (tombstones and unknown ids are dropped), and only the blobs
/// they use. References that point outside the set are cut so the result validates: bindings to elements that are
/// not kept become unbound ends, bound text whose container is gone becomes free text, frame membership of a missing
/// frame is dropped. Use it to export or copy a selection.
pub fn subset(doc: &Document, keep: &std::collections::HashSet<drafftink_scene::ElementId>) -> Document {
    use drafftink_scene::ElementKind;
    let mut out = Document { files: Default::default(), elements: Vec::new(), ..doc.clone() };
    for e in doc.elements.iter().filter(|e| !e.deleted && keep.contains(&e.id)) {
        let mut e = e.clone();
        if e.frame_id.as_ref().is_some_and(|f| !keep.contains(f)) {
            e.frame_id = None;
        }
        match &mut e.kind {
            ElementKind::Line(d) | ElementKind::Arrow(d) => {
                for b in [&mut d.start_binding, &mut d.end_binding] {
                    if b.as_ref().is_some_and(|b| !keep.contains(&b.target)) {
                        *b = None;
                    }
                }
            }
            ElementKind::Text(t) if t.container_id.as_ref().is_some_and(|c| !keep.contains(c)) => {
                t.container_id = None;
                t.label_position = None;
            }
            _ => {}
        }
        if let ElementKind::Image(i) = &e.kind
            && let Some(f) = &i.file_id
            && let Some(blob) = doc.files.get(f)
        {
            out.files.insert(f.clone(), blob.clone());
        }
        out.elements.push(e);
    }
    out
}
