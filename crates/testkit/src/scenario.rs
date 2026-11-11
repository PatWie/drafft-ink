//! Scenarios: one JSON file, one test.
//!
//! A scenario file lives in `tests/scenarios/<family>/<name>.json`, where `<family>` is a command family (or
//! another topic directory such as `headline`). `build.rs` generates one `#[test]` for each file, so adding a file
//! adds a test with no Rust edit. `docs/testing.md` has the guide; this page is the reference.
//!
//! ```json
//! {
//!   "about": "What this proves, in a sentence.",
//!   "ports": "full",
//!   "requires": { "commands": ["element.move"], "gpu": false },
//!   "steps": [ ... ]
//! }
//! ```
//!
//! * `about` (required): the sentence a reader needs to know what a failure means.
//! * `ports`: `"full"` (default, every fake port) or `"headless"` (clock and ids only).
//! * `requires.commands`: the scenario waits for commands that are not registered yet. While one is missing the
//!   test is *skipped with a message* naming it; the accounting test checks the names are real commands.
//! * `requires.gpu`: needs a GPU adapter for pixel probes. Skipped without one, unless `DRAFFTINK_REQUIRE_GPU=1`.
//!
//! # Steps
//!
//! Every step is an object with exactly one *verb* key and the keys that go with it. Any step may carry a
//! `"note"` string, and `{"note": "..."}` alone is a step that does nothing.
//!
//! | Verb | Keys | Does |
//! |---|---|---|
//! | `execute` | `args`, `refused`, `text`, `expect`, `save` | runs the command through `Session::execute` |
//! | `batch` | `opts`, `refused`, `text`, `expect`, `save` | runs `[{id, args, as}]` through `execute_batch` |
//! | `assert` | `args`, `refused`, `text`, `expect`, `save` | runs a **read-only** command and checks its output |
//! | `undo`, `redo` | `refused`, `text`, `expect`, `save` | `history.undo` / `history.redo` with that many steps |
//! | `harness` | `args`, `fails`, `expect`, `save` | calls a [`crate::Harness`] method |
//! | `snapshot` | `of_command` or `of_harness`, `args`, `select` | compares a text with a stored file |
//!
//! * A step must succeed unless it has `refused: "<code>"`; then it must be refused with that code, and `text`
//!   (a string or a list) must appear in the refusal message. `expect` then checks the refusal as JSON
//!   (`code`, `message`, `command`, `suggest`, `details`).
//! * `expect` is a list of [checks](crate::expect) on the output (the success output, or the refusal).
//! * `save: {"name": "$.path"}` stores values of the output; later steps use them as `{{name}}` in `args` and in
//!   expected values. A whole string `"{{name}}"` keeps the value's type.
//! * `snapshot` writes nothing unless `DRAFFTINK_UPDATE_SNAPSHOTS=1`. The file is
//!   `<family>/snapshots/<file stem>__<name>.txt`. `select` picks a part of the output; a string is stored as
//!   it is, anything else as pretty JSON. A snapshot file is part of the test: review its diff like code.
//!
//! Unknown keys are errors, so a typo cannot turn a check into a no-op.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use drafftink_engine::{Origin, Refusal};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::expect::Check;
use crate::harness::{self, GpuStatus, Harness, HarnessError, PixelArgs};
use crate::path::{self, Vars};

// ---------------------------------------------------------------------------------------------
// The harness methods a scenario may call
// ---------------------------------------------------------------------------------------------

fn parse_args<T: DeserializeOwned>(method: &str, args: &Value) -> Result<T, HarnessError> {
    serde_json::from_value(args.clone())
        .map_err(|e| HarnessError::BadArgs { method: method.to_owned(), why: e.to_string() })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StageFixtureArgs {
    path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoadGeneratedArgs {
    count: usize,
    #[serde(default = "one")]
    seed: u64,
}

fn one() -> u64 {
    1
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdvanceArgs {
    ms: u64,
}

/// Declares the scenario-callable harness methods once: the name list and the dispatcher come from the same
/// lines, so they cannot disagree. `tests/accounting.rs` checks the list against the methods of `Harness`.
macro_rules! scenario_methods {
    ($($name:literal => |$h:ident, $args:ident| $body:expr),+ $(,)?) => {
        /// The harness methods a scenario's `"harness"` step can name.
        pub const HARNESS_METHODS: &[&str] = &[$($name),+];

        fn dispatch(h: &mut Harness, method: &str, args: &Value) -> Result<Value, HarnessError> {
            match method {
                $($name => { let $h = h; let $args = args; $body })+
                other => Err(HarnessError::BadArgs {
                    method: other.to_owned(),
                    why: format!("no such harness method. Methods: {}", HARNESS_METHODS.join(", ")),
                }),
            }
        }
    };
}

scenario_methods! {
    "stage_fixture" => |h, a| { let p: StageFixtureArgs = parse_args("stage_fixture", a)?; h.stage_fixture(&p.path) },
    "load_generated" => |h, a| { let p: LoadGeneratedArgs = parse_args("load_generated", a)?; h.load_generated(p.count, p.seed) },
    "advance_clock" => |h, a| { let p: AdvanceArgs = parse_args("advance_clock", a)?; h.advance_clock(p.ms) },
    "pixel_probe" => |h, a| { let p: PixelArgs = parse_args("pixel_probe", a)?; h.pixel_probe(&p) },
}

// ---------------------------------------------------------------------------------------------
// Reading a scenario
// ---------------------------------------------------------------------------------------------

/// How the session is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PortsMode {
    /// Every fake port.
    #[default]
    Full,
    /// Clock and ids only.
    Headless,
}

/// What a scenario needs before it can run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Requires {
    /// Commands that must be registered.
    pub commands: Vec<String>,
    /// A GPU adapter.
    pub gpu: bool,
}

/// What a step does after the call returned.
#[derive(Debug, Clone, Default)]
pub struct Then {
    /// The refusal code the step must end with; `None` means it must succeed.
    pub refused: Option<String>,
    /// Text the refusal message (or the harness error) must contain.
    pub text: Vec<String>,
    /// Checks on the output.
    pub expect: Vec<Check>,
    /// `name -> path` pairs stored for later steps.
    pub save: Vec<(String, String)>,
}

/// Where a snapshot's text comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A read-only command.
    Command(String),
    /// A harness method.
    Harness(String),
}

/// The verb of a step.
#[derive(Debug, Clone)]
pub enum StepKind {
    /// Run a command.
    Execute {
        /// Command id.
        id: String,
        /// Arguments.
        args: Value,
        /// Afterwards.
        then: Then,
    },
    /// Run a batch.
    Batch {
        /// The steps, `{id, args, as}`.
        steps: Vec<Value>,
        /// Batch options.
        opts: Value,
        /// Afterwards.
        then: Then,
    },
    /// Run a read-only command.
    Assert {
        /// Command id.
        id: String,
        /// Arguments.
        args: Value,
        /// Afterwards.
        then: Then,
    },
    /// `history.undo` or `history.redo`.
    History {
        /// True for undo.
        undo: bool,
        /// Steps.
        steps: u64,
        /// Afterwards.
        then: Then,
    },
    /// Call a harness method.
    Harness {
        /// Method name.
        method: String,
        /// Arguments.
        args: Value,
        /// A substring of the error the call must fail with.
        fails: Option<String>,
        /// Afterwards.
        then: Then,
    },
    /// Compare with a stored text.
    Snapshot {
        /// Snapshot name.
        name: String,
        /// Where the text comes from.
        source: Source,
        /// Arguments of the source.
        args: Value,
        /// A path into the output.
        select: Option<String>,
    },
    /// Only a note.
    Note,
}

/// One step with its position.
#[derive(Debug, Clone)]
pub struct Step {
    /// Zero-based position.
    pub index: usize,
    /// The `note`, if any.
    pub note: Option<String>,
    /// What it does.
    pub kind: StepKind,
}

/// A parsed scenario file.
#[derive(Debug, Clone)]
pub struct Scenario {
    /// What it proves.
    pub about: String,
    /// How the session is built.
    pub ports: PortsMode,
    /// What it waits for.
    pub requires: Requires,
    /// The steps.
    pub steps: Vec<Step>,
}

const VERBS: &[&str] = &["execute", "batch", "assert", "undo", "redo", "harness", "snapshot"];

fn object<'a>(v: &'a Value, what: &str) -> Result<&'a serde_json::Map<String, Value>, String> {
    v.as_object().ok_or_else(|| format!("{what} must be an object, got {v}"))
}

fn only_keys(m: &serde_json::Map<String, Value>, allowed: &[&str], what: &str) -> Result<(), String> {
    match m.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => Err(format!("unknown key `{k}` in {what}. Keys: {}", allowed.join(", "))),
        None => Ok(()),
    }
}

fn string_of(m: &serde_json::Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match m.get(key) {
        None => Ok(None),
        Some(Value::String(s)) if !s.is_empty() => Ok(Some(s.clone())),
        Some(other) => Err(format!("`{key}` must be a non-empty string, got {other}")),
    }
}

fn parse_then(m: &serde_json::Map<String, Value>) -> Result<Then, String> {
    let refused = string_of(m, "refused")?;
    let text = match m.get("text") {
        None => Vec::new(),
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| x.as_str().map(str::to_owned).ok_or_else(|| format!("`text` items must be strings, got {x}")))
            .collect::<Result<_, _>>()?,
        Some(other) => return Err(format!("`text` must be a string or a list of strings, got {other}")),
    };
    if !text.is_empty() && refused.is_none() && !m.contains_key("fails") {
        return Err("`text` checks a refusal message, so it needs `refused` (or `fails` on a harness step)".to_owned());
    }
    let expect = match m.get("expect") {
        None => Vec::new(),
        Some(Value::Array(a)) => a
            .iter()
            .enumerate()
            .map(|(i, c)| Check::parse(c).map_err(|e| format!("expect[{i}]: {e}")))
            .collect::<Result<_, _>>()?,
        Some(other) => return Err(format!("`expect` must be a list of checks, got {other}")),
    };
    let save = match m.get("save") {
        None => Vec::new(),
        Some(Value::Object(s)) => s
            .iter()
            .map(|(name, p)| {
                let p = p.as_str().ok_or_else(|| format!("`save.{name}` must be a path string, got {p}"))?;
                path::validate(p).map_err(|e| e.to_string())?;
                Ok((name.clone(), p.to_owned()))
            })
            .collect::<Result<_, String>>()?,
        Some(other) => return Err(format!("`save` must be an object of name: path, got {other}")),
    };
    Ok(Then { refused, text, expect, save })
}

fn args_of(m: &serde_json::Map<String, Value>) -> Result<Value, String> {
    match m.get("args") {
        None => Ok(json!({})),
        Some(v @ Value::Object(_)) => Ok(v.clone()),
        Some(other) => Err(format!("`args` must be an object, got {other}")),
    }
}

const THEN_KEYS: &[&str] = &["refused", "text", "expect", "save", "note"];

fn keys_with(extra: &[&'static str]) -> Vec<&'static str> {
    extra.iter().chain(THEN_KEYS).copied().collect()
}

fn parse_step(index: usize, v: &Value) -> Result<Step, String> {
    let m = object(v, "a step")?;
    let verbs: Vec<&str> = VERBS.iter().copied().filter(|k| m.contains_key(*k)).collect();
    let note = match m.get("note") {
        None => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(other) => return Err(format!("`note` must be a string, got {other}")),
    };
    let kind = match verbs.as_slice() {
        [] => {
            only_keys(m, &["note"], "a note step")?;
            if note.is_none() {
                return Err(format!("a step needs one of: {}, or a `note`", VERBS.join(", ")));
            }
            StepKind::Note
        }
        [verb] => match *verb {
            "execute" | "assert" => {
                only_keys(m, &keys_with(&[verb, "args"]), &format!("an `{verb}` step"))?;
                let id = string_of(m, verb)?.ok_or("missing command id")?;
                let (args, then) = (args_of(m)?, parse_then(m)?);
                if *verb == "execute" {
                    StepKind::Execute { id, args, then }
                } else {
                    StepKind::Assert { id, args, then }
                }
            }
            "batch" => {
                only_keys(m, &keys_with(&["batch", "opts"]), "a `batch` step")?;
                let Some(Value::Array(steps)) = m.get("batch") else {
                    return Err("`batch` must be a list of {id, args, as}".to_owned());
                };
                for (i, s) in steps.iter().enumerate() {
                    let sm = object(s, &format!("batch[{i}]"))?;
                    only_keys(sm, &["id", "args", "as"], &format!("batch[{i}]"))?;
                    if string_of(sm, "id")?.is_none() {
                        return Err(format!("batch[{i}] needs an `id`"));
                    }
                }
                StepKind::Batch {
                    steps: steps.clone(),
                    opts: m.get("opts").cloned().unwrap_or_else(|| json!({})),
                    then: parse_then(m)?,
                }
            }
            "undo" | "redo" => {
                only_keys(m, &keys_with(&[verb]), &format!("an `{verb}` step"))?;
                let steps = match m.get(*verb) {
                    Some(Value::Bool(true)) => 1,
                    Some(n) => n
                        .as_u64()
                        .filter(|n| *n >= 1)
                        .ok_or_else(|| format!("`{verb}` is true or a step count of 1 or more, got {n}"))?,
                    None => 1,
                };
                StepKind::History { undo: *verb == "undo", steps, then: parse_then(m)? }
            }
            "harness" => {
                only_keys(m, &keys_with(&["harness", "args", "fails"]), "a `harness` step")?;
                let method = string_of(m, "harness")?.ok_or("missing method name")?;
                if !HARNESS_METHODS.contains(&method.as_str()) {
                    return Err(format!("no harness method `{method}`. Methods: {}", HARNESS_METHODS.join(", ")));
                }
                let fails = string_of(m, "fails")?;
                let then = parse_then(m)?;
                if fails.is_some() && (then.refused.is_some() || !then.save.is_empty()) {
                    return Err("`fails` cannot be combined with `refused` or `save`".to_owned());
                }
                StepKind::Harness { method, args: args_of(m)?, fails, then }
            }
            _ => {
                only_keys(m, &["snapshot", "of_command", "of_harness", "args", "select", "note"], "a `snapshot` step")?;
                let name = string_of(m, "snapshot")?.ok_or("missing snapshot name")?;
                if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                    return Err(format!("snapshot name `{name}` may use letters, digits, `_` and `-` only"));
                }
                let source = match (string_of(m, "of_command")?, string_of(m, "of_harness")?) {
                    (Some(c), None) => Source::Command(c),
                    (None, Some(h)) if HARNESS_METHODS.contains(&h.as_str()) => Source::Harness(h),
                    (None, Some(h)) => {
                        return Err(format!("no harness method `{h}`. Methods: {}", HARNESS_METHODS.join(", ")));
                    }
                    _ => return Err("a snapshot needs exactly one of `of_command` or `of_harness`".to_owned()),
                };
                let select = string_of(m, "select")?;
                if let Some(s) = &select {
                    path::validate(s).map_err(|e| e.to_string())?;
                }
                StepKind::Snapshot { name, source, args: args_of(m)?, select }
            }
        },
        many => return Err(format!("a step has one verb, this one has {}", many.join(" and "))),
    };
    Ok(Step { index, note, kind })
}

impl Scenario {
    /// Reads a scenario from JSON text. Errors name the step.
    pub fn parse(text: &str) -> Result<Scenario, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
        let m = object(&v, "a scenario")?;
        only_keys(m, &["about", "ports", "requires", "steps"], "the scenario")?;
        let about = string_of(m, "about")?.ok_or("`about` is required: one sentence on what this scenario proves")?;
        let ports = match m.get("ports").and_then(Value::as_str) {
            None if !m.contains_key("ports") => PortsMode::Full,
            Some("full") => PortsMode::Full,
            Some("headless") => PortsMode::Headless,
            _ => return Err("`ports` is \"full\" or \"headless\"".to_owned()),
        };
        let requires = match m.get("requires") {
            None => Requires::default(),
            Some(r) => {
                let rm = object(r, "`requires`")?;
                only_keys(rm, &["commands", "gpu"], "`requires`")?;
                let commands = match rm.get("commands") {
                    None => Vec::new(),
                    Some(Value::Array(a)) => a
                        .iter()
                        .map(|x| {
                            x.as_str()
                                .map(str::to_owned)
                                .ok_or_else(|| format!("`requires.commands` holds command ids, got {x}"))
                        })
                        .collect::<Result<_, _>>()?,
                    Some(other) => return Err(format!("`requires.commands` must be a list, got {other}")),
                };
                let gpu = match rm.get("gpu") {
                    None => false,
                    Some(Value::Bool(b)) => *b,
                    Some(other) => return Err(format!("`requires.gpu` must be true or false, got {other}")),
                };
                Requires { commands, gpu }
            }
        };
        let Some(Value::Array(raw)) = m.get("steps") else {
            return Err("`steps` must be a list".to_owned());
        };
        if raw.is_empty() {
            return Err("`steps` is empty: a scenario with no steps proves nothing".to_owned());
        }
        let steps = raw
            .iter()
            .enumerate()
            .map(|(i, s)| parse_step(i, s).map_err(|e| format!("step {i}: {e}")))
            .collect::<Result<_, _>>()?;
        Ok(Scenario { about, ports, requires, steps })
    }

    /// Every command id the scenario names (steps, batches, undo and redo, snapshot sources). A step that is
    /// meant to be refused as `unknown_command` names a command that does not exist, so it is left out.
    pub fn commands(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for s in &self.steps {
            match &s.kind {
                StepKind::Execute { id, then, .. } | StepKind::Assert { id, then, .. } => {
                    if then.refused.as_deref() != Some("unknown_command") {
                        out.insert(id.clone());
                    }
                }
                StepKind::Batch { steps, .. } => {
                    out.extend(steps.iter().filter_map(|b| b.get("id").and_then(Value::as_str)).map(str::to_owned));
                }
                StepKind::History { undo, .. } => {
                    out.insert(if *undo { "history.undo" } else { "history.redo" }.to_owned());
                }
                StepKind::Snapshot { source: Source::Command(c), .. } => {
                    out.insert(c.clone());
                }
                _ => {}
            }
        }
        out
    }

    /// Every harness method the scenario calls (steps and snapshot sources).
    pub fn harness_calls(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for s in &self.steps {
            match &s.kind {
                StepKind::Harness { method, .. } | StepKind::Snapshot { source: Source::Harness(method), .. } => {
                    out.insert(method.clone());
                }
                _ => {}
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------------------------
// Running one
// ---------------------------------------------------------------------------------------------

/// How a run behaves. [`Options::from_env`] reads `DRAFFTINK_UPDATE_SNAPSHOTS`, `DRAFFTINK_SCENARIO_TRACE` and
/// `DRAFFTINK_SCENARIO_KEEP_GOING`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Write snapshot files instead of comparing.
    pub update_snapshots: bool,
    /// Print every step and its output.
    pub trace: bool,
    /// Do not stop at the first failing step; report every failure at the end. For exploring a new scenario.
    pub keep_going: bool,
}

impl Options {
    /// From the environment.
    pub fn from_env() -> Self {
        let on = |k: &str| std::env::var(k).is_ok_and(|v| !v.is_empty() && v != "0");
        Self {
            update_snapshots: on("DRAFFTINK_UPDATE_SNAPSHOTS"),
            trace: on("DRAFFTINK_SCENARIO_TRACE"),
            keep_going: on("DRAFFTINK_SCENARIO_KEEP_GOING"),
        }
    }
}

/// How a run ended without failing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Every step passed.
    Passed {
        /// How many steps ran.
        steps: usize,
    },
    /// The scenario is waiting for something this machine or this build does not have.
    Skipped {
        /// The reason, in plain words.
        reason: String,
    },
}

fn refusal_json(r: &Refusal) -> Value {
    serde_json::to_value(r).unwrap_or_else(|_| json!({ "code": r.code, "message": r.message }))
}

struct Run<'a> {
    h: Harness,
    vars: Vars,
    opts: Options,
    snapshots: &'a Path,
    stem: String,
}

enum Called {
    Out(Value),
    Refused(Value, String, String),
    Failed(String),
}

impl Run<'_> {
    fn sub(&self, v: &Value) -> Result<Value, String> {
        path::substitute(v, &self.vars)
    }

    fn call(&mut self, kind: &StepKind) -> Result<Called, String> {
        let h = &mut self.h;
        Ok(match kind {
            StepKind::Execute { id, args, .. } => {
                let args = path::substitute(args, &self.vars)?;
                match h.session.execute_from(Origin::Test, id, args) {
                    Ok(v) => Called::Out(v),
                    Err(r) => Called::Refused(refusal_json(&r), r.code.to_owned(), r.message),
                }
            }
            StepKind::Assert { id, args, .. } => {
                let Some(spec) = h.session.spec(id) else {
                    let r = h.session.registry().unknown(id);
                    return Ok(Called::Refused(refusal_json(&r), r.code.to_owned(), r.message));
                };
                if !spec.reads_only {
                    return Err(format!(
                        "`assert` runs read-only commands, and `{id}` changes state. Use `execute` for it, then `assert` a read-only command such as `engine.state`."
                    ));
                }
                let args = path::substitute(args, &self.vars)?;
                match h.session.execute_from(Origin::Test, id, args) {
                    Ok(v) => Called::Out(v),
                    Err(r) => Called::Refused(refusal_json(&r), r.code.to_owned(), r.message),
                }
            }
            StepKind::Batch { steps, opts, .. } => {
                let batch = path::substitute(&json!({ "steps": steps, "opts": opts }), &self.vars)?;
                match h.session.execute_batch_json(Origin::Test, &batch) {
                    Ok(v) => Called::Out(v),
                    Err(r) => Called::Refused(refusal_json(&r), r.code.to_owned(), r.message),
                }
            }
            StepKind::History { undo, steps, .. } => {
                let id = if *undo { "history.undo" } else { "history.redo" };
                match h.session.execute_from(Origin::Test, id, json!({ "steps": steps })) {
                    Ok(v) => Called::Out(v),
                    Err(r) => Called::Refused(refusal_json(&r), r.code.to_owned(), r.message),
                }
            }
            StepKind::Harness { method, args, .. } => {
                let args = path::substitute(args, &self.vars)?;
                match dispatch(h, method, &args) {
                    Ok(v) => Called::Out(v),
                    Err(e) => Called::Failed(e.to_string()),
                }
            }
            StepKind::Snapshot { .. } | StepKind::Note => Called::Out(Value::Null),
        })
    }

    fn then_of(kind: &StepKind) -> Option<&Then> {
        match kind {
            StepKind::Execute { then, .. }
            | StepKind::Batch { then, .. }
            | StepKind::Assert { then, .. }
            | StepKind::History { then, .. }
            | StepKind::Harness { then, .. } => Some(then),
            StepKind::Snapshot { .. } | StepKind::Note => None,
        }
    }

    fn step(&mut self, step: &Step) -> Result<(), String> {
        let StepKind::Snapshot { name, source, args, select } = &step.kind else {
            return self.ordinary(step);
        };
        let args = self.sub(args)?;
        let out = match source {
            Source::Command(id) => {
                let spec = self.h.session.spec(id);
                if spec.is_some_and(|s| !s.reads_only) {
                    return Err(format!("a snapshot reads a read-only command, and `{id}` changes state"));
                }
                self.h
                    .session
                    .execute_from(Origin::Test, id, args)
                    .map_err(|r| format!("`{id}` was refused: {}", r.message))?
            }
            Source::Harness(method) => dispatch(&mut self.h, method, &args).map_err(|e| e.to_string())?,
        };
        let picked = match select {
            Some(p) => path::select(&out, p)
                .map_err(|e| e.to_string())?
                .cloned()
                .ok_or_else(|| format!("`select` {p} leads nowhere in {out}"))?,
            None => out,
        };
        let mut text = match &picked {
            Value::String(s) => s.clone(),
            other => serde_json::to_string_pretty(other).map_err(|e| e.to_string())?,
        };
        if !text.ends_with('\n') {
            text.push('\n');
        }
        let file = self.snapshots.join(format!("{}__{name}.txt", self.stem));
        compare_snapshot(&file, &text, self.opts.update_snapshots)
    }

    fn ordinary(&mut self, step: &Step) -> Result<(), String> {
        let called = self.call(&step.kind)?;
        let Some(then) = Self::then_of(&step.kind) else { return Ok(()) };
        let fails = match &step.kind {
            StepKind::Harness { fails, .. } => fails.as_deref(),
            _ => None,
        };
        if self.opts.trace {
            match &called {
                Called::Out(v) => eprintln!("  step {}: ok {v}", step.index),
                Called::Refused(_, code, msg) => eprintln!("  step {}: refused {code}: {msg}", step.index),
                Called::Failed(e) => eprintln!("  step {}: failed: {e}", step.index),
            }
        }
        let value = match (called, &then.refused, fails) {
            (Called::Out(v), None, None) => v,
            (Called::Out(v), Some(code), _) => {
                return Err(format!(
                    "expected a refusal with code `{code}`, but the step succeeded with {}",
                    compact(&v)
                ));
            }
            (Called::Out(v), None, Some(f)) => {
                return Err(format!("expected the harness call to fail with `{f}`, but it returned {}", compact(&v)));
            }
            (Called::Refused(_, code, msg), None, _) => {
                return Err(format!("expected success, but the step was refused with `{code}`: {msg}"));
            }
            (Called::Refused(v, code, msg), Some(want), _) => {
                if &code != want {
                    return Err(format!("expected refusal `{want}`, got `{code}`: {msg}"));
                }
                for t in &then.text {
                    if !msg.contains(t.as_str()) {
                        return Err(format!("the refusal message does not contain `{t}`. It says: {msg}"));
                    }
                }
                v
            }
            (Called::Failed(e), _, None) => return Err(format!("the harness call failed: {e}")),
            (Called::Failed(e), _, Some(f)) => {
                if !e.contains(f) {
                    return Err(format!("the harness error does not contain `{f}`. It says: {e}"));
                }
                for t in &then.text {
                    if !e.contains(t.as_str()) {
                        return Err(format!("the harness error does not contain `{t}`. It says: {e}"));
                    }
                }
                Value::String(e)
            }
        };
        for (i, c) in then.expect.iter().enumerate() {
            c.run(&value, &self.vars).map_err(|e| format!("expect[{i}] failed: {e}\n  output: {}", compact(&value)))?;
        }
        for (name, p) in &then.save {
            let got = path::select(&value, p).map_err(|e| e.to_string())?;
            let Some(got) = got else {
                return Err(format!("`save` {name}: nothing at {p} in {}", compact(&value)));
            };
            self.vars.insert(name.clone(), got.clone());
        }
        Ok(())
    }
}

fn compact(v: &Value) -> String {
    let t = v.to_string();
    if t.chars().count() > 1200 {
        format!("{}... ({} bytes)", t.chars().take(1200).collect::<String>(), t.len())
    } else {
        t
    }
}

fn compare_snapshot(file: &Path, text: &str, update: bool) -> Result<(), String> {
    let shown = file.display();
    if update {
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        return std::fs::write(file, text).map_err(|e| format!("cannot write {shown}: {e}"));
    }
    let stored = std::fs::read_to_string(file).map_err(|e| {
        format!("snapshot {shown} cannot be read ({e}). To create it, run the test with DRAFFTINK_UPDATE_SNAPSHOTS=1, read the file, and commit it.")
    })?;
    if stored == text {
        return Ok(());
    }
    let first = stored
        .lines()
        .zip(text.lines())
        .position(|(a, b)| a != b)
        .unwrap_or_else(|| stored.lines().count().min(text.lines().count()));
    Err(format!(
        "snapshot {shown} differs at line {}.\n  stored: {}\n  now:    {}\nIf the change is intended, run with DRAFFTINK_UPDATE_SNAPSHOTS=1 and review the diff of the file.",
        first + 1,
        stored.lines().nth(first).unwrap_or("(end of file)"),
        text.lines().nth(first).unwrap_or("(end of file)"),
    ))
}

/// Runs a parsed scenario. `snapshots` is the directory of snapshot files and `stem` the file name without
/// extension. A failure message starts with the step number and verb.
pub fn run(sc: &Scenario, snapshots: &Path, stem: &str, opts: Options) -> Result<Outcome, String> {
    let h = match sc.ports {
        PortsMode::Full => Harness::new(),
        PortsMode::Headless => Harness::headless(),
    };
    let missing: Vec<&str> =
        sc.requires.commands.iter().filter(|c| h.session.spec(c).is_none()).map(String::as_str).collect();
    if !missing.is_empty() {
        return Ok(Outcome::Skipped {
            reason: format!("waiting for {} (not registered yet); the steps below run once it is", missing.join(", ")),
        });
    }
    if sc.requires.gpu
        && let GpuStatus::Missing(why) = harness::gpu_status()
    {
        if harness::gpu_required() {
            return Err(format!(
                "this scenario needs a GPU adapter and DRAFFTINK_REQUIRE_GPU is set, but there is none: {why}"
            ));
        }
        return Ok(Outcome::Skipped {
            reason: format!("no GPU adapter here ({why}); set DRAFFTINK_REQUIRE_GPU=1 to fail instead"),
        });
    }
    let mut run = Run { h, vars: BTreeMap::new(), opts, snapshots, stem: stem.to_owned() };
    let mut failures = Vec::new();
    for step in &sc.steps {
        let result = run.step(step).map_err(|e| {
            let verb = match &step.kind {
                StepKind::Execute { id, .. } => format!("execute {id}"),
                StepKind::Assert { id, .. } => format!("assert {id}"),
                StepKind::Batch { .. } => "batch".to_owned(),
                StepKind::History { undo, .. } => if *undo { "undo" } else { "redo" }.to_owned(),
                StepKind::Harness { method, .. } => format!("harness {method}"),
                StepKind::Snapshot { name, .. } => format!("snapshot {name}"),
                StepKind::Note => "note".to_owned(),
            };
            let note = step.note.as_deref().map(|n| format!("\n  note: {n}")).unwrap_or_default();
            format!("step {} ({verb}) failed: {e}{note}", step.index)
        });
        match result {
            Ok(()) => {}
            Err(e) if opts.keep_going => failures.push(e),
            Err(e) => return Err(e),
        }
    }
    if !failures.is_empty() {
        return Err(failures.join("\n"));
    }
    Ok(Outcome::Passed { steps: sc.steps.len() })
}

/// Reads and runs one scenario file.
pub fn run_file(path: &Path, opts: Options) -> Result<Outcome, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let sc = Scenario::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let dir = path.parent().map(|p| p.join("snapshots")).unwrap_or_else(|| PathBuf::from("snapshots"));
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("scenario");
    run(&sc, &dir, stem, opts).map_err(|e| format!("{}: {e}", path.display()))
}

/// What the generated tests call. Prints a skip to stderr; panics with the failure text.
#[allow(clippy::panic)]
pub fn run_file_or_panic(path: &str) {
    match run_file(Path::new(path), Options::from_env()) {
        Ok(Outcome::Passed { steps }) => eprintln!("scenario passed ({steps} steps): {path}"),
        Ok(Outcome::Skipped { reason }) => eprintln!("SCENARIO SKIPPED: {reason}\n  {path}"),
        Err(e) => panic!("\n{e}\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_text(text: &str) -> Result<Outcome, String> {
        let sc = Scenario::parse(text)?;
        let dir = std::env::temp_dir().join("drafftink-testkit-unit");
        run(&sc, &dir, "unit", Options::default())
    }

    fn wrap(steps: &str) -> String {
        format!(r#"{{"about": "unit", "steps": {steps}}}"#)
    }

    #[test]
    fn the_dispatcher_knows_every_listed_method_and_nothing_else() {
        let mut h = Harness::new();
        for m in HARNESS_METHODS {
            // `{}` is wrong for most methods; the point is that the answer is not "no such method".
            let r = dispatch(&mut h, m, &json!({"__bogus": 1}));
            let e = r.unwrap_err().to_string();
            assert!(!e.contains("no such harness method"), "{m}: {e}");
        }
        let e = dispatch(&mut h, "nope", &json!({})).unwrap_err().to_string();
        assert!(e.contains("no such harness method") && e.contains("stage_fixture"), "{e}");
    }

    #[test]
    fn a_passing_scenario_runs_every_step_kind() {
        let steps = r#"[
            {"note": "just a note"},
            {"execute": "view.set", "args": {"zoom": 2}, "expect": [{"path": "$.changed", "eq": true}], "save": {"z": "$.zoom"}},
            {"assert": "view.get", "expect": [{"path": "$.zoom", "eq": "{{z}}"}]},
            {"harness": "advance_clock", "args": {"ms": 5}, "expect": [{"path": "$.now_ms", "gt": 0}]},
            {"harness": "load_generated", "args": {"count": 6}, "expect": [{"path": "$.elements", "eq": 6}]},
            {"execute": "history.undo", "refused": "nothing_to_undo"},
            {"undo": 1, "refused": "nothing_to_undo"},
            {"batch": [{"id": "view.zoom_in"}, {"id": "view.zoom_in"}], "expect": [{"path": "$.completed", "eq": 2}]}
        ]"#;
        assert_eq!(run_text(&wrap(steps)), Ok(Outcome::Passed { steps: 8 }));
    }

    #[test]
    fn a_failing_check_names_the_step_the_check_and_the_output() {
        let e = run_text(&wrap(
            r#"[{"execute": "view.set", "args": {"zoom": 2}, "expect": [{"path": "$.zoom", "eq": 99}]}]"#,
        ))
        .unwrap_err();
        assert!(
            e.contains("step 0 (execute view.set)")
                && e.contains("expect[0]")
                && e.contains("$.zoom")
                && e.contains("output:"),
            "{e}"
        );
    }

    #[test]
    fn success_where_a_refusal_is_expected_and_the_reverse_both_fail() {
        let e = run_text(&wrap(r#"[{"execute": "view.get", "refused": "bad_args"}]"#)).unwrap_err();
        assert!(e.contains("expected a refusal with code `bad_args`"), "{e}");
        let e = run_text(&wrap(r#"[{"execute": "history.undo"}]"#)).unwrap_err();
        assert!(e.contains("expected success") && e.contains("nothing_to_undo"), "{e}");
        let e = run_text(&wrap(r#"[{"execute": "history.undo", "refused": "locked"}]"#)).unwrap_err();
        assert!(e.contains("expected refusal `locked`, got `nothing_to_undo`"), "{e}");
        let e = run_text(&wrap(r#"[{"execute": "history.undo", "refused": "nothing_to_undo", "text": "zzz"}]"#))
            .unwrap_err();
        assert!(e.contains("does not contain `zzz`"), "{e}");
    }

    #[test]
    fn assert_refuses_to_run_a_command_that_changes_state() {
        let e = run_text(&wrap(r#"[{"assert": "view.zoom_in"}]"#)).unwrap_err();
        assert!(e.contains("read-only"), "{e}");
    }

    #[test]
    fn a_harness_error_can_be_expected_by_its_text() {
        let ok = wrap(
            r#"[{"harness": "stage_fixture", "args": {"path": "../x"}, "fails": "not a path inside tests/fixtures"}]"#,
        );
        assert_eq!(run_text(&ok), Ok(Outcome::Passed { steps: 1 }));
        let bad = wrap(r#"[{"harness": "stage_fixture", "args": {"path": "../x"}, "fails": "something else"}]"#);
        assert!(run_text(&bad).is_err());
        let unexpected = wrap(r#"[{"harness": "stage_fixture", "args": {"path": "../x"}}]"#);
        assert!(run_text(&unexpected).unwrap_err().contains("harness call failed"));
    }

    #[test]
    fn a_scenario_that_waits_for_a_command_is_skipped_with_its_name_and_one_that_needs_none_runs() {
        let waiting = r#"{"about": "w", "requires": {"commands": ["element.move_far_away"]}, "steps": [{"execute": "element.move_far_away"}]}"#;
        match run_text(waiting).unwrap() {
            Outcome::Skipped { reason } => assert!(reason.contains("element.move_far_away"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn headless_ports_make_port_commands_refuse() {
        let text = r#"{"about": "h", "ports": "headless", "steps": [{"execute": "view.screenshot", "refused": "port_unavailable"}]}"#;
        assert_eq!(run_text(text), Ok(Outcome::Passed { steps: 1 }));
    }

    #[test]
    fn typos_and_ambiguities_are_errors_when_the_file_is_read() {
        for (text, needle) in [
            (r#"{"steps": [{"note": "x"}]}"#, "`about` is required"),
            (r#"{"about": "a", "steps": []}"#, "empty"),
            (r#"{"about": "a", "steps": [{"execute": "x", "arg": {}}]}"#, "unknown key `arg`"),
            (r#"{"about": "a", "steps": [{"execute": "x", "assert": "y"}]}"#, "one verb"),
            (r#"{"about": "a", "steps": [{"args": {}}]}"#, "unknown key"),
            (r#"{"about": "a", "steps": [{"execute": "x", "text": "t"}]}"#, "needs `refused`"),
            (r#"{"about": "a", "steps": [{"harness": "nope"}]}"#, "no harness method"),
            (r#"{"about": "a", "steps": [{"execute": "x", "expect": [{"path": "$.a"}]}]}"#, "expect[0]"),
            (r#"{"about": "a", "steps": [{"snapshot": "a b", "of_command": "x"}]}"#, "letters, digits"),
            (r#"{"about": "a", "ports": "none", "steps": [{"note": "x"}]}"#, "ports"),
            (r#"{"about": "a", "extra": 1, "steps": [{"note": "x"}]}"#, "unknown key `extra`"),
            (r#"{"about": "a", "steps": [{"undo": 0}]}"#, "step count"),
        ] {
            let e = Scenario::parse(text).unwrap_err();
            assert!(e.contains(needle), "{text}\n  -> {e}\n  (wanted `{needle}`)");
        }
    }

    #[test]
    fn commands_and_harness_calls_are_listed_for_the_accounting_test() {
        let sc = Scenario::parse(&wrap(
            r#"[{"execute": "view.set", "args": {"zoom": 2}}, {"undo": 2, "refused": "nothing_to_undo"},
                {"batch": [{"id": "view.zoom_in"}]}, {"assert": "view.get"},
                {"harness": "advance_clock", "args": {"ms": 1}},
                {"snapshot": "s", "of_harness": "load_generated"}, {"snapshot": "t", "of_command": "engine.state"}]"#,
        ))
        .unwrap();
        let cmds: Vec<String> = sc.commands().into_iter().collect();
        assert_eq!(cmds, ["engine.state", "history.undo", "view.get", "view.set", "view.zoom_in"]);
        let unknown = Scenario::parse(&wrap(r#"[{"execute": "no.such", "refused": "unknown_command"}]"#)).unwrap();
        assert!(unknown.commands().is_empty());
        let calls: Vec<String> = sc.harness_calls().into_iter().collect();
        assert_eq!(calls, ["advance_clock", "load_generated"]);
    }

    #[test]
    fn snapshots_are_compared_written_only_on_request_and_missing_ones_fail() {
        let dir = std::env::temp_dir().join(format!("drafftink-testkit-snap-{}", std::process::id()));
        let file = dir.join("x__s.txt");
        let err = compare_snapshot(&file, "one\ntwo\n", false).unwrap_err();
        assert!(err.contains("DRAFFTINK_UPDATE_SNAPSHOTS=1"), "{err}");
        compare_snapshot(&file, "one\ntwo\n", true).unwrap();
        compare_snapshot(&file, "one\ntwo\n", false).unwrap();
        let err = compare_snapshot(&file, "one\nTWO\n", false).unwrap_err();
        assert!(err.contains("line 2") && err.contains("stored: two") && err.contains("now:    TWO"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
