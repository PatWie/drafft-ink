//! The MCP service without the MCP library: tool list, tool calls, prompts. [`crate::server`] puts it on the
//! wire. Keeping them apart makes the logic testable with plain values.

use std::sync::Arc;

use drafftink_engine::Registry;
use serde_json::{Map, Value, json};

use crate::backend::Backend;
use crate::catalog::{Catalog, FileTool, Tool, ToolKind};
use crate::guide;
use crate::outcome::{self, Outcome, error_text};

/// A call named a tool that does not exist. This is a protocol error, not a tool error.
#[derive(Debug, thiserror::Error)]
#[error("unknown tool `{name}`. Tools: {known}")]
pub struct UnknownTool {
    /// The name that was asked for.
    pub name: String,
    /// The real names.
    pub known: String,
}

/// The tools and prompts of one server, and the backend they run on.
pub struct Service {
    backend: Backend,
    catalog: Catalog,
    registry: Arc<Registry>,
}

type Args = Map<String, Value>;

impl Service {
    /// A service over `backend`, with tools generated from the global registry.
    pub fn new(backend: Backend) -> Self {
        let registry = Registry::global();
        let catalog = Catalog::build(&registry);
        Self { backend, catalog, registry }
    }

    /// The tool definitions.
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// The backend.
    pub fn backend(&self) -> &Backend {
        &self.backend
    }

    /// The text for the `initialize` result: what the server is and where to learn more.
    pub fn instructions(&self) -> String {
        guide::instructions(&self.registry, self.backend.mode())
    }

    /// The full guide, for `prompts/get`.
    pub fn guide(&self) -> String {
        guide::guide(&self.registry, self.backend.mode())
    }

    /// Runs a tool call.
    pub async fn call(&self, name: &str, args: Args) -> Result<Outcome, UnknownTool> {
        let Some(tool) = self.catalog.find(name) else {
            let known = self.catalog.tools().iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", ");
            return Err(UnknownTool { name: name.to_owned(), known });
        };
        if let Err(problem) = check_keys(tool, &args) {
            return Ok(Outcome::error(problem));
        }
        Ok(match &tool.kind {
            ToolKind::Run(_) => self.run(tool, &args).await,
            ToolKind::Batch => self.batch(&args).await,
            ToolKind::Commands => self.list_commands(&args).await,
            ToolKind::Describe => self.describe(&args).await,
            ToolKind::Recent => self.passthrough("recent", Value::Object(args)).await,
            ToolKind::Status => Outcome::text(self.backend.status().await.to_string()),
            ToolKind::File(ft) => self.file(*ft, &args).await,
        })
    }

    async fn passthrough(&self, method: &str, params: Value) -> Outcome {
        match self.backend.call(method, params).await {
            Ok(v) => outcome::success(v),
            Err(e) => Outcome::error(error_text(&e)),
        }
    }

    async fn execute(&self, id: &str, args: Value) -> Outcome {
        self.passthrough("execute", json!({ "id": id, "args": args })).await
    }

    fn tool_listing(&self, id: &str) -> Option<&Tool> {
        self.catalog.tools().iter().find(|t| matches!(t.kind, ToolKind::Run(_)) && listed_ids(t).contains(&id))
    }

    async fn run(&self, tool: &Tool, args: &Args) -> Outcome {
        let Some(command) = args.get("command").and_then(Value::as_str) else {
            return Outcome::error(format!(
                "`command` is required. Use one of the commands listed in the `{}` tool description.",
                tool.name
            ));
        };
        let command_args = match args.get("args") {
            None | Some(Value::Null) => json!({}),
            Some(v @ Value::Object(_)) => v.clone(),
            Some(_) => {
                return Outcome::error(
                    "`args` must be an object, for example {\"x\": 1}. Omit it for a command without arguments.",
                );
            }
        };
        if !listed_ids(tool).contains(&command) {
            if let Some(spec) = self.registry.find(command) {
                if let Some(other) = self.tool_listing(spec.id) {
                    return Outcome::error(format!(
                        "`{}` is called with the `{}` tool, not `{}`.",
                        spec.id, other.name, tool.name
                    ));
                }
                if let Some(ft) = FileTool::ALL.iter().find(|f| f.command() == spec.id) {
                    return Outcome::error(format!(
                        "`{}` is called with the `{}` tool, which takes a `path`.",
                        spec.id,
                        ft.tool_name()
                    ));
                }
            } else {
                return Outcome::error(self.registry.unknown(command).message);
            }
        }
        self.execute(command, command_args).await
    }

    async fn batch(&self, args: &Args) -> Outcome {
        let Some(steps) = args.get("steps").and_then(Value::as_array) else {
            return Outcome::error("`steps` is required: a list of {command, args?, as?}.");
        };
        let mut out = Vec::with_capacity(steps.len());
        for (i, step) in steps.iter().enumerate() {
            let Some(obj) = step.as_object() else {
                return Outcome::error(format!(
                    "Step {i} must be an object like {{\"command\": \"style.set\", \"args\": {{}}}}."
                ));
            };
            if let Some(extra) = obj.keys().find(|k| !matches!(k.as_str(), "command" | "args" | "as")) {
                return Outcome::error(format!(
                    "Step {i}: unknown key `{extra}`. A step has `command`, `args` and `as`."
                ));
            }
            let Some(command) = obj.get("command").and_then(Value::as_str) else {
                return Outcome::error(format!("Step {i}: `command` is required."));
            };
            let mut step_json = json!({ "id": command, "args": obj.get("args").cloned().unwrap_or_else(|| json!({})) });
            if let (Some(name), Some(m)) = (obj.get("as"), step_json.as_object_mut()) {
                m.insert("as".to_owned(), name.clone());
            }
            out.push(step_json);
        }
        let mut opts = Map::new();
        for (from, to) in [("label", "label"), ("atomic", "atomic"), ("dry_run", "dry_run")] {
            if let Some(v) = args.get(from) {
                opts.insert(to.to_owned(), v.clone());
            }
        }
        match self.backend.call("batch", json!({ "steps": out, "opts": opts })).await {
            Ok(v) => outcome::batch(v),
            Err(e) => Outcome::error(error_text(&e)),
        }
    }

    async fn list_commands(&self, args: &Args) -> Outcome {
        let mut params = args.clone();
        params.remove("include_schemas");
        let listing = match self.backend.call("commands", Value::Object(params)).await {
            Ok(v) => v,
            Err(e) => return Outcome::error(error_text(&e)),
        };
        let rows: Vec<Value> = listing
            .get("commands")
            .and_then(Value::as_array)
            .map(|all| {
                all.iter()
                    .filter(|c| c.pointer("/facts/agent") == Some(&Value::Bool(true)))
                    .map(|c| {
                        let available = match c.get("available") {
                            Some(Value::Bool(true)) => json!(true),
                            Some(a) => a.get("message").cloned().unwrap_or(json!(false)),
                            None => json!(false),
                        };
                        json!({ "id": c.get("id"), "summary": c.get("summary"), "available": available })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Outcome::text(json!({ "count": rows.len(), "commands": rows }).to_string())
    }

    async fn describe(&self, args: &Args) -> Outcome {
        let Some(id) = args.get("command").and_then(Value::as_str) else {
            return Outcome::error("`command` is required: the id of the command to describe, for example `view.pan`.");
        };
        self.passthrough("describe", json!({ "id": id })).await
    }

    async fn file(&self, ft: FileTool, args: &Args) -> Outcome {
        let mut out = Map::new();
        let path = args.get("path").cloned();
        for (key, value) in args {
            if key != "path" {
                out.insert(key.clone(), value.clone());
            }
        }
        match ft {
            FileTool::Open => {
                let Some(p) = path else { return Outcome::error("`path` is required: the file to open.") };
                out.insert("source".to_owned(), json!({ "path": p }));
            }
            FileTool::Save => {
                if let Some(p) = path {
                    out.insert("to".to_owned(), json!({ "path": p }));
                }
            }
            FileTool::Export => {
                out.insert("to".to_owned(), path.map_or_else(|| json!("inline"), |p| json!({ "path": p })));
            }
        }
        self.execute(ft.command(), Value::Object(out)).await
    }
}

/// The command ids a `Run` tool accepts, read from its schema.
fn listed_ids(tool: &Tool) -> Vec<&str> {
    tool.input_schema
        .pointer("/properties/command/enum")
        .and_then(Value::as_array)
        .map(|ids| ids.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// Rejects an argument the tool does not have, naming the ones it has (so a model corrects itself).
fn check_keys(tool: &Tool, args: &Args) -> Result<(), String> {
    let known: Vec<&str> = tool
        .input_schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|p| p.keys().map(String::as_str).collect())
        .unwrap_or_default();
    match args.keys().find(|k| !known.contains(&k.as_str())) {
        None => Ok(()),
        Some(bad) if known.is_empty() => Err(format!("`{}` takes no arguments, and `{bad}` was given.", tool.name)),
        Some(bad) => {
            Err(format!("`{}`: unknown argument `{bad}`. Its arguments are: {}.", tool.name, known.join(", ")))
        }
    }
}
