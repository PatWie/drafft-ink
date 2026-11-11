//! Workspace tooling: `cargo xtask <gate>`.
//!
//! Every check is a [`gates::Gate`]. `cargo xtask ci` runs them all in order and prints a summary
//! table. See `docs/architecture.md` for what the layering gate enforces.
//!
//! xtask runs natively only, so it may read the wall clock (clippy.toml forbids that elsewhere
//! because `Instant::now` panics on wasm32).
#![allow(clippy::disallowed_methods)]

mod e2e;
mod exceptions;
mod gates;
mod harness;
mod layers;
#[cfg(test)]
mod parity;
mod perf;
mod size;
mod skips;
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
  wasm            cargo check and clippy -D warnings every crate at L0-L5, plus the wasm apps, for wasm32-unknown-unknown
  standards       AGENTS.md stays short and cites only files that exist; toolchain pins agree
  exceptions [--check]
                  docs/exceptions.md against the source: every row has a test that exists, every gesture command has a row,
                  every non-command write is listed (runs the registry half in crates/engine)
  deny            cargo deny: licences (AGPL-3.0-only compatible), advisories, bans, sources
  test [ARGS]     cargo nextest run (cargo test if nextest cannot be installed), plus doc tests; prints how many tests
                  skipped themselves (`SKIPPED:` lines); DRAFFTINK_REQUIRE_GPU=1 makes a missing GPU a failure
  fixtures        the legacy fixtures against the facts the old code wrote (crates/io/tests/facts.rs); the old app is
                  DRAFFTINK_OLD_REPO or found next to the main checkout; DRAFFTINK_REQUIRE_OLD=1 fails when it is missing
  harness         harness accounting: methods have reasons and callers, every family has scenarios (docs/testing.md)
  perf [--bench|--idle|--app [native|web|all]]
                  work-counter tests at 1k/10k/100k elements checked against perf/budgets.json;
                  --bench: curated criterion benches against the timing limits (not part of ci);
                  --idle: only the idle and hover tests (perf/README.md);
                  --app: the real app under Xvfb with real input, checked with timing against the
                  native/... or web/... budgets (needs a GPU; not part of ci); the web run builds the app with
                  --features scenarios itself; --no-build reuses the last build
  size [--no-build|--wasm FILE|--native]
                  builds the web app (scripts/build-web.sh), measures the wasm module and the fonts the page loads,
                  checks them against the `size` section of perf/budgets.json (xtask/src/size.rs);
                  --native: builds the shipped binaries (profile dist) and checks their sizes instead (not part of ci)
  ci              all of the above in order, with a summary table

e2e [--types|--web|--native|--no-build]
                  the browser (Playwright) and native (bridge) end-to-end suites on a private Xvfb; needs a display
                  and a GPU, so it is not part of ci (docs/testing.md); `cargo xtask e2e --help`

gates that are named but not written are listed in xtask/pending-gates.txt (none now)
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
        Some("e2e") => e2e::run_e2e(&root(), rest),
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
