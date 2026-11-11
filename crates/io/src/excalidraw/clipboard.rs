//! Writing the Excalidraw clipboard (`excalidraw/clipboard`), the counterpart of `import_clipboard`.

use drafftink_scene::Document;

use super::export;
use crate::error::{FormatName, ImportReport, IoError};

/// Writes the live elements of `doc` as the JSON Excalidraw puts on the clipboard, so that a copy pastes into
/// Excalidraw. Returns the text and what could not be written (an image without its picture).
///
/// Refuses a document that breaks a model rule ([`IoError::Document`]).
pub fn export_clipboard(doc: &Document) -> Result<(String, ImportReport), IoError> {
    let check = doc.validate();
    if check.has_errors() {
        return Err(IoError::Document(drafftink_scene::DocumentError::Invalid(check)));
    }
    let mut report = ImportReport::default();
    let out = export::export_elements(&doc.elements, &doc.files, true, &mut report);
    let root = serde_json::json!({
        "type": "excalidraw/clipboard",
        "elements": out.elements,
        "files": out.files,
    });
    let text =
        serde_json::to_string(&root).map_err(|e| IoError::invalid(FormatName::ExcalidrawClipboard, e.to_string()))?;
    Ok((text, report))
}
