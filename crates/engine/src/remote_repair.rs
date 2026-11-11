//! The repair pass that follows a merge of remote edits (docs/crdt.md section 10a).
//!
//! Concurrent edits can each be valid and merge into a scene that is not: two peers move different targets of one
//! arrow, a delete races a bind, two peers label one shape, group operations nest differently. The peer that made
//! an edit ran the invariant pass for *its* state ([`crate::invariants`]), but nobody ran it for the merge. This
//! module does, for the elements the merge changed and the ones that refer to them:
//!
//! 1. **References** (`drafftink_edit::repair::dangling_references`): arrow ends bound to a missing or deleted
//!    shape are freed, labels without a usable container become free text, frame children without a frame leave it.
//! 2. **Groups** (`inconsistent_groups`): a group whose members disagree about the groups around it is dissolved.
//! 3. **Z-order** (`tied_order_keys`, `children_below_frame`, `labels_above_container`): elements that share a key get distinct ones, in the
//!    order readers already show them; a frame child above its frame and a label below its container move back.
//! 4. **Arrows**: `binding::update_bound_arrows` solves the arrows bound to what changed, so an arrow whose two
//!    targets were moved by two peers ends on both.
//! 5. **Labels** follow the arrows step 4 moved; a container that grows to hold its label is followed by its arrows
//!    in the next round (at most [`MAX_ROUNDS`](crate::invariants::MAX_ROUNDS)).
//!
//! Every step is a deterministic function of the merged scene and writes only what differs, so peers that hold the
//! same scene write the same values and a peer that receives a repair has nothing left to fix. The writes are one
//! Loro commit per step with a system origin ([`TabDoc::repair`](crate::store::TabDoc::repair)): they are synced like
//! any local write, never enter the undo history, and are copied into the working store without touching the
//! journal of a running gesture or batch. The caller records them in the ring with origin `remote`.

use std::collections::BTreeSet;

use drafftink_edit::binding::{self, BindingError};
use drafftink_edit::repair::{self, RepairError};
use drafftink_edit::text_bind::{GrowAnchor, TextBindError};
use drafftink_scene::{Change, ChangeKind, ChangeSet, ElementId, Fields, StoreError};

use crate::invariants::MAX_ROUNDS;
use crate::services::Services;
use crate::session::Session;
use crate::state::TabId;
use crate::store::TabDoc;

/// What one repair pass did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Repaired {
    /// Every element the pass wrote.
    pub touched: BTreeSet<ElementId>,
    /// Arrows that lost an end binding.
    pub freed_ends: usize,
    /// Texts that stopped being labels.
    pub released_labels: usize,
    /// Elements that left a frame.
    pub released_children: usize,
    /// Labels that took the groups of their container.
    pub label_groups: usize,
    /// Groups that were dissolved.
    pub dissolved_groups: Vec<String>,
    /// Arrows that were solved again.
    pub solved_arrows: usize,
}

impl Repaired {
    /// True when the pass wrote nothing.
    pub(crate) fn is_empty(&self) -> bool {
        self.touched.is_empty()
    }

    fn note(&mut self, cs: &ChangeSet) {
        self.touched.extend(cs.ids().cloned());
    }
}

#[derive(Debug, thiserror::Error)]
enum PassError {
    #[error(transparent)]
    Repair(#[from] RepairError),
    #[error(transparent)]
    Binding(#[from] BindingError),
    #[error(transparent)]
    Text(#[from] TextBindError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// The fields whose change moves or reshapes an element, or changes what is inside it.
fn geometry() -> Fields {
    Fields::POSITION | Fields::SIZE | Fields::ANGLE | Fields::FLIP | Fields::KIND
}

impl Session {
    /// Repairs the merge that produced `cs` in `tab`. Nothing to do for a view-only session (it never writes; the
    /// peers that can edit repair), a document blocked by a newer schema, or an empty change.
    pub(crate) fn repair_merge(&mut self, tab: TabId, cs: &ChangeSet) -> Result<Repaired, String> {
        if cs.is_empty() || self.view_only {
            return Ok(Repaired::default());
        }
        let Session { tabs, services, .. } = self;
        let Some(t) = tabs.iter_mut().find(|t| t.id == tab) else { return Ok(Repaired::default()) };
        if t.doc.blocked_by_schema().is_some() {
            return Ok(Repaired::default());
        }
        pass(&mut t.doc, services, &cs.elements).map_err(|e| e.to_string())
    }
}

/// True when the change can have broken what a rule looks at: an element was added or removed, or a field in
/// `fields` changed. A restyle or a rename starts no transaction at all.
fn touches(changed: &[Change], fields: Fields) -> bool {
    changed.iter().any(|c| c.kind != ChangeKind::Updated || c.fields.intersects(fields))
}

fn pass(doc: &mut TabDoc, services: &Services, changed: &[Change]) -> Result<Repaired, PassError> {
    let mut out = Repaired::default();
    // Each step also sees what the steps before it wrote.
    let mut seen: Vec<Change> = changed.to_vec();

    if touches(&seen, Fields::KIND | Fields::FRAME | Fields::GROUPS | Fields::DELETED) {
        let (released, cs) = doc.repair::<_, PassError>(|tx| Ok(repair::dangling_references(tx, &seen)?))?;
        out.note(&cs);
        out.freed_ends = released.arrow_ends.len();
        out.released_labels = released.labels.len();
        out.released_children = released.frame_children.len();
        out.label_groups = released.label_groups.len();
        seen.extend(cs.elements.iter().cloned());
    }
    if touches(&seen, Fields::GROUPS | Fields::DELETED) {
        let (dissolved, cs) = doc.repair::<_, PassError>(|tx| Ok(repair::inconsistent_groups(tx, &seen)?))?;
        out.note(&cs);
        out.dissolved_groups = dissolved.iter().map(ToString::to_string).collect();
        seen.extend(cs.elements.iter().cloned());
    }
    if touches(&seen, Fields::INDEX) {
        let (_, cs) = doc.repair::<_, PassError>(|tx| Ok(repair::tied_order_keys(tx, &seen)?))?;
        out.note(&cs);
        seen.extend(cs.elements.iter().cloned());
    }
    if touches(&seen, Fields::INDEX | Fields::FRAME | Fields::KIND) {
        let (_, cs) = doc.repair::<_, PassError>(|tx| Ok(repair::children_below_frame(tx, &seen)?))?;
        out.note(&cs);
        seen.extend(cs.elements.iter().cloned());
        let (_, cs) = doc.repair::<_, PassError>(|tx| Ok(repair::labels_above_container(tx, &seen)?))?;
        out.note(&cs);
    }

    let mut moved: Vec<ElementId> = changed
        .iter()
        .filter(|c| c.kind != ChangeKind::Updated || c.fields.intersects(geometry() | Fields::DELETED))
        .map(|c| c.id.clone())
        .collect();
    for _ in 0..MAX_ROUNDS {
        if moved.is_empty() {
            break;
        }
        let (arrows, cs) = doc.repair::<_, PassError>(|tx| Ok(binding::update_bound_arrows(tx, &moved)?))?;
        out.note(&cs);
        out.solved_arrows += arrows.len();
        if arrows.is_empty() {
            break;
        }
        let (_, cs) = doc.repair::<_, PassError>(|tx| {
            Ok(drafftink_edit::text_bind::refresh_labels(tx, services.edit(), &arrows, GrowAnchor::Top)?)
        })?;
        out.note(&cs);
        moved = cs
            .elements
            .iter()
            .filter(|c| c.kind == ChangeKind::Updated && c.fields.intersects(geometry()))
            .map(|c| c.id.clone())
            .collect();
    }
    Ok(out)
}

#[cfg(test)]
#[path = "remote_repair_tests.rs"]
mod tests;
