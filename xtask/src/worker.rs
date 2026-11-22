//! `cargo xtask worker`: the Cloudflare Worker host of the relay (`apps/drafftink-worker`).
//!
//! The gate needs Node, so it is a job of its own in CI (`.github/workflows/ci.yml`, job `worker`). It
//!
//! 1. checks the tools: Node 22 or newer, `wasm-bindgen` in the version the workspace pins, and the
//!    `wasm32-unknown-unknown` target of the Rust toolchain (each missing tool is named with how to get it);
//! 2. runs `npm ci`, `npm run typecheck` and `npm test` in `apps/drafftink-worker`;
//! 3. runs the transport parity suite of the server against a local `wrangler dev`
//!    (`cargo nextest run -p drafftink-server parity` with `DRAFFTINK_PARITY_WRANGLER=1`), and fails when any
//!    test of it skipped itself: with the variable set, the suite has to run.
//!
//! ```text
//! cargo xtask worker                 all three steps
//! cargo xtask worker --check-tools   only the tool checks
//! ```
//!
//! Nothing is deployed, and no Cloudflare login is used.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::tools::target_dir;
use crate::{cargo, run, skips, tools, wasm};

/// The Worker's folder, relative to the workspace root.
const WORKER_DIR: &str = "apps/drafftink-worker";
/// The oldest Node major version the Worker's dependencies support (`engines` in its `package.json`).
const MIN_NODE_MAJOR: u32 = 22;
/// The environment variable that makes the parity suite start `wrangler dev` instead of skipping that transport.
const PARITY_ENV: &str = "DRAFFTINK_PARITY_WRANGLER";
/// The test filter of the parity suite.
const PARITY_FILTER: &str = "parity";

/// What the command line asked for.
#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    /// Run only the tool checks.
    check_tools: bool,
}

fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut options = Options::default();
    for arg in args {
        match arg.as_str() {
            "--check-tools" => options.check_tools = true,
            other => {
                return Err(format!("unknown argument `{other}`; `cargo xtask worker` takes only `--check-tools`"));
            }
        }
    }
    Ok(options)
}

/// Runs the gate. The notes are what the `ci` summary shows next to the result.
pub fn run_gate(root: &Path, args: &[String]) -> Result<Vec<String>, String> {
    let options = parse_args(args)?;
    check_tools(root)?;
    if options.check_tools {
        println!("worker: the tools are installed");
        return Ok(Vec::new());
    }
    let worker_dir = root.join(WORKER_DIR);
    for npm_args in [&["ci", "--no-audit", "--no-fund"][..], &["run", "typecheck"], &["test"]] {
        run(Command::new("npm").args(npm_args).current_dir(&worker_dir))?;
    }
    parity(root).map(|note| vec![note])
}

// ---- the tool checks ----

/// Checks every tool and reports all the problems at once, so one run tells what to install.
fn check_tools(root: &Path) -> Result<(), String> {
    let cargo_toml = std::fs::read_to_string(root.join("Cargo.toml")).map_err(|e| format!("Cargo.toml: {e}"))?;
    let pinned = pinned_wasm_bindgen(&cargo_toml)
        .ok_or("Cargo.toml has no `wasm-bindgen = \"=x.y.z\"` line, so the version to install is unknown")?;
    let problems: Vec<String> = [
        node_problem(probe("node", &["--version"], root).as_deref()),
        wasm_bindgen_problem(probe("wasm-bindgen", &["--version"], root).as_deref(), &pinned),
        wasm_target_problem(root),
    ]
    .into_iter()
    .flatten()
    .collect();
    if problems.is_empty() { Ok(()) } else { Err(problems.join("\n")) }
}

/// The standard output of `program args`, or `None` when it cannot start or fails.
fn probe(program: &str, args: &[&str], dir: &Path) -> Option<String> {
    let output = Command::new(program).args(args).current_dir(dir).stderr(Stdio::null()).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The version that `Cargo.toml` pins for the `wasm-bindgen` crate (`wasm-bindgen = "=0.2.130"`), which the
/// installed command line tool must equal.
fn pinned_wasm_bindgen(cargo_toml: &str) -> Option<String> {
    cargo_toml.lines().find_map(|line| {
        let value = line.strip_prefix("wasm-bindgen")?.trim_start().strip_prefix('=')?.trim_start();
        let version = value.strip_prefix('"')?.strip_prefix('=')?.split('"').next()?;
        (!version.is_empty() && version.chars().all(|c| c.is_ascii_digit() || c == '.')).then(|| version.to_owned())
    })
}

/// Why the output of `node --version` does not show Node 22 or newer, or `None` when it does.
fn node_problem(version_output: Option<&str>) -> Option<String> {
    let install = "Install it from https://nodejs.org (or with your system's package manager or nvm).";
    let Some(output) = version_output else {
        return Some(format!("Node.js {MIN_NODE_MAJOR} or newer is needed, and `node` was not found. {install}"));
    };
    match parse_node_major(output) {
        Ok(major) if major >= MIN_NODE_MAJOR => None,
        Ok(major) => Some(format!(
            "Node.js {MIN_NODE_MAJOR} or newer is needed, and `node` is version {major}. {install} \
             If several are installed, put the newer one first on PATH."
        )),
        Err(e) => Some(format!("Node.js {MIN_NODE_MAJOR} or newer is needed: {e}. {install}")),
    }
}

/// The major version in the output of `node --version`, which is `v22.12.0` (a `v`, then three numbers).
fn parse_node_major(output: &str) -> Result<u32, String> {
    let text = output.trim();
    let numbers = text.strip_prefix('v').ok_or_else(|| format!("`node --version` printed `{text}`"))?;
    let mut parts = numbers.split('.').map(|part| part.parse::<u32>());
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(Ok(major)), Some(Ok(_)), Some(Ok(_)), None) => Ok(major),
        _ => Err(format!("`node --version` printed `{text}`, not a version like v22.12.0")),
    }
}

/// Why the installed `wasm-bindgen` is not the pinned version, or `None` when it is.
fn wasm_bindgen_problem(version_output: Option<&str>, pinned: &str) -> Option<String> {
    let install = format!("cargo install wasm-bindgen-cli --version {pinned} --locked");
    let Some(output) = version_output else {
        return Some(format!("`wasm-bindgen` was not found. Install it with: {install}"));
    };
    match parse_wasm_bindgen_version(output) {
        Ok(found) if found == pinned => None,
        Ok(found) => Some(format!(
            "`wasm-bindgen` is version {found}, and Cargo.toml pins {pinned}; the two must match. \
             Install the pinned one with: {install} --force"
        )),
        Err(e) => Some(format!("{e}. Install the pinned version with: {install} --force")),
    }
}

/// The version in the output of `wasm-bindgen --version`: `wasm-bindgen 0.2.130 (abc1234)`.
fn parse_wasm_bindgen_version(output: &str) -> Result<String, String> {
    let text = output.trim();
    let mut words = text.split_whitespace();
    match (words.next(), words.next()) {
        (Some("wasm-bindgen"), Some(version))
            if version.split('.').count() == 3
                && version.split('.').all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit())) =>
        {
            Ok(version.to_owned())
        }
        _ => Err(format!("`wasm-bindgen --version` printed `{text}`, not a version like `wasm-bindgen 0.2.130`")),
    }
}

/// Whether the Rust toolchain that cargo uses here has the standard library for `wasm32-unknown-unknown`.
fn wasm_target_problem(root: &Path) -> Option<String> {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let libdir = probe(&rustc, &["--print", "target-libdir", "--target", wasm::TARGET], root);
    let installed = libdir.is_some_and(|dir| has_std_library(Path::new(dir.trim())));
    (!installed)
        .then(|| format!("The Rust target {0} is not installed. Install it with: rustup target add {0}", wasm::TARGET))
}

/// Whether a target library folder holds the standard library (`libstd-<hash>.rlib`).
fn has_std_library(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .is_ok_and(|entries| entries.flatten().any(|e| e.file_name().to_string_lossy().starts_with("libstd-")))
}

// ---- the parity run ----

/// The junit file of the `ci` profile (`.config/nextest.toml`), which keeps the output of passing tests.
fn junit_path(root: &Path) -> PathBuf {
    target_dir(root).join("nextest/ci/junit.xml")
}

/// Runs the parity suite against `wrangler dev`. Returns the note for the summary.
fn parity(root: &Path) -> Result<String, String> {
    tools::ensure(root, "nextest", "cargo-nextest", None)
        .map_err(|e| format!("cargo-nextest is needed (cargo install cargo-nextest --locked): {e}"))?;
    let junit = junit_path(root);
    // A stale file from an earlier run would be counted again.
    let _ = std::fs::remove_file(&junit);
    let outcome = run(cargo(root)
        .args(["nextest", "run", "--profile", "ci", "-p", "drafftink-server", PARITY_FILTER])
        .env(PARITY_ENV, "1"));
    let skipped = std::fs::read_to_string(&junit).map(|text| skips::count(&text)).unwrap_or_default();
    let note = skips::describe(&skipped);
    println!("worker: {note}");
    outcome?;
    skip_verdict(&skipped).map(|()| note)
}

/// The parity suite must not skip when the gate asked for `wrangler dev`.
fn skip_verdict(skipped: &[skips::SkipGroup]) -> Result<(), String> {
    let total: usize = skipped.iter().map(|g| g.count).sum();
    if total == 0 {
        Ok(())
    } else {
        Err(format!(
            "{PARITY_ENV}=1 is set, so the parity suite must run against `wrangler dev`, but it {}. \
             Look for the `{}` lines above for the reason",
            skips::describe(skipped),
            skips::MARKER
        ))
    }
}

// ---- the Makefile must not relax Access in a deploy ----

/// The targets whose recipe lines contain `needle`. A target starts at a line `name:` (not an assignment) at the
/// beginning of a line; its recipe is the tab-indented lines after it.
#[cfg(test)]
fn recipes_containing(makefile: &str, needle: &str) -> std::collections::BTreeSet<String> {
    let mut found = std::collections::BTreeSet::new();
    let mut target: Option<&str> = None;
    for line in makefile.lines() {
        if let Some(recipe) = line.strip_prefix('\t') {
            if let Some(name) = target
                && recipe.contains(needle)
            {
                found.insert(name.to_owned());
            }
        } else if !line.starts_with('#') && !line.trim().is_empty() {
            let head = line.split(':').next().unwrap_or_default();
            let is_rule = line.contains(':')
                && !line.contains(":=")
                && !line.contains("?=")
                && !head.contains(['=', ' ', '$'])
                && !head.is_empty();
            target = is_rule.then_some(head);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_versions_are_read() {
        assert_eq!(parse_node_major("v22.12.0\n"), Ok(22));
        assert_eq!(parse_node_major("  v18.19.1  "), Ok(18));
        assert_eq!(parse_node_major("v100.0.0"), Ok(100));
    }

    #[test]
    fn odd_node_output_is_an_error_and_never_a_panic() {
        for text in [
            "",
            "\n",
            "v",
            "v22",
            "v22.x",
            "v22.12",
            "v22.12.0.1",
            "22.12.0",
            "V22.12.0",
            "v-1.0.0",
            "v22..0",
            "node",
            "v99999999999.0.0",
            "v22.12.0-nightly",
            "garbage \u{0}\u{1F600}",
        ] {
            assert!(parse_node_major(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn node_22_or_newer_passes_and_older_or_missing_fails_with_how_to_install() {
        assert_eq!(node_problem(Some("v22.0.0")), None);
        assert_eq!(node_problem(Some("v24.1.0")), None);
        let old = node_problem(Some("v18.19.1")).unwrap_or_default();
        assert!(old.contains("22 or newer") && old.contains("version 18") && old.contains("nodejs.org"), "{old}");
        let missing = node_problem(None).unwrap_or_default();
        assert!(missing.contains("not found") && missing.contains("nodejs.org"), "{missing}");
        let garbage = node_problem(Some("hello")).unwrap_or_default();
        assert!(garbage.contains("hello"), "{garbage}");
    }

    #[test]
    fn wasm_bindgen_versions_are_read() {
        assert_eq!(parse_wasm_bindgen_version("wasm-bindgen 0.2.130\n").as_deref(), Ok("0.2.130"));
        assert_eq!(parse_wasm_bindgen_version("wasm-bindgen 0.2.130 (abc1234 2026-11-01)").as_deref(), Ok("0.2.130"));
    }

    #[test]
    fn odd_wasm_bindgen_output_is_an_error_and_never_a_panic() {
        for text in [
            "",
            "wasm-bindgen",
            "wasm-bindgen 0.2",
            "wasm-bindgen 0.2.x",
            "wasm-bindgen 0.2.130.1",
            "wasm-bindgen ..",
            "wasm-bindgen-cli 0.2.130",
            "0.2.130",
            "error: no such command",
            "\u{0}\u{1F600}",
        ] {
            assert!(parse_wasm_bindgen_version(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn only_the_pinned_wasm_bindgen_passes() {
        assert_eq!(wasm_bindgen_problem(Some("wasm-bindgen 0.2.130 (x)"), "0.2.130"), None);
        let other = wasm_bindgen_problem(Some("wasm-bindgen 0.2.129"), "0.2.130").unwrap_or_default();
        assert!(other.contains("0.2.129") && other.contains("pins 0.2.130") && other.contains("--force"), "{other}");
        let missing = wasm_bindgen_problem(None, "0.2.130").unwrap_or_default();
        assert!(missing.contains("cargo install wasm-bindgen-cli --version 0.2.130 --locked"), "{missing}");
        assert!(wasm_bindgen_problem(Some("???"), "0.2.130").is_some());
    }

    #[test]
    fn the_pin_is_read_from_cargo_toml() {
        let text = "[workspace.dependencies]\nwasm-bindgen-futures = \"0.4\"\nwasm-bindgen = \"=0.2.130\" # equal\n";
        assert_eq!(pinned_wasm_bindgen(text).as_deref(), Some("0.2.130"));
        for odd in ["", "wasm-bindgen = \"0.2.130\"", "wasm-bindgen = \"=\"", "wasm-bindgen = \"=x\"", "wasm-bindgen ="]
        {
            assert_eq!(pinned_wasm_bindgen(odd), None, "{odd:?}");
        }
    }

    #[test]
    fn the_workspace_pins_a_wasm_bindgen_version() {
        let text = std::fs::read_to_string(crate::root().join("Cargo.toml")).unwrap_or_default();
        assert!(pinned_wasm_bindgen(&text).is_some(), "Cargo.toml needs a `wasm-bindgen = \"=x.y.z\"` line");
    }

    #[test]
    fn a_target_folder_has_the_standard_library_or_not() {
        let dir = std::env::temp_dir().join(format!("xtask-worker-libdir-{}", std::process::id()));
        assert!(!has_std_library(&dir), "a folder that does not exist");
        assert!(std::fs::create_dir_all(&dir).is_ok());
        assert!(!has_std_library(&dir), "an empty folder");
        assert!(std::fs::write(dir.join("libstd-0123abcd.rlib"), b"").is_ok());
        assert!(has_std_library(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn arguments_are_checked() {
        assert_eq!(parse_args(&[]), Ok(Options::default()));
        assert_eq!(parse_args(&["--check-tools".to_owned()]), Ok(Options { check_tools: true }));
        assert!(parse_args(&["--nope".to_owned()]).is_err());
    }

    #[test]
    fn a_skipped_parity_test_fails_the_gate() {
        assert_eq!(skip_verdict(&[]), Ok(()));
        let skipped = [skips::SkipGroup { kind: "other", count: 2 }];
        let message = skip_verdict(&skipped).unwrap_err();
        assert!(message.contains("skipped 2") && message.contains("DRAFFTINK_PARITY_WRANGLER"), "{message}");
    }
    #[test]
    fn the_recipe_finder_reads_targets_and_recipes() {
        let makefile =
            "VAR = x\n\na: ## text\n\t@echo one\n\techo two\nb: export Y = 1\nb:\n\techo two\n# c:\n\techo more\n";
        assert_eq!(recipes_containing(makefile, "two").into_iter().collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(recipes_containing(makefile, "one").into_iter().collect::<Vec<_>>(), ["a"]);
        assert!(recipes_containing(makefile, "VAR").is_empty(), "an assignment is no recipe");
    }

    /// `make cf-dev` runs the private environment without Access, for local use. No other recipe may do it: a deployed
    /// Worker without the check is open to everyone who finds its address.
    #[test]
    fn only_cf_dev_can_switch_access_off() {
        let path = crate::root().join("Makefile");
        let makefile = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(makefile.contains("ACCESS_REQUIRED:false"), "{} has no cf-dev switch to guard", path.display());
        for needle in ["ACCESS_REQUIRED:false", "CF_DEV_VARS", "ACCESS_REQUIRED=false"] {
            let owners: Vec<String> = recipes_containing(&makefile, needle).into_iter().collect();
            assert!(
                owners.iter().all(|t| t == "cf-dev"),
                "`{needle}` is in the recipe of {owners:?}; only cf-dev may have it"
            );
        }
        assert_eq!(recipes_containing(&makefile, "CF_DEV_VARS").len(), 1, "cf-dev uses the switch");
    }
}
