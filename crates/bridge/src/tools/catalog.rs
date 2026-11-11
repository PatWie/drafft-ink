//! The tool list, generated from the command registry.
//!
//! Decision: **one tool per family**, not one per command. The registry holds well over a hundred agent
//! commands, and MCP clients degrade with that many tools (some cap the total across servers at 40, and
//! every definition is paid for in context on every turn). So each family with state-changing commands
//! gets one tool `{command, args}` whose description lists its commands with their argument signatures.
//! Read-only commands from all families go to a single `inspect` tool, so a client can auto-approve reads
//! (`readOnlyHint`) without approving edits. The rest is a handful of fixed tools: `batch`, `commands`,
//! `describe`, `recent`, `status`, and the file tools `open`, `save`, `export` when `doc.*` provides them.
//!
//! Nothing is maintained by hand: a new command appears in its family's tool as soon as it is registered
//! with `agent: true`, and `tools/list` is snapshot-tested against the registry.

use std::collections::BTreeMap;

use drafftink_engine::spec::Status;
use drafftink_engine::{CommandSpec, Registry};
use serde_json::{Value, json};

use super::signature::signature;

/// The longest worked example that a description repeats.
const MAX_EXAMPLE_CHARS: usize = 110;

/// What a tool does when called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolKind {
    /// Runs one command of a family, or one read-only command of any family.
    Run(Scope),
    /// Runs several commands as one undo step.
    Batch,
    /// Lists commands with live availability.
    Commands,
    /// The schema, example and refusals of one command.
    Describe,
    /// The newest calls with their outcomes.
    Recent,
    /// Which mode the server is in and whether it is connected.
    Status,
    /// A shortcut for a `doc.*` command with a file path.
    File(FileTool),
}

/// Which commands a [`ToolKind::Run`] tool accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// The state-changing commands of one family.
    Family(String),
    /// The read-only commands of every family.
    Reads,
}

/// The file shortcuts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileTool {
    /// `doc.open` with a path.
    Open,
    /// `doc.save` with a path.
    Save,
    /// `doc.export` with a path.
    Export,
}

impl FileTool {
    /// All of them.
    pub const ALL: [FileTool; 3] = [Self::Open, Self::Save, Self::Export];

    /// The tool name.
    pub const fn tool_name(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Save => "save",
            Self::Export => "export",
        }
    }

    /// The command behind it.
    pub const fn command(self) -> &'static str {
        match self {
            Self::Open => "doc.open",
            Self::Save => "doc.save",
            Self::Export => "doc.export",
        }
    }
}

/// The hints of the MCP tool annotations. They are honest, because clients may auto-approve on them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hints {
    /// The tool changes nothing.
    pub read_only: bool,
    /// The tool can delete or overwrite (everything that edits can, though most edits are undoable).
    pub destructive: bool,
    /// The tool touches files, the network or the clipboard.
    pub open_world: bool,
}

/// One tool definition.
#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    /// The name the client calls.
    pub name: String,
    /// A short title for menus.
    pub title: String,
    /// What the model reads.
    pub description: String,
    /// JSON Schema of the arguments.
    pub input_schema: Value,
    /// Annotation hints.
    pub hints: Hints,
    /// What it does.
    pub kind: ToolKind,
}

/// Every tool of one server.
#[derive(Debug, Clone)]
pub struct Catalog {
    tools: Vec<Tool>,
}

/// A command has a tool of its own when its registration says so (`CommandSpec::own_agent_tool`), so no family
/// tool lists it.
fn is_listed(spec: &CommandSpec) -> bool {
    spec.agent && matches!(spec.status, Status::Stable) && !spec.own_agent_tool
}

fn takes_files_or_network(spec: &CommandSpec) -> bool {
    let ports = spec.ports.names();
    ports.contains(&"files") || ports.contains(&"net") || ports.contains(&"clipboard")
}

fn command_line(spec: &CommandSpec) -> String {
    let schema = spec.args_schema().to_value();
    let mut line = format!(
        "- {} - {} Args: {}.",
        spec.id,
        spec.summary.trim_end_matches('.').to_owned() + ".",
        signature(&schema)
    );
    if !spec.example.is_empty() && spec.example.chars().count() <= MAX_EXAMPLE_CHARS {
        line.push_str(&format!(" Example: {}", spec.example));
    }
    line
}

fn run_schema(ids: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": {
            "command": { "type": "string", "enum": ids, "description": "The command to run." },
            "args": { "type": "object", "description": "The command's arguments, as listed in the tool description. Omit for none." }
        },
        "required": ["command"],
        "additionalProperties": false
    })
}

fn run_tool(name: &str, title: &str, intro: &str, specs: &[&CommandSpec], read_only: bool) -> Tool {
    let ids: Vec<&str> = specs.iter().map(|s| s.id).collect();
    let lines: Vec<String> = specs.iter().map(|s| command_line(s)).collect();
    Tool {
        name: name.to_owned(),
        title: title.to_owned(),
        description: format!(
            "{intro}\n{}\nCall `describe` for a command's full schema and refusals.",
            lines.join("\n")
        ),
        input_schema: run_schema(&ids),
        hints: Hints {
            read_only,
            destructive: !read_only,
            open_world: specs.iter().any(|s| takes_files_or_network(s)),
        },
        kind: ToolKind::Run(if read_only {
            Scope::Reads
        } else {
            Scope::Family(specs.first().map_or(String::new(), |s| s.family.to_owned()))
        }),
    }
}

fn fixed(name: &str, title: &str, description: &str, schema: Value, hints: Hints, kind: ToolKind) -> Tool {
    Tool {
        name: name.to_owned(),
        title: title.to_owned(),
        description: description.to_owned(),
        input_schema: schema,
        hints,
        kind,
    }
}

const READ: Hints = Hints { read_only: true, destructive: false, open_world: false };
const EDIT: Hints = Hints { read_only: false, destructive: true, open_world: false };
const FILES: Hints = Hints { read_only: false, destructive: true, open_world: true };

fn object(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false })
}

impl Catalog {
    /// Builds the tool list for a registry.
    pub fn build(reg: &Registry) -> Self {
        let file_commands: Vec<&str> =
            FileTool::ALL.iter().filter(|t| reg.find(t.command()).is_some()).map(|t| t.command()).collect();
        let listed: Vec<&CommandSpec> =
            reg.all().iter().copied().filter(|s| is_listed(s) && !file_commands.contains(&s.id)).collect();

        let mut tools = Vec::new();
        let reads: Vec<&CommandSpec> = listed.iter().copied().filter(|s| s.reads_only).collect();
        if !reads.is_empty() {
            tools.push(run_tool(
                "inspect",
                "Read the document and the editor",
                "Run a read-only command. Nothing here changes the document. Start with `doc.outline` to see what is on the canvas.",
                &reads,
                true,
            ));
        }
        let mut families: BTreeMap<&str, Vec<&CommandSpec>> = BTreeMap::new();
        for spec in listed.iter().copied().filter(|s| !s.reads_only) {
            families.entry(spec.family).or_default().push(spec);
        }
        for (family, specs) in &families {
            let intro = format!("Run a `{family}` command. Give `command` and its `args`. Commands:");
            tools.push(run_tool(family, &format!("Edit: {family}"), &intro, specs, false));
        }

        tools.push(fixed(
            "batch",
            "Run several commands as one undo step",
            "Runs commands in order as ONE atomic, undoable step. Every step is checked before the first runs, and if one refuses everything is rolled back. Name a step with `as` and use its result in later steps: \"$name\" is the id of the first element it created, \"$name.created\" is the list of all created ids (spliced into an array it sits in), and \"$name.<node>\" is the id of a diagram node made by `diagram.create`. Write a literal text that starts with a dollar sign as \"$$text\". Use `dry_run` to try it without keeping anything.",
            object(
                json!({
                    "steps": {
                        "type": "array",
                        "minItems": 1,
                        "items": object(json!({
                            "command": { "type": "string", "description": "The command id, for example `style.set`." },
                            "args": { "type": "object" },
                            "as": { "type": "string", "description": "A name for later steps to refer to as \"$name\"." }
                        }), &["command"])
                    },
                    "label": { "type": "string", "description": "The name of the undo step." },
                    "atomic": { "type": "boolean", "description": "Roll everything back at the first refusal. Default true." },
                    "dry_run": { "type": "boolean", "description": "Run everything, report, then roll everything back." }
                }),
                &["steps"],
            ),
            EDIT,
            ToolKind::Batch,
        ));
        tools.push(fixed(
            "commands",
            "List commands",
            "Lists commands with whether each can run right now (and why not). Filter by `family` or `search` text. Use it when unsure what exists.",
            object(
                json!({
                    "family": { "type": "string" },
                    "search": { "type": "string", "description": "Text to find in the id, title or summary." },
                    "available_only": { "type": "boolean", "description": "Leave out commands that refuse right now." }
                }),
                &[],
            ),
            READ,
            ToolKind::Commands,
        ));
        tools.push(fixed(
            "describe",
            "Describe a command",
            "The full argument schema, output schema, worked example and refusal codes of one command.",
            object(json!({ "command": { "type": "string", "description": "The command id." } }), &["command"]),
            READ,
            ToolKind::Describe,
        ));
        tools.push(fixed(
            "recent",
            "Recent calls",
            "The newest commands run in this editor, by anyone (you, the user, a collaborator), with their outcomes. Use it to see what the user changed since your last call, or after a timeout.",
            object(
                json!({
                    "limit": { "type": "integer", "minimum": 1, "maximum": 256, "description": "How many entries. Default 50." },
                    "mutating_only": { "type": "boolean" },
                    "since_seq": { "type": "integer", "description": "Only entries after this sequence number." }
                }),
                &[],
            ),
            READ,
            ToolKind::Recent,
        ));
        tools.push(fixed(
            "status",
            "Server status",
            "Which mode this server is in (headless, attached to the desktop app, or relayed to the browser tab), whether the editor is reachable, and how to connect if it is not.",
            object(json!({}), &[]),
            READ,
            ToolKind::Status,
        ));
        for ft in FileTool::ALL.into_iter().filter(|t| file_commands.contains(&t.command())) {
            tools.push(file_tool(ft));
        }
        Self { tools }
    }

    /// The tools, in the order of `tools/list`.
    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    /// A tool by name.
    pub fn find(&self, name: &str) -> Option<&Tool> {
        self.tools.iter().find(|t| t.name == name)
    }
}

fn file_tool(ft: FileTool) -> Tool {
    match ft {
        FileTool::Open => fixed(
            "open",
            "Open a document from a file",
            "Opens a drafft.ink, Excalidraw or PNG-with-scene file by path (a shortcut for `doc.open`). Replaces the current document unless `mode` says otherwise. The path must be one the server may read.",
            object(
                json!({
                    "path": { "type": "string", "description": "An absolute path, or one relative to the server's working folder." },
                    "mode": { "type": "string", "enum": ["replace", "new_tab", "merge"], "description": "Default replace." },
                    "on_unsaved": { "type": "string", "enum": ["refuse", "discard", "save"], "description": "What to do with unsaved changes when replacing. Default refuse." }
                }),
                &["path"],
            ),
            FILES,
            ToolKind::File(ft),
        ),
        FileTool::Save => fixed(
            "save",
            "Save the document to a file",
            "Saves the document (a shortcut for `doc.save`). Without `path` it saves to where the document was last saved or opened.",
            object(
                json!({
                    "path": { "type": "string" },
                    "format": { "type": "string", "enum": ["drafftink", "excalidraw"], "description": "Default drafftink." },
                    "overwrite": { "type": "boolean", "description": "Replace an existing file that is not this document's own. Default false." }
                }),
                &[],
            ),
            FILES,
            ToolKind::File(ft),
        ),
        FileTool::Export => fixed(
            "export",
            "Export the document or a part of it",
            "Exports to a file (a shortcut for `doc.export`). `svg` and `json` always work. `png` needs a renderer: attached to the desktop app or relayed to the browser it works; headless it refuses and says to use `svg`. Without `path` the result comes back inline (a PNG as an image).",
            object(
                json!({
                    "path": { "type": "string", "description": "Where to write. Omit to get the result inline." },
                    "format": { "type": "string", "enum": ["png", "svg", "json", "excalidraw"], "description": "Default png." },
                    "scope": { "type": "string", "enum": ["auto", "all", "selection", "ids", "frame"] },
                    "ids": { "type": "array", "items": { "type": "string" }, "description": "Element ids or @names, for scope ids." },
                    "scale": { "type": "number" },
                    "background": { "type": "boolean" }
                }),
                &[],
            ),
            FILES,
            ToolKind::File(ft),
        ),
    }
}
