//! The UI is thin: this test reads the source of the crate and fails when a file reaches into the
//! session or the document in a way that changes state. Only `execute`, `execute_from` and
//! `execute_batch` may change state; everything else on a session is a read.
//!
//! It is a grep, so it is deliberately strict and simple: a new read goes on the allow-list below with a
//! reason, a new write does not belong in this crate.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};

fn sources() -> Vec<(PathBuf, String)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
    files.sort();
    files.into_iter().map(|p| (p.clone(), strip(&fs::read_to_string(&p).unwrap()))).collect()
}

/// Source without comments and without the contents of string literals, line structure kept.
fn strip(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let b: Vec<char> = src.chars().collect();
    let (mut i, mut in_str) = (0, false);
    while i < b.len() {
        let c = b[i];
        if in_str {
            if c == '\\' {
                i += 2;
                continue;
            }
            if c == '"' {
                in_str = false;
                out.push('"');
            } else if c == '\n' {
                out.push('\n');
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_str = true;
            out.push('"');
        } else if c == '\'' && i + 2 < b.len() && b[i + 2] == '\'' {
            // a char literal such as '"'
            out.push_str("' '");
            i += 3;
            continue;
        } else if c == '/' && b.get(i + 1) == Some(&'/') {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
            continue;
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

fn name(p: &Path) -> String {
    p.strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")).join("src")).unwrap().to_string_lossy().replace('\\', "/")
}

/// The files that hold a session. Everything else draws from the model and returns actions.
const MAY_NAME_SESSION: &[&str] = &["shell.rs", "model.rs", "action.rs"];

#[test]
fn only_the_shell_the_model_and_the_dispatcher_name_the_session() {
    for (p, src) in sources() {
        let n = name(&p);
        let words: Vec<&str> = src.split(|c: char| !c.is_alphanumeric() && c != '_').collect();
        if words.contains(&"Session") && !MAY_NAME_SESSION.contains(&n.as_str()) && n != "lib.rs" {
            panic!("{n} names `Session`; surfaces draw from the model and return actions");
        }
    }
}

#[test]
fn a_mutable_session_appears_only_where_commands_are_executed() {
    let mut found = Vec::new();
    for (p, src) in sources() {
        let n = name(&p);
        for (no, line) in src.lines().enumerate() {
            if line.contains("&mut Session") {
                found.push(format!("{n}:{}", no + 1));
                assert!(n == "shell.rs" || n == "action.rs", "{n}:{} takes `&mut Session`", no + 1);
            }
        }
    }
    assert!(!found.is_empty(), "the shell and the sink take one: {found:?}");
}

/// Methods and fields that change a session, a tab or a document. None may appear in the UI.
const FORBIDDEN: &[&str] = &[
    "tab_mut",
    "set_raw",
    "commit_since",
    "commit_untracked",
    "undo_steps",
    "redo_steps",
    "rollback_to",
    "apply_remote",
    "queue_write",
    "mark_dirty",
    "take_dirty",
    "set_defaults",
    "restore_soft",
    "clear_history",
    "set_active_index",
    "alloc_tab_id",
    "gesture_begin_typed",
    "gesture_update",
    "gesture_end_typed",
    "gesture_cancel",
    "gesture_begin_json",
    "SceneTx",
    "TabDoc",
    "DocStore",
    "WorkStore",
    "load_settings",
    "restore_workspace",
    "workspace_tick",
    "flush_workspace",
    "set_remote_presence",
    "run_nested",
    "warn(",
    ".ext.",
];

#[test]
fn no_call_that_changes_state_except_execute() {
    for (p, src) in sources() {
        let n = name(&p);
        for tok in FORBIDDEN {
            assert!(
                !src.contains(tok),
                "{n} contains `{tok}`: only execute / execute_from / execute_batch may change state"
            );
        }
    }
}

#[test]
fn execute_calls_are_the_allowed_ones() {
    let allowed = ["execute", "execute_from", "execute_batch"];
    let mut seen = 0;
    for (p, src) in sources() {
        let n = name(&p);
        let mut rest = src.as_str();
        while let Some(i) = rest.find("execute") {
            let tail = &rest[i..];
            let end = tail.find(|c: char| !c.is_alphanumeric() && c != '_').unwrap_or(tail.len());
            let word = &tail[..end];
            assert!(allowed.contains(&word), "{n}: `{word}` is not an allowed way to run a command");
            seen += 1;
            rest = &tail[end..];
        }
    }
    assert!(seen > 0);
}

/// Reads on a session in the three files that hold one. Each is a field or method that does not change state.
const READS: &[&str] = &[
    "execute_from",
    "commands",
    "spec",
    "tab",
    "tabs",
    "active_index",
    "tool",
    "settings",
    "view_only",
    "text_edit",
    "gesture_running",
    "targets",
    "store",
];

#[test]
fn what_the_model_reads_from_the_session_is_on_the_read_list() {
    for file in ["model.rs", "shell.rs"] {
        let (p, src) = sources().into_iter().find(|(p, _)| name(p) == file).unwrap();
        let n = name(&p);
        for var in ["s.", "session."] {
            let mut rest = src.as_str();
            while let Some(i) = rest.find(var) {
                let before = rest[..i].chars().last();
                let tail = &rest[i + var.len()..];
                rest = tail;
                if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    continue; // `ts.`, `props.` ...
                }
                let end = tail.find(|c: char| !c.is_alphanumeric() && c != '_').unwrap_or(tail.len());
                let word = &tail[..end];
                if word.is_empty() {
                    continue;
                }
                assert!(READS.contains(&word), "{n}: `{var}{word}` is not on the read list");
                let after = tail[end..].trim_start();
                assert!(
                    !(after.starts_with("= ") || after.starts_with("+=") || after.starts_with("-=")),
                    "{n}: assignment to `{var}{word}`"
                );
            }
        }
    }
}

#[test]
fn the_tool_and_settings_of_a_session_are_only_read() {
    let mut reads = 0;
    for (p, src) in sources() {
        let n = name(&p);
        for field in ["tool", "settings"] {
            for pat in [format!("s.{field}."), format!("session.{field}.")] {
                let mut rest = src.as_str();
                while let Some(i) = rest.find(&pat) {
                    let tail = &rest[i + pat.len()..];
                    rest = tail;
                    let end = tail.find(|c: char| !c.is_alphanumeric() && c != '_').unwrap_or(tail.len());
                    let word = &tail[..end];
                    let ok: &[&str] = if field == "tool" {
                        &["active", "lock", "temporary", "effective", "defaults", "style_defaults"]
                    } else {
                        &["iter", "get", "revision", "bool_or", "f64_or", "u64_or"]
                    };
                    assert!(ok.contains(&word), "{n}: `{pat}{word}` is not a read");
                    reads += 1;
                }
            }
        }
    }
    assert!(reads > 0);
}

#[test]
fn widgets_never_write_through_the_model() {
    // The model is a snapshot: it has no `&mut self` method that takes a session besides refresh.
    let (_, src) = sources().into_iter().find(|(p, _)| name(p) == "model.rs").unwrap();
    for line in src.lines().filter(|l| l.contains("Session")) {
        assert!(!line.contains("&mut Session"), "model.rs: {line}");
    }
}
