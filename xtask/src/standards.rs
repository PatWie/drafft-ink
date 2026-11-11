//! `cargo xtask standards`: repository rules that keep the agent instructions honest.
//!
//! - AGENTS.md stays under [`AGENTS_MAX_CHARS`] characters.
//! - Every file path AGENTS.md mentions in backticks exists, or is listed in
//!   `xtask/planned-paths.txt` (a shrink-only list of docs that are promised but not written).
//! - Every `cargo xtask <name>` AGENTS.md mentions is a command xtask has.
//! - The toolchain in `.github/workflows/ci.yml` equals the one in `rust-toolchain.toml`.
//! - `xtask/pending-gates.txt`, `xtask/planned-paths.txt` and `crates/engine/unregistered.txt` only shrink compared with
//!   `$XTASK_BASE_REF` (default `origin/main`). Skipped when git cannot read that ref.

use std::collections::BTreeSet;
use std::path::Path;

use crate::gates::Gate;
use crate::tools::git;

pub const AGENTS_MAX_CHARS: usize = 5000;
const PLANNED_PATHS: &str = "xtask/planned-paths.txt";
const PENDING_GATES: &str = "xtask/pending-gates.txt";
const UNREGISTERED: &str = "crates/engine/unregistered.txt";
const PATH_EXTENSIONS: &[&str] = &[".md", ".toml", ".rs", ".yml", ".yaml", ".json", ".txt", ".py", ".sh"];

/// The entries of a list file: one per line, `#` starts a comment.
pub fn entries(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or_default().trim())
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The text of each backtick span on a line, in order.
fn spans(text: &str) -> Vec<&str> {
    text.lines().flat_map(|line| line.split('`').skip(1).step_by(2)).collect()
}

fn looks_like_path(span: &str) -> bool {
    let plain = !span.is_empty()
        && span.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-'))
        && !span.starts_with('/')
        && !span.starts_with("http");
    plain && (span.contains('/') || PATH_EXTENSIONS.iter().any(|e| span.ends_with(e)))
}

/// File paths mentioned in backticks, without a trailing slash, deduplicated, in order.
pub fn mentioned_paths(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for span in spans(text).into_iter().filter(|s| looks_like_path(s)) {
        let path = span.trim_end_matches('/').to_owned();
        if !out.contains(&path) {
            out.push(path);
        }
    }
    out
}

/// The `<name>` of each `cargo xtask <name>` in backticks.
pub fn mentioned_xtask_commands(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for span in spans(text) {
        let name = span.strip_prefix("cargo xtask ").and_then(|rest| rest.split_whitespace().next());
        if let Some(name) = name
            && !out.iter().any(|n| n == name)
        {
            out.push(name.to_owned());
        }
    }
    out
}

/// Problems in AGENTS.md given a way to tell whether a path exists. Pure, for tests.
pub fn check_agents(text: &str, planned: &[String], exists: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut problems = Vec::new();
    let chars = text.chars().count();
    if chars >= AGENTS_MAX_CHARS {
        problems.push(format!(
            "AGENTS.md has {chars} characters; keep it under {AGENTS_MAX_CHARS} (move detail into docs/)"
        ));
    }
    let paths = mentioned_paths(text);
    for path in &paths {
        let is_planned = planned.iter().any(|p| p == path);
        match (exists(path), is_planned) {
            (false, false) => problems.push(format!(
                "AGENTS.md mentions `{path}`, which does not exist; write it or list it in {PLANNED_PATHS}"
            )),
            (true, true) => problems.push(format!("`{path}` exists now; remove it from {PLANNED_PATHS}")),
            _ => {}
        }
    }
    for path in planned {
        if !paths.contains(path) {
            problems.push(format!("{PLANNED_PATHS} lists `{path}`, which AGENTS.md does not mention; remove the line"));
        }
    }
    for name in mentioned_xtask_commands(text) {
        if name != "ci" && Gate::from_name(&name).is_none() {
            problems.push(format!("AGENTS.md mentions `cargo xtask {name}`, which xtask does not have"));
        }
    }
    problems
}

/// The pinned toolchain in `rust-toolchain.toml`: `channel = "..."`.
pub fn toolchain_channel(toml: &str) -> Option<String> {
    toml.lines().find_map(|l| {
        let (key, value) = l.split_once('=')?;
        (key.trim() == "channel").then(|| value.trim().trim_matches('"').to_owned())
    })
}

/// The toolchain CI installs: the `RUST_TOOLCHAIN: "x"` line in the workflow's `env`.
pub fn workflow_toolchain(yaml: &str) -> Option<String> {
    yaml.lines().find_map(|l| {
        let value = l.trim().strip_prefix("RUST_TOOLCHAIN:")?;
        Some(value.trim().trim_matches('"').to_owned())
    })
}

/// Entries in `now` that `base` does not have.
pub fn added(base: &[String], now: &[String]) -> Vec<String> {
    let base: BTreeSet<&String> = base.iter().collect();
    now.iter().filter(|e| !base.contains(e)).cloned().collect()
}

pub fn run(root: &Path) -> Result<(), String> {
    let read = |rel: &str| std::fs::read_to_string(root.join(rel)).map_err(|e| format!("{rel}: {e}"));
    let mut problems = Vec::new();

    let planned = entries(&read(PLANNED_PATHS)?);
    problems.extend(check_agents(&read("AGENTS.md")?, &planned, &|p| root.join(p).exists()));

    match (toolchain_channel(&read("rust-toolchain.toml")?), workflow_toolchain(&read(".github/workflows/ci.yml")?)) {
        (Some(a), Some(b)) if a == b => {}
        (Some(a), Some(b)) => problems.push(format!(
            "rust-toolchain.toml pins {a} but .github/workflows/ci.yml installs {b}; make them the same"
        )),
        (None, _) => problems.push("rust-toolchain.toml has no `channel = \"...\"` line".into()),
        (_, None) => problems.push(".github/workflows/ci.yml has no `RUST_TOOLCHAIN: \"...\"` line in env".into()),
    }

    let base_ref = std::env::var("XTASK_BASE_REF").unwrap_or_else(|_| "origin/main".into());
    for rel in [PENDING_GATES, PLANNED_PATHS, UNREGISTERED] {
        match git(root, &["show", &format!("{base_ref}:{rel}")]) {
            Some(base_text) => {
                for entry in added(&entries(&base_text), &entries(&read(rel)?)) {
                    problems.push(format!("{rel}: `{entry}` is new compared with {base_ref}; the list only shrinks"));
                }
            }
            None => println!("{} standards: {base_ref}:{rel} not readable; the shrink-only check did not run", crate::skips::MARKER),
        }
    }

    if problems.is_empty() {
        println!("standards: ok");
        return Ok(());
    }
    for p in &problems {
        eprintln!("  {p}");
    }
    Err(format!("{} standards problem(s)", problems.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exists_in<'a>(files: &'a [&'a str]) -> impl Fn(&str) -> bool + 'a {
        move |p| files.contains(&p)
    }

    #[test]
    fn extracts_paths_but_not_code_or_commands() {
        let text = "Read `docs/plan.md` and `crates/engine/`. Run `cargo xtask layers`. `Session::execute` / `x.y` and `ATTRIBUTION.md`, `docs/*.md`, `a b/c`.";
        assert_eq!(mentioned_paths(text), ["docs/plan.md", "crates/engine", "ATTRIBUTION.md"]);
    }

    #[test]
    fn extracts_xtask_commands() {
        let text = "`cargo xtask perf --idle` and `cargo xtask layers`, again `cargo xtask layers`";
        assert_eq!(mentioned_xtask_commands(text), ["perf", "layers"]);
    }

    #[test]
    fn missing_path_fails_unless_planned() {
        let text = "See `docs/a.md` and `docs/b.md`.";
        let problems = check_agents(text, &["docs/b.md".into()], &exists_in(&["docs/a.md"]));
        assert!(problems.is_empty(), "{problems:?}");
        let problems = check_agents(text, &[], &exists_in(&["docs/a.md"]));
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("docs/b.md") && problems[0].contains("does not exist"));
    }

    #[test]
    fn planned_path_that_now_exists_or_is_not_mentioned_fails() {
        let text = "See `docs/a.md`.";
        let problems = check_agents(text, &["docs/a.md".into()], &exists_in(&["docs/a.md"]));
        assert!(problems.iter().any(|p| p.contains("remove it")), "{problems:?}");
        let problems = check_agents(text, &["docs/old.md".into()], &exists_in(&["docs/a.md"]));
        assert!(problems.iter().any(|p| p.contains("does not mention")), "{problems:?}");
    }

    #[test]
    fn length_limit() {
        let ok = "x".repeat(AGENTS_MAX_CHARS - 1);
        assert!(check_agents(&ok, &[], &|_| true).is_empty());
        let long = "x".repeat(AGENTS_MAX_CHARS);
        assert!(check_agents(&long, &[], &|_| true)[0].contains("under 5000"));
    }

    #[test]
    fn length_counts_characters_not_bytes() {
        let text = "é".repeat(AGENTS_MAX_CHARS - 1);
        assert!(text.len() > AGENTS_MAX_CHARS);
        assert!(check_agents(&text, &[], &|_| true).is_empty());
    }

    #[test]
    fn unknown_xtask_command_fails() {
        let problems =
            check_agents("`cargo xtask frobnicate`, `cargo xtask ci`, `cargo xtask perf --idle`", &[], &|_| true);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("frobnicate"));
    }

    #[test]
    fn toolchain_lines() {
        assert_eq!(toolchain_channel("[toolchain]\nchannel = \"1.97.1\"\n").as_deref(), Some("1.97.1"));
        assert_eq!(workflow_toolchain("env:\n  RUST_TOOLCHAIN: \"1.97.1\"\n").as_deref(), Some("1.97.1"));
        assert_eq!(toolchain_channel("[toolchain]\n"), None);
    }

    #[test]
    fn added_entries() {
        let base = entries("# c\nperf\nsize\n");
        let now = entries("perf\nharness # new\n");
        assert_eq!(added(&base, &now), ["harness"]);
    }
}
