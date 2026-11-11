//! Pictures that a batch step waits for: the batch completes, the picture follows.
//!
//! The browser's `Renderer` port reads a texture back from the GPU, so its first answer is
//! `PortError::NotReady` and the command refuses with `render_pending`. A single command is run again by the
//! shell. A batch step that is refused this way is not a failure and does not take the batch back: when no later
//! step can change what the picture shows ([`picture_neutral`]), the batch carries on and commits as one undo
//! step, the step's result is a placeholder, and the session keeps the step as a [`DeferredRender`] with a
//! [`RenderToken`], the way `doc.save` keeps a write the port finishes later (`saves.rs`). The shell calls
//! [`Session::finish_render`] when the port wakes it; the step then runs as a command of its own, on the document
//! the batch left, and its output is what the batch would have returned.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;

use crate::fingerprint::Fingerprint;
use crate::refusal::Refusal;
use crate::ring::Origin;
use crate::session::Session;
use crate::spec::{Mutates, CommandSpec};
use crate::state::TabId;
use drafftink_scene::ElementId;

/// The code of the refusal of a command whose picture is still being drawn.
pub(crate) const RENDER_PENDING: &str = "render_pending";

/// Names a batch step whose picture is not ready yet. The batch result lists it; [`Session::finish_render`] takes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct RenderToken(pub u64);

/// A batch step that waits for its picture.
#[derive(Debug, Clone, Serialize)]
pub struct PendingRender {
    /// Index of the step in the batch.
    pub index: usize,
    /// Its command id.
    pub id: String,
    /// Hand this to [`Session::finish_render`].
    pub token: RenderToken,
}

/// What a deferred step needs to run later, and what the document was like when the batch ended.
#[derive(Debug)]
pub(crate) struct DeferredRender {
    pub(crate) tab: TabId,
    pub(crate) origin: Origin,
    pub(crate) spec: &'static CommandSpec,
    /// The arguments after `$name` references were replaced.
    pub(crate) args: Value,
    pub(crate) print: Fingerprint,
    pub(crate) selection: BTreeSet<ElementId>,
}

/// How far a deferred step has come.
#[derive(Debug, Clone, PartialEq)]
pub enum Finished {
    /// The picture is still being drawn. Call again when the port wakes the shell.
    Waiting,
    /// The step ran: this is its output.
    Done(Value),
}

/// True when running `spec` after a picture step cannot change what that picture shows. Port writes (a file, the
/// clipboard) and reads do not; a change to the document, the selection, the view, the tool, a setting or a tab
/// does. A step that is not neutral makes the batch wait the old way: everything is taken back and run again.
pub(crate) fn picture_neutral(spec: &CommandSpec) -> bool {
    spec.reads_only || spec.mutates.0 & !Mutates::PORTS.0 == 0
}

impl Session {
    /// A token for the next deferred step.
    pub(crate) fn next_render_token(&mut self) -> RenderToken {
        self.next_render += 1;
        RenderToken(self.next_render)
    }

    /// Remembers a deferred step of the batch that just committed. `origin` is who ran the batch.
    pub(crate) fn defer_render(
        &mut self,
        token: RenderToken,
        origin: Origin,
        spec: &'static CommandSpec,
        args: Value,
    ) {
        let deferred = DeferredRender {
            tab: self.tab().id,
            origin,
            spec,
            args,
            print: self.tab().doc.fingerprint(),
            selection: self.selection().clone(),
        };
        self.renders.push((token, deferred));
    }

    /// Runs a deferred step if its picture is ready. `Finished::Waiting` keeps it for the next call; a refusal
    /// ends it. The step is refused when the document, the selection or the active tab is no longer what the batch
    /// left, because the picture would then show something else than the batch's state.
    pub fn finish_render(&mut self, token: RenderToken) -> Result<Finished, Refusal> {
        let Some(i) = self.renders.iter().position(|(t, _)| *t == token) else {
            return Err(Refusal::custom(
                "render_unknown",
                "batch",
                format!(
                    "No picture is waiting under token {}. It was already delivered, or this session did not start it.",
                    token.0
                ),
            ));
        };
        let unchanged = self.renders.get(i).is_some_and(|(_, d)| {
            self.tab().id == d.tab && self.tab().doc.fingerprint() == d.print && self.selection() == &d.selection
        });
        if !unchanged {
            let (_, d) = self.renders.remove(i);
            return Err(Refusal::custom(
                "render_stale",
                d.spec.id,
                format!(
                    "The document or the selection changed while the picture for `{}` was being drawn, so it was not made. The rest of the batch is applied. Run `{}` again.",
                    d.spec.id, d.spec.id
                ),
            ));
        }
        let (origin, spec, args) = match self.renders.get(i) {
            Some((_, d)) => (d.origin, d.spec, d.args.clone()),
            None => return Ok(Finished::Waiting),
        };
        match self.execute_spec(origin, spec, crate::executor::Input::Json(&args), Some(&args)) {
            Err(r) if r.code == RENDER_PENDING => Ok(Finished::Waiting),
            Err(r) => {
                self.renders.remove(i);
                Err(r)
            }
            Ok(out) => {
                self.renders.remove(i);
                Ok(Finished::Done(out))
            }
        }
    }
}
