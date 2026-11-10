//! `history`: undo and redo (docs/commands.md §2.15, §1.8).
//!
//! Local undo per peer. The work is in `drafftink-doc` (Loro's undo manager: this peer's steps only, the inverse
//! is an ordinary edit that travels to collaborators) and in [`TabDoc::undo`](crate::store::TabDoc::undo), which
//! copies what the step touched into the working store and returns the editor state to restore. These commands
//! add the gates, the refusals, the step count and the restore of the selection and editing group.
//!
//! The commands run in their own transaction (`own_transaction`): an undo is not itself an undo step, and the
//! executor's rollback journal must not wrap it. `history.undo` and `history.redo` cannot be steps of a batch.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::args;
use crate::refusal::Refusal;
use crate::session::Session;
use crate::spec::{
    Batch, CommandSpec, Facts, Gate, Mutates, NoArgs, PortSet, Redraw, RingMode, Scope, Selection, TextGate, Binding,
    Typed, Undo,
};
use crate::store::UndoOutcome;
use drafftink_scene::SceneRead;

/// Arguments of `history.undo` and `history.redo`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct StepsArgs {
    /// How many steps to take back (or re-apply), from 1 to the `history.depth` setting. Asking for more than
    /// exist takes all that exist.
    pub steps: u32,
}

impl Default for StepsArgs {
    fn default() -> Self {
        Self { steps: 1 }
    }
}

/// Result of `history.undo`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct UndoOut {
    /// True: something was taken back.
    pub changed: bool,
    /// Labels of the steps taken back, newest first.
    pub undone: Vec<String>,
    /// Another step can be taken back.
    pub can_undo: bool,
    /// A step can be re-applied.
    pub can_redo: bool,
}

/// Result of `history.redo`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct RedoOut {
    /// True: something was re-applied.
    pub changed: bool,
    /// Labels of the steps re-applied, oldest first.
    pub redone: Vec<String>,
    /// A step can be taken back.
    pub can_undo: bool,
    /// Another step can be re-applied.
    pub can_redo: bool,
}

/// Result of `history.clear`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ClearOut {
    /// True: there was history to drop.
    pub changed: bool,
    /// Undo steps dropped.
    pub undo_dropped: usize,
    /// Redo steps dropped.
    pub redo_dropped: usize,
}

/// Result of `history.state`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct StateOut {
    /// A step can be taken back.
    pub can_undo: bool,
    /// A step can be re-applied.
    pub can_redo: bool,
    /// Label of the step undo would take back.
    pub undo_label: Option<String>,
    /// Label of the step redo would re-apply.
    pub redo_label: Option<String>,
    /// Steps on the undo stack.
    pub undo_depth: usize,
    /// Steps on the redo stack.
    pub redo_depth: usize,
}

fn nothing_to_undo(id: &str) -> Refusal {
    Refusal::custom("nothing_to_undo", id, "There is nothing to undo in this tab.")
}

fn nothing_to_redo(id: &str) -> Refusal {
    Refusal::custom("nothing_to_redo", id, "There is nothing to redo in this tab.")
}

fn can_undo(s: &Session) -> Result<(), Refusal> {
    if s.tab().doc.can_undo() { Ok(()) } else { Err(nothing_to_undo("history.undo")) }
}

fn can_redo(s: &Session) -> Result<(), Refusal> {
    if s.tab().doc.can_redo() { Ok(()) } else { Err(nothing_to_redo("history.redo")) }
}

/// `steps` checked against 1 ..= `history.depth`.
fn steps_of(s: &Session, a: &StepsArgs) -> Result<usize, Refusal> {
    let max = s.settings.history_depth();
    args::in_range(s.cmd(), "steps", f64::from(a.steps), 1.0, max as f64)?;
    Ok(a.steps as usize)
}

/// After a step came back, the restored selection may name elements that are gone (a collaborator deleted
/// them meanwhile, or the step created them and the other direction removed them).
fn drop_stale_state(s: &mut Session) {
    let t = s.tab_mut();
    let live = |id: &drafftink_scene::ElementId| t.doc.store().get(id).is_some_and(|e| !e.deleted);
    let gone: Vec<_> = t.observed.selection.iter().filter(|id| !live(id)).cloned().collect();
    for id in gone {
        t.observed.selection.remove(&id);
    }
    if t.observed.linear_editor.as_ref().is_some_and(|l| !live(&l.element)) {
        t.observed.linear_editor = None;
    }
}

fn undo(s: &mut Session, a: StepsArgs) -> Result<UndoOut, Refusal> {
    let n = steps_of(s, &a)?;
    can_undo(s)?;
    let out: UndoOutcome = s.undo_steps(n)?;
    if out.labels.is_empty() {
        // Every remaining step had become a no-op (a collaborator already reverted the same things) and was
        // consumed. Nothing visible changed.
        return Err(nothing_to_undo(s.cmd()));
    }
    drop_stale_state(s);
    Ok(UndoOut {
        changed: true,
        undone: out.labels,
        can_undo: s.tab().doc.can_undo(),
        can_redo: s.tab().doc.can_redo(),
    })
}

fn redo(s: &mut Session, a: StepsArgs) -> Result<RedoOut, Refusal> {
    let n = steps_of(s, &a)?;
    can_redo(s)?;
    let out = s.redo_steps(n)?;
    if out.labels.is_empty() {
        return Err(nothing_to_redo(s.cmd()));
    }
    drop_stale_state(s);
    Ok(RedoOut {
        changed: true,
        redone: out.labels,
        can_undo: s.tab().doc.can_undo(),
        can_redo: s.tab().doc.can_redo(),
    })
}

fn clear(s: &mut Session, _: NoArgs) -> Result<ClearOut, Refusal> {
    let (u, r) = (s.tab().doc.undo_depth(), s.tab().doc.redo_depth());
    s.tab_mut().doc.clear_history();
    Ok(ClearOut { changed: u + r > 0, undo_dropped: u, redo_dropped: r })
}

fn state(s: &mut Session, _: NoArgs) -> Result<StateOut, Refusal> {
    let d = &s.tab().doc;
    Ok(StateOut {
        can_undo: d.can_undo(),
        can_redo: d.can_redo(),
        undo_label: d.undo_label(),
        redo_label: d.redo_label(),
        undo_depth: d.undo_depth(),
        redo_depth: d.redo_depth(),
    })
}

/// Undo and redo change the document and the editor state, in their own transaction, outside batches, and not
/// while a gesture or a text edit runs (inside the text editor, undo is the editor's own).
const STEP: Facts = Facts {
    undo: Undo::NoUndo,
    reads_only: false,
    mutates: Mutates::DOC.with(Mutates::OBSERVED),
    needs_selection: Selection::No,
    ports: PortSet::NONE,
    batch: Batch::Never,
    view_only: Gate::Refused,
    while_gesture: Gate::Refused,
    while_text_edit: TextGate::Refused,
    redraw: Redraw::Scene,
    ring: RingMode::Always,
    palette: true,
    agent: true,
};

const UNDO_KEYS: &[Binding] = &[Binding::new(&["Mod+Z"], Scope::Canvas)];
const REDO_KEYS: &[Binding] = &[Binding::new(&["Mod+Shift+Z", "Mod+Y"], Scope::Canvas)];

pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "history.undo",
        "history",
        "Undo",
        "Takes back the last steps of this peer in the active tab and restores the selection from before them.",
        &Typed::new(undo),
        STEP,
    )
    .available(can_undo)
    .refusals(&["nothing_to_undo"])
    .shortcuts(UNDO_KEYS)
    .own_transaction()
    .example(r#"{"steps":1}"#),
    CommandSpec::new(
        "history.redo",
        "history",
        "Redo",
        "Re-applies the steps that were undone in the active tab and restores the selection from after them.",
        &Typed::new(redo),
        STEP,
    )
    .available(can_redo)
    .refusals(&["nothing_to_redo"])
    .shortcuts(REDO_KEYS)
    .own_transaction()
    .example(r#"{"steps":1}"#),
    CommandSpec::new(
        "history.clear",
        "history",
        "Clear history",
        "Drops the undo and redo stacks of the active tab. The document stays as it is.",
        &Typed::new(clear),
        Facts {
            mutates: Mutates::OBSERVED,
            view_only: Gate::Allowed,
            palette: false,
            ..STEP
        },
    )
    .own_transaction(),
    CommandSpec::new(
        "history.state",
        "history",
        "History state",
        "Reports whether undo and redo are possible, their labels and the depth of both stacks.",
        &Typed::new(state),
        Facts { ring: RingMode::Always, palette: true, ..Facts::READ_ONLY },
    ),
];

#[cfg(test)]
#[path = "history_tests.rs"]
mod tests;
