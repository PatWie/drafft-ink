//! Workspace tooling: `cargo xtask <gate>`.
//!
//! Every check is a [`gates::Gate`]. `cargo xtask ci` runs them all in order and prints a summary
//! table. See `docs/architecture.md` for what the layering gate enforces.
//!
//! xtask runs natively only, so it may read the wall clock (clippy.toml forbids that elsewhere
//! because `Instant::now` panics on wasm32).
#![allow(clippy::disallowed_methods)]

mod gates;
mod harness;
mod layers;
mod perf;
mod standards;
mod tools;
mod wasm;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use gates::Gate;

const USAGE: &str = "\
usage: cargo xtask <gate> [args]

gates:
  fmt             cargo fmt --all --check
  clippy          cargo clippy on all targets and features, -D warnings
  layers [--list] enforce the crate layering (docs/architecture.md); --list prints the assignments
  wasm            cargo check every crate at L0-L5, plus the wasm apps, for wasm32-unknown-unknown
  standards       AGENTS.md stays short and cites only files that exist; toolchain pins agree
  deny            cargo deny: licences (AGPL-3.0-only compatible), advisories, bans, sources
  test [ARGS]     cargo nextest run (cargo test if nextest cannot be installed), plus doc tests
  fixtures        the legacy fixtures against the facts the old code wrote (crates/io/tests/facts.rs)
  harness         harness accounting: methods have reasons and callers, every family has scenarios (docs/testing.md)
  perf [--bench|--idle]
                  work-counter tests at 1k/10k/100k elements checked against perf/budgets.json;
                  --bench: curated criterion benches against the timing limits (not part of ci);
                  --idle: only the idle and hover tests (perf/README.md)
  ci              all of the above in order, then the pending gates, with a summary table

pending gates (not yet implemented; each must be listed in xtask/pending-gates.txt to pass):
  size
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let name = args.first().map(String::as_str);
    let rest = args.get(1..).unwrap_or_default();
    let result = match name {
        None | Some("-h" | "--help" | "help") => {
            print!("{USAGE}");
            Ok(())
        }
        Some("ci") => gates::run_ci(&root()),
        Some(other) => match Gate::from_name(other) {
            Some(gate) => gates::run_one(gate, &root(), rest),
            None => Err(format!("unknown command `{other}`\n\n{USAGE}")),
        },
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Workspace root: the parent of the xtask crate.
pub fn root() -> PathBuf {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    here.parent().unwrap_or(here).to_path_buf()
}

/// A `cargo` command that runs in the workspace root.
pub fn cargo(root: &Path) -> Command {
    let mut command = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    command.current_dir(root);
    command
}

/// Run a command with inherited output. The error names the command and its exit status.
pub fn run(command: &mut Command) -> Result<(), String> {
    let shown = format!(
        "{} {}",
        command.get_program().to_string_lossy(),
        command.get_args().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" ")
    );
    println!("$ {shown}");
    match command.status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("`{shown}` failed with {status}")),
        Err(e) => Err(format!("cannot run `{shown}`: {e}")),
    }
}
