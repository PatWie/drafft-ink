//! Document saves: the saved state of a tab follows the result of the write.
//!
//! `doc.save` queues a [`PortWrite::Save`]. The session performs it after the command succeeded and marks the
//! tab saved only when the port reports success. A failed write refuses the command and leaves the tab dirty.
//! A port that finishes later (the browser) answers [`crate::ports::WriteOutcome::Pending`]: the tab stays
//! dirty until the shell calls [`Session::on_write_done`] with the token.

use serde_json::{Value, json};

use crate::cmd::doc::files;
use crate::ports::{Performed, PortError, PortWrite, SaveDone, SaveWrite, WriteToken};
use crate::refusal::Refusal;
use crate::session::Session;
use crate::spec::Redraw;

/// Most flush rounds: finishing a save queues the writes of the recent list, and those queue nothing.
const MAX_ROUNDS: usize = 4;

/// What flushing the queued writes left behind.
#[derive(Debug, Default)]
pub(crate) struct Flushed {
    /// Notes for the output of the command.
    pub warnings: Vec<String>,
    /// The first document or file write that failed (`doc.save`, `doc.export` to a file): the command must
    /// refuse, so that it does not report a success that did not happen.
    pub failed_save: Option<Refusal>,
    /// Document writes that the port finishes later.
    pub pending: Vec<WriteToken>,
}

impl Flushed {
    /// Adds `"pending_write"` to the output when a document write is still open, and clears `changed`: the
    /// document is not saved yet.
    pub fn annotate(&self, out: Value) -> Value {
        let Value::Object(mut o) = out else { return out };
        if !self.pending.is_empty() {
            o.insert("changed".to_owned(), Value::Bool(false));
            o.insert("pending_write".to_owned(), json!(self.pending.iter().map(|t| t.0).collect::<Vec<_>>()));
        }
        Value::Object(o)
    }
}

/// A phrase for a file in a refusal.
fn target_phrase(f: &crate::ports::FileRef) -> String {
    match f {
        crate::ports::FileRef::Path { path } => format!("`{}`", crate::refusal::echo(path)),
        other => files::describe(other),
    }
}

impl Session {
    /// Performs queued writes in order and applies the result of each document save. A failed write does
    /// not stop the others.
    pub(crate) fn flush_writes(&mut self, writes: Vec<PortWrite>) -> Flushed {
        let mut out = Flushed::default();
        let mut round = writes;
        for _ in 0..MAX_ROUNDS {
            for w in round {
                match self.ports.perform(&w) {
                    Ok(Performed::Done) => {
                        if let PortWrite::Save(s) = w {
                            self.apply_saved(&s.done);
                        }
                    }
                    Ok(Performed::Pending(t)) => {
                        if let PortWrite::Save(s) = w {
                            self.saves.push((t, s.done));
                        }
                        out.pending.push(t);
                    }
                    Err(msg) => match w {
                        PortWrite::Save(s) => {
                            let r = Refusal::not_saved(self.cmd(), &target_phrase(&s.done.target), &msg);
                            out.failed_save.get_or_insert(r);
                        }
                        PortWrite::File { file, .. } => {
                            let r = Refusal::not_exported(self.cmd(), &target_phrase(&file), &msg);
                            out.failed_save.get_or_insert(r);
                        }
                        _ => out.warnings.push(msg),
                    },
                }
            }
            round = std::mem::take(&mut self.run.writes);
            if round.is_empty() {
                break;
            }
        }
        out
    }

    /// Writes a document now, inside the running command, for the commands that must know the result before
    /// they replace the document. Done marks the tab saved. A failed or pending write is a refusal.
    pub(crate) fn save_now(&mut self, id: &str, save: SaveWrite) -> Result<(), Refusal> {
        let phrase = target_phrase(&save.done.target);
        match self.ports.perform(&PortWrite::Save(save.clone())) {
            Ok(Performed::Done) => {
                self.apply_saved(&save.done);
                Ok(())
            }
            Ok(Performed::Pending(t)) => {
                self.saves.push((t, save.done));
                Err(Refusal::custom(
                    "io_failed",
                    id,
                    format!(
                        "The save to {phrase} has started but is not confirmed yet, so the document was not replaced. Wait until it is saved, then run `{id}` again."
                    ),
                ))
            }
            Err(msg) => Err(Refusal::not_saved(id, &phrase, &msg)),
        }
    }

    /// Writes the documents that the running command queued so far, now. For a command that runs `doc.save`
    /// and then discards the saved document (`tab.close` with `on_unsaved=save`): it must know that the file
    /// is safe first. Other queued writes stay queued.
    pub(crate) fn settle_saves(&mut self, id: &str) -> Result<(), Refusal> {
        let (saves, rest): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.run.writes).into_iter().partition(|w| matches!(w, PortWrite::Save(_)));
        self.run.writes = rest;
        for w in saves {
            if let PortWrite::Save(save) = w {
                self.save_now(id, save)?;
            }
        }
        Ok(())
    }

    /// The shell calls this when a write that answered `Pending` has ended. On success the tab counts as
    /// saved as of the revision that was written (edits since then keep it dirty); on failure it stays
    /// dirty and the refusal says that the document was NOT saved.
    pub fn on_write_done(&mut self, token: WriteToken, result: Result<(), PortError>) -> Result<(), Refusal> {
        let Some(i) = self.saves.iter().position(|(t, _)| *t == token) else {
            return Err(Refusal::custom(
                "io_failed",
                "doc.save",
                format!(
                    "No save is waiting for write {}. It was already confirmed, or this session did not start it.",
                    token.0
                ),
            ));
        };
        let (_, done) = self.saves.remove(i);
        match result {
            Ok(()) => {
                self.apply_saved(&done);
                let writes = std::mem::take(&mut self.run.writes);
                // Only plain storage writes are left (the recent list), so the result has nothing to add.
                let _ = self.flush_writes(writes);
                self.mark_dirty(Redraw::Overlay);
                Ok(())
            }
            Err(e) => Err(Refusal::not_saved("doc.save", &target_phrase(&done.target), &e.to_string())),
        }
    }

    /// Marks the tab saved at the written revision, and with `remember` makes the target the save target and
    /// a recent file. A tab that was closed meanwhile is left alone.
    fn apply_saved(&mut self, done: &SaveDone) {
        let Some(t) = self.tab_by_id_mut(done.tab) else { return };
        t.saved_revision = done.revision;
        if done.remember && !matches!(done.target, crate::ports::FileRef::Download { .. }) {
            t.ext.set(files::SaveTarget(Some(done.target.clone())));
        }
        if done.remember
            && let Err(r) = files::remember(self, &done.target)
        {
            self.warn(format!("the recent list was not updated: {}", r.message));
        }
    }
}
