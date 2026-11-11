//! The pure rules of WebMCP (docs/agents.md, section 6): which tools the page registers with the browser, in
//! the shape the API takes, and how a tool result is returned. Nothing here touches the browser, so all of it
//! is unit-tested natively. The browser glue is in `webmcp_page`.
//!
//! WebMCP is the emerging browser API that lets a page register tools for the browser's own agents
//! (`document.modelContext.registerTool`, W3C Web Machine Learning Community Group draft of November 2026; the
//! first previews had it on `navigator.modelContext`). The tools are the ones every other door serves, generated
//! from the command registry by [`drafftink_bridge::tools`], so a model sees the same tools through MCP over
//! stdio, over HTTP, and here.

use drafftink_bridge::tools::catalog::{Scope, Tool, ToolKind};
use drafftink_bridge::tools::outcome::{Block, Outcome};
use serde_json::{Value, json};

/// The address query that turns the registration off: `?webmcp=off` (also `0` and `false`).
const QUERY_KEY: &str = "webmcp";

/// True unless the page address asks for WebMCP to be off.
pub fn wanted(search: &str) -> bool {
    let value = search.trim_start_matches('?').split('&').find_map(|p| p.strip_prefix("webmcp="));
    !matches!(value, Some("off" | "0" | "false" | "no"))
}

/// The name of the query key, for messages.
pub const fn query_key() -> &'static str {
    QUERY_KEY
}

/// True when a tool's result carries text from the document (labels, names, links), which can hold words meant
/// for the model. The browser and the agent treat such a result with care (`untrustedContentHint`).
fn returns_document_text(tool: &Tool) -> bool {
    matches!(tool.kind, ToolKind::Run(Scope::Reads) | ToolKind::Recent)
}

/// A tool as the `registerTool` dictionary, without `execute`, which the glue adds.
///
/// `consequentialHint` stays false for every tool: an edit is one undo step, and a file tool writes through the
/// browser's download, so nothing is irreversible without the user.
pub fn descriptor(tool: &Tool) -> Value {
    json!({
        "name": tool.name,
        "title": tool.title,
        "description": tool.description,
        "inputSchema": tool.input_schema,
        "annotations": {
            "readOnlyHint": tool.hints.read_only,
            "untrustedContentHint": returns_document_text(tool),
            "consequentialHint": false,
        },
    })
}

/// A tool result in the MCP shape, `{content: [...], isError}`. WebMCP serialises whatever a tool returns, and
/// this shape keeps a picture a picture and a refusal a refusal for an agent that already speaks MCP.
pub fn result(outcome: &Outcome) -> Value {
    let content: Vec<Value> = outcome
        .blocks
        .iter()
        .map(|b| match b {
            Block::Text(text) => json!({ "type": "text", "text": text }),
            Block::Image { data, mime } => json!({ "type": "image", "data": data, "mimeType": mime }),
        })
        .collect();
    json!({ "content": content, "isError": outcome.is_error })
}

/// The arguments of a call as the object the service takes. No argument is `{}`; anything else than an object is
/// an error in words the model can act on.
pub fn arguments(input: Value) -> Result<serde_json::Map<String, Value>, String> {
    match input {
        Value::Null => Ok(serde_json::Map::new()),
        Value::Object(map) => Ok(map),
        other => Err(format!("The arguments must be an object like {{\"command\": \"doc.outline\"}}, and got {other}.")),
    }
}

#[cfg(test)]
mod tests {
    use drafftink_bridge::tools::catalog::Catalog;
    use drafftink_engine::Registry;

    use super::*;

    fn catalog() -> Catalog {
        Catalog::build(&Registry::global())
    }

    #[test]
    fn the_registration_is_on_unless_the_address_turns_it_off() {
        assert!(wanted(""));
        assert!(wanted("?room=r1&perf=1"));
        assert!(wanted("?webmcp=on"));
        for off in ["?webmcp=off", "?x=1&webmcp=0", "webmcp=false", "?webmcp=no&y=2"] {
            assert!(!wanted(off), "{off}");
        }
        assert!(wanted("?notwebmcp=off"), "only the key `{}` counts", query_key());
    }

    #[test]
    fn every_generated_tool_is_a_valid_webmcp_tool() {
        let catalog = catalog();
        assert!(catalog.tools().len() >= 10, "one tool per family plus the fixed ones");
        for tool in catalog.tools() {
            let d = descriptor(tool);
            let name = d["name"].as_str().unwrap();
            // The draft: 1 to 128 characters of letters, digits, `_`, `-` and `.`.
            assert!((1..=128).contains(&name.len()), "{name}");
            assert!(name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')), "{name}");
            assert!(!d["description"].as_str().unwrap().is_empty(), "{name}");
            assert_eq!(d["inputSchema"]["type"], "object", "{name}");
            assert!(d["annotations"]["readOnlyHint"].is_boolean(), "{name}");
        }
    }

    #[test]
    fn the_page_offers_the_same_tools_as_the_other_doors() {
        let names: Vec<String> = catalog().tools().iter().map(|t| t.name.clone()).collect();
        for want in ["inspect", "element", "style", "diagram", "arrange", "batch", "commands", "describe", "recent"] {
            assert!(names.iter().any(|n| n == want), "{want} is missing from {names:?}");
        }
    }

    #[test]
    fn hints_tell_reads_from_edits_and_mark_document_text() {
        let catalog = catalog();
        let hints = |name: &str| descriptor(catalog.find(name).unwrap())["annotations"].clone();
        assert_eq!(hints("inspect")["readOnlyHint"], true);
        assert_eq!(hints("inspect")["untrustedContentHint"], true);
        assert_eq!(hints("recent")["untrustedContentHint"], true);
        assert_eq!(hints("diagram")["readOnlyHint"], false);
        assert_eq!(hints("diagram")["untrustedContentHint"], false);
        assert_eq!(hints("describe")["readOnlyHint"], true);
        assert_eq!(hints("diagram")["consequentialHint"], false);
    }

    #[test]
    fn a_result_keeps_pictures_and_errors_apart_from_text() {
        let ok = result(&Outcome::text("{\"changed\":true}"));
        assert_eq!(ok, json!({ "content": [{ "type": "text", "text": "{\"changed\":true}" }], "isError": false }));
        let refused = result(&Outcome::error("Nothing to fit. Try `view.center`."));
        assert_eq!(refused["isError"], true);
        let picture = Outcome {
            blocks: vec![Block::Text("{}".into()), Block::Image { data: "AAAA".into(), mime: "image/png".into() }],
            is_error: false,
        };
        assert_eq!(result(&picture)["content"][1], json!({ "type": "image", "data": "AAAA", "mimeType": "image/png" }));
    }

    #[test]
    fn arguments_must_be_an_object_or_nothing() {
        assert_eq!(arguments(Value::Null).unwrap().len(), 0);
        assert_eq!(arguments(json!({ "command": "doc.outline" })).unwrap().len(), 1);
        let bad = arguments(json!("doc.outline")).unwrap_err();
        assert!(bad.contains("must be an object"), "{bad}");
    }
}
