//! `Session::apply_remote`: a collaborator's update enters the document (docs/commands.md §1.12).
//!
//! It is not a command. Nobody asked for it, so it has no availability, no arguments and no undo step: it takes
//! the same document path as a local edit, only in the other direction. The update goes into the replicated
//! store (`drafftink-doc`), which names the elements it touched; those are copied into the working store the
//! commands read. This peer's undo stack never sees it: undo reverts this peer's own steps only, and Loro
//! rebases them over what arrived.
//!
//! What it does besides:
//!
//! * raises the dirty reason, so the shell draws one frame;
//! * drops from the selection, and from the point editor, what the update deleted;
//! * writes one ring entry per received batch (`remote.apply`, origin `remote`, the counts as arguments), so
//!   `engine.recent` shows that something arrived without listing every element;
//! * does **not** run the invariant pass. The peer that made the edit ran it, and its fixes arrive in the same
//!   update; two peers solving the same arrow would only race.

use drafftink_scene::{ChangeKind, ChangeSet, SceneRead};
use serde_json::json;

use crate::refusal::Refusal;
use crate::ring::{Origin, Outcome, Record};
use crate::session::Session;
use crate::spec::Redraw;
use crate::state::TabId;
use drafftink_profile::now_ns;

/// The id under which a received batch appears in the ring.
pub const REMOTE_RING_ID: &str = "remote.apply";

impl Session {
    /// Applies a Loro update or snapshot received for `tab`. Returns what changed in the scene.
    ///
    /// A bad update is a refusal and leaves the document as it was. Works on any tab, not only the active one,
    /// and while a gesture or a text edit runs: the update goes into the document, and the running gesture ends
    /// against the new state.
    pub fn apply_remote(&mut self, tab: TabId, bytes: &[u8]) -> Result<ChangeSet, Refusal> {
        let t0 = now_ns();
        let res = self.apply_remote_inner(tab, bytes);
        let dur = now_ns().saturating_sub(t0) / 1000;
        let args = json!({
            "tab": tab.to_string(),
            "bytes": bytes.len(),
            "elements": res.as_ref().map_or(0, |cs| cs.elements.len()),
        });
        let outcome = match &res {
            Ok(cs) => Outcome::ok(!cs.is_empty(), None),
            Err(r) => Outcome::refused(r),
        };
        let t_ms = self.now_ms();
        self.ring.push(Record {
            t_ms,
            id: REMOTE_RING_ID,
            args: Some(&args),
            source: Origin::Remote,
            outcome,
            duration_us: dur,
            steps: None,
            frames: None,
            mutating: true,
        });
        res
    }

    fn apply_remote_inner(&mut self, tab: TabId, bytes: &[u8]) -> Result<ChangeSet, Refusal> {
        let Some(t) = self.tab_by_id_mut(tab) else {
            return Err(no_such_tab(tab));
        };
        let cs = t.doc.apply_remote(bytes).map_err(|e| {
            Refusal::parse_failed(
                REMOTE_RING_ID,
                "an update from a collaborator",
                &e.to_string(),
                "The update was ignored and the document is unchanged. If edits keep missing, rejoin the room to get the whole document.",
            )
        })?;
        self.after_remote(tab, &cs);
        Ok(cs)
    }

    /// What follows every remote change, whichever way it reached the replicated store: drops what the change
    /// deleted from the selection and the point editor, and raises the dirty reason.
    pub(crate) fn after_remote(&mut self, tab: TabId, cs: &ChangeSet) {
        if cs.is_empty() {
            return;
        }
        let Some(t) = self.tab_by_id_mut(tab) else { return };
        let gone: Vec<_> = cs
            .elements
            .iter()
            .filter(|c| c.kind == ChangeKind::Removed || t.doc.store().get(&c.id).is_none_or(|e| e.deleted))
            .map(|c| c.id.clone())
            .collect();
        for id in &gone {
            t.observed.selection.remove(id);
        }
        if t.observed.linear_editor.as_ref().is_some_and(|l| gone.contains(&l.element)) {
            t.observed.linear_editor = None;
        }
        self.mark_dirty(Redraw::Scene);
    }

    /// Takes a change set the collab client already imported into the replicated store: copies it into the
    /// working store of `tab`, then does what [`Session::apply_remote`] does. One ring entry per call.
    pub(crate) fn mirror_remote(&mut self, tab: TabId, cs: &ChangeSet) -> Result<(), Refusal> {
        let res = self.mirror_import_slice(tab, cs);
        let args = json!({ "tab": tab.to_string(), "elements": cs.elements.len() });
        let outcome = match &res {
            Ok(()) => Outcome::ok(!cs.is_empty(), None),
            Err(r) => Outcome::refused(r),
        };
        let t_ms = self.now_ms();
        self.ring.push(Record {
            t_ms,
            id: REMOTE_RING_ID,
            args: Some(&args),
            source: Origin::Remote,
            outcome,
            duration_us: 0,
            steps: None,
            frames: None,
            mutating: true,
        });
        res
    }
}

impl Session {
    /// Like [`Session::mirror_remote`] without the ring entry: one slice of a staged import. A big room is hundreds
    /// of slices, and the ring would hold nothing else. [`Session::record_import_done`] writes the one entry.
    pub(crate) fn mirror_import_slice(&mut self, tab: TabId, cs: &ChangeSet) -> Result<(), Refusal> {
        let res = match self.tab_by_id_mut(tab) {
            Some(t) => t.doc.mirror_remote(cs).map_err(|e| {
                Refusal::internal(REMOTE_RING_ID, format!("a collaborator's change could not be shown: {e}"))
            }),
            None => Err(no_such_tab(tab)),
        };
        if res.is_ok() {
            self.after_remote(tab, cs);
        }
        res
    }

    /// The ring entry of a staged import that is complete: `items` elements, files and metas came in.
    pub(crate) fn record_import_done(&mut self, tab: TabId, items: usize) {
        let args = json!({ "tab": tab.to_string(), "elements": items });
        let t_ms = self.now_ms();
        self.ring.push(Record {
            t_ms,
            id: REMOTE_RING_ID,
            args: Some(&args),
            source: Origin::Remote,
            outcome: Outcome::ok(items > 0, None),
            duration_us: 0,
            steps: None,
            frames: None,
            mutating: true,
        });
    }
}

fn no_such_tab(tab: TabId) -> Refusal {
    Refusal::custom(
        "no_such_tab",
        REMOTE_RING_ID,
        format!("There is no tab {tab}, so the update was dropped. It may have been closed."),
    )
}

#[cfg(test)]
#[path = "remote_tests.rs"]
mod tests;
