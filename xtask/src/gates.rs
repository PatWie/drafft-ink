//! The gates and the `ci` runner.
//!
//! To add a gate: add a [`Gate`] variant, name it in `name`/`from_name`/`ALL`, and give it an arm in
//! `run`. A gate that is not written yet is a stub ([`Gate::is_stub`]); it passes only while
//! `xtask/pending-gates.txt` lists it, and the list is rejected once a listed gate is implemented.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::{cargo, exceptions, harness, layers, perf, run, size, skips, standards, tools, wasm};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Gate {
    Fmt,
    Clippy,
    Layers,
    Wasm,
    Standards,
    Exceptions,
    Deny,
    Test,
    Perf,
    // Pending: reserved names with no implementation yet.
    Size,
    Fixtures,
    Harness,
}

impl Gate {
    /// Every gate, in the order `ci` runs them. Cheap checks first.
    pub const ALL: &'static [Gate] = &[
        Gate::Fmt,
        Gate::Standards,
        Gate::Exceptions,
        Gate::Layers,
        Gate::Clippy,
        Gate::Wasm,
        Gate::Deny,
        Gate::Test,
        Gate::Perf,
        Gate::Size,
        Gate::Fixtures,
        Gate::Harness,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Gate::Fmt => "fmt",
            Gate::Clippy => "clippy",
            Gate::Layers => "layers",
            Gate::Wasm => "wasm",
            Gate::Standards => "standards",
            Gate::Exceptions => "exceptions",
            Gate::Deny => "deny",
            Gate::Test => "test",
            Gate::Perf => "perf",
            Gate::Size => "size",
            Gate::Fixtures => "fixtures",
            Gate::Harness => "harness",
        }
    }

    pub fn from_name(name: &str) -> Option<Gate> {
        Gate::ALL.iter().copied().find(|g| g.name() == name)
    }

    /// True while the gate has no implementation. When you write one, drop it from this match.
    pub fn is_stub(self) -> bool {
        false
    }

    /// Runs the gate. The notes are facts the summary table shows next to the result (for example how many tests
    /// skipped themselves).
    fn run(self, root: &Path, args: &[String]) -> Result<Vec<String>, String> {
        let plain = |result: Result<(), String>| result.map(|()| Vec::new());
        match self {
            Gate::Fmt => plain(run(cargo(root).args(["fmt", "--all", "--check"]))),
            Gate::Clippy => plain(clippy(root)),
            Gate::Layers => plain(layers::run(root, args.iter().any(|a| a == "--list"))),
            Gate::Wasm => plain(wasm::run(root)),
            Gate::Standards => plain(standards::run(root)),
            Gate::Exceptions => plain(exceptions::run_gate(root, args)),
            Gate::Deny => plain(deny(root)),
            Gate::Test => test(root, args),
            Gate::Fixtures => fixtures(root),
            Gate::Harness => plain(harness::run_gate(root)),
            Gate::Perf => plain(perf::run_gate(root, args)),
            Gate::Size => plain(size::run_gate(root, args)),
        }
    }
}

/// Clippy twice. First on every target of the workspace with `clippy.toml`. Then on the library code of every
/// crate below L6, natively and for wasm32, with `xtask/clippy-below-l6/clippy.toml`, which adds the file system,
/// the network, threads, processes and UI types to the disallowed lists (those crates do no I/O of their own).
fn clippy(root: &Path) -> Result<(), String> {
    let mut workspace = cargo(root);
    workspace.args(["clippy", "--workspace", "--all-targets", "--all-features"]);
    for krate in layers::vendored() {
        workspace.args(["--exclude", krate]);
    }
    run(workspace.args(["--", "-D", "warnings"]))?;
    let strict_config = root.join("xtask/clippy-below-l6");
    for target in [None, Some(wasm::TARGET)] {
        let mut command = cargo(root);
        command.arg("clippy").env("CLIPPY_CONF_DIR", &strict_config);
        if let Some(target) = target {
            command.args(["--target", target]);
        }
        for package in layers::libraries_below_l6() {
            command.args(["-p", package]);
        }
        run(command.args(["--lib", "--all-features", "--", "-D", "warnings"]))?;
    }
    Ok(())
}

/// Where the old app is checked out, for the gates that compare with it: `DRAFFTINK_OLD_REPO`, else the
/// `drafft-ink` directory next to this checkout, else next to the main worktree (a worktree such as
/// `drafftink-wt/<task>` has the old app beside the main checkout, not beside itself).
fn old_repo(root: &Path) -> Option<PathBuf> {
    let has_core = |p: &Path| p.join("crates/drafftink-core").is_dir();
    if let Some(from_env) = std::env::var_os("DRAFFTINK_OLD_REPO").filter(|v| !v.is_empty()) {
        // An explicit choice is used as given; a wrong one is reported by the caller as missing.
        return Some(PathBuf::from(from_env)).filter(|p| has_core(p));
    }
    let beside = |dir: &Path| dir.parent().map(|p| p.join("drafft-ink"));
    let main_worktree = tools::git(root, &["worktree", "list", "--porcelain"])
        .and_then(|out| out.lines().find_map(|l| l.strip_prefix("worktree ").map(PathBuf::from)));
    [beside(root), main_worktree.as_deref().and_then(beside)].into_iter().flatten().find(|p| has_core(p))
}

/// Fixture facts: recompute every legacy fixture's facts with the new code and compare with the
/// checked-in `.facts.json` (crates/io/tests/facts.rs). Then regenerate the facts from the old code and
/// compare bytes (the facts are derived from the fixtures by a tool, never typed by hand).
///
/// Without the old app the second half cannot run. That is printed as a `SKIPPED:` line and counted in the
/// summary, and it fails when `DRAFFTINK_REQUIRE_OLD=1` (the CI jobs that have the old app set it).
fn fixtures(root: &Path) -> Result<Vec<String>, String> {
    run(cargo(root).args(["test", "-p", "drafftink-io", "--test", "facts"]))?;
    let regen = root.join("tests/fixtures/generator/regen.sh");
    match old_repo(root) {
        Some(old) if regen.exists() => {
            println!("fixtures: old app at {}", old.display());
            let mut command = std::process::Command::new("bash");
            command.arg(regen).arg("--check").current_dir(root).env("DRAFFTINK_OLD_REPO", &old);
            run(&mut command)?;
            Ok(vec!["regenerated from the old app".to_owned()])
        }
        found => {
            let reason = if found.is_some() {
                "tests/fixtures/generator/regen.sh is missing"
            } else {
                "the old app was not found (set DRAFFTINK_OLD_REPO, or check it out next to the repo)"
            };
            println!("{} fixtures: regeneration check: {reason}", skips::MARKER);
            if skips::env_flag("DRAFFTINK_REQUIRE_OLD") {
                return Err(format!("the regeneration check could not run and DRAFFTINK_REQUIRE_OLD is set: {reason}"));
            }
            Ok(vec!["SKIPPED regeneration check (no old app)".to_owned()])
        }
    }
}

fn deny(root: &Path) -> Result<(), String> {
    tools::ensure(root, "deny", "cargo-deny", Some("0.20.2"))?;
    // Duplicate versions are warnings (deny.toml); everything else fails.
    run(cargo(root).args(["deny", "--locked", "--all-features", "check", "advisories", "bans", "licenses", "sources"]))
}

fn test(root: &Path, args: &[String]) -> Result<Vec<String>, String> {
    match tools::ensure(root, "nextest", "cargo-nextest", None) {
        Ok(()) => {
            let junit = root.join("target/nextest/ci/junit.xml");
            // A stale file from an earlier run would be counted again.
            let _ = std::fs::remove_file(&junit);
            let outcome = run(cargo(root)
                .args(["nextest", "run", "--profile", "ci", "--workspace", "--all-features", "--no-tests=pass"])
                .args(args));
            let groups = std::fs::read_to_string(&junit).map(|text| skips::count(&text)).unwrap_or_default();
            let note = skips::describe(&groups);
            println!("test: {note}");
            outcome?;
            // nextest does not run doc tests.
            run(cargo(root).args(["test", "--workspace", "--all-features", "--doc"]))?;
            without_schemas(root)?;
            without_hand_fonts(root)?;
            Ok(vec![note])
        }
        Err(e) => {
            eprintln!("warning: {e}\nfalling back to cargo test");
            run(cargo(root).args(["test", "--workspace", "--all-features"]).args(args))?;
            without_schemas(root).and_then(|()| without_hand_fonts(root)).map(|()| Vec::new())
        }
    }
}

/// The engine as the web build has it, without the `schemas` feature: `--all-features` always turns the feature on,
/// so the behaviour without it (empty schemas, the refusals while the table is missing, the table that fills them)
/// has a test target of its own, `crates/engine/tests/without_schemas.rs`.
fn without_schemas(root: &Path) -> Result<(), String> {
    run(cargo(root).args(["test", "-p", "drafftink-engine", "--no-default-features", "--test", "without_schemas"]))
}

/// The typesetter as a package from crates.io has it, without the feature `hand-fonts`: `--all-features` always turns
/// the feature on, so the Noto Sans stand-in for GelPen, GelPen Serif and Vanilla Extract has a run of its own. A
/// single `-p` build does not unify features with the other workspace members, which is the point.
fn without_hand_fonts(root: &Path) -> Result<(), String> {
    run(cargo(root).args(["test", "-p", "drafftink-typeset"]))
}

/// How a gate ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    /// A stub that `xtask/pending-gates.txt` lists.
    Pending,
    Fail(String),
}

impl Outcome {
    fn label(&self) -> &'static str {
        match self {
            Outcome::Pass => "pass",
            Outcome::Pending => "pending",
            Outcome::Fail(_) => "FAIL",
        }
    }
}

/// Parse `pending-gates.txt`: one gate name per line, `#` starts a comment.
pub fn parse_pending(text: &str) -> Result<BTreeSet<Gate>, String> {
    let mut out = BTreeSet::new();
    for (number, line) in text.lines().enumerate() {
        let name = line.split('#').next().unwrap_or_default().trim();
        if name.is_empty() {
            continue;
        }
        let gate = Gate::from_name(name)
            .ok_or_else(|| format!("xtask/pending-gates.txt:{}: `{name}` is not a gate", number + 1))?;
        if !gate.is_stub() {
            return Err(format!(
                "xtask/pending-gates.txt:{}: `{name}` is implemented; take it off the list (the list only shrinks)",
                number + 1
            ));
        }
        out.insert(gate);
    }
    Ok(out)
}

/// What running a stub gate does, given the pending list.
pub fn stub_outcome(gate: Gate, pending: &BTreeSet<Gate>) -> Outcome {
    if pending.contains(&gate) {
        Outcome::Pending
    } else {
        Outcome::Fail(format!(
            "`{}` is not yet implemented and is not listed in xtask/pending-gates.txt; the list only shrinks, so write the gate",
            gate.name()
        ))
    }
}

pub fn read_pending(root: &Path) -> Result<BTreeSet<Gate>, String> {
    let path = root.join("xtask/pending-gates.txt");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_pending(&text)
}

/// Runs a gate. The notes are what its summary row says besides the result.
fn execute(gate: Gate, root: &Path, args: &[String]) -> (Outcome, Vec<String>) {
    if gate.is_stub() {
        println!("{}: not yet implemented", gate.name());
        let outcome = match read_pending(root) {
            Ok(pending) => stub_outcome(gate, &pending),
            Err(e) => Outcome::Fail(e),
        };
        return (outcome, Vec::new());
    }
    match gate.run(root, args) {
        Ok(notes) => (Outcome::Pass, notes),
        Err(e) => (Outcome::Fail(e), Vec::new()),
    }
}

/// `cargo xtask <gate>`: success unless the gate fails. A listed stub succeeds.
pub fn run_one(gate: Gate, root: &Path, args: &[String]) -> Result<(), String> {
    match execute(gate, root, args).0 {
        Outcome::Pass | Outcome::Pending => Ok(()),
        Outcome::Fail(e) => Err(e),
    }
}

/// One row of the `ci` summary.
pub struct Row {
    gate: Gate,
    outcome: Outcome,
    took: Duration,
    /// Facts about the run, such as how many tests skipped themselves.
    notes: Vec<String>,
}

impl Row {
    #[cfg(test)]
    fn new(gate: Gate, outcome: Outcome, took: Duration) -> Self {
        Self { gate, outcome, took, notes: Vec::new() }
    }
}

/// `cargo xtask ci`: every gate in order (a failure does not stop the rest), then a table.
pub fn run_ci(root: &Path) -> Result<(), String> {
    let mut results: Vec<Row> = Vec::new();
    for &gate in Gate::ALL {
        println!("\n==> {}", gate.name());
        let start = Instant::now();
        let (outcome, notes) = execute(gate, root, &[]);
        results.push(Row { gate, outcome, took: start.elapsed(), notes });
    }
    print!("\n{}", summary(&results));
    let failed: Vec<&str> =
        results.iter().filter(|r| matches!(r.outcome, Outcome::Fail(_))).map(|r| r.gate.name()).collect();
    if failed.is_empty() { Ok(()) } else { Err(format!("failed: {}", failed.join(", "))) }
}

/// The summary table. Notes follow the time of their row, and failure messages follow the table.
pub fn summary(results: &[Row]) -> String {
    let mut out = String::from("gate        result   time\n");
    out.push_str("----------  -------  --------\n");
    for r in results {
        let notes = if r.notes.is_empty() { String::new() } else { format!("  {}", r.notes.join("; ")) };
        out.push_str(&format!(
            "{:<10}  {:<7}  {:>7.1}s{notes}\n",
            r.gate.name(),
            r.outcome.label(),
            r.took.as_secs_f64()
        ));
    }
    let total: Duration = results.iter().map(|r| r.took).sum();
    out.push_str(&format!("{:<10}  {:<7}  {:>7.1}s\n", "total", "", total.as_secs_f64()));
    for r in results {
        if let Outcome::Fail(message) = &r.outcome {
            out.push_str(&format!("\n{}: {message}\n", r.gate.name()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_are_unique() {
        let names: BTreeSet<&str> = Gate::ALL.iter().map(|g| g.name()).collect();
        assert_eq!(names.len(), Gate::ALL.len());
        for &gate in Gate::ALL {
            assert_eq!(Gate::from_name(gate.name()), Some(gate));
        }
        assert_eq!(Gate::from_name("nope"), None);
    }

    #[test]
    fn pending_file_parses_comments_and_blank_lines() {
        let pending = parse_pending("# header\n\n   # indented comment\n").unwrap();
        assert_eq!(pending, BTreeSet::new());
    }

    #[test]
    fn pending_file_rejects_unknown_and_implemented_gates() {
        assert!(parse_pending("bogus\n").unwrap_err().contains("not a gate"));
        assert!(parse_pending("layers\n").unwrap_err().contains("only shrinks"));
        assert!(parse_pending("perf\n").unwrap_err().contains("only shrinks"), "perf is written");
        assert!(parse_pending("size # wasm size budget\n").unwrap_err().contains("only shrinks"), "size is written");
    }

    #[test]
    fn stub_passes_only_when_listed() {
        let pending = BTreeSet::from([Gate::Size]);
        assert_eq!(stub_outcome(Gate::Size, &pending), Outcome::Pending);
        assert!(matches!(stub_outcome(Gate::Perf, &pending), Outcome::Fail(m) if m.contains("pending-gates.txt")));
    }

    #[test]
    fn summary_lists_each_gate_and_failure_text() {
        let mut test = Row::new(Gate::Test, Outcome::Pass, Duration::from_secs(3));
        test.notes.push("skipped 2 (gpu 2)".into());
        let rows = vec![
            Row::new(Gate::Fmt, Outcome::Pass, Duration::from_millis(1200)),
            Row::new(Gate::Layers, Outcome::Fail("bad edge".into()), Duration::from_secs(2)),
            Row::new(Gate::Size, Outcome::Pending, Duration::ZERO),
            test,
        ];
        let text = summary(&rows);
        assert!(text.contains("3.0s  skipped 2 (gpu 2)"), "{text}");
        assert!(text.contains("fmt") && text.contains("pass") && text.contains("1.2s"));
        assert!(text.contains("FAIL") && text.contains("pending"));
        assert!(text.contains("layers: bad edge"));
    }

    #[test]
    fn stubs_are_the_pending_gate_names() {
        let stubs: Vec<_> = Gate::ALL.iter().filter(|g| g.is_stub()).map(|g| g.name()).collect();
        assert!(stubs.is_empty(), "every gate is written: {stubs:?}");
    }
}
