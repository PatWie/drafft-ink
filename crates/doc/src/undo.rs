//! Undo and redo on top of Loro's `UndoManager`.
//!
//! One committed transaction is one undo step (the manager's merge interval is 0). Only this peer's own commits
//! are recorded, so undo never reverts what a collaborator did: Loro rebases the stored inverse over remote
//! edits. The inverse is applied as an ordinary local edit and travels to peers like any other.
//!
//! Each step carries a metadata slot with a label and the observed editor state before and after the step. The
//! store does not interpret it. Undo returns the state to restore (`before`), redo returns `after`.

use std::sync::{Arc, Mutex, PoisonError};

use drafftink_scene::{ChangeSet, ElementId, GroupId};
use loro::{LoroValue, UndoItemMeta, UndoManager};
use serde::{Deserialize, Serialize};

use crate::codec::{json_to_loro, loro_to_json};

/// The editor state that an undo step restores (docs/commands.md section 1.1): what was selected, which groups
/// are selected as a whole, which group is being edited, and which linear element is in point-edit mode.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedState {
    /// Selected elements.
    #[serde(default)]
    pub selection: Vec<ElementId>,
    /// Groups selected as a unit.
    #[serde(default)]
    pub selected_groups: Vec<GroupId>,
    /// The group whose members are being edited, if any.
    #[serde(default)]
    pub editing_group: Option<GroupId>,
    /// The line or arrow in point-edit mode, if any.
    #[serde(default)]
    pub point_editor: Option<ElementId>,
}

/// The metadata of one undo step.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndoInfo {
    /// Human label of the step ("Move", "Change color").
    #[serde(default)]
    pub label: String,
    /// Editor state before the step; undo restores it.
    #[serde(default)]
    pub before: ObservedState,
    /// Editor state after the step; redo restores it.
    #[serde(default)]
    pub after: ObservedState,
}

impl UndoInfo {
    pub(crate) fn to_loro(&self) -> Option<LoroValue> {
        let v = serde_json::to_value(self).ok()?;
        json_to_loro(&v, 0).ok()
    }

    pub(crate) fn from_loro(v: &LoroValue) -> UndoInfo {
        loro_to_json(v, 0).ok().and_then(|j| serde_json::from_value(j).ok()).unwrap_or_default()
    }
}

/// The result of one undo or redo.
#[derive(Debug, Clone, PartialEq)]
pub struct UndoStep {
    /// The label the step was committed with.
    pub label: String,
    /// The editor state to restore: the state before the step for undo, after it for redo.
    pub restore: ObservedState,
    /// What changed in the scene. Empty when the step became a no-op because a collaborator already changed the
    /// same things (the step is consumed anyway; the engine may undo once more).
    pub changes: ChangeSet,
}

/// State shared between the store and the undo manager's callbacks.
#[derive(Default)]
pub(crate) struct UndoShared {
    /// Metadata for the next pushed local step.
    pub pending: Option<LoroValue>,
    /// Metadata to put on the item that undo or redo pushes on the opposite stack.
    pub carry: Option<LoroValue>,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Builds the undo manager: one step per commit, `sys:` commits excluded, metadata from `shared`.
pub(crate) fn make_manager(doc: &loro::LoroDoc, depth: usize, shared: &Arc<Mutex<UndoShared>>) -> UndoManager {
    let mut undo = UndoManager::new(doc);
    undo.set_max_undo_steps(depth);
    undo.set_merge_interval(0);
    undo.add_exclude_origin_prefix(crate::schema::SYSTEM_ORIGIN_PREFIX);
    let shared = Arc::clone(shared);
    undo.set_on_push(Some(Box::new(move |_kind, _span, _event| {
        let mut g = lock(&shared);
        let value = g.carry.clone().or_else(|| g.pending.take()).unwrap_or(LoroValue::Null);
        UndoItemMeta { value, cursors: Vec::new() }
    })));
    undo
}
