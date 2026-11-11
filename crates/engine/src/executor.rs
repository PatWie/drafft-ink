//! `Session::execute`: the single entry for user actions (docs/commands.md §1.4).
//!
//! Order: look up, gates (view-only, gesture, text edit), `available`, argument parsing, then the
//! transaction: remember the document position and the small session state, run the handler under
//! `catch_unwind`, roll back on `Err` or panic, run the invariant pass, push one undo step, flush port
//! writes, set the dirty reason and write the ring entry.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};

use serde_json::{Map, Value, json};

use crate::refusal::Refusal;
use crate::ring::{Origin, Outcome, Record};
use crate::session::{Session, SoftState};
use crate::spec::{CommandSpec, Gate, Mutates, RingMode, Selection, Status, TextGate, Txn, Undo};
use crate::state::TabId;
use crate::store::{CoalesceReq, DocMark, StepMeta, Summary};
use drafftink_profile::{Counter, inc, now_ns, span};

/// Where arguments come from.
#[derive(Clone, Copy)]
pub(crate) enum Input<'a> {
    /// JSON from a script, the bridge or a test. Validated against the schema.
    Json(&'a Value),
    /// Already typed, from a shell that builds the struct directly (no JSON per pointer event).
    Typed(&'a dyn Any),
}

/// What a `Commit` gate did before its command ran.
#[derive(Default)]
pub(crate) struct GateCommit {
    /// The state before the commit, when the commit changed no document content. A refusal restores it.
    restore: Option<SoftState>,
    /// The ring entry of the commit, written once the command's outcome is known.
    record: Option<GateRecord>,
}

struct GateRecord {
    spec: &'static CommandSpec,
    args: Value,
    outcome: Outcome,
}

enum Parsed<'a> {
    Owned(Box<dyn Any>),
    Borrowed(&'a dyn Any),
}

impl Parsed<'_> {
    fn any(&self) -> &dyn Any {
        match self {
            Self::Owned(b) => b.as_ref(),
            Self::Borrowed(r) => *r,
        }
    }
}

/// Top-level call or a step inside a batch.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Top,
    Batch,
}

/// What one run of a command produced.
pub(crate) struct StepDone {
    /// The output object with the common keys filled in.
    pub out: Value,
    /// Whether anything changed.
    pub changed: bool,
    /// The undo label when a step was pushed (top level only).
    pub undo_label: Option<String>,
}

/// The message of a caught panic.
pub fn panic_message(p: &(dyn Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "a panic without a message".to_owned()
    }
}

/// Runs `f`, turning a panic into an `internal` refusal for `id`.
pub(crate) fn guard<T>(id: &str, f: impl FnOnce() -> Result<T, Refusal>) -> Result<T, Refusal> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(p) => Err(Refusal::internal(id, panic_message(p.as_ref()))),
    }
}

impl Session {
    /// Runs a command from a script. The origin recorded in the ring is `script`.
    pub fn execute(&mut self, id: &str, args: Value) -> Result<Value, Refusal> {
        self.execute_from(Origin::Script, id, args)
    }

    /// Runs a command that only reads (`reads_only`) and leaves no ring entry when it succeeds: for the UI, which
    /// reads state on every frame and would otherwise fill the ring. A refusal is recorded as for any call. A
    /// command that changes things is refused with `not_a_query` before it runs.
    pub fn query(&mut self, id: &str, args: Value) -> Result<Value, Refusal> {
        if let Some(spec) = self.registry().find(id)
            && !spec.reads_only
        {
            return Err(Refusal::custom(
                "not_a_query",
                id,
                format!(
                    "`{id}` changes things, so it cannot be a query. Run it with `execute`, which records it in the ring."
                ),
            ));
        }
        self.run.quiet_ok = true;
        let res = self.execute_from(Origin::Ui, id, args);
        self.run.quiet_ok = false;
        res
    }

    /// Runs a command and records who asked.
    pub fn execute_from(&mut self, origin: Origin, id: &str, args: Value) -> Result<Value, Refusal> {
        match self.registry().find(id) {
            Some(spec) => self.execute_spec(origin, spec, Input::Json(&args), Some(&args)),
            None => {
                let r = self.registry().unknown(id);
                self.record_refusal(origin, id, Some(&args), &r, 0);
                Err(r)
            }
        }
    }

    /// Runs a command with an argument struct, without building JSON. `A` must be the command's argument
    /// type; a mismatch is an `internal` refusal.
    pub fn execute_typed<A: Any>(&mut self, origin: Origin, id: &str, args: &A) -> Result<Value, Refusal> {
        match self.registry().find(id) {
            Some(spec) => self.execute_spec(origin, spec, Input::Typed(args), None),
            None => {
                let r = self.registry().unknown(id);
                self.record_refusal(origin, id, None, &r, 0);
                Err(r)
            }
        }
    }

    pub(crate) fn execute_spec(
        &mut self,
        origin: Origin,
        spec: &'static CommandSpec,
        input: Input<'_>,
        json: Option<&Value>,
    ) -> Result<Value, Refusal> {
        if self.run.depth > 0 {
            let r = Refusal::internal(
                spec.id,
                format!(
                    "`{}` was called from inside `{}`; commands run other commands with run_nested only",
                    spec.id, self.run.cmd
                ),
            );
            return Err(r);
        }
        let t0 = now_ns();
        self.run.ring_handled = false;
        self.run.origin = origin;
        inc(Counter::Commands);
        let res = {
            let _s = span("command");
            self.run_pipeline(spec, input, origin, Mode::Top)
        };
        let dur = now_ns().saturating_sub(t0) / 1000;
        if !self.run.ring_handled {
            let owned;
            let args_json: Option<&Value> = match (json, input) {
                (Some(j), _) => Some(j),
                (None, Input::Typed(a)) => {
                    owned = spec.handler.to_json(a);
                    owned.as_ref()
                }
                (None, Input::Json(j)) => Some(j),
            };
            match &res {
                Ok(done) => {
                    let skip = spec.ring == RingMode::Quiet || origin == Origin::Remote || self.run.quiet_ok;
                    if !skip {
                        self.record(
                            origin,
                            spec,
                            args_json,
                            Outcome::ok(done.changed, done.undo_label.clone()),
                            dur,
                            None,
                            None,
                        );
                    }
                }
                Err(r) => self.record_refusal_spec(origin, spec, args_json, r, dur),
            }
        }
        self.run.ring_handled = false;
        res.map(|d| d.out)
    }

    /// The pipeline shared by top-level calls, batch steps and the end of a gesture.
    pub(crate) fn run_pipeline(
        &mut self,
        spec: &'static CommandSpec,
        input: Input<'_>,
        origin: Origin,
        mode: Mode,
    ) -> Result<StepDone, Refusal> {
        let gate = self.check_gates(spec, origin, mode, false)?;
        let result = self.run_after_gates(spec, input, mode);
        self.settle_gate(gate, origin, result.is_ok());
        result
    }

    /// Steps 3 to 8, after the gates have run.
    fn run_after_gates(
        &mut self,
        spec: &'static CommandSpec,
        input: Input<'_>,
        mode: Mode,
    ) -> Result<StepDone, Refusal> {
        self.check_available(spec)?;
        let parsed = match input {
            Input::Json(v) => Parsed::Owned(spec.handler.parse(spec.id, v)?),
            Input::Typed(a) => Parsed::Borrowed(a),
        };
        match spec.txn {
            Txn::Own => self.run_own(spec, &parsed),
            Txn::Standard => self.run_standard(spec, &parsed, mode),
        }
    }

    /// Step 2: the gates that come from facts.
    pub(crate) fn check_gates(
        &mut self,
        spec: &'static CommandSpec,
        origin: Origin,
        mode: Mode,
        ignore_gesture: bool,
    ) -> Result<GateCommit, Refusal> {
        if let Status::Planned(reason) = spec.status {
            return Err(Refusal::not_supported(spec.id, reason));
        }
        if self.view_only && spec.view_only == Gate::Refused {
            return Err(Refusal::view_only(spec.id));
        }
        self.schema_gate(spec)?;
        if !ignore_gesture && self.gesture.is_some() && spec.while_gesture == Gate::Refused {
            return Err(Refusal::gesture_active(spec.id));
        }
        if self.text_edit.is_some() {
            match spec.while_text_edit {
                TextGate::Refused => return Err(Refusal::text_editing(spec.id)),
                // `text.edit_end` is the commit itself: it must not trigger another one.
                TextGate::Commit if spec.id != "text.edit_end" => return self.commit_text_edit(origin, mode),
                TextGate::Commit | TextGate::Allowed => {}
            }
        }
        Ok(GateCommit::default())
    }

    /// A document that a newer version of the app changed is read-only here: a command that mutates the
    /// document refuses, unless it replaces the document or ends an edit (`CommandSpec::allowed_when_blocked`).
    fn schema_gate(&self, spec: &'static CommandSpec) -> Result<(), Refusal> {
        if spec.mutates.contains(Mutates::DOC)
            && !spec.works_when_blocked
            && self.tab().doc.blocked_by_schema().is_some()
        {
            return Err(Refusal::newer_schema(spec.id));
        }
        Ok(())
    }

    /// Commits the running text edit as its own undo step (`text.edit_end` with `commit: true`).
    ///
    /// The commit runs before the command can validate its arguments. [`Session::settle_gate`] takes it
    /// back when the command then refuses and the commit changed nothing in the document, so a refused
    /// command leaves the running text edit alone.
    fn commit_text_edit(&mut self, origin: Origin, mode: Mode) -> Result<GateCommit, Refusal> {
        let mut gate = GateCommit::default();
        if let Some(end) = self.registry().find("text.edit_end") {
            let before = self.soft_state();
            // The editor's own result when it staged one, else an empty commit (an absent `text` keeps the content).
            let args = self.staged_text_edit().unwrap_or_else(|| json!({ "commit": true }));
            // A failed commit has rolled itself back: the edit is still running.
            let done = self.run_pipeline(end, Input::Json(&args), origin, mode)?;
            gate.restore = (!done.changed).then_some(before);
            if mode == Mode::Top {
                gate.record = Some(GateRecord { spec: end, args, outcome: Outcome::ok(done.changed, done.undo_label) });
            }
        }
        // The family clears the state when it commits. If it is not built yet, the state must not stay.
        self.text_edit = None;
        Ok(gate)
    }

    /// Finishes a gate commit once the command has succeeded or refused. A refusal after a commit that
    /// changed no document content puts the text edit and the selection back: the refused command
    /// changed nothing, and the commit leaves no ring entry. Otherwise the commit gets its ring entry,
    /// which precedes the command's.
    pub(crate) fn settle_gate(&mut self, gate: GateCommit, origin: Origin, succeeded: bool) {
        if !succeeded && let Some(before) = &gate.restore {
            self.restore_soft(before);
            return;
        }
        if let Some(r) = gate.record {
            self.record(origin, r.spec, Some(&r.args), r.outcome, 0, None, None);
        }
    }

    fn check_available(&self, spec: &'static CommandSpec) -> Result<(), Refusal> {
        guard(spec.id, || (spec.available)(self)).map_err(|r| r.for_command(spec.id))
    }

    /// `Session::availability` for a spec: gates as hints, `available`, then the target checks that the
    /// default arguments (the current selection) would hit.
    pub(crate) fn availability_of(&self, spec: &'static CommandSpec) -> Result<(), Refusal> {
        self.state_availability(spec)?;
        if spec.needs_selection == Selection::Required && self.selection().is_empty() {
            return Err(Refusal::no_target(spec.id));
        }
        if let Some(check) = spec.default_targets {
            guard(spec.id, || check(self)).map_err(|r| r.for_command(spec.id))?;
        }
        Ok(())
    }

    /// The part of availability that depends on session state alone: planned status, the gates as hints
    /// and `available`.
    pub(crate) fn state_availability(&self, spec: &'static CommandSpec) -> Result<(), Refusal> {
        if let Status::Planned(reason) = spec.status {
            return Err(Refusal::not_supported(spec.id, reason));
        }
        if self.view_only && spec.view_only == Gate::Refused {
            return Err(Refusal::view_only(spec.id));
        }
        if spec.mutates.contains(Mutates::DOC)
            && !spec.works_when_blocked
            && self.tab().doc.blocked_by_schema().is_some()
        {
            return Err(Refusal::newer_schema(spec.id));
        }
        if self.gesture.is_some() && spec.while_gesture == Gate::Refused {
            return Err(Refusal::gesture_active(spec.id));
        }
        if self.text_edit.is_some() && spec.while_text_edit == TextGate::Refused {
            return Err(Refusal::text_editing(spec.id));
        }
        self.check_available(spec)
    }

    /// Whether `id` can run now, with the refusal text a disabled control shows.
    pub fn availability(&self, id: &str) -> Result<(), Refusal> {
        match self.registry().find(id) {
            Some(spec) => self.availability_of(spec),
            None => Err(self.registry().unknown(id)),
        }
    }

    /// Runs the handler with the command id set and panics turned into `internal`.
    pub(crate) fn invoke(&mut self, spec: &'static CommandSpec, args: &dyn Any) -> Result<Value, Refusal> {
        let prev = std::mem::replace(&mut self.run.cmd, spec.id);
        self.run.depth += 1;
        let r = catch_unwind(AssertUnwindSafe(|| spec.handler.run(self, args)));
        self.run.depth = self.run.depth.saturating_sub(1);
        self.run.cmd = prev;
        match r {
            Ok(r) => r.map_err(|e| e.for_command(spec.id)),
            Err(p) => Err(Refusal::internal(spec.id, panic_message(p.as_ref()))),
        }
    }

    /// Runs a command from inside another command's handler: availability and arguments are checked, but
    /// no transaction, undo step or ring entry. The outer command owns those.
    pub fn run_nested(&mut self, id: &str, args: Value) -> Result<Value, Refusal> {
        let Some(spec) = self.registry().find(id) else {
            return Err(self.registry().unknown(id));
        };
        self.check_available(spec)?;
        let parsed = spec.handler.parse(spec.id, &args)?;
        self.invoke(spec, parsed.as_ref())
    }

    fn run_standard(
        &mut self,
        spec: &'static CommandSpec,
        parsed: &Parsed<'_>,
        mode: Mode,
    ) -> Result<StepDone, Refusal> {
        let tab_id = self.tab().id;
        let soft = self.soft_state();
        let mark = self.tab().doc.mark();
        let writes_len = self.run.writes.len();
        let warn_len = self.run.warnings.len();
        if mode == Mode::Top {
            self.run.warnings.clear();
            self.run.writes.clear();
            self.run.history_reset = false;
        }
        let result =
            self.invoke(spec, parsed.any()).and_then(|v| self.invariant_pass(tab_id, mark, spec.id).map(|()| v));
        match result {
            Err(r) => {
                let r = self.rollback(tab_id, mark, &soft, r);
                self.run.writes.truncate(if mode == Mode::Top { 0 } else { writes_len });
                self.run.warnings.truncate(if mode == Mode::Top { 0 } else { warn_len });
                Err(r.for_command(spec.id))
            }
            Ok(out) => self.finish_ok(spec, out, tab_id, mark, &soft, mode, warn_len),
        }
    }

    /// The invariant pass of §1.4 step 8 (bindings, bound text, frame membership, group integrity, z-order
    /// keys). It runs once per command, only when the document changed. The domain rules live in
    /// `drafftink-edit`; each lands here as one call when its module is implemented.
    fn invariant_pass(&mut self, tab_id: TabId, mark: DocMark, id: &str) -> Result<(), Refusal> {
        let changed = self.tab_by_id(tab_id).is_some_and(|t| t.doc.changed_since(mark));
        if !changed {
            return Ok(());
        }
        crate::invariants::run(self, tab_id, mark)
            .map_err(|e| Refusal::internal(id, format!("an invariant broke: {e}")))
    }

    /// Takes back document and soft state to the start of a command or batch.
    pub(crate) fn rollback(&mut self, tab_id: TabId, mark: DocMark, soft: &SoftState, r: Refusal) -> Refusal {
        let mut r = r;
        if let Some(t) = self.tab_by_id_mut(tab_id)
            && let Err(e) = t.doc.rollback_to(mark)
        {
            r = Refusal::internal(
                &r.command,
                format!("rolling back failed: {e}. Reload the document from the last save"),
            );
        }
        self.restore_soft(soft);
        r
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_ok(
        &mut self,
        spec: &'static CommandSpec,
        out: Value,
        tab_id: TabId,
        mark: DocMark,
        soft: &SoftState,
        mode: Mode,
        warn_len: usize,
    ) -> Result<StepDone, Refusal> {
        let mut undo_label = None;
        let summary: Summary;
        let mut pending = crate::saves::Flushed::default();
        if mode == Mode::Top {
            let depth = self.settings.history_depth();
            let reset = self.run.history_reset;
            let obs_after = self.tab_by_id(tab_id).map(|t| t.observed.clone()).unwrap_or_default();
            let undoable = matches!(spec.undo, Undo::Undoable | Undo::Gesture) && !reset;
            let now_ms = self.now_ms();
            let in_gesture_end = self.run.gesture_end;
            match self.tab_by_id_mut(tab_id) {
                Some(t) => {
                    // A gesture's final step is never merged: it is the whole drag, however close to the last one.
                    let coalesce = spec.coalesce.filter(|_| !in_gesture_end).map(|c| CoalesceReq {
                        key: c.key,
                        window_ms: c.ms,
                        now_ms,
                    });
                    let meta = StepMeta {
                        label: spec.title,
                        undoable,
                        before: &soft.observed,
                        after: &obs_after,
                        depth,
                        coalesce,
                    };
                    match t.doc.commit_since(mark, &meta) {
                        Ok(s) => {
                            if s.changed && undoable {
                                undo_label = Some(spec.title.to_owned());
                            }
                            summary = s;
                        }
                        Err(e) => {
                            // The replicated store refused the write: take the command back.
                            let r = Refusal::internal(spec.id, format!("saving the change failed: {e}"));
                            let r = self.rollback(tab_id, mark, soft, r);
                            self.run.writes.clear();
                            self.run.warnings.clear();
                            return Err(r);
                        }
                    }
                }
                None => summary = Summary::default(),
            }
            let writes = std::mem::take(&mut self.run.writes);
            let flushed = self.flush_writes(writes);
            if let Some(r) = flushed.failed_save {
                // Only `doc.save` queues a document write, and it changes nothing else: refuse.
                self.run.warnings.clear();
                return Err(r);
            }
            self.run.warnings.extend(flushed.warnings.iter().cloned());
            pending = flushed;
        } else {
            summary = self.tab_by_id(tab_id).map(|t| t.doc.summary_since(mark)).unwrap_or_default();
        }
        let warnings: Vec<String> = if mode == Mode::Top {
            std::mem::take(&mut self.run.warnings)
        } else {
            self.run.warnings.split_off(warn_len.min(self.run.warnings.len()))
        };
        let out = pending.annotate(out);
        let (out, changed) = merge_common(out, summary.changed, undo_label.as_deref(), &summary, warnings);
        if changed {
            self.mark_dirty(spec.redraw);
        }
        Ok(StepDone { out, changed, undo_label })
    }

    /// Commands that manage their own state (gesture protocol, history).
    fn run_own(&mut self, spec: &'static CommandSpec, parsed: &Parsed<'_>) -> Result<StepDone, Refusal> {
        self.run.warnings.clear();
        self.run.writes.clear();
        self.run.history_reset = false;
        match self.invoke(spec, parsed.any()) {
            Ok(out) => {
                let writes = std::mem::take(&mut self.run.writes);
                let flushed = self.flush_writes(writes);
                if let Some(r) = flushed.failed_save {
                    self.run.warnings.clear();
                    return Err(r);
                }
                self.run.warnings.extend(flushed.warnings.iter().cloned());
                let out = flushed.annotate(out);
                let warnings = std::mem::take(&mut self.run.warnings);
                let (out, changed) = merge_common(out, false, None, &Summary::default(), warnings);
                if changed {
                    self.mark_dirty(spec.redraw);
                }
                Ok(StepDone { out, changed, undo_label: None })
            }
            Err(r) => {
                self.run.writes.clear();
                self.run.warnings.clear();
                Err(r)
            }
        }
    }

    // ---- ring ----

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record(
        &mut self,
        origin: Origin,
        spec: &CommandSpec,
        args: Option<&Value>,
        outcome: Outcome,
        duration_us: u64,
        steps: Option<usize>,
        frames: Option<usize>,
    ) {
        let t_ms = self.now_ms();
        self.ring.push(Record {
            t_ms,
            id: spec.id,
            args,
            source: origin,
            outcome,
            duration_us,
            steps,
            frames,
            mutating: !spec.reads_only,
        });
    }

    pub(crate) fn record_refusal_spec(
        &mut self,
        origin: Origin,
        spec: &CommandSpec,
        args: Option<&Value>,
        r: &Refusal,
        dur: u64,
    ) {
        self.record(origin, spec, args, Outcome::refused(r), dur, None, None);
    }

    /// Records a refusal for an id that has no spec.
    pub(crate) fn record_refusal(&mut self, origin: Origin, id: &str, args: Option<&Value>, r: &Refusal, dur: u64) {
        let t_ms = self.now_ms();
        self.ring.push(Record {
            t_ms,
            id,
            args,
            source: origin,
            outcome: Outcome::refused(r),
            duration_us: dur,
            steps: None,
            frames: None,
            mutating: true,
        });
    }
}

/// Fills the common output keys (docs/commands.md §1.6) around the handler's payload. A payload that
/// has its own `changed` keeps it. Returns the output and the final `changed`.
pub(crate) fn merge_common(
    payload: Value,
    doc_changed: bool,
    undo_label: Option<&str>,
    summary: &Summary,
    warnings: Vec<String>,
) -> (Value, bool) {
    let mut obj: Map<String, Value> = match payload {
        Value::Object(m) => m,
        Value::Null => Map::new(),
        other => {
            let mut m = Map::new();
            m.insert("value".to_owned(), other);
            m
        }
    };
    let changed = obj.get("changed").and_then(Value::as_bool).unwrap_or(doc_changed);
    obj.insert("changed".to_owned(), Value::Bool(changed));
    if let Some(l) = undo_label {
        obj.entry("undo_label").or_insert_with(|| Value::String(l.to_owned()));
    }
    if !summary.created.is_empty() {
        obj.entry("created").or_insert_with(|| json!(summary.created.iter().map(|i| i.as_str()).collect::<Vec<_>>()));
    }
    if !summary.deleted.is_empty() {
        obj.entry("deleted").or_insert_with(|| json!(summary.deleted.iter().map(|i| i.as_str()).collect::<Vec<_>>()));
    }
    let mut all: Vec<Value> = match obj.remove("warnings") {
        Some(Value::Array(a)) => a,
        _ => Vec::new(),
    };
    all.extend(warnings.into_iter().map(Value::String));
    obj.insert("warnings".to_owned(), Value::Array(all));
    (Value::Object(obj), changed)
}
