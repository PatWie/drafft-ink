//! The gates and the `ci` runner.
//!
//! To add a gate: add a [`Gate`] variant, name it in `name`/`from_name`/`ALL`, and give it an arm in
//! `run`. A gate that is not written yet is a stub ([`Gate::is_stub`]); it passes only while
//! `xtask/pending-gates.txt` lists it, and the list is rejected once a listed gate is implemented.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::{cargo, layers, run, standards, tools, wasm};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Gate {
    Fmt,
    Clippy,
    Layers,
    Wasm,
    Standards,
    Deny,
    Test,
    // Pending: reserved names with no implementation yet.
    Perf,
    Size,
    Fixtures,
    Harness,
}

impl Gate {
    /// Every gate, in the order `ci` runs them. Cheap checks first.
    pub const ALL: &'static [Gate] = &[
        Gate::Fmt,
        Gate::Standards,
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
        matches!(self, Gate::Perf | Gate::Size | Gate::Harness)
    }

    fn run(self, root: &Path, args: &[String]) -> Result<(), String> {
        match self {
            Gate::Fmt => run(cargo(root).args(["fmt", "--all", "--check"])),
            Gate::Clippy => run(cargo(root).args([
                "clippy",
                "--workspace",
                "--all-targets",
                "--all-features",
                "--",
                "-D",
                "warnings",
            ])),
            Gate::Layers => layers::run(root, args.iter().any(|a| a == "--list")),
            Gate::Wasm => wasm::run(root),
            Gate::Standards => standards::run(root),
            Gate::Deny => deny(root),
            Gate::Test => test(root, args),
            Gate::Fixtures => fixtures(root),
            Gate::Perf | Gate::Size | Gate::Harness => Err(format!("{}: no implementation", self.name())),
        }
    }
}

/// Fixture facts: recompute every legacy fixture's facts with the new code and compare with the
/// checked-in `.facts.json` (crates/io/tests/facts.rs). When the old app is checked out next to
/// this repo, also regenerate the facts from the old code and compare bytes (the facts are derived
/// from the fixtures by a tool, never typed by hand).
fn fixtures(root: &Path) -> Result<(), String> {
    run(cargo(root).args(["test", "-p", "drafftink-io", "--test", "facts"]))?;
    let regen = root.join("tests/fixtures/generator/regen.sh");
    let old = root.join("../drafft-ink/crates");
    if regen.exists() && old.exists() {
        run(std::process::Command::new("bash").arg(regen).arg("--check").current_dir(root))
    } else {
        eprintln!("fixtures: old app not found next to the repo; skipped the regeneration check");
        Ok(())
    }
}

fn deny(root: &Path) -> Result<(), String> {
    tools::ensure(root, "deny", "cargo-deny", Some("0.20.2"))?;
    // Duplicate versions are warnings (deny.toml); everything else fails.
    run(cargo(root).args(["deny", "--locked", "--all-features", "check", "advisories", "bans", "licenses", "sources"]))
}

fn test(root: &Path, args: &[String]) -> Result<(), String> {
    match tools::ensure(root, "nextest", "cargo-nextest", None) {
        Ok(()) => {
            run(cargo(root).args(["nextest", "run", "--workspace", "--all-features", "--no-tests=pass"]).args(args))?;
            // nextest does not run doc tests.
            run(cargo(root).args(["test", "--workspace", "--all-features", "--doc"]))
        }
        Err(e) => {
            eprintln!("warning: {e}\nfalling back to cargo test");
            run(cargo(root).args(["test", "--workspace", "--all-features"]).args(args))
        }
    }
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

fn execute(gate: Gate, root: &Path, args: &[String]) -> Outcome {
    if gate.is_stub() {
        println!("{}: not yet implemented", gate.name());
        return match read_pending(root) {
            Ok(pending) => stub_outcome(gate, &pending),
            Err(e) => Outcome::Fail(e),
        };
    }
    match gate.run(root, args) {
        Ok(()) => Outcome::Pass,
        Err(e) => Outcome::Fail(e),
    }
}

/// `cargo xtask <gate>`: success unless the gate fails. A listed stub succeeds.
pub fn run_one(gate: Gate, root: &Path, args: &[String]) -> Result<(), String> {
    match execute(gate, root, args) {
        Outcome::Pass | Outcome::Pending => Ok(()),
        Outcome::Fail(e) => Err(e),
    }
}

/// `cargo xtask ci`: every gate in order (a failure does not stop the rest), then a table.
pub fn run_ci(root: &Path) -> Result<(), String> {
    let mut results: Vec<(Gate, Outcome, Duration)> = Vec::new();
    for &gate in Gate::ALL {
        println!("\n==> {}", gate.name());
        let start = Instant::now();
        let outcome = execute(gate, root, &[]);
        results.push((gate, outcome, start.elapsed()));
    }
    print!("\n{}", summary(&results));
    let failed: Vec<&str> =
        results.iter().filter(|(_, o, _)| matches!(o, Outcome::Fail(_))).map(|(g, _, _)| g.name()).collect();
    if failed.is_empty() { Ok(()) } else { Err(format!("failed: {}", failed.join(", "))) }
}

/// The summary table. Failure messages follow the table.
pub fn summary(results: &[(Gate, Outcome, Duration)]) -> String {
    let mut out = String::from("gate        result   time\n");
    out.push_str("----------  -------  --------\n");
    for (gate, outcome, took) in results {
        out.push_str(&format!("{:<10}  {:<7}  {:>7.1}s\n", gate.name(), outcome.label(), took.as_secs_f64()));
    }
    let total: Duration = results.iter().map(|(_, _, d)| *d).sum();
    out.push_str(&format!("{:<10}  {:<7}  {:>7.1}s\n", "total", "", total.as_secs_f64()));
    for (gate, outcome, _) in results {
        if let Outcome::Fail(message) = outcome {
            out.push_str(&format!("\n{}: {message}\n", gate.name()));
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
        let pending = parse_pending("# header\n\nperf # later\nsize\n").unwrap();
        assert_eq!(pending, BTreeSet::from([Gate::Perf, Gate::Size]));
    }

    #[test]
    fn pending_file_rejects_unknown_and_implemented_gates() {
        assert!(parse_pending("bogus\n").unwrap_err().contains("not a gate"));
        assert!(parse_pending("layers\n").unwrap_err().contains("only shrinks"));
    }

    #[test]
    fn stub_passes_only_when_listed() {
        let pending = BTreeSet::from([Gate::Perf]);
        assert_eq!(stub_outcome(Gate::Perf, &pending), Outcome::Pending);
        assert!(matches!(stub_outcome(Gate::Size, &pending), Outcome::Fail(m) if m.contains("pending-gates.txt")));
    }

    #[test]
    fn summary_lists_each_gate_and_failure_text() {
        let rows = vec![
            (Gate::Fmt, Outcome::Pass, Duration::from_millis(1200)),
            (Gate::Layers, Outcome::Fail("bad edge".into()), Duration::from_secs(2)),
            (Gate::Perf, Outcome::Pending, Duration::ZERO),
        ];
        let text = summary(&rows);
        assert!(text.contains("fmt") && text.contains("pass") && text.contains("1.2s"));
        assert!(text.contains("FAIL") && text.contains("pending"));
        assert!(text.contains("layers: bad edge"));
    }

    #[test]
    fn stubs_are_the_pending_gate_names() {
        let stubs: Vec<_> = Gate::ALL.iter().filter(|g| g.is_stub()).map(|g| g.name()).collect();
        assert_eq!(stubs, ["perf", "size", "harness"]);
    }
}
