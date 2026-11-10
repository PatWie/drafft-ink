//! MCP protocol tests against the real server, in headless mode.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::collections::BTreeSet;

use common::{Client, headless, is_error, text_of};
use drafftink_engine::Registry;
use drafftink_engine::spec::Status;
use serde_json::{Value, json};

#[tokio::test]
async fn initialize_names_the_server_and_points_at_the_guide() {
    let mut c = Client::start(headless()).await;
    let init = c.result("initialize", json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "t", "version": "1" } })).await;
    assert_eq!(init["serverInfo"]["name"], "drafftink-mcp");
    assert!(init["capabilities"]["tools"].is_object());
    assert!(init["capabilities"]["prompts"].is_object());
    let text = init["instructions"].as_str().unwrap();
    assert!(text.contains("doc.outline") && text.contains("drafftink_guide") && text.contains("batch"), "{text}");
}

/// Every id that a tool's `command` enum lists.
fn listed(tool: &Value) -> Vec<String> {
    tool["inputSchema"]["properties"]["command"]["enum"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

#[tokio::test]
async fn tools_list_matches_the_registry() {
    let mut c = Client::start(headless()).await;
    let tools = c.tools().await;
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    let unique: BTreeSet<&str> = names.iter().copied().collect();
    assert_eq!(unique.len(), names.len(), "tool names are unique: {names:?}");
    for fixed in ["inspect", "batch", "commands", "describe", "recent", "status"] {
        assert!(names.contains(&fixed), "{fixed} in {names:?}");
    }

    let reg = Registry::global();
    let own_tool = ["engine.commands", "engine.describe", "engine.recent"];
    let file_tools = ["doc.open", "doc.save", "doc.export"];
    let expected: BTreeSet<String> = reg
        .all()
        .iter()
        .filter(|s| s.agent && matches!(s.status, Status::Stable) && !own_tool.contains(&s.id))
        .filter(|s| !file_tools.contains(&s.id) || reg.find(s.id).is_none())
        .map(|s| s.id.to_owned())
        .collect();
    let mut seen = BTreeSet::new();
    for tool in &tools {
        for id in listed(tool) {
            assert!(seen.insert(id.clone()), "{id} is listed by two tools");
        }
    }
    let covered: BTreeSet<String> = seen.iter().filter(|id| !file_tools.contains(&id.as_str())).cloned().collect();
    let expected_without_files: BTreeSet<String> =
        expected.iter().filter(|id| !file_tools.contains(&id.as_str())).cloned().collect();
    assert_eq!(covered, expected_without_files, "tools/list and the registry disagree");
    // No tool offers a command that is not for agents or is planned.
    for id in &seen {
        let spec = reg.find(id).unwrap();
        assert!(spec.agent && matches!(spec.status, Status::Stable), "{id}");
    }
}

#[tokio::test]
async fn every_tool_is_a_closed_object_with_honest_hints() {
    let mut c = Client::start(headless()).await;
    let reg = Registry::global();
    for tool in c.tools().await {
        let name = tool["name"].as_str().unwrap();
        let schema = &tool["inputSchema"];
        assert_eq!(schema["type"], "object", "{name}");
        assert_eq!(schema["additionalProperties"], json!(false), "{name}");
        assert!(tool["description"].as_str().is_some_and(|d| d.len() > 20), "{name}");
        let hints = &tool["annotations"];
        let ids = listed(&tool);
        if ids.is_empty() {
            continue;
        }
        let all_read = ids.iter().all(|id| reg.find(id).is_some_and(|s| s.reads_only));
        let any_write = ids.iter().any(|id| reg.find(id).is_some_and(|s| !s.reads_only));
        assert_eq!(
            hints["readOnlyHint"] == json!(true),
            all_read,
            "{name}: readOnlyHint must equal 'every command only reads'"
        );
        assert!(
            !(any_write && hints["readOnlyHint"] == json!(true)),
            "{name} lists an editing command but claims to be read-only"
        );
        // The description names every command of the tool, with its summary.
        let description = tool["description"].as_str().unwrap();
        for id in &ids {
            assert!(description.contains(&format!("- {id} - ")), "{name}: {id} missing from the description");
        }
    }
}

#[tokio::test]
async fn a_family_tool_runs_a_command() {
    let mut c = Client::start(headless()).await;
    let r = c.tool("view", json!({ "command": "view.zoom_in" })).await;
    assert!(!is_error(&r), "{r}");
    let out: Value = serde_json::from_str(&text_of(&r)).unwrap();
    assert_eq!(out["changed"], true);
    let view = c.tool("inspect", json!({ "command": "view.get" })).await;
    assert!(!is_error(&view), "{view}");
}

#[tokio::test]
async fn a_refusal_is_a_tool_error_with_the_plain_words() {
    let mut c = Client::start(headless()).await;
    let r = c.tool("view", json!({ "command": "view.zoom_to_fit" })).await;
    assert!(is_error(&r), "{r}");
    let text = text_of(&r);
    assert!(text.contains("`view.center`"), "{text}");
    assert!(!text.contains("rpc") && !text.contains("-3201"), "no protocol noise in the model's text: {text}");
    // The refusal did not change anything: the camera is where it was.
    let again = c.tool("view", json!({ "command": "view.zoom_to_fit" })).await;
    assert_eq!(text_of(&again), text);
}

#[tokio::test]
async fn bad_arguments_name_the_key_and_the_fix() {
    let mut c = Client::start(headless()).await;
    let r = c.tool("view", json!({ "command": "view.pan", "args": { "dx": 1, "dyy": 2 } })).await;
    assert!(is_error(&r));
    assert!(text_of(&r).contains("dy"), "{}", text_of(&r));
    let r = c.tool("view", json!({ "command": "view.zoom_inn" })).await;
    assert!(is_error(&r));
    assert!(text_of(&r).contains("view.zoom_in"), "{}", text_of(&r));
    let r = c.tool("view", json!({ "command": "view.pan", "args": [1] })).await;
    assert!(is_error(&r) && text_of(&r).contains("`args` must be an object"), "{}", text_of(&r));
}

#[tokio::test]
async fn a_command_called_through_the_wrong_tool_is_told_the_right_one() {
    let mut c = Client::start(headless()).await;
    let r = c.tool("inspect", json!({ "command": "view.zoom_in" })).await;
    assert!(is_error(&r) && text_of(&r).contains("`view` tool"), "{}", text_of(&r));
    let r = c.tool("view", json!({ "command": "view.get" })).await;
    assert!(is_error(&r) && text_of(&r).contains("`inspect` tool"), "{}", text_of(&r));
    let r = c.tool("tab", json!({ "command": "view.zoom_in" })).await;
    assert!(is_error(&r) && text_of(&r).contains("`view` tool"), "{}", text_of(&r));
}

#[tokio::test]
async fn commands_that_are_not_for_agents_stay_out_of_reach() {
    let mut c = Client::start(headless()).await;
    let r = c.tool("view", json!({ "command": "view.set_viewport", "args": { "width": 1, "height": 1 } })).await;
    assert!(is_error(&r), "{r}");
    assert!(text_of(&r).contains("not available to agents"), "{}", text_of(&r));
}

#[tokio::test]
async fn an_unknown_argument_lists_the_known_ones() {
    let mut c = Client::start(headless()).await;
    let r = c.tool("view", json!({ "command": "view.zoom_in", "nope": 1 })).await;
    assert!(is_error(&r));
    assert!(text_of(&r).contains("command, args") || text_of(&r).contains("args, command"), "{}", text_of(&r));
}

#[tokio::test]
async fn an_unknown_tool_is_a_protocol_error() {
    let mut c = Client::start(headless()).await;
    let r = c.request("tools/call", json!({ "name": "nope", "arguments": {} })).await;
    assert_eq!(r["error"]["code"], -32602, "{r}");
    assert!(r["error"]["message"].as_str().unwrap().contains("inspect"), "{r}");
}

#[tokio::test]
async fn a_batch_is_atomic_over_mcp() {
    let mut c = Client::start(headless()).await;
    let ok = c.tool("batch", json!({ "steps": [ { "command": "view.zoom_in" }, { "command": "view.pan", "args": { "dx": 3, "dy": 4 } } ], "label": "Look around" })).await;
    assert!(!is_error(&ok), "{ok}");
    let out: Value = serde_json::from_str(text_of(&ok).lines().next().unwrap()).unwrap();
    assert_eq!(out["completed"], 2);
    let bad =
        c.tool("batch", json!({ "steps": [ { "command": "view.zoom_in" }, { "command": "view.zoom_to_fit" } ] })).await;
    assert!(is_error(&bad), "{bad}");
    let text = text_of(&bad);
    assert!(text.starts_with("Step 1 (`view.zoom_to_fit`) refused:"), "{text}");
    assert!(text.contains("`view.center`") && text.contains("rolled back"), "{text}");
    let r = c.tool("batch", json!({ "steps": [ { "command": "view.zoom_in", "bogus": 1 } ] })).await;
    assert!(is_error(&r) && text_of(&r).contains("unknown key `bogus`"), "{}", text_of(&r));
    // An empty batch is not an error: the engine says that nothing ran.
    let r = c.tool("batch", json!({ "steps": [] })).await;
    assert!(!is_error(&r) && text_of(&r).contains("nothing was run"), "{r}");
}

#[tokio::test]
async fn commands_describe_recent_and_status_work() {
    let mut c = Client::start(headless()).await;
    let list = c.tool("commands", json!({ "family": "view" })).await;
    let v: Value = serde_json::from_str(&text_of(&list)).unwrap();
    let rows = v["commands"].as_array().unwrap();
    assert!(rows.iter().any(|r| r["id"] == "view.pan" && r["available"] == json!(true)));
    assert!(rows.iter().all(|r| r["id"] != "view.set_viewport"), "commands that are not for agents are not listed");
    let none = c.tool("commands", json!({ "family": "nope" })).await;
    assert!(is_error(&none) && text_of(&none).contains("Families:"), "{}", text_of(&none));

    let d = c.tool("describe", json!({ "command": "view.pan" })).await;
    let dv: Value = serde_json::from_str(&text_of(&d)).unwrap();
    assert!(dv["args_schema"].is_object() && dv["example"].is_object());

    c.tool("view", json!({ "command": "view.zoom_in" })).await;
    let recent = c.tool("recent", json!({ "limit": 3 })).await;
    let rv: Value = serde_json::from_str(&text_of(&recent)).unwrap();
    let last = rv["entries"].as_array().unwrap().last().unwrap().clone();
    assert_eq!((last["id"].as_str(), last["source"].as_str()), (Some("view.zoom_in"), Some("agent")));

    let status = c.tool("status", json!({})).await;
    let sv: Value = serde_json::from_str(&text_of(&status)).unwrap();
    assert_eq!((sv["mode"].as_str(), sv["connected"].clone()), (Some("headless"), json!(true)));
    assert!(sv["note"].as_str().unwrap().contains("SVG"));
}

#[tokio::test]
async fn the_guide_is_a_prompt() {
    let mut c = Client::start(headless()).await;
    let list = c.result("prompts/list", json!({})).await;
    assert_eq!(list["prompts"][0]["name"], "drafftink_guide");
    let got = c.result("prompts/get", json!({ "name": "drafftink_guide" })).await;
    let text = got["messages"][0]["content"]["text"].as_str().unwrap();
    for must in ["doc.outline", "diagram.create", "element.query", "batch", "history.undo"] {
        assert!(text.contains(must), "{must}");
    }
    let bad = c.request("prompts/get", json!({ "name": "nope" })).await;
    assert!(bad.get("error").is_some());
}

#[tokio::test]
async fn stdout_carries_only_protocol_messages() {
    // Every line the client read in the other tests parsed as JSON-RPC; here a burst of mixed calls does too.
    let mut c = Client::start(headless()).await;
    for _ in 0..5 {
        c.tool("view", json!({ "command": "view.zoom_in" })).await;
        c.tool("view", json!({ "command": "view.zoom_to_fit" })).await;
        c.request("ping", json!({})).await;
    }
}
