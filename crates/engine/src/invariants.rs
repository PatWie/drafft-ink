//! The invariant pass that runs after every command that changed the document (docs/commands.md §1.4
//! step 8): bindings, bound text, frame membership, group integrity, z-order keys.
//!
//! The rules live in `drafftink-edit`; this module only decides *when* each one runs and in which order, with
//! one call per rule. The pass reads what the command did from the journal ([`TabDoc::changes_since`]) and
//! writes its follow-ups into the same journal, so the follow-ups roll back with the command and join its undo
//! step.
//!
//! Order, in rounds:
//!
//! 1. **Arrows.** `binding::update_bound_arrows` for every element whose geometry changed (position, size,
//!    angle, flip, or its points and text, which are in its kind), was added, or was deleted. Arrows bound to
//!    them follow, and elbow arrows are routed again.
//! 2. **Bound text.** `text_bind::refresh_labels` for the changed elements and the arrows step 1 moved: a label
//!    follows its container, a container grows to hold its label.
//! 3. **Frame membership.** A marked call site at the end of [`round`]; see there.
//!
//! Step 2 can move a container (it grew), which arrows bound to it must follow, so the pass repeats until a round
//! writes no label. A round that changes nothing ends the pass, so a command that touched no geometry costs one
//! scan of the change set. Both steps are idempotent, which is what makes the repeat safe. The pass stops after
//! [`MAX_ROUNDS`] rounds; a document that still changes then is left as it is.
//!
//! The pass never runs for remote updates ([`Session::apply_remote`](crate::Session::apply_remote)): it reads a
//! command's journal, and the peer that made the edit ran it already. What the merge of two peers' edits needs is
//! another pass over the merged scene, [`crate::remote_repair`], which writes the same value on every peer.

use drafftink_edit::binding::{self, BindingError};
use drafftink_edit::text_bind::{self, GrowAnchor, TextBindError};
use drafftink_scene::{ChangeKind, ElementId, Fields, SceneStore, StoreError};

use crate::services::Services;
use crate::session::Session;
use crate::state::TabId;
use crate::store::{DocMark, TabDoc};

/// The most rounds of arrows then labels.
pub(crate) const MAX_ROUNDS: usize = 4;

/// The fields whose change moves or reshapes an element, or changes what is inside it.
fn geometry() -> Fields {
    Fields::POSITION | Fields::SIZE | Fields::ANGLE | Fields::FLIP | Fields::KIND
}

#[derive(Debug, thiserror::Error)]
enum PassError {
    #[error(transparent)]
    Binding(#[from] BindingError),
    #[error(transparent)]
    Text(#[from] TextBindError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Restores the document invariants after a command. An error rolls the command back.
pub(crate) fn run(session: &mut Session, tab: TabId, mark: DocMark) -> Result<(), String> {
    let Session { tabs, services, .. } = session;
    let Some(t) = tabs.iter_mut().find(|t| t.id == tab) else { return Ok(()) };
    pass(&mut t.doc, services, mark).map_err(|e| e.to_string())
}

fn pass(doc: &mut TabDoc, services: &Services, mark: DocMark) -> Result<(), PassError> {
    for _ in 0..MAX_ROUNDS {
        if !round(doc, services, mark)? {
            break;
        }
    }
    Ok(())
}

/// One round. Returns true when the label step wrote something, which may have moved a container.
fn round(doc: &mut TabDoc, services: &Services, mark: DocMark) -> Result<bool, PassError> {
    let cs = doc.changes_since(mark);
    let moved: Vec<ElementId> = cs
        .elements
        .iter()
        .filter(|c| {
            c.kind != ChangeKind::Updated || c.fields.intersects(geometry()) || c.fields.intersects(Fields::DELETED)
        })
        .map(|c| c.id.clone())
        .collect();
    if moved.is_empty() {
        return Ok(false);
    }

    // 1. Arrows bound to what moved, and elbow arrows among it.
    let (arrows, _) = doc.edit::<Vec<ElementId>, PassError>(|tx| Ok(binding::update_bound_arrows(tx, &moved)?))?;

    // 2. Labels of what changed or was routed again.
    let mut labelled: Vec<ElementId> = cs
        .elements
        .iter()
        .filter(|c| c.kind != ChangeKind::Removed && (c.kind == ChangeKind::Added || c.fields.intersects(geometry())))
        .map(|c| c.id.clone())
        .collect();
    labelled.extend(arrows);
    labelled.sort();
    labelled.dedup();
    let before = doc.store().revision();
    doc.edit::<Vec<ElementId>, PassError>(|tx| {
        Ok(text_bind::refresh_labels(tx, services.edit(), &labelled, GrowAnchor::Top)?)
    })?;
    // FRAME-MEMBERSHIP (call site, not wired): `drafftink_edit::frame` has the rules (`update_membership_after_move`,
    // `recompute_after_resize`), but they are not invariants of every change: both say "run it on the committed move
    // (resize) only" and the first needs the drop point of the pointer, which the change set does not carry. The
    // `element.move` and `element.resize` commands (family `transform`) call them with the pointer from their
    // arguments, before this pass runs. When a rule appears that depends on the change set alone (for example
    // "a child never outlives its frame"), it is called here as one call, after the label step.
    Ok(doc.store().revision() != before)
}
