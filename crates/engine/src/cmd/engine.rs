//! `engine`: introspection of the registry and the session (docs/commands.md §2.21).
//!
//! `window.drafftink.commands()`, `recent()` and the MCP tool list are built from these. All four are
//! read-only and allowed in every state.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::refusal::{Refusal, SHARED_CODES, echo};
use crate::ring::RingEntry;
use crate::schemas::SchemaState;
use crate::session::Session;
use crate::spec::{Batch, CommandSpec, Facts, Gate, NoArgs, Selection, Status, TextGate, Typed};

/// Arguments of `engine.commands`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct CommandsArgs {
    /// Only this family.
    pub family: Option<String>,
    /// Case-insensitive text to find in the id, title or summary.
    pub search: Option<String>,
    /// Leave out commands that refuse right now.
    pub available_only: bool,
    /// Include the argument and output JSON Schemas.
    pub include_schemas: bool,
}

/// Arguments of `engine.describe`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DescribeArgs {
    /// The command id.
    pub id: String,
}

/// Arguments of `engine.recent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct RecentArgs {
    /// How many entries, 1 to 256.
    pub limit: i64,
    /// Leave out read-only commands.
    pub mutating_only: bool,
    /// Only entries after this sequence number.
    pub since_seq: Option<u64>,
}

impl Default for RecentArgs {
    fn default() -> Self {
        Self { limit: 50, mutating_only: false, since_seq: None }
    }
}

/// Output of `engine.recent`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct RecentOut {
    /// The newest entries, oldest first.
    pub entries: Vec<RingEntry>,
    /// The sequence number the next entry will get.
    pub next_seq: u64,
    /// The ring's capacity.
    pub capacity: usize,
}

/// The facts of a spec as JSON.
pub fn facts_json(s: &CommandSpec) -> Value {
    let gate = |g: Gate| if g == Gate::Allowed { "ok" } else { "refused" };
    json!({
        "undo": s.undo.name(),
        "coalesce": s.coalesce.map(|c| json!({ "key": c.key, "ms": c.ms })),
        "reads_only": s.reads_only,
        "mutates": s.mutates.names(),
        "needs_selection": match s.needs_selection { Selection::No => "no", Selection::Soft => "soft", Selection::Required => "required" },
        "ports": s.ports.names(),
        "batch": match s.batch { Batch::Allowed => "allowed", Batch::Last => "last", Batch::Never => "never" },
        "view_only": gate(s.view_only),
        "while_gesture": gate(s.while_gesture),
        "while_text_edit": match s.while_text_edit { TextGate::Allowed => "ok", TextGate::Refused => "refused", TextGate::Commit => "commit" },
        "redraw": format!("{:?}", s.redraw).to_lowercase(),
        "ring": format!("{:?}", s.ring).to_lowercase(),
        "palette": s.palette,
        "agent": s.agent,
        "status": match s.status { Status::Stable => json!("stable"), Status::Planned(r) => json!({ "planned": r }) },
    })
}

fn shortcuts_json(s: &CommandSpec) -> Value {
    let list = |mac: bool| -> Vec<Value> {
        s.shortcuts
            .iter()
            .flat_map(|b| {
                b.chords(mac).iter().map(move |k| {
                    let keys = if mac { k.replace("Mod", "Cmd") } else { k.replace("Mod", "Ctrl") };
                    let args: Value = if b.args.is_empty() { Value::Null } else { serde_json::from_str(b.args).unwrap_or(Value::Null) };
                    json!({ "keys": keys, "args": args, "when": b.when, "title": b.title })
                })
            })
            .collect()
    };
    json!({ "mac": list(true), "other": list(false) })
}

fn availability_json(a: &Result<(), Refusal>) -> Value {
    match a {
        Ok(()) => Value::Bool(true),
        Err(r) => json!({ "code": r.code, "message": r.message }),
    }
}

fn info_json(s: &Session, spec: &'static CommandSpec, schemas: bool) -> Value {
    let mut v = json!({
        "id": spec.id,
        "title": spec.title,
        "family": spec.family,
        "summary": spec.summary,
        "undo": spec.undo.name(),
        "reads_only": spec.reads_only,
        "shortcuts": shortcuts_json(spec),
        "available": availability_json(&s.availability_of(spec)),
        "facts": facts_json(spec),
    });
    if schemas && let Value::Object(m) = &mut v {
        m.insert("args_schema".to_owned(), spec.args_schema().to_value());
        m.insert("output_schema".to_owned(), output_schema_with_common(spec));
    }
    v
}

/// The output schema of a command with the common keys of §1.6 added to its properties.
fn output_schema_with_common(spec: &CommandSpec) -> Value {
    let mut schema = spec.output_schema().to_value();
    let common = json!({
        "changed": { "type": "boolean", "description": "True when the command changed any state." },
        "undo_label": { "type": "string", "description": "The label of the undo step the command pushed." },
        "created": { "type": "array", "items": { "type": "string" }, "description": "Ids of elements the command made." },
        "deleted": { "type": "array", "items": { "type": "string" }, "description": "Ids of elements the command removed." },
        "warnings": { "type": "array", "items": { "type": "string" }, "description": "Non-fatal notes." },
    });
    if let Value::Object(m) = &mut schema {
        let props = m.entry("properties").or_insert_with(|| json!({}));
        if let (Value::Object(p), Value::Object(c)) = (props, common) {
            for (k, v) in c {
                p.entry(k).or_insert(v);
            }
        }
    }
    schema
}

/// The refusal codes a command can return: the ones implied by its facts, `bad_args`, `internal`, and
/// its own `refusals`.
pub fn refusal_codes(spec: &CommandSpec) -> Vec<&'static str> {
    let mut codes: Vec<&'static str> = vec!["bad_args", "internal"];
    if spec.view_only == Gate::Refused {
        codes.push("view_only");
    }
    if spec.while_gesture == Gate::Refused {
        codes.push("gesture_active");
    }
    if spec.while_text_edit == TextGate::Refused {
        codes.push("text_editing");
    }
    if spec.needs_selection == Selection::Required {
        codes.push("no_target");
        codes.push("not_found");
    }
    if !spec.ports.is_empty() {
        codes.push("port_unavailable");
    }
    if matches!(spec.status, Status::Planned(_)) {
        codes.push("not_supported");
    }
    for c in spec.refusals {
        if !codes.contains(c) {
            codes.push(c);
        }
    }
    codes
}

/// Refuses while the schemas are not there: a build without them (the web) installs them from a file, and a
/// reader that asked for schemas must not be given empty ones in place of the real ones.
fn schemas_ready(id: &str) -> Result<(), Refusal> {
    match crate::schemas::state() {
        SchemaState::Ready => Ok(()),
        SchemaState::Loading => Err(Refusal::custom(
            "schemas_loading",
            id,
            "The argument schemas of the commands are still loading; the page fetches them when it starts. Try again in a moment.",
        )),
        SchemaState::Failed(why) => Err(Refusal::custom(
            "schemas_unavailable",
            id,
            format!("The argument schemas of the commands could not be loaded ({why}). Reload the page. `engine.commands` without `include_schemas` still works."),
        )),
    }
}

fn commands(s: &mut Session, a: CommandsArgs) -> Result<Value, Refusal> {
    if a.include_schemas {
        schemas_ready(s.cmd())?;
    }
    let needle = a.search.as_deref().map(str::to_lowercase);
    if let Some(f) = &a.family
        && !s.registry().families().contains(&f.as_str())
    {
        let fams = s.registry().families().join(", ");
        return Err(Refusal::bad_arg(s.cmd(), "family", format!("no family `{}`. Families: {fams}", echo(f))));
    }
    let mut out = Vec::new();
    for spec in s.registry().all().iter().copied() {
        if a.family.as_deref().is_some_and(|f| f != spec.family) {
            continue;
        }
        if let Some(n) = &needle
            && !(spec.id.contains(n.as_str())
                || spec.title.to_lowercase().contains(n.as_str())
                || spec.summary.to_lowercase().contains(n.as_str()))
        {
            continue;
        }
        let info = info_json(s, spec, a.include_schemas);
        if a.available_only && info.get("available") != Some(&Value::Bool(true)) {
            continue;
        }
        out.push(info);
    }
    Ok(json!({ "commands": out, "count": out.len(), "changed": false }))
}

fn describe(s: &mut Session, a: DescribeArgs) -> Result<Value, Refusal> {
    schemas_ready(s.cmd())?;
    let Some(spec) = s.registry().find(&a.id) else {
        return Err(s.registry().unknown(&a.id).raised_by(s.cmd()));
    };
    let mut v = info_json(s, spec, true);
    let example: Value = if spec.example.is_empty() { json!({}) } else { serde_json::from_str(spec.example).unwrap_or(json!({})) };
    let codes = refusal_codes(spec);
    if let Value::Object(m) = &mut v {
        m.insert("example".to_owned(), json!({ "id": spec.id, "args": example }));
        m.insert("gesture".to_owned(), Value::Bool(spec.is_gesture()));
        m.insert("changed".to_owned(), Value::Bool(false));
        m.insert(
            "refusals".to_owned(),
            json!(codes
                .iter()
                .map(|c| match SHARED_CODES.iter().find(|s| s.code == *c) {
                    Some(info) => json!({ "code": c, "message": info.template }),
                    None => json!({ "code": c }),
                })
                .collect::<Vec<_>>()),
        );
    }
    Ok(v)
}

fn recent(s: &mut Session, a: RecentArgs) -> Result<RecentOut, Refusal> {
    let limit = usize::try_from(a.limit)
        .ok()
        .filter(|l| (1..=256).contains(l))
        .ok_or_else(|| Refusal::bad_arg(s.cmd(), "limit", "must be a whole number from 1 to 256"))?;
    Ok(RecentOut {
        entries: s.ring().recent(limit, a.mutating_only, a.since_seq),
        next_seq: s.ring().next_seq(),
        capacity: s.ring().capacity(),
    })
}

fn state(s: &mut Session, _: NoArgs) -> Result<Value, Refusal> {
    let mut v = s.state_json();
    if let Value::Object(m) = &mut v {
        m.insert("changed".to_owned(), Value::Bool(false));
    }
    Ok(v)
}

/// The engine family.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "engine.commands",
        "engine",
        "List commands",
        "Lists every command with its title, summary, shortcuts and whether it can run right now, with the reason if it cannot.",
        &Typed::new(commands),
        Facts::READ_ONLY,
    )
    .own_agent_tool()
    .refusals(&["schemas_loading", "schemas_unavailable"])
    .example(r#"{"family":"view","available_only":true}"#),
    CommandSpec::new(
        "engine.describe",
        "engine",
        "Describe command",
        "Describes one command: summary, argument and output JSON Schemas, facts, the refusal codes it can return and an example.",
        &Typed::new(describe),
        Facts::READ_ONLY,
    )
    .own_agent_tool()
    .refusals(&["unknown_command", "schemas_loading", "schemas_unavailable"])
    .example(r#"{"id":"view.pan"}"#),
    CommandSpec::new(
        "engine.recent",
        "engine",
        "Recent commands",
        "Returns the newest entries of the recent-command ring, oldest first, with arguments, outcome and duration.",
        &Typed::new(recent),
        Facts::READ_ONLY,
    )
    .own_agent_tool()
    .example(r#"{"limit":10,"mutating_only":true}"#),
    CommandSpec::new(
        "engine.state",
        "engine",
        "Session state",
        "Returns tabs, tool, view-only mode, selection summary, running gesture, text edit, revision and settings revision.",
        &Typed::new(state),
        Facts::READ_ONLY,
    ),
];

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
