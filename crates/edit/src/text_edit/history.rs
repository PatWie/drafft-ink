//! The editor's own undo: snapshots of the text, its colours and the selection.
//!
//! It lives and dies with one edit session. The document sees only the end result, as one undo step.

use super::runs::StyleRuns;

/// Most steps kept.
const MAX_STEPS: usize = 200;
/// Most bytes of text kept over all steps; the oldest steps go first.
const MAX_BYTES: usize = 16 * 1024 * 1024;

/// What an edit did, to decide whether the next one joins the same undo step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    /// Characters typed one after the other.
    Typing,
    /// Characters deleted one after the other.
    Deleting,
    /// Anything else: always a step of its own.
    Other,
}

/// The state an undo goes back to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Snapshot {
    pub(super) text: String,
    pub(super) runs: StyleRuns,
    pub(super) anchor: usize,
    pub(super) focus: usize,
}

impl Snapshot {
    fn weight(&self) -> usize {
        self.text.len() + 1
    }
}

/// Undo and redo stacks.
#[derive(Debug, Clone, Default)]
pub(super) struct History {
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// Kind of the last edit and where its caret ended, while nothing else happened.
    last: Option<(Kind, usize)>,
}

impl History {
    /// Notes that the state `before` is about to change by an edit of `kind`. Edits of the same joinable kind in a
    /// row, with the caret where the last one left it, share one step.
    pub(super) fn record(&mut self, before: Snapshot, kind: Kind, joins: bool) {
        let joined = joins
            && kind != Kind::Other
            && self.last.is_some_and(|(k, caret)| k == kind && caret == before.focus && before.anchor == before.focus);
        self.redo.clear();
        if !joined {
            self.undo.push(before);
            self.trim();
        }
        self.last = Some((kind, usize::MAX));
    }

    /// Tells where the edit that was just recorded left the caret.
    pub(super) fn settle(&mut self, caret: usize) {
        if let Some((_, at)) = self.last.as_mut() {
            *at = caret;
        }
    }

    /// The caret or selection moved by itself: the next edit starts a new step.
    pub(super) fn break_run(&mut self) {
        self.last = None;
    }

    /// Takes the state to go back to, remembering `now` for redo.
    pub(super) fn undo(&mut self, now: Snapshot) -> Option<Snapshot> {
        let back = self.undo.pop()?;
        self.redo.push(now);
        self.last = None;
        Some(back)
    }

    /// Takes the state to go forward to, remembering `now` for undo.
    pub(super) fn redo(&mut self, now: Snapshot) -> Option<Snapshot> {
        let forward = self.redo.pop()?;
        self.undo.push(now);
        self.last = None;
        Some(forward)
    }

    /// Steps that can be undone.
    pub(super) fn undo_len(&self) -> usize {
        self.undo.len()
    }

    fn trim(&mut self) {
        let mut bytes: usize = self.undo.iter().map(Snapshot::weight).sum();
        let mut drop = self.undo.len().saturating_sub(MAX_STEPS);
        while bytes > MAX_BYTES && drop < self.undo.len().saturating_sub(1) {
            bytes -= self.undo.get(drop).map_or(0, Snapshot::weight);
            drop += 1;
        }
        self.undo.drain(..drop);
    }
}
