//! Arrow bindings: acquiring, keeping and releasing the attachment of arrow ends to shapes.
//!
//! Contract used by other modules (keep this signature; the binding owner fills in the body):
//! after any change to the geometry of elements (move, resize, rotate, delete, restore), the
//! caller passes the changed ids to [`update_bound_arrows`] inside the same transaction, and every
//! arrow bound to one of them is re-solved.

use drafftink_scene::{ElementId, SceneTx};

/// Errors from binding rules.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum BindingError {
    /// The element does not exist.
    #[error("element {0} does not exist")]
    Missing(ElementId),
}

/// Re-solve every arrow bound to any of `changed` (and arrows in `changed` themselves).
/// Returns the ids of arrows whose geometry changed.
pub fn update_bound_arrows<T: SceneTx>(_tx: &mut T, _changed: &[ElementId]) -> Result<Vec<ElementId>, BindingError> {
    Ok(Vec::new())
}
