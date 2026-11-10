//! JSON-RPC round trips against a real `Session` behind the native server.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::Duration;

use common::{TOKEN, config, runtime, start};
use drafftink_bridge::client::{BridgeClient, ClientError};
use drafftink_bridge::rpc::{INVALID_PARAMS, METHOD_NOT_FOUND, REFUSED};
use serde_json::{Value, json};

async fn connect(port: u16) -> BridgeClient {
    BridgeClient::connect(&format!("ws://127.0.0.1:{port}/"), TOKEN).await.expect("connects")
}

fn rpc_error(r: Result<Value, ClientError>) -> drafftink_bridge::RpcError {
    match r {
        Err(ClientError::Rpc(e)) => e,
        other => panic!("expected an RPC error, got {other:?}"),
    }
}

#[test]
fn execute_runs_a_command_and_returns_its_output() {
    let shell = start(config());
    runtime().block_on(async {
        let mut c = connect(shell.server().port()).await;
        let before = c.call("execute", json!({ "id": "view.get" })).await.expect("view.get");
        let out = c.call("execute", json!({ "id": "view.zoom_in" })).await.expect("zoom_in");
        assert_eq!(out["changed"], true, "{out}");
        let after = c.call("execute", json!({ "id": "view.get", "args": {} })).await.expect("view.get");
        assert_ne!(before, after);
    });
}

#[test]
fn a_refusal_is_an_error_with_the_plain_words_and_the_whole_refusal_as_data() {
    let shell = start(config());
    runtime().block_on(async {
        let mut c = connect(shell.server().port()).await;
        let e = rpc_error(c.call("execute", json!({ "id": "view.zoom_to_fit" })).await);
        assert_eq!(e.code, REFUSED);
        assert!(e.message.contains("`view.center`"), "{}", e.message);
        let data = e.data.expect("data");
        assert_eq!(data["code"], "nothing_to_do");
        assert_eq!(data["command"], "view.zoom_to_fit");
        assert_eq!(data["message"], json!(e.message));
        let e = rpc_error(c.call("execute", json!({ "id": "view.zoom_inn" })).await);
        assert_eq!(e.data.expect("data")["code"], "unknown_command");
        assert!(e.message.contains("view.zoom_in"), "{}", e.message);
    });
}

#[test]
fn bad_arguments_name_the_key() {
    let shell = start(config());
    runtime().block_on(async {
        let mut c = connect(shell.server().port()).await;
        let e = rpc_error(c.call("execute", json!({ "id": "view.pan", "args": { "dx": 1, "dyy": 2 } })).await);
        assert_eq!(e.data.expect("data")["code"], "bad_args");
        assert!(e.message.contains("dy"), "{}", e.message);
    });
}

#[test]
fn bad_params_are_invalid_params_not_refusals() {
    let shell = start(config());
    runtime().block_on(async {
        let mut c = connect(shell.server().port()).await;
        for params in [json!({}), json!({ "id": 5 }), json!({ "id": "view.get", "extra": 1 })] {
            assert_eq!(rpc_error(c.call("execute", params).await).code, INVALID_PARAMS);
        }
        assert_eq!(rpc_error(c.call("nope", json!({})).await).code, METHOD_NOT_FOUND);
    });
}

#[test]
fn commands_describe_and_recent_work() {
    let shell = start(config());
    runtime().block_on(async {
        let mut c = connect(shell.server().port()).await;
        let list = c.call("commands", json!({ "family": "view" })).await.expect("commands");
        assert!(list["count"].as_u64().unwrap_or(0) > 3);
        let one = c.call("describe", json!({ "id": "view.pan" })).await.expect("describe");
        assert_eq!(one["id"], "view.pan");
        assert!(one["args_schema"].is_object());
        let e = rpc_error(c.call("describe", json!({ "id": "view.nope" })).await);
        assert!(e.message.contains("Unknown command"), "{}", e.message);
        c.call("execute", json!({ "id": "view.zoom_in" })).await.expect("zoom");
        let recent = c.call("recent", json!({ "limit": 5 })).await.expect("recent");
        let entries = recent["entries"].as_array().expect("entries");
        let last = entries.last().expect("an entry");
        assert_eq!(last["id"], "view.zoom_in");
        assert_eq!(last["source"], "agent", "every bridge call is recorded with origin agent: {last}");
        assert!(c.call("state", json!({})).await.expect("state")["tabs"].is_array());
    });
}

#[test]
fn a_batch_is_atomic_and_a_failed_step_is_reported() {
    let shell = start(config());
    runtime().block_on(async {
        let mut c = connect(shell.server().port()).await;
        let ok = c
            .call("batch", json!({ "steps": [ { "id": "view.zoom_in" }, { "id": "view.pan", "args": { "dx": 5, "dy": 5 } } ] }))
            .await
            .expect("batch");
        assert_eq!(ok["completed"], 2, "{ok}");
        let bad = c
            .call("batch", json!({ "steps": [ { "id": "view.zoom_in" }, { "id": "view.zoom_to_fit" } ] }))
            .await
            .expect("a failed batch is a normal result");
        assert_eq!(bad["rolled_back"], true, "{bad}");
        assert_eq!(bad["failed"]["index"], 1);
        assert!(bad["failed"]["refusal"]["message"].as_str().unwrap_or("").contains("view.center"));
    });
}

#[test]
fn commands_that_are_not_for_agents_are_refused_even_in_a_batch() {
    let shell = start(config());
    runtime().block_on(async {
        let mut c = connect(shell.server().port()).await;
        let e = rpc_error(c.call("execute", json!({ "id": "gesture.begin", "args": {} })).await);
        assert_eq!(e.data.expect("data")["code"], "not_for_agents");
        let e = rpc_error(c.call("batch", json!({ "steps": [ { "id": "tool.hold", "args": {} } ] })).await);
        assert_eq!(e.data.expect("data")["code"], "not_for_agents");
    });
}

#[test]
fn subscribers_hear_about_changes_and_others_do_not() {
    let shell = start(config());
    runtime().block_on(async {
        let port = shell.server().port();
        let mut watcher = connect(port).await;
        let mut worker = connect(port).await;
        let mut quiet = connect(port).await;
        watcher.call("subscribe", json!({})).await.expect("subscribe");
        worker.call("execute", json!({ "id": "view.zoom_in" })).await.expect("zoom");
        let (method, params) = watcher.next_event(Duration::from_secs(3)).await.expect("a change notification");
        assert_eq!(method, "changed");
        assert!(params["seq"].as_u64().unwrap_or(0) > 0, "{params}");
        assert!(quiet.next_event(Duration::from_millis(300)).await.is_none());
        watcher.call("unsubscribe", json!({})).await.expect("unsubscribe");
    });
}

#[test]
fn requests_from_several_connections_all_run_on_the_shell() {
    let shell = start(config());
    runtime().block_on(async {
        let port = shell.server().port();
        let mut handles = Vec::new();
        for _ in 0..4 {
            handles.push(tokio::spawn(async move {
                let mut c = connect(port).await;
                for _ in 0..10 {
                    c.call("execute", json!({ "id": "view.zoom_in" })).await.expect("zoom");
                }
            }));
        }
        for h in handles {
            h.await.expect("task");
        }
        let mut c = connect(port).await;
        let r = c.call("recent", json!({ "limit": 256 })).await.expect("recent");
        assert!(r["entries"].as_array().map_or(0, Vec::len) >= 40, "{r}");
    });
}
