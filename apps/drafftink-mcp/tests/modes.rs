//! The three modes end to end: attached to a (simulated) desktop shell, relayed to a (fake) page, and the
//! headless flowchart of the worked example.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::Duration;

use common::{Client, headless, is_error, text_of};
use drafftink_bridge::page_sim::FakePage;
use drafftink_bridge::relay::{RelayConfig, RelayServer};
use drafftink_bridge::server::BridgeConfig;
use drafftink_bridge::shell_sim::ShellSim;
use drafftink_engine::Registry;
use drafftink_engine::testing::{headless_session, session_typeset};
use drafftink_mcp::backend::{Backend, Target};
use serde_json::{Value, json};

fn tab_count(state: &Value) -> usize {
    state["tabs"].as_array().map_or(0, Vec::len)
}

fn app(dir: &std::path::Path) -> ShellSim {
    let cfg = BridgeConfig { dir: Some(dir.to_owned()), ..BridgeConfig::default() };
    ShellSim::start(cfg, || session_typeset().0).unwrap()
}

#[tokio::test]
async fn attached_mode_drives_the_running_app_found_through_its_token_file() {
    let dir = tempfile::tempdir().unwrap();
    let shell = app(dir.path());
    let backend = Backend::attached(Target::Discover { dir: dir.path().to_owned(), port: None });
    let mut c = Client::start(backend).await;

    let status: Value = serde_json::from_str(&text_of(&c.tool("status", json!({})).await)).unwrap();
    assert_eq!((status["mode"].as_str(), status["connected"].clone()), (Some("attached"), json!(true)));
    assert_eq!(status["app"]["server"], "drafftink");

    let r = c.tool("tab", json!({ "command": "tab.new" })).await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let r = c.tool("view", json!({ "command": "view.zoom_to_fit" })).await;
    assert!(
        is_error(&r) && text_of(&r).contains("`view.center`"),
        "refusal text crosses the bridge unchanged: {}",
        text_of(&r)
    );
    assert_eq!(tab_count(&shell.stop()), 2, "the app's own session did the work");
}

#[tokio::test]
async fn attached_mode_reconnects_after_the_app_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let first = app(dir.path());
    let backend = Backend::attached(Target::Discover { dir: dir.path().to_owned(), port: None });
    let mut c = Client::start(backend).await;
    assert!(!is_error(&c.tool("tab", json!({ "command": "tab.new" })).await));
    first.stop();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let down = c.tool("view", json!({ "command": "view.zoom_in" })).await;
    assert!(is_error(&down), "{down}");
    assert!(text_of(&down).contains("drafft.ink app") || text_of(&down).contains("connection"), "{}", text_of(&down));

    let second = app(dir.path());
    let r = c.tool("tab", json!({ "command": "tab.new" })).await;
    assert!(!is_error(&r), "a new launch has a new port and token, and both are re-read: {}", text_of(&r));
    assert_eq!(tab_count(&second.stop()), 2);
}

#[tokio::test]
async fn attached_mode_says_how_to_fix_a_missing_app() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Backend::attached(Target::Discover { dir: dir.path().to_owned(), port: None });
    let mut c = Client::start(backend).await;
    let r = c.tool("view", json!({ "command": "view.zoom_in" })).await;
    assert!(is_error(&r) && text_of(&r).contains("Start the desktop app"), "{}", text_of(&r));
    let status: Value = serde_json::from_str(&text_of(&c.tool("status", json!({})).await)).unwrap();
    assert_eq!(status["connected"], false);
}

#[tokio::test]
async fn relay_mode_pairs_a_page_and_drives_it() {
    const PAIRING: &str = "pairpairpairpairpairpairpairpairpairpairpairpairpairpairpairpair";
    let relay =
        RelayServer::start(RelayConfig { pairing_token: Some(PAIRING.to_owned()), ..RelayConfig::default() }).unwrap();
    let url = relay.url();
    let backend = Backend::Relay { relay, page_url: "https://drafft.ink/".to_owned() };
    let mut c = Client::start(backend).await;

    let status: Value = serde_json::from_str(&text_of(&c.tool("status", json!({})).await)).unwrap();
    assert_eq!((status["connected"].clone(), status["paired"].clone()), (json!(false), json!(false)));
    let link = status["pairing_link"].as_str().unwrap();
    assert!(link.starts_with("https://drafft.ink/#drafftink-agent-port="), "{link}");
    let before = c.tool("view", json!({ "command": "view.zoom_in" })).await;
    assert!(is_error(&before) && text_of(&before).contains("pairing"), "{}", text_of(&before));

    let page = FakePage::connect_hosted(&url, PAIRING, || session_typeset().0).unwrap();
    let status: Value = serde_json::from_str(&text_of(&c.tool("status", json!({})).await)).unwrap();
    assert_eq!((status["connected"].clone(), status["paired"].clone()), (json!(true), json!(true)));
    assert!(status.get("pairing_link").is_none(), "a used pairing link is not offered again");

    let r = c.tool("tab", json!({ "command": "tab.new" })).await;
    assert!(!is_error(&r), "{}", text_of(&r));
    let r = c.tool("view", json!({ "command": "view.zoom_to_fit" })).await;
    assert!(is_error(&r) && text_of(&r).contains("`view.center`"), "{}", text_of(&r));
    assert_eq!(tab_count(&page.disconnect()), 2, "the page's own session did the work");

    tokio::time::sleep(Duration::from_millis(200)).await;
    let gone = c.tool("view", json!({ "command": "view.zoom_in" })).await;
    assert!(is_error(&gone) && text_of(&gone).contains("Reload"), "{}", text_of(&gone));
}

/// The flowchart of `docs/agents.md`, in the richest form this build supports.
fn flowchart_calls(reg: &Registry) -> Option<Vec<(&'static str, Value)>> {
    if reg.find("diagram.create").is_some() {
        return Some(vec![(
            "diagram",
            json!({ "command": "diagram.create", "args": {
                "nodes": [
                    { "id": "start", "label": "Start", "shape": "ellipse" },
                    { "id": "check", "label": "Valid?", "shape": "diamond" },
                    { "id": "ok", "label": "Save" },
                    { "id": "bad", "label": "Show error" }
                ],
                "edges": [
                    { "from": "start", "to": "check" },
                    { "from": "check", "to": "ok", "label": "yes" },
                    { "from": "check", "to": "bad", "label": "no" }
                ],
                "layout": { "algorithm": "layered", "direction": "down" }
            } }),
        )]);
    }
    if reg.find("element.create").is_some() {
        let rect = |name: &str, x: i32, y: i32| json!({ "command": "element.create", "args": { "kind": "rectangle", "name": name, "bounds": { "x": x, "y": y, "w": 140, "h": 60 } } });
        return Some(vec![(
            "batch",
            json!({ "steps": [
                { "command": "element.create", "args": rect("start", 0, 0)["args"], "as": "start" },
                { "command": "element.create", "args": rect("check", 0, 140)["args"], "as": "check" },
                { "command": "element.connect", "args": { "from": "$start", "to": "$check" } },
                { "command": "element.relabel", "args": { "id": "$start", "text": "Start" } },
                { "command": "element.relabel", "args": { "id": "$check", "text": "Valid?" } }
            ] }),
        )]);
    }
    None
}

#[tokio::test]
async fn a_scripted_client_builds_a_flowchart_headless_and_reads_the_outline() {
    let reg = Registry::global();
    let mut c = Client::start(headless()).await;
    let Some(calls) = flowchart_calls(&reg) else {
        // This build registers no command that creates elements yet (`diagram.create` and `element.create` come
        // from other branches). The attempt must then fail the way the guide promises: a protocol error that
        // lists the real tools, the guide says what is missing, and nothing changed.
        let r =
            c.request("tools/call", json!({ "name": "diagram", "arguments": { "command": "diagram.create" } })).await;
        assert_eq!(r["error"]["code"], -32602, "{r}");
        assert!(r["error"]["message"].as_str().unwrap().contains("Tools: inspect"), "{r}");
        let guide = c.result("prompts/get", json!({ "name": "drafftink_guide" })).await;
        assert!(guide["messages"][0]["content"]["text"].as_str().unwrap().contains("Not available in this build"));
        return;
    };
    for (tool, args) in calls {
        let r = c.tool(tool, args).await;
        assert!(!is_error(&r), "{}", text_of(&r));
    }
    let outline = c.tool("inspect", json!({ "command": "doc.outline" })).await;
    assert!(!is_error(&outline), "{}", text_of(&outline));
    let text = text_of(&outline);
    for label in ["Start", "Valid?"] {
        assert!(text.contains(label), "outline lacks {label}: {text}");
    }
    // One undo step takes the whole diagram away again.
    let undo = c.tool("history", json!({ "command": "history.undo" })).await;
    assert!(!is_error(&undo), "{}", text_of(&undo));
    let after = text_of(&c.tool("inspect", json!({ "command": "doc.outline" })).await);
    assert!(!after.contains("Start"), "{after}");
}

#[tokio::test]
async fn headless_png_export_is_refused_with_the_way_out_when_doc_export_exists() {
    let reg = Registry::global();
    if reg.find("doc.export").is_none() {
        return;
    }
    let mut c = Client::start(common::headless_with(headless_session)).await;
    let r = c.tool("export", json!({ "format": "png" })).await;
    assert!(is_error(&r), "{r}");
    assert!(text_of(&r).contains("svg"), "{}", text_of(&r));
}
