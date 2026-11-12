//! The subcommands of the real `drafftink` binary, run as processes without a display.
//!
//! `drafftink convert|run|outline|inspect|commands|schemas` must give the output, notes and exit code of
//! `drafftink-cli`, and `drafftink mcp` must speak MCP on stdio as `drafftink-mcp` does. The reference for the
//! first is the library that the `drafftink-cli` binary is a wrapper of: it parses the same words with its own
//! parser and runs them in this process.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Output, Stdio};

use clap::Parser;
use drafftink_cli::cli::{Cli, execute};
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

/// What `drafftink-cli` gives for the arguments: standard output, standard error and the exit code.
fn reference(dir: &Path, args: &[&str]) -> (Vec<u8>, Vec<u8>, i32) {
    let words = std::iter::once("drafftink-cli").chain(args.iter().copied());
    let cli = match Cli::try_parse_from(words) {
        Ok(cli) => cli,
        Err(e) => return (Vec::new(), e.render().to_string().into_bytes(), e.exit_code()),
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    // The commands read relative paths from the current folder, which is the test's: run them there.
    let previous = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir).unwrap();
    let result = execute(&cli.command, &mut out, &mut err);
    std::env::set_current_dir(previous).unwrap();
    let code = match result {
        Ok(()) => 0,
        Err(e) => {
            writeln!(err, "drafftink-cli: {e}").unwrap();
            1
        }
    };
    (out, err, code)
}

/// `drafftink`'s answer to the same arguments. Its error line starts with `drafftink:`, which is the one thing
/// that differs from `drafftink-cli:`; the line is renamed so that the rest is compared byte for byte.
fn subcommand(dir: &Path, args: &[&str]) -> (Vec<u8>, Vec<u8>, i32) {
    let Output { stdout, stderr, status } = drafftink(dir, args).output().unwrap();
    let stderr: String = String::from_utf8(stderr)
        .unwrap()
        .split_inclusive('\n')
        .map(|line| {
            line.strip_prefix("drafftink: ").map_or_else(|| line.to_owned(), |rest| format!("drafftink-cli: {rest}"))
        })
        .collect();
    (stdout, stderr.into_bytes(), status.code().unwrap())
}

const BOXES: &str = r#"{"id":"element.create","args":{"kind":"rectangle","bounds":{"x":0,"y":0,"w":120,"h":60},"name":"first"}}
{"id":"element.create","args":{"kind":"ellipse","bounds":{"x":240,"y":0,"w":120,"h":60},"name":"second"}}
"#;

/// A folder with a script and the document that `run --seed` makes of it.
fn workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("boxes.jsonl"), BOXES).unwrap();
    let (_, err, code) = reference(dir.path(), &["run", "boxes.jsonl", "--seed", "7", "--out", "boxes.drafftink"]);
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&err));
    dir
}

fn assert_same_as_cli(dir: &Path, args: &[&str]) {
    let want = reference(dir, args);
    let got = subcommand(dir, args);
    assert_eq!(String::from_utf8_lossy(&got.0), String::from_utf8_lossy(&want.0), "stdout of {args:?}");
    assert_eq!(String::from_utf8_lossy(&got.1), String::from_utf8_lossy(&want.1), "stderr of {args:?}");
    assert_eq!(got.2, want.2, "exit code of {args:?}");
}

#[test]
fn outline_inspect_and_commands_print_what_drafftink_cli_prints() {
    let dir = workspace();
    for args in [
        &["outline", "boxes.drafftink"][..],
        &["outline", "boxes.drafftink", "--json"],
        &["outline", "boxes.drafftink", "--max-nodes", "1"],
        &["inspect", "boxes.drafftink"],
        &["commands"],
        &["commands", "--family", "style"],
        &["commands", "--search", "undo"],
        &["commands", "element.create"],
        &["schemas"],
    ] {
        assert_same_as_cli(dir.path(), args);
    }
}

#[test]
fn convert_and_run_write_what_drafftink_cli_writes() {
    let dir = workspace();
    for target in ["copy.json", "copy.excalidraw", "copy.svg", "copy.drafftink"] {
        let args = ["convert", "boxes.drafftink", target];
        assert_same_as_cli(dir.path(), &args);
    }
    // The same input gives the same bytes through both doors.
    let written = |name: &str| std::fs::read(dir.path().join(name)).unwrap();
    let (_, _, code) = subcommand(dir.path(), &["convert", "boxes.drafftink", "again.excalidraw"]);
    assert_eq!(code, 0);
    assert_eq!(written("again.excalidraw"), written("copy.excalidraw"));
    let (via_app, _, code) = subcommand(dir.path(), &["run", "boxes.jsonl", "--seed", "3", "--out", "a.drafftink"]);
    assert_eq!(code, 0);
    let (via_cli, _, _) = reference(dir.path(), &["run", "boxes.jsonl", "--seed", "3", "--out", "b.drafftink"]);
    assert_eq!(via_app, via_cli);
    assert_eq!(written("a.drafftink"), written("b.drafftink"));
}

#[test]
fn failures_have_the_same_message_and_exit_code() {
    let dir = workspace();
    for args in [
        &["outline", "missing.drafftink"][..],
        &["convert", "boxes.drafftink", "out.png"],
        &["convert", "boxes.drafftink", "out.unknown"],
        &["commands", "no.such.command"],
        &["run", "missing.jsonl"],
    ] {
        assert_same_as_cli(dir.path(), args);
        assert_eq!(subcommand(dir.path(), args).2, 1, "{args:?}");
    }
}

#[test]
fn a_mistake_in_the_arguments_exits_with_2_and_opens_no_window() {
    let dir = workspace();
    for args in [&["outline"][..], &["convert", "only-one"], &["run", "s.jsonl", "--room", "r"]] {
        let out = drafftink(dir.path(), args).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
        let text = String::from_utf8_lossy(&out.stderr);
        assert!(text.contains("Usage: drafftink"), "{text}");
    }
}

#[test]
fn a_subcommand_needs_no_display() {
    let dir = workspace();
    let out = drafftink(dir.path(), &["outline", "boxes.drafftink"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("@first rectangle") && text.contains("@second ellipse"), "{text}");
    // The window path prints this line when it ends. A tool never gets that far.
    assert!(!String::from_utf8_lossy(&out.stderr).contains("frames="));
}

#[test]
fn help_lists_the_subcommands_and_explains_each_one() {
    let dir = tempfile::tempdir().unwrap();
    let help = |args: &[&str]| {
        let out = drafftink(dir.path(), args).output().unwrap();
        assert!(out.status.success(), "{args:?}");
        String::from_utf8(out.stdout).unwrap()
    };
    let all = help(&["--help"]);
    for name in ["run", "convert", "outline", "inspect", "commands", "schemas", "mcp"] {
        assert!(all.lines().any(|l| l.trim_start().starts_with(&format!("{name} "))), "{name}\n{all}");
    }
    assert!(help(&["help", "mcp"]).contains("--attach"));
    assert_eq!(help(&["help", "outline"]), help(&["outline", "--help"]));
    assert!(help(&["help", "convert"]).contains("Usage: drafftink convert"));
}

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
}

#[test]
fn mcp_answers_initialize_and_tools_list_on_stdio_and_ends_with_its_input() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = McpServer::start(dir.path(), &["mcp"]);
    server.send(&json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "1" } } }));
    let init = server.response(1);
    assert!(init["result"]["capabilities"]["tools"].is_object(), "{init}");
    server.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    server.send(&json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
    let list = server.response(2);
    let names: Vec<&str> =
        list["result"]["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"view"), "{names:?}");
    assert!(names.len() > 3, "{names:?}");
    assert!(server.finish().success());
}

#[test]
fn mcp_refuses_conflicting_modes_like_drafftink_mcp() {
    let dir = tempfile::tempdir().unwrap();
    for args in [&["mcp", "--attach", "--relay"][..], &["mcp", "--port", "5"], &["mcp", "--relay-port", "5"]] {
        let out = drafftink(dir.path(), args).stdin(Stdio::null()).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("error"), "{args:?}");
    }
}
