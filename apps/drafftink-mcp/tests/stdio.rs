//! The real binary over real pipes: stdout carries only the protocol.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

fn spawn(args: &[&str]) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_drafftink-mcp"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

#[test]
fn the_binary_serves_mcp_on_stdio_and_exits_when_stdin_closes() {
    let mut child = spawn(&[]);
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let init = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "t", "version": "1" } } });
    writeln!(stdin, "{init}").unwrap();
    writeln!(stdin, "{}", json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).unwrap();
    writeln!(stdin, "{}", json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "view", "arguments": { "command": "view.zoom_in" } } })).unwrap();
    for want in [1, 2] {
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        let v: Value =
            serde_json::from_str(&line).unwrap_or_else(|e| panic!("stdout must be JSON-RPC only: {line:?} {e}"));
        assert_eq!(v["id"], want, "{v}");
        assert!(v.get("result").is_some(), "{v}");
    }
    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success(), "{status}");
}

#[test]
fn conflicting_modes_are_refused_with_a_usage_message() {
    for args in [
        vec!["--attach", "--relay"],
        vec!["--port", "5"],
        vec!["--relay-port", "5"],
        vec!["--attach", "--allow-any-path", "--allow-dir", "."],
    ] {
        let out = spawn(&args).wait_with_output().unwrap();
        assert!(!out.status.success(), "{args:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("error"), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
    }
}

#[test]
fn attach_without_a_running_app_starts_and_explains_itself_on_the_first_call() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = spawn(&["--attach", "--bridge-dir", dir.path().to_str().unwrap()]);
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    writeln!(stdin, "{}", json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "t", "version": "1" } } })).unwrap();
    writeln!(stdin, "{}", json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).unwrap();
    writeln!(
        stdin,
        "{}",
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "status", "arguments": {} } })
    )
    .unwrap();
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    line.clear();
    out.read_line(&mut line).unwrap();
    assert!(line.contains("Start the desktop app") || line.contains("no running"), "{line}");
    drop(stdin);
    child.wait().unwrap();
}
