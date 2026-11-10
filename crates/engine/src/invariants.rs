//! The invariant pass that runs after every command that changed the document (docs/commands.md §1.4
//! step 8): bindings, bound text, frame membership, group integrity, z-order keys.
//!
//! The rules live in `drafftink-edit`. Each one becomes a call here when its module is implemented, so
//! the executor never changes. Today the store's own per-element validation (`validate_element` on every
//! insert and update) is the only check, and the pass has nothing to add.

use crate::session::Session;
use crate::state::TabId;

/// Restores the document invariants after a command. An error rolls the command back.
pub(crate) fn run(session: &mut Session, tab: TabId) -> Result<(), String> {
    let _ = (session, tab);
    Ok(())
}
