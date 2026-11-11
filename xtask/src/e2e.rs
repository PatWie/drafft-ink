//! `cargo xtask e2e`: the two end-to-end suites that drive the real apps (`docs/testing.md`, "Browser and native e2e").
//!
//! * **Browser**: Playwright with TypeScript (`e2e/web`) drives `window.drafftink` in headed Chromium on Xvfb with
//!   WebGPU, against `drafftink-server --web-dir dist`.
//! * **Native**: Rust tests (`e2e/native`) start `drafftink --bridge` on the same Xvfb and drive it through the
//!   bridge client with the token file.
//!
//! The gate is not part of `cargo xtask ci`: it needs a display, a GPU (or the SwiftShader fallback), Node, Chromium
//! and about ten minutes. It has its own CI workflow (`.github/workflows/e2e.yml`).
//!
//! Steps, in order, each stopping the run when it fails:
//!
//! 1. Node and the npm packages of `e2e/web` (`npm ci`; Node from `PATH` or `~/.local/opt/node/bin`).
//! 2. The command types: `engine.commands` with schemas is dumped by `cargo run -p drafftink-e2e --example
//!    command_schemas` and `gen-command-types.ts --check` fails when `e2e/web/generated` is out of date
//!    (`--types` rewrites it).
//! 3. `tsc --noEmit` (the specs are typechecked against those types), then the accounting check of the harness,
//!    the families and the stories, and its self-test.
//! 4. Builds: the web app (`scripts/build-web.sh --profile release`), the server and the desktop app.
//! 5. A private Xvfb, then the browser suite, then the native suite. Both always run; the summary counts them.
//!
//! The summary (passed, failed, skipped per suite) is printed and written to `target/e2e/summary.json`.

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use serde_json::{Value, json};

use crate::{cargo, run};

const USAGE: &str = "\
usage: cargo xtask e2e [--types] [--web] [--native] [--no-build] [--display :N]

  --types       regenerate e2e/web/generated from the command registry and stop
  --web         only the browser suite       --native      only the native suite
  --no-build    use the web app (dist/), server and desktop app that are already built
  --display :N  use this X display instead of starting a private Xvfb

environment:
  DRAFFTINK_REQUIRE_GPU=1       a missing display, adapter or Chromium fails instead of skipping
  DRAFFTINK_E2E_ADAPTER=swiftshader   use Chromium's CPU adapter (slow; for machines without a GPU)
  DRAFFTINK_E2E_FILTER=text     run only the browser tests whose title matches (Playwright -g)
";

/// What one suite did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Counts {
    passed: u32,
    failed: u32,
    skipped: u32,
}

/// Options of the command line.
#[derive(Debug, Default)]
struct Options {
    types_only: bool,
    web: bool,
    native: bool,
    build: bool,
    display: Option<String>,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut o = Options { build: true, ..Options::default() };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--types" => o.types_only = true,
            "--web" => o.web = true,
            "--native" => o.native = true,
            "--no-build" => o.build = false,
            "--display" => o.display = Some(it.next().ok_or("--display needs a value like :99")?.clone()),
            "-h" | "--help" => return Err(USAGE.to_owned()),
            other => return Err(format!("unknown argument `{other}`\n\n{USAGE}")),
        }
    }
    if !o.web && !o.native {
        o.web = true;
        o.native = true;
    }
    Ok(o)
}

fn require_gpu() -> bool {
    std::env::var("DRAFFTINK_REQUIRE_GPU").is_ok_and(|v| !v.is_empty() && v != "0")
}

/// `PATH` with the user-local Node (`~/.local/opt/node/bin`) added, when it exists.
fn path_with_node() -> String {
    let path = std::env::var("PATH").unwrap_or_default();
    let local = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/opt/node/bin"));
    match local.filter(|p| p.join("node").exists()) {
        Some(dir) => format!("{}:{path}", dir.display()),
        None => path,
    }
}

fn node_command(program: &str, dir: &Path) -> Command {
    let mut c = Command::new(program);
    c.current_dir(dir).env("PATH", path_with_node());
    c
}

fn has_node() -> bool {
    node_command("node", Path::new("."))
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// A private Xvfb that dies with the guard.
struct Xvfb {
    child: Child,
    display: String,
}

impl Xvfb {
    fn start() -> Result<Self, String> {
        let mut child = Command::new("Xvfb")
            .args(["-displayfd", "1", "-screen", "0", "1600x1100x24", "+extension", "XTEST", "-nolisten", "tcp"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot start Xvfb ({e}); install it (apt install xvfb) or pass --display :N"))?;
        let stdout = child.stdout.take().ok_or("Xvfb has no stdout")?;
        let mut line = String::new();
        BufReader::new(stdout).read_line(&mut line).map_err(|e| format!("Xvfb did not report a display: {e}"))?;
        let number = line.trim();
        if number.is_empty() {
            return Err("Xvfb exited before reporting a display".to_owned());
        }
        Ok(Self { child, display: format!(":{number}") })
    }
}

impl Drop for Xvfb {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Steps 1 to 3: packages, types, typecheck, accounting.
fn prepare(root: &Path, types_only: bool) -> Result<(), String> {
    let web = root.join("e2e/web");
    if !has_node() {
        return Err("Node.js is not installed. Put a Node tarball in ~/.local/opt/node (https://nodejs.org/dist/v22.23.4/node-v22.23.4-linux-x64.tar.xz), or install node 22 on PATH".to_owned());
    }
    if !web.join("node_modules/.package-lock.json").exists() {
        run(node_command("npm", &web).args(["ci", "--no-audit", "--no-fund"]))?;
    }
    let generated = web.join("generated");
    let json_path = generated.join("commands.json");
    let dump = cargo(root)
        .args(["run", "--quiet", "-p", "drafftink-e2e", "--example", "command_schemas"])
        .output()
        .map_err(|e| format!("cannot run the schema dump: {e}"))?;
    if !dump.status.success() {
        return Err(format!("the schema dump failed:\n{}", String::from_utf8_lossy(&dump.stderr)));
    }
    fs::create_dir_all(&generated).map_err(|e| e.to_string())?;
    fs::write(&json_path, &dump.stdout).map_err(|e| e.to_string())?;
    if types_only {
        return run(node_command("npx", &web).args(["tsx", "scripts/gen-command-types.ts"]));
    }
    run(node_command("npx", &web).args(["tsx", "scripts/gen-command-types.ts", "--check"]))?;
    run(node_command("npx", &web).args(["tsc", "--noEmit"]))?;
    run(node_command("npx", &web).args(["tsx", "scripts/accounting.ts", "--self-test"]))?;
    run(node_command("npx", &web).args(["tsx", "scripts/accounting.ts"]))
}

fn build(root: &Path) -> Result<(), String> {
    run(Command::new("bash")
        .arg(root.join("scripts/build-web.sh"))
        .args(["--profile", "release", "--no-precompress"])
        .current_dir(root))?;
    run(cargo(root).args(["build", "--release", "-p", "drafftink-server", "-p", "drafftink"]))
}

/// The browser suite. Reads Playwright's JSON report for the counts.
fn run_web(root: &Path, display: &str) -> Result<Counts, String> {
    let web = root.join("e2e/web");
    let report = web.join("results/report.json");
    let _ = fs::remove_file(&report);
    let mut cmd = node_command("npx", &web);
    cmd.args(["playwright", "test"]).env("DISPLAY", display).env("DRAFFTINK_E2E", "1");
    if let Ok(filter) = std::env::var("DRAFFTINK_E2E_FILTER") {
        cmd.args(["-g", &filter]);
    }
    let status = run(&mut cmd);
    let text =
        fs::read_to_string(&report).map_err(|e| format!("Playwright wrote no report ({}): {e}", report.display()))?;
    let value: Value = serde_json::from_str(&text).map_err(|e| format!("cannot read the Playwright report: {e}"))?;
    let counts = playwright_counts(&value);
    status.map(|()| counts).or_else(|e| if counts.failed > 0 { Ok(counts) } else { Err(e) })
}

/// Passed, failed and skipped tests of a Playwright JSON report (`stats`).
fn playwright_counts(report: &Value) -> Counts {
    let n = |k: &str| {
        report
            .pointer(&format!("/stats/{k}"))
            .and_then(Value::as_u64)
            .map_or(0, |v| u32::try_from(v).unwrap_or(u32::MAX))
    };
    Counts { passed: n("expected") + n("flaky"), failed: n("unexpected"), skipped: n("skipped") }
}

/// The native suite: `cargo test -p drafftink-e2e` with the output parsed.
fn run_native(root: &Path, display: &str) -> Result<Counts, String> {
    println!("$ cargo test -p drafftink-e2e -- --nocapture --test-threads=4");
    let mut child = cargo(root)
        .args(["test", "-p", "drafftink-e2e", "--", "--nocapture", "--test-threads=4"])
        .env("DISPLAY", display)
        .env("DRAFFTINK_E2E", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("cannot run cargo test: {e}"))?;
    let mut out = String::new();
    child.stdout.take().ok_or("no stdout")?.read_to_string(&mut out).map_err(|e| e.to_string())?;
    let status = child.wait().map_err(|e| e.to_string())?;
    print!("{out}");
    let counts = cargo_test_counts(&out);
    if status.success() || counts.failed > 0 {
        Ok(counts)
    } else {
        Err(format!("cargo test failed with {status} before running the native suite"))
    }
}

/// Sums the `test result:` lines of `cargo test`, and counts the `E2E-SKIP` lines as skipped (such a test passes).
fn cargo_test_counts(output: &str) -> Counts {
    let mut c = Counts::default();
    for line in output.lines() {
        if line.starts_with("E2E-SKIP") {
            c.skipped += 1;
        }
        let Some(rest) = line.strip_prefix("test result: ") else { continue };
        let field = |name: &str| {
            rest.split(';')
                .find_map(|part| {
                    part.trim().trim_start_matches("ok. ").trim_start_matches("FAILED. ").strip_suffix(name)
                })
                .and_then(|n| n.trim().parse::<u32>().ok())
                .unwrap_or(0)
        };
        c.passed += field("passed");
        c.failed += field("failed");
    }
    // A test that skipped passed; keep it in `skipped` only.
    c.passed = c.passed.saturating_sub(c.skipped);
    c
}

fn summarize(root: &Path, rows: &[(&str, Option<Counts>)]) -> Result<bool, String> {
    println!("\n{:<10} {:>7} {:>7} {:>8}", "suite", "passed", "failed", "skipped");
    let mut ok = true;
    let mut entries = serde_json::Map::new();
    for (name, counts) in rows {
        match counts {
            Some(c) => {
                println!("{name:<10} {:>7} {:>7} {:>8}", c.passed, c.failed, c.skipped);
                ok &= c.failed == 0;
                entries.insert(
                    (*name).to_owned(),
                    json!({ "passed": c.passed, "failed": c.failed, "skipped": c.skipped }),
                );
            }
            None => {
                println!("{name:<10} did not run");
                ok = false;
            }
        }
    }
    let dir = root.join("target/e2e");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    fs::write(
        dir.join("summary.json"),
        serde_json::to_string_pretty(&Value::Object(entries)).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(ok)
}

/// `cargo xtask e2e`.
pub fn run_e2e(root: &Path, args: &[String]) -> Result<(), String> {
    let o = parse(args)?;
    prepare(root, o.types_only)?;
    if o.types_only {
        return Ok(());
    }
    if o.build {
        build(root)?;
    }
    let private;
    let display = match o.display.or_else(|| std::env::var("DRAFFTINK_E2E_DISPLAY").ok()) {
        Some(d) => d,
        None => match Xvfb::start() {
            Ok(x) => {
                private = x;
                private.display.clone()
            }
            Err(e) if !require_gpu() => {
                println!("e2e skipped: {e}");
                return Ok(());
            }
            Err(e) => return Err(e),
        },
    };
    println!("e2e: display {display}");
    let web = if o.web { Some(run_web(root, &display)) } else { None };
    let native = if o.native { Some(run_native(root, &display)) } else { None };
    let rows = [
        ("browser", web.as_ref().map(|r| r.as_ref().ok().copied())),
        ("native", native.as_ref().map(|r| r.as_ref().ok().copied())),
    ];
    let shown: Vec<(&str, Option<Counts>)> = rows.iter().filter_map(|(n, r)| r.map(|c| (*n, c))).collect();
    let ok = summarize(root, &shown)?;
    for err in [web, native].into_iter().flatten().filter_map(Result::err) {
        eprintln!("e2e: {err}");
    }
    if ok { Ok(()) } else { Err("the end-to-end suites did not pass".to_owned()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_flag_runs_both_suites_and_builds() {
        let o = parse(&[]).unwrap();
        assert!(o.web && o.native && o.build && !o.types_only);
    }

    #[test]
    fn flags_pick_suites_and_unknown_flags_are_errors() {
        let o = parse(&["--web".to_owned(), "--no-build".to_owned()]).unwrap();
        assert!(o.web && !o.native && !o.build);
        assert!(parse(&["--nope".to_owned()]).unwrap_err().contains("unknown argument"));
        assert!(parse(&["--display".to_owned()]).is_err());
    }

    #[test]
    fn the_playwright_report_gives_counts() {
        let report = json!({ "stats": { "expected": 40, "unexpected": 2, "skipped": 1, "flaky": 1 } });
        assert_eq!(playwright_counts(&report), Counts { passed: 41, failed: 2, skipped: 1 });
        assert_eq!(playwright_counts(&json!({})), Counts::default());
    }

    #[test]
    fn cargo_test_output_gives_counts_and_skips() {
        let out = "running 6 tests\nE2E-SKIP a: no display\ntest result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.0s\n\
                   test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.0s\n";
        assert_eq!(cargo_test_counts(out), Counts { passed: 7, failed: 1, skipped: 1 });
    }
}
