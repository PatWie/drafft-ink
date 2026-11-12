//! The subcommands of the real `drafftink` binary, run as processes without a display.
//!
//! `drafftink` has one subcommand that works, `mcp`, and it is only the bridge: it attaches to the running app unless
//! `--relay` is given. The tools that need no screen moved to `drafftink-headless` (its tests check their output), so
//! here they must give one line that says where they went, and a non-zero exit code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

/// `drafftink` with the arguments, in a folder, and with no display of any kind.
fn drafftink(dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_drafftink"));
    command
        .args(args)
        .current_dir(dir)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("WAYLAND_SOCKET")
        .env_remove("DRAFFTINK_BRIDGE_TOKEN");
    command
}

const MOVED: &str = "That is part of drafftink-headless: `cargo install drafftink-headless`, then `drafftink-headless";

#[test]
fn a_tool_that_moved_says_where_it_went_and_exits_with_2_and_opens_no_window() {
    let dir = tempfile::tempdir().unwrap();
    for (args, name) in [
        (&["convert", "a.drafftink", "b.excalidraw"][..], "convert"),
        (&["convert"], "convert"),
        (&["convert", "--help"], "convert"),
        (&["run", "script.jsonl", "--seed", "3"], "run"),
        (&["outline", "a.drafftink", "--json"], "outline"),
        (&["inspect", "a.drafftink"], "inspect"),
        (&["commands", "--family", "style"], "commands"),
        (&["schemas"], "schemas"),
        (&["mcp", "--allow-dir", "."], "mcp"),
        (&["mcp", "--allow-any-path"], "mcp"),
    ] {
        let out = drafftink(dir.path(), args).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
        let text = String::from_utf8(out.stderr).unwrap();
        assert_eq!(text.lines().count(), 1, "one line for {args:?}: {text}");
        assert_eq!(text.trim_end(), format!("{MOVED} {name} …`"), "{args:?}");
        // The window path prints this line when it ends. A subcommand never gets that far.
        assert!(!text.contains("frames="), "{args:?}");
    }
}

#[test]
fn a_file_named_like_a_subcommand_still_reaches_the_window_path() {
    // `./convert` is a file name. With no display the window cannot open, and that failure is not the message about
    // a tool that moved.
    let dir = tempfile::tempdir().unwrap();
    for args in [&["./convert"][..], &["--", "run"]] {
        let out = drafftink(dir.path(), args).output().unwrap();
        assert!(!String::from_utf8_lossy(&out.stderr).contains(MOVED), "{args:?}");
    }
}

#[test]
fn help_lists_mcp_and_points_at_drafftink_headless() {
    let dir = tempfile::tempdir().unwrap();
    let help = |args: &[&str]| {
        let out = drafftink(dir.path(), args).output().unwrap();
        assert!(out.status.success(), "{args:?}");
        String::from_utf8(out.stdout).unwrap()
    };
    let all = help(&["--help"]);
    assert!(all.lines().any(|l| l.trim_start().starts_with("mcp ")), "{all}");
    assert!(all.contains("cargo install drafftink-headless"), "{all}");
    for name in ["convert", "run", "outline", "inspect", "commands", "schemas"] {
        assert!(!all.lines().any(|l| l.trim_start().starts_with(&format!("{name} "))), "{name}\n{all}");
    }
    let mcp = help(&["mcp", "--help"]);
    assert!(mcp.contains("--attach") && mcp.contains("--relay"), "{mcp}");
    assert!(!mcp.contains("--allow-dir"), "the headless options are not advertised: {mcp}");
    assert_eq!(help(&["help", "mcp"]), mcp);
}

#[test]
fn mcp_refuses_conflicting_modes() {
    let dir = tempfile::tempdir().unwrap();
    for args in [&["mcp", "--attach", "--relay"][..], &["mcp", "--relay-port", "5"], &["mcp", "--relay", "--port", "5"]]
    {
        let out = drafftink(dir.path(), args).stdin(Stdio::null()).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("error"), "{args:?}");
    }
}

/// A `drafftink mcp` process and its pipes.
struct McpServer {
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
    stdout: BufReader<std::process::ChildStdout>,
}

impl McpServer {
    fn start(dir: &Path, args: &[&str]) -> Self {
        let mut child =
            drafftink(dir, args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self { child, stdin, stdout }
    }

    fn send(&mut self, message: &Value) {
        writeln!(self.stdin.as_mut().unwrap(), "{message}").unwrap();
    }

    /// The response with this id. Standard output must carry nothing but JSON-RPC.
    fn response(&mut self, id: u64) -> Value {
        loop {
            let mut line = String::new();
            assert!(self.stdout.read_line(&mut line).unwrap() > 0, "the server closed standard output");
            let value: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("not JSON-RPC: {line:?}: {e}"));
            if value["id"] == json!(id) {
                return value;
            }
        }
    }

    /// Closes standard input, which ends a stdio server, and gives the exit status.
    fn finish(mut self) -> std::process::ExitStatus {
        drop(self.stdin.take());
        self.child.wait().unwrap()
    }

    fn initialize(&mut self) {
        self.send(&json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "1" } } }));
        let init = self.response(1);
        assert!(init["result"]["capabilities"]["tools"].is_object(), "{init}");
        self.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    }
}

#[test]
fn mcp_attaches_by_default_and_explains_on_the_first_call_that_no_app_is_running() {
    // An empty folder has no token file: no app is running there. Neither `--attach` nor any mode is given.
    let bridge_dir = tempfile::tempdir().unwrap();
    let mut server = McpServer::start(bridge_dir.path(), &["mcp", "--bridge-dir", bridge_dir.path().to_str().unwrap()]);
    server.initialize();
    server.send(
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "status", "arguments": {} } }),
    );
    let status = server.response(2).to_string();
    assert!(status.contains("Start the desktop app") || status.contains("no running"), "{status}");
    assert!(status.contains("attached"), "the default mode is attach: {status}");
    assert!(server.finish().success());
}

#[test]
fn mcp_with_a_bridge_option_alone_still_attaches() {
    // `--port` needs `--attach` in `drafftink-headless mcp`; here attach is the default, so it is accepted.
    let dir = tempfile::tempdir().unwrap();
    let mut server =
        McpServer::start(dir.path(), &["mcp", "--port", "9", "--bridge-dir", dir.path().to_str().unwrap()]);
    server.initialize();
    assert!(server.finish().success());
}
