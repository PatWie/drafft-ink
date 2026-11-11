//! The things the command line does with a session. Each one is a few commands of the registry; no rule of the
//! editor is repeated here.

use std::io::Write;
use std::path::Path;

use drafftink_engine::{BatchStep, Origin, Session};
use serde_json::{Value, json};

use crate::error::CliError;
use crate::format::{InFormat, OutFormat, Source};

/// A path as a JSON string. Paths that are not valid UTF-8 are shown with replacement characters; the files
/// port takes a string.
fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn command(s: &mut Session, id: &str, args: Value) -> Result<Value, CliError> {
    Ok(s.execute_from(Origin::Script, id, args)?)
}

/// The `warnings` of a command's output.
fn warnings_of(out: &Value) -> Vec<String> {
    out.get("warnings")
        .and_then(Value::as_array)
        .map(|w| w.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default()
}

/// Reads `path` into the session, replacing the document. Returns the notes about what could not be kept.
pub fn open(s: &mut Session, path: &Path, from: InFormat) -> Result<Vec<String>, CliError> {
    let out = match from.resolve(path) {
        Source::Document(format) => command(
            s,
            "doc.open",
            json!({ "source": { "path": path_arg(path) }, "format": format, "mode": "replace" }),
        )?,
        Source::Mermaid => {
            let text =
                std::fs::read_to_string(path).map_err(|source| CliError::Read { path: path.to_owned(), source })?;
            command(s, "diagram.import_mermaid", json!({ "source": { "text": text }, "at": [0, 0], "select": false }))?
        }
    };
    Ok(warnings_of(&out))
}

/// Writes the document of the session to `path`. Returns the notes about what could not be kept.
pub fn save(s: &mut Session, path: &Path, to: OutFormat) -> Result<Vec<String>, CliError> {
    let target = json!({ "path": path_arg(path) });
    let out = match to {
        OutFormat::Native => command(
            s,
            "doc.save",
            json!({ "to": target, "format": "drafftink", "remember": false, "overwrite": true }),
        )?,
        OutFormat::Excalidraw => command(
            s,
            "doc.save",
            json!({ "to": target, "format": "excalidraw", "remember": false, "overwrite": true }),
        )?,
        OutFormat::Svg => command(s, "doc.export", json!({ "format": "svg", "scope": "all", "to": target }))?,
        OutFormat::Png => return Err(CliError::NeedsGpu),
    };
    Ok(warnings_of(&out))
}

fn write_line(out: &mut dyn Write, v: &Value) -> Result<(), CliError> {
    serde_json::to_writer(&mut *out, v)?;
    out.write_all(b"\n")?;
    Ok(())
}

/// Runs the steps one after the other, writing one JSON line per step: `{"step", "id", "ok", "result"}` or
/// `{"step", "id", "ok": false, "refusal"}`. It stops at the first refusal unless `keep_going`.
pub fn run_steps(s: &mut Session, steps: &[BatchStep], keep_going: bool, out: &mut dyn Write) -> Result<(), CliError> {
    let mut refused = 0usize;
    for (i, step) in steps.iter().enumerate() {
        let n = i + 1;
        if step.as_name.is_some() {
            return Err(CliError::Format(format!(
                "step {n} is named with `as`, which only a batch understands. Run with --batch so that later steps can use `$name`."
            )));
        }
        let args = if step.args.is_null() { json!({}) } else { step.args.clone() };
        match s.execute_from(Origin::Script, &step.id, args) {
            Ok(result) => write_line(out, &json!({ "step": n, "id": step.id, "ok": true, "result": result }))?,
            Err(r) => {
                write_line(
                    out,
                    &json!({ "step": n, "id": step.id, "ok": false, "refusal": serde_json::to_value(&r)? }),
                )?;
                refused += 1;
                if !keep_going {
                    return Err(CliError::StepRefused { step: n, message: r.message });
                }
            }
        }
    }
    if refused == 0 { Ok(()) } else { Err(CliError::StepsRefused(refused)) }
}

/// Runs a batch object as one transaction and writes its output as one JSON line. A batch that stopped at a
/// refusing step is an error (and was rolled back unless `opts.atomic` says otherwise).
pub fn run_batch(s: &mut Session, batch: &Value, out: &mut dyn Write) -> Result<(), CliError> {
    let result = s.execute_batch_json(Origin::Script, batch)?;
    write_line(out, &result)?;
    match result.get("failed").filter(|f| !f.is_null()) {
        None => Ok(()),
        Some(f) => Err(CliError::StepRefused {
            step: f.get("index").and_then(Value::as_u64).map_or(0, |i| usize::try_from(i).unwrap_or(0) + 1),
            message: f.pointer("/refusal/message").and_then(Value::as_str).unwrap_or("a step refused").to_owned(),
        }),
    }
}

/// The text outline of the document (`doc.outline`), or its JSON form.
pub fn outline(s: &mut Session, json_form: bool, max_nodes: u32) -> Result<String, CliError> {
    let format = if json_form { "json" } else { "text" };
    let out = command(s, "doc.outline", json!({ "format": format, "max_nodes": max_nodes }))?;
    if json_form {
        return Ok(serde_json::to_string_pretty(out.get("outline").unwrap_or(&Value::Null))?);
    }
    Ok(out.get("text").and_then(Value::as_str).unwrap_or_default().to_owned())
}

/// The summary of the document (`doc.inspect`) as pretty JSON.
pub fn inspect(s: &mut Session) -> Result<String, CliError> {
    let out = command(s, "doc.inspect", json!({}))?;
    Ok(serde_json::to_string_pretty(&out)?)
}

/// The commands of the registry, as lines `id  title` grouped by family, or as JSON with the schemas.
pub fn commands(
    s: &mut Session,
    family: Option<&str>,
    search: Option<&str>,
    json_form: bool,
) -> Result<String, CliError> {
    let out =
        command(s, "engine.commands", json!({ "family": family, "search": search, "include_schemas": json_form }))?;
    if json_form {
        return Ok(serde_json::to_string_pretty(&out)?);
    }
    let list = out.get("commands").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let width = list.iter().filter_map(|c| c.get("id").and_then(Value::as_str)).map(str::len).max().unwrap_or(0);
    let mut text = String::new();
    for c in list {
        let field = |k: &str| c.get(k).and_then(Value::as_str).unwrap_or_default();
        let note = if c.get("reads_only").and_then(Value::as_bool) == Some(true) { "  (read-only)" } else { "" };
        text.push_str(&format!("{:<width$}  {}{note}\n", field("id"), field("title")));
    }
    Ok(text)
}

/// One command with its schemas, facts, refusals and example (`engine.describe`), as pretty JSON.
pub fn describe(s: &mut Session, id: &str) -> Result<String, CliError> {
    let out = command(s, "engine.describe", json!({ "id": id }))?;
    Ok(serde_json::to_string_pretty(&out)?)
}
