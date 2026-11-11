//! Batches (docs/commands.md §1.7): many commands, one transaction, one undo step.
//!
//! `execute_batch` runs a preflight over every step, then the steps inside one transaction. The first
//! refusal rolls back the document and the small session state to the start of the batch (`atomic`), or
//! keeps the steps that ran (`atomic: false`). Port writes are queued and flushed after the last step.
//! A step that changes tabs, settings, ports or collab state cannot be rolled back and is listed in
//! `not_rolled_back`.
//!
//! References: a step may be named with `as`. Later string values `"$name"` stand for the first created
//! id (or `ids[0]`) of that step, `"$name.created"` for the whole list, `"$name.key"` for a key of the
//! output (or of its `nodes` map), and `"$1"` for step 1 (0-based). A string that starts with `$$` is the
//! literal text with one `$` removed.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::executor::{Input, Mode};
use crate::refusal::{Refusal, echo};
use crate::renders::{PendingRender, RENDER_PENDING, picture_neutral};
use crate::ring::{Origin, Outcome};
use crate::session::Session;
use crate::spec::{Batch, CommandSpec, Mutates};
use crate::store::StepMeta;
use drafftink_profile::now_ns;

/// Most steps in one batch.
pub const MAX_STEPS: usize = 10_000;
/// Largest batch JSON in bytes.
pub const MAX_BYTES: usize = 10 * 1024 * 1024;

/// One step of a batch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BatchStep {
    /// The command id.
    pub id: String,
    /// The arguments.
    #[serde(default)]
    pub args: Value,
    /// A name that later steps can refer to as `"$name"`.
    #[serde(rename = "as", default, skip_serializing_if = "Option::is_none")]
    pub as_name: Option<String>,
}

impl BatchStep {
    /// A step without a name.
    pub fn new(id: &str, args: Value) -> Self {
        Self { id: id.to_owned(), args, as_name: None }
    }

    /// Names the step.
    #[must_use]
    pub fn named(mut self, name: &str) -> Self {
        self.as_name = Some(name.to_owned());
        self
    }
}

/// Batch options.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct BatchOpts {
    /// The undo label. Default: the first step's title and the number of other steps.
    pub label: Option<String>,
    /// Roll everything back at the first refusal.
    pub atomic: bool,
    /// Run everything, report the outputs, then roll everything back.
    pub dry_run: bool,
}

impl Default for BatchOpts {
    fn default() -> Self {
        Self { label: None, atomic: true, dry_run: false }
    }
}

/// The step that stopped the batch.
#[derive(Debug, Clone, Serialize)]
pub struct BatchFailure {
    /// Index of the step.
    pub index: usize,
    /// Its command id.
    pub id: String,
    /// Why.
    pub refusal: Refusal,
}

/// A step whose effect a rollback cannot take back.
#[derive(Debug, Clone, Serialize)]
pub struct NotRolledBack {
    /// Index of the step.
    pub index: usize,
    /// Its command id.
    pub id: String,
    /// The state classes outside the transaction that it changed.
    pub classes: Vec<&'static str>,
}

/// The result of a batch.
#[derive(Debug, Clone, Serialize)]
pub struct BatchOutput {
    /// Steps that ran to completion.
    pub completed: usize,
    /// The output of each completed step.
    pub results: Vec<Value>,
    /// The step that stopped the batch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed: Option<BatchFailure>,
    /// The document and session state were put back to the start of the batch.
    pub rolled_back: bool,
    /// The undo label when a step was pushed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub undo_label: Option<String>,
    /// Steps a rollback does not cover.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub not_rolled_back: Vec<NotRolledBack>,
    /// A plain-words remark, for example that nothing ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// True when the batch ran, the state changed and it stayed.
    pub changed: bool,
    /// Steps whose picture the `Renderer` port is still drawing (the browser reads it back from the GPU). The
    /// batch is complete and committed; the result of each of these steps is a placeholder until the shell hands
    /// the token to `Session::finish_render`, which runs the step and returns its output. Only set when no step
    /// after the picture step can change what it shows; see `renders.rs`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pending: Vec<PendingRender>,
    /// A step needs a picture that the `Renderer` port is still drawing, and a later step changes what the picture
    /// shows, so it cannot wait. Nothing stays applied, even with `atomic` off: run the same batch again when the
    /// port wakes the shell, and it completes with the results of all steps. Not set when a step outside the
    /// transaction (tabs, settings, ports, collab) already ran, because running it again would repeat that step.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pending_render: bool,
}

impl BatchOutput {
    /// True when every step ran and nothing was rolled back.
    pub fn ok(&self) -> bool {
        self.failed.is_none()
    }

    /// True when the batch waits for a picture and is to be run again: see [`BatchOutput::pending_render`].
    pub fn pending_render(&self) -> bool {
        self.pending_render
    }

    /// The output as JSON.
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

fn batch_refusal(code: &'static str, id: &str, message: String) -> Refusal {
    Refusal::custom(code, id, message)
}

fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 40
        && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && !n.bytes().all(|b| b.is_ascii_digit())
}

/// A parsed `$ref`.
struct RefTok<'a> {
    base: &'a str,
    field: Option<&'a str>,
}

fn parse_ref(s: &str) -> Option<RefTok<'_>> {
    let rest = s.strip_prefix('$')?;
    if rest.starts_with('$') {
        return None;
    }
    let (base, field) = match rest.split_once('.') {
        Some((b, f)) => (b, Some(f)),
        None => (rest, None),
    };
    let word = |w: &str| !w.is_empty() && w.len() <= 40 && w.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    if word(base) && field.is_none_or(word) { Some(RefTok { base, field }) } else { None }
}

fn collect_refs<'a>(v: &'a Value, out: &mut Vec<RefTok<'a>>) {
    match v {
        Value::String(s) => out.extend(parse_ref(s)),
        Value::Array(a) => a.iter().for_each(|x| collect_refs(x, out)),
        Value::Object(m) => m.values().for_each(|x| collect_refs(x, out)),
        _ => {}
    }
}

struct Resolver<'a> {
    names: &'a HashMap<String, usize>,
    results: &'a [Value],
}

impl Resolver<'_> {
    fn output(&self, base: &str) -> Result<&Value, String> {
        let idx = match base.parse::<usize>() {
            Ok(i) => i,
            Err(_) => *self.names.get(base).ok_or_else(|| format!("`${base}` names no earlier step"))?,
        };
        self.results.get(idx).ok_or_else(|| format!("`${base}` refers to step {idx}, which has not run"))
    }

    fn resolve(&self, tok: &RefTok<'_>) -> Result<Value, String> {
        let out = self.output(tok.base)?;
        let shown = match tok.field {
            Some(f) => format!("${}.{f}", tok.base),
            None => format!("${}", tok.base),
        };
        match tok.field {
            None => out
                .get("created")
                .and_then(|c| c.get(0))
                .or_else(|| out.get("ids").and_then(|c| c.get(0)))
                .cloned()
                .ok_or_else(|| format!("`{shown}` has no created id; that step created nothing and returned no `ids`")),
            Some("created" | "deleted" | "ids") => {
                Ok(out.get(tok.field.unwrap_or("created")).cloned().unwrap_or_else(|| json!([])))
            }
            Some(f) => out
                .get("nodes")
                .and_then(|n| n.get(f))
                .or_else(|| out.get(f))
                .cloned()
                .ok_or_else(|| format!("`{shown}` is not in the output of that step")),
        }
    }

    fn substitute(&self, v: &Value) -> Result<Value, String> {
        Ok(match v {
            Value::String(s) => match parse_ref(s) {
                Some(tok) => self.resolve(&tok)?,
                None => match s.strip_prefix("$$") {
                    Some(rest) => Value::String(format!("${rest}")),
                    None => v.clone(),
                },
            },
            Value::Array(a) => {
                let mut out = Vec::with_capacity(a.len());
                for x in a {
                    let is_list_ref = matches!(x, Value::String(s) if parse_ref(s).is_some_and(|t| matches!(t.field, Some("created" | "deleted" | "ids"))));
                    let y = self.substitute(x)?;
                    match (is_list_ref, y) {
                        (true, Value::Array(items)) => out.extend(items),
                        (_, y) => out.push(y),
                    }
                }
                Value::Array(out)
            }
            Value::Object(m) => {
                let mut out = Map::new();
                for (k, x) in m {
                    out.insert(k.clone(), self.substitute(x)?);
                }
                Value::Object(out)
            }
            other => other.clone(),
        })
    }
}

const OUTSIDE_TXN: &[(Mutates, &str)] =
    &[(Mutates::TABS, "tabs"), (Mutates::SETTINGS, "settings"), (Mutates::PORTS, "ports"), (Mutates::COLLAB, "collab")];

impl Session {
    /// Runs a batch given as JSON: `{"steps": [...], "opts": {...}}`. Malformed batches and limit
    /// violations are refusals; a batch that runs and fails is a normal result with `failed` set.
    pub fn execute_batch_json(&mut self, origin: Origin, batch: &Value) -> Result<Value, Refusal> {
        const ID: &str = "batch";
        let Some(obj) = batch.as_object() else {
            return Err(Refusal::bad_args(ID, "a batch is an object with `steps` and optional `opts`"));
        };
        if let Some(bad) = obj.keys().find(|k| !matches!(k.as_str(), "steps" | "opts")) {
            return Err(Refusal::bad_args(ID, format!("unknown key `{}`. Valid keys: `steps`, `opts`", echo(bad))));
        }
        let steps_v = obj.get("steps").ok_or_else(|| Refusal::bad_args(ID, "`steps` is missing"))?;
        let n = steps_v.as_array().map_or(0, Vec::len);
        if n > MAX_STEPS {
            return Err(Refusal::limit_exceeded(ID, n, "steps", MAX_STEPS));
        }
        if steps_v.is_array() && serde_json::to_vec(batch).map_or(0, |b| b.len()) > MAX_BYTES {
            return Err(Refusal::limit_exceeded(ID, n, "steps (over 10 MB of JSON)", MAX_STEPS));
        }
        let steps: Vec<BatchStep> = serde_path_to_error::deserialize(steps_v)
            .map_err(|e| Refusal::bad_args(ID, format!("`steps.{}`: {}", e.path(), echo(&e.inner().to_string()))))?;
        let opts: BatchOpts = match obj.get("opts") {
            Some(o) => serde_path_to_error::deserialize(o)
                .map_err(|e| Refusal::bad_args(ID, format!("`opts.{}`: {}", e.path(), echo(&e.inner().to_string()))))?,
            None => BatchOpts::default(),
        };
        Ok(self.execute_batch(origin, &steps, &opts).to_json())
    }

    /// Checks every step before step 0 runs: known id, batch rule, argument shape, names and references.
    fn preflight(&self, steps: &[BatchStep]) -> Result<(), Box<(usize, Refusal)>> {
        let mut names: HashMap<&str, usize> = HashMap::new();
        let last = steps.len().saturating_sub(1);
        for (i, step) in steps.iter().enumerate() {
            let fail = |r: Refusal| Box::new((i, r));
            let Some(spec) = self.registry().find(&step.id) else {
                return Err(fail(self.registry().unknown(&step.id)));
            };
            match spec.batch {
                Batch::Never => {
                    return Err(fail(batch_refusal(
                        "not_batchable",
                        spec.id,
                        format!("`{}` cannot be part of a batch. Run it on its own with `execute`.", spec.id),
                    )));
                }
                Batch::Last if i != last => {
                    return Err(fail(batch_refusal(
                        "must_be_last",
                        spec.id,
                        format!(
                            "`{}` may only be the last step of a batch, and it is step {i} of {}. Move it to the end or run it on its own.",
                            spec.id,
                            steps.len()
                        ),
                    )));
                }
                _ => {}
            }
            let mut refs = Vec::new();
            collect_refs(&step.args, &mut refs);
            for r in &refs {
                let ok = match r.base.parse::<usize>() {
                    Ok(idx) => idx < i,
                    Err(_) => names.contains_key(r.base),
                };
                if !ok {
                    return Err(fail(Refusal::bad_args(
                        spec.id,
                        format!(
                            "`${}` does not refer to an earlier step. Name a step with `as` before using it",
                            r.base
                        ),
                    )));
                }
            }
            if refs.is_empty() {
                spec.handler.parse(spec.id, &step.args).map_err(fail)?;
            } else {
                crate::args::check_top_keys(spec.id, &step.args, &spec.handler.arg_keys()).map_err(fail)?;
            }
            if let Some(name) = &step.as_name {
                if !valid_name(name) {
                    return Err(fail(Refusal::bad_args(
                        spec.id,
                        format!(
                            "the step name `{}` must be letters, digits and underscores, and not only digits",
                            echo(name)
                        ),
                    )));
                }
                if names.insert(name.as_str(), i).is_some() {
                    return Err(fail(Refusal::bad_args(
                        spec.id,
                        format!("the step name `{}` is used twice", echo(name)),
                    )));
                }
            }
        }
        if let Some(first) = steps.first()
            && let Some(spec) = self.registry().find(&first.id)
        {
            self.state_availability(spec).map_err(|r| Box::new((0, r)))?;
        }
        Ok(())
    }

    /// Runs the steps in one transaction. See the module docs.
    pub fn execute_batch(&mut self, origin: Origin, steps: &[BatchStep], opts: &BatchOpts) -> BatchOutput {
        let t0 = now_ns();
        let mut out = self.run_batch(origin, steps, opts);
        out.changed = out.changed && !out.rolled_back;
        let dur = now_ns().saturating_sub(t0) / 1000;
        let ids: Vec<&str> = steps.iter().take(20).map(|s| s.id.as_str()).collect();
        let args = json!({ "steps": steps.len(), "ids": ids, "atomic": opts.atomic, "dry_run": opts.dry_run });
        let outcome = match &out.failed {
            Some(f) => Outcome::refused(&f.refusal),
            None => Outcome::ok(out.changed, out.undo_label.clone()),
        };
        let t_ms = self.now_ms();
        self.ring.push(crate::ring::Record {
            t_ms,
            id: "batch",
            args: Some(&args),
            source: origin,
            outcome,
            duration_us: dur,
            steps: Some(steps.len()),
            frames: None,
            mutating: true,
        });
        out
    }

    fn run_batch(&mut self, origin: Origin, steps: &[BatchStep], opts: &BatchOpts) -> BatchOutput {
        let empty = |note: &str| BatchOutput {
            completed: 0,
            results: Vec::new(),
            failed: None,
            rolled_back: false,
            undo_label: None,
            not_rolled_back: Vec::new(),
            note: Some(note.to_owned()),
            changed: false,
            pending: Vec::new(),
            pending_render: false,
        };
        if self.run.depth > 0 {
            let mut o = empty("nothing was run");
            o.failed = Some(BatchFailure {
                index: 0,
                id: "batch".to_owned(),
                refusal: Refusal::internal("batch", "a batch was started from inside a command"),
            });
            return o;
        }
        if steps.is_empty() {
            return empty("the batch has no steps, so nothing was run");
        }
        if steps.len() > MAX_STEPS {
            let mut o = empty("nothing was run");
            o.failed = Some(BatchFailure {
                index: 0,
                id: "batch".to_owned(),
                refusal: Refusal::limit_exceeded("batch", steps.len(), "steps", MAX_STEPS),
            });
            return o;
        }
        if let Err(e) = self.preflight(steps) {
            let (index, refusal) = *e;
            let mut o = empty("nothing was run: the batch failed its check before step 0");
            o.failed =
                Some(BatchFailure { index, id: steps.get(index).map(|s| s.id.clone()).unwrap_or_default(), refusal });
            return o;
        }

        let tab_id = self.tab().id;
        let soft = self.soft_state();
        let mark = self.tab().doc.mark();
        let dirty_before = self.dirty;
        self.run.warnings.clear();
        self.run.writes.clear();
        self.run.history_reset = false;
        self.run.ring_handled = false;
        self.run.origin = origin;

        let mut results: Vec<Value> = Vec::with_capacity(steps.len());
        let mut names: HashMap<String, usize> = HashMap::new();
        let mut not_rolled_back = Vec::new();
        let mut failed: Option<BatchFailure> = None;
        let mut deferred: Vec<(PendingRender, &'static CommandSpec, Value)> = Vec::new();
        let mut first_title = "";
        for (i, step) in steps.iter().enumerate() {
            let Some(spec) = self.registry().find(&step.id) else { continue };
            if i == 0 {
                first_title = spec.title;
            }
            let args = match (Resolver { names: &names, results: &results }).substitute(&step.args) {
                Ok(a) => a,
                Err(msg) => {
                    failed =
                        Some(BatchFailure { index: i, id: step.id.clone(), refusal: Refusal::bad_args(spec.id, msg) });
                    break;
                }
            };
            match self.run_pipeline(spec, Input::Json(&args), origin, Mode::Batch) {
                Ok(done) => {
                    if let Some(n) = &step.as_name {
                        names.insert(n.clone(), i);
                    }
                    let classes: Vec<&'static str> =
                        OUTSIDE_TXN.iter().filter(|(m, _)| spec.mutates.contains(*m)).map(|(_, n)| *n).collect();
                    if !classes.is_empty() {
                        not_rolled_back.push(NotRolledBack { index: i, id: spec.id.to_owned(), classes });
                    }
                    results.push(done.out);
                    // Preflight keeps tab-changing commands last by their `batch` fact. A command that only
                    // sometimes changes the tab (or lies about it) is caught here, before the next step would
                    // edit a document this batch holds no journal for.
                    if self.tab().id != tab_id
                        && let Some(next) = steps.get(i + 1)
                    {
                        failed = Some(BatchFailure {
                            index: i + 1,
                            id: next.id.clone(),
                            refusal: batch_refusal(
                                "must_be_last",
                                spec.id,
                                format!(
                                    "`{}` made another tab the active one, so the {} step(s) after it were not run. A batch edits one tab: move `{}` to the end or run the steps after it in a second batch.",
                                    spec.id,
                                    steps.len() - i - 1,
                                    spec.id
                                ),
                            ),
                        });
                        break;
                    }
                }
                Err(refusal) => {
                    // A picture that is not drawn yet is a wait, not a failure, when nothing after it can change
                    // what it shows: the batch goes on and the step follows (`renders.rs`).
                    let later = steps.get(i + 1..).unwrap_or_default();
                    let can_wait = refusal.code == RENDER_PENDING
                        && !opts.dry_run
                        && later.iter().all(|s| self.registry().find(&s.id).is_none_or(picture_neutral));
                    if can_wait {
                        let token = self.next_render_token();
                        if let Some(n) = &step.as_name {
                            names.insert(n.clone(), i);
                        }
                        results.push(json!({ "pending_render": token }));
                        deferred.push((PendingRender { index: i, id: step.id.clone(), token }, spec, args));
                        continue;
                    }
                    failed = Some(BatchFailure { index: i, id: step.id.clone(), refusal });
                    break;
                }
            }
        }
        let completed = results.len();
        // A picture that is still being drawn is not a failure: take everything back and let the caller run the
        // batch again. That is only safe when the rollback covers every step that ran.
        let pending_render =
            not_rolled_back.is_empty() && failed.as_ref().is_some_and(|f| f.refusal.code == RENDER_PENDING);
        let rollback_all = failed.is_some() && (opts.atomic || pending_render) || opts.dry_run;
        if rollback_all {
            let r = Refusal::custom("batch_rolled_back", "batch", "rolled back");
            let r = self.rollback(tab_id, mark, &soft, r);
            if r.code == "internal" {
                // Rolling back itself failed: say so instead of claiming the state was restored.
                failed = Some(BatchFailure { index: completed, id: "batch".to_owned(), refusal: r });
            }
            self.run.writes.clear();
            self.run.warnings.clear();
            self.dirty = dirty_before;
            // A rollback that itself failed replaced the refusal: that is no longer a wait.
            let pending_render = pending_render && failed.as_ref().is_some_and(|f| f.refusal.code == RENDER_PENDING);
            let note = if opts.dry_run && failed.is_none() {
                Some("dry run: everything was rolled back".to_owned())
            } else if pending_render {
                Some(format!(
                    "step {} (`{}`) waits for a picture that is still being drawn, so nothing was applied yet. Run the batch again when it is ready",
                    failed.as_ref().map_or(0, |f| f.index),
                    failed.as_ref().map_or("", |f| f.id.as_str()),
                ))
            } else {
                failed.as_ref().map(|f| {
                    format!(
                        "step {} (`{}`) was refused, so all {} earlier step(s) were rolled back",
                        f.index, f.id, completed
                    )
                })
            };
            return BatchOutput {
                completed,
                results,
                failed,
                rolled_back: true,
                undo_label: None,
                not_rolled_back,
                note,
                changed: false,
                pending: Vec::new(),
                pending_render,
            };
        }

        // Success, or a non-atomic failure: what ran stays, as one undo step.
        let depth = self.settings.history_depth();
        let reset = self.run.history_reset;
        let title = opts.label.clone().unwrap_or_else(|| match steps.len() {
            1 => first_title.to_owned(),
            n => format!("{first_title} + {} more", n - 1),
        });
        let mut changed = false;
        let mut undo_label = None;
        let obs_after = self.tab_by_id(tab_id).map(|t| t.observed.clone()).unwrap_or_default();
        let mut save_error = None;
        if let Some(t) = self.tab_by_id_mut(tab_id) {
            let meta = StepMeta {
                label: &title,
                undoable: !reset,
                before: &soft.observed,
                after: &obs_after,
                depth,
                coalesce: None,
            };
            match t.doc.commit_since(mark, &meta) {
                Ok(summary) => {
                    changed = summary.changed;
                    if summary.changed && !reset {
                        undo_label = Some(title.clone());
                    }
                }
                Err(e) => save_error = Some(e),
            }
        }
        if let Some(e) = save_error {
            let r = Refusal::internal("batch", format!("saving the change failed: {e}"));
            let r = self.rollback(tab_id, mark, &soft, r);
            self.run.writes.clear();
            self.run.warnings.clear();
            self.dirty = dirty_before;
            return BatchOutput {
                completed,
                results,
                failed: Some(BatchFailure { index: completed, id: "batch".to_owned(), refusal: r }),
                rolled_back: true,
                undo_label: None,
                not_rolled_back,
                note: Some("the batch could not be saved, so it was rolled back".to_owned()),
                changed: false,
                pending: Vec::new(),
            pending_render: false,
            };
        }
        let writes = std::mem::take(&mut self.run.writes);
        let flushed = self.flush_writes(writes);
        self.run.warnings.clear();
        let mut flush_notes = flushed.warnings;
        if let Some(r) = flushed.failed_save {
            // The steps ran and stay; only the file was not written.
            flush_notes.push(r.message);
        }
        let mut note = None;
        if !flush_notes.is_empty() {
            note = Some(flush_notes.join("; "));
        }
        if let Some(f) = &failed {
            note = Some(format!(
                "step {} (`{}`) was refused. The {} earlier step(s) stay applied as one undo step because atomic is off",
                f.index, f.id, completed
            ));
        }
        let pending = deferred
            .into_iter()
            .map(|(pending, spec, args)| {
                self.defer_render(pending.token, origin, spec, args);
                pending
            })
            .collect();
        BatchOutput {
            completed,
            results,
            failed,
            rolled_back: false,
            undo_label,
            not_rolled_back,
            note,
            changed,
            pending,
            pending_render: false,
        }
    }
}
