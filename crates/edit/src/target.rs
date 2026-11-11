//! The error every rule shares for "this element cannot be the target".

use drafftink_scene::ElementId;

/// Why an element cannot be the target of an operation. Nothing was changed.
///
/// The error enum of each rule holds this as one `Target` variant, so the two cases are declared once and
/// mean the same everywhere.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TargetError {
    /// No live element has this id.
    #[error("element {0} does not exist or is deleted")]
    NotFound(ElementId),
    /// Some of the elements are locked. Unlock them first; nothing was changed.
    #[error("{} element(s) are locked ({}). Unlock them first", .0.len(), ids_text(.0))]
    Locked(Vec<ElementId>),
}

/// The first five ids, comma separated, for a message.
pub(crate) fn ids_text(ids: &[ElementId]) -> String {
    let mut s: Vec<&str> = ids.iter().take(5).map(ElementId::as_str).collect();
    if ids.len() > 5 {
        s.push("...");
    }
    s.join(", ")
}
