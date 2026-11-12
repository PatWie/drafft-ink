//! `cargo xtask perf`: the performance gate (`docs/performance.md` section 5, `perf/README.md`).
//!
//! Three tiers:
//!
//! * **Work counters** (the default, part of `cargo xtask ci`). Runs `tests/perf_work.rs` of every crate that has
//!   one, in release mode, at 1k, 10k and 100k elements. The tests assert on counted work and write a
//!   `ScenarioReport` for each scenario they drove into `target/perf/work`; the gate then checks the reports against
//!   the deterministic limits in `perf/budgets.json`. Nothing here depends on the speed of the machine.
//! * **Timing** (`--bench`, not part of `ci`: timing on a busy machine is noisy). Runs a curated set of criterion
//!   benches with short measurement times, reads their medians and checks them against the `bench` limits in
//!   `perf/budgets.json`, which have generous headroom.
//!
//! * **App scenarios** (`--app [native|web]`, not part of `ci`: it needs a GPU and a display). Runs the real app
//!   under a private Xvfb with real input (`scripts/perf_scenarios.py`) and checks the reports, timing included,
//!   against the `native/...` or `web/...` entries of `perf/budgets.json`.
//!
//! `--idle` runs only the idle and hover tests, the quick check that `AGENTS.md` names.
//!
//! xtask uses no workspace crate, so the budget arithmetic lives in the `perf_check` example of
//! `drafftink-profile`, which this gate runs.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::tools::target_dir;
use crate::{cargo, run};

const BUDGETS: &str = "perf/budgets.json";
const PERF_OUT_ENV: &str = "DRAFFTINK_PERF_OUT";

/// A criterion benchmark target and the benchmarks of it that the timing tier runs.
struct Target {
    package: &'static str,
    bench: &'static str,
    /// Criterion full ids (`group/function/parameter`). The filter regex is built from them.
    ids: &'static [&'static str],
    /// Environment for the run (the doc benches choose their sizes this way).
    env: &'static [(&'static str, &'static str)],
}

/// The curated set. Each bench is cheap and stands for a rule of `docs/performance.md`: hit testing and culling stay
/// off the size of the scene, a local edit and a remote update cost one element, a frame costs its visible elements.
const TARGETS: &[Target] = &[
    Target {
        package: "drafftink-geom",
        bench: "spatial",
        ids: &["geom/index_query_point/10000", "geom/index_query_rect_800x600/10000", "geom/update_one/10000"],
        env: &[],
    },
    Target {
        package: "drafftink-canvas",
        bench: "canvas",
        ids: &[
            "canvas/hit/10000",
            "canvas/hit/100000",
            "canvas/index_apply_one_move/10000",
            "canvas/canvas_hover_move/10000",
        ],
        env: &[],
    },
    Target {
        package: "drafftink-render",
        bench: "compose",
        ids: &[
            "compose/zoomed_in_warm/10k",
            "compose/zoomed_in_warm/100k",
            "compose/pan_warm/100k",
            "compose/drag_one_element/10k",
        ],
        env: &[],
    },
    Target {
        package: "drafftink-doc",
        bench: "store",
        ids: &["local_edit_commit/move_one_element/10000", "apply_remote_single_element/move/10000"],
        env: &[("DOC_BENCH_SIZES", "10000")],
    },
    Target {
        package: "drafftink-edit",
        bench: "arrows_binding",
        ids: &["update_bound_arrows_1k_arrows_1k_shapes_100_moved/indexed"],
        env: &[],
    },
    Target {
        package: "drafftink-sketch",
        bench: "sketch",
        ids: &["rect_outline_300x200", "hachure_fill_300x200"],
        env: &[],
    },
    Target { package: "drafftink-typeset", bench: "typeset", ids: &["text_1k_labels/warm"], env: &[] },
    Target { package: "drafftink-profile", bench: "span_overhead", ids: &["span/on", "always_on/count"], env: &[] },
];

pub fn run_gate(root: &Path, args: &[String]) -> Result<(), String> {
    let flag = |name: &str| args.iter().any(|a| a == name);
    if let Some(at) = args.iter().position(|a| a == "--app") {
        let others = args.iter().enumerate().any(|(i, a)| a.starts_with("--") && i != at && a != "--no-build");
        if others {
            return Err(
                "perf: --app is a run of its own; it takes `native`, `web` or `all` after it, and --no-build".into()
            );
        }
        let which = args.get(at + 1).map(String::as_str).filter(|a| !a.starts_with("--"));
        return app(root, which, !flag("--no-build"));
    }
    if let Some(unknown) = args.iter().find(|a| !matches!(a.as_str(), "--bench" | "--idle")) {
        return Err(format!("perf: unknown argument `{unknown}`; use --bench, --idle or --app"));
    }
    if flag("--bench") && flag("--idle") {
        return Err("perf: --bench and --idle are separate runs".into());
    }
    if flag("--bench") {
        return bench(root);
    }
    if flag("--idle") {
        return idle(root);
    }
    work(root)
}

/// The crates that have a `tests/perf_work.rs`, as package names.
fn work_packages(root: &Path) -> Result<Vec<String>, String> {
    let dir = root.join("crates");
    let entries = fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut out = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if !path.join("tests/perf_work.rs").is_file() {
            continue;
        }
        let manifest = path.join("Cargo.toml");
        let text = fs::read_to_string(&manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
        out.push(package_name(&text).ok_or_else(|| format!("{}: no package name", manifest.display()))?);
    }
    out.sort();
    Ok(out)
}

/// The `name = "..."` of the `[package]` table.
fn package_name(manifest: &str) -> Option<String> {
    let mut in_package = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
        } else if in_package && let Some(rest) = line.strip_prefix("name") {
            return rest.trim_start().strip_prefix('=')?.trim().trim_matches('"').to_owned().into();
        }
    }
    None
}

fn perf_check(root: &Path) -> std::process::Command {
    let mut c = cargo(root);
    c.args(["run", "--quiet", "-p", "drafftink-profile", "--example", "perf_check", "--"]);
    c
}

fn work(root: &Path) -> Result<(), String> {
    let packages = work_packages(root)?;
    if packages.is_empty() {
        return Err("perf: no crate has tests/perf_work.rs".into());
    }
    let out = target_dir(root).join("perf/work");
    reset_dir(&out)?;
    run(perf_check(root).args(["validate", BUDGETS]))?;
    let mut test = cargo(root);
    test.args(["test", "--release", "--test", "perf_work"]);
    for p in &packages {
        test.args(["-p", p]);
    }
    test.env(PERF_OUT_ENV, &out);
    run(&mut test)?;
    run(perf_check(root).args(["check", BUDGETS]).arg(&out).args(["--require-all", "--skip=bench"]))
}

fn idle(root: &Path) -> Result<(), String> {
    let mut test = cargo(root);
    test.args(["test", "--release", "-p", "drafftink-testkit", "--test", "perf_work", "--", "idle", "hover"]);
    run(&mut test)
}

fn reset_dir(dir: &Path) -> Result<(), String> {
    if dir.exists() {
        fs::remove_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))
}

// ---- app scenarios -----------------------------------------------------------------------------------------------

/// Where the scenarios of one platform run.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Platform {
    Native,
    Web,
}

impl Platform {
    fn name(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Web => "web",
        }
    }
}

/// The scenario keys that `perf/budgets.json` budgets for `platform` (`native/pan@10k` gives `pan@10k`), in file order.
fn budgeted_keys(budgets: &Value, platform: Platform) -> Vec<String> {
    let prefix = format!("{}/", platform.name());
    budgets
        .get("scenarios")
        .and_then(Value::as_object)
        .map(|scenarios| scenarios.keys().filter_map(|k| k.strip_prefix(&prefix)).map(str::to_owned).collect())
        .unwrap_or_default()
}

/// The Python that has Xlib, Pillow and Playwright: `$PERF_PY`, else `~/.venvs/perf/bin/python`, else `python3`.
fn python() -> String {
    if let Ok(py) = std::env::var("PERF_PY") {
        return py;
    }
    let venv = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".venvs/perf/bin/python"));
    match venv {
        Some(p) if p.is_file() => p.display().to_string(),
        _ => "python3".to_owned(),
    }
}

/// Where the web build with the generated scenes goes. It is not `dist/`: that one is the shipped build, and the
/// scenes are not for shipping.
fn scenario_dist(root: &Path) -> PathBuf {
    target_dir(root).join("perf/web-dist")
}

/// What the web scenarios need: the web app built with `--features scenarios`, and the server that serves it.
fn build_web(root: &Path, dist: &Path) -> Result<(), String> {
    let mut script = std::process::Command::new("bash");
    script.current_dir(root).env("DIST_DIR", dist).arg("scripts/build-web.sh").args([
        "--features",
        "scenarios",
        "--no-precompress",
        "--no-fallback",
    ]);
    run(&mut script)?;
    let mut server = cargo(root);
    server.args(["build", "--release", "-p", "drafftink-server"]);
    run(&mut server)
}

fn app(root: &Path, which: Option<&str>, build: bool) -> Result<(), String> {
    let platforms = match which {
        None | Some("native") => vec![Platform::Native],
        Some("web") => vec![Platform::Web],
        Some("all") => vec![Platform::Native, Platform::Web],
        Some(other) => return Err(format!("perf: `--app {other}`: use native, web or all")),
    };
    let text = fs::read_to_string(root.join(BUDGETS)).map_err(|e| format!("{BUDGETS}: {e}"))?;
    let budgets: Value = serde_json::from_str(&text).map_err(|e| format!("{BUDGETS}: {e}"))?;
    run(perf_check(root).args(["validate", BUDGETS]))?;
    let release = target_dir(root).join("release");
    let mut failures = Vec::new();
    for platform in platforms {
        let keys = budgeted_keys(&budgets, platform);
        if keys.is_empty() {
            return Err(format!("perf: {BUDGETS} has no `{}/...` entries to run", platform.name()));
        }
        let out = target_dir(root).join("perf").join(format!("app-{}", platform.name()));
        reset_dir(&out)?;
        let mut driver = std::process::Command::new(python());
        driver.current_dir(root).arg("scripts/perf_scenarios.py").arg(platform.name()).args(&keys);
        match platform {
            Platform::Native => {
                if build {
                    let mut build = cargo(root);
                    // `--perf-scenario` is compiled in only for this build; the shipped binary has no way to swap the document.
                    build.args(["build", "--release", "-p", "drafftink", "--features", "scenarios,hand-fonts"]);
                    run(&mut build)?;
                }
                driver.arg("--binary").arg(release.join(exe("drafftink")));
            }
            Platform::Web => {
                let dist = scenario_dist(root);
                if build {
                    build_web(root, &dist)?;
                } else if !dist.join("pkg").is_dir() {
                    return Err(format!(
                        "perf: --no-build, but {} has no web build. Run `cargo xtask perf --app web` once without it.",
                        dist.display()
                    ));
                }
                driver.arg("--dist").arg(&dist).arg("--server").arg(release.join(exe("drafftink-server")));
            }
        }
        driver.arg("--out").arg(&out);
        run(&mut driver)?;
        let prefix = format!("--prefix={}/", platform.name());
        let check = run(perf_check(root).args(["check", BUDGETS]).arg(&out).args([
            "--timing",
            "--require-all",
            prefix.as_str(),
        ]));
        if let Err(e) = check {
            failures.push(format!("{}: {e}", platform.name()));
        }
    }
    if failures.is_empty() { Ok(()) } else { Err(failures.join("; ")) }
}

/// `name` with the platform's executable suffix.
fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

// ---- timing tier ------------------------------------------------------------------------------------------------

fn bench(root: &Path) -> Result<(), String> {
    let criterion = target_dir(root).join("criterion");
    for id in TARGETS.iter().flat_map(|t| t.ids) {
        // A result left by an earlier run must not pass for this one.
        let dir = criterion.join(id);
        if dir.exists() {
            fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
    }
    for target in TARGETS {
        let filter = format!("^({})$", target.ids.iter().map(|i| regex_escape(i)).collect::<Vec<_>>().join("|"));
        let mut bench = cargo(root);
        bench.args(["bench", "-p", target.package, "--bench", target.bench, "--", &filter]);
        bench.args(["--warm-up-time", "0.3", "--measurement-time", "1", "--sample-size", "10", "--noplot"]);
        bench.envs(target.env.iter().copied());
        run(&mut bench)?;
    }
    let mut medians: Vec<(String, f64)> = Vec::new();
    for id in TARGETS.iter().flat_map(|t| t.ids) {
        medians.push(((*id).to_owned(), median_ns(&criterion.join(id))?));
    }
    println!("\nmedians:");
    for (id, ns) in &medians {
        println!("  {id:<64} {}", pretty(*ns));
    }
    let out = target_dir(root).join("perf/bench");
    reset_dir(&out)?;
    let report = out.join("bench.json");
    let mut make = perf_check(root);
    make.arg("bench-report").arg(&report);
    for (id, ns) in &medians {
        make.arg(format!("{id}={}", ns / 1000.0));
    }
    run(&mut make)?;
    run(perf_check(root).args(["check", BUDGETS]).arg(&out).args(["--timing", "--require-all", "--only=bench"]))
}

/// `a.b/c` as a regex that matches only itself.
fn regex_escape(s: &str) -> String {
    s.chars()
        .flat_map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '/' { vec![c] } else { vec!['\\', c] })
        .collect()
}

/// The median of the last run of one benchmark, in nanoseconds.
fn median_ns(dir: &Path) -> Result<f64, String> {
    let path: PathBuf = dir.join("new/estimates.json");
    let text = fs::read_to_string(&path)
        .map_err(|e| format!("{}: {e}. The bench did not run; check the id in xtask/src/perf.rs", path.display()))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    v.pointer("/median/point_estimate")
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("{}: no median.point_estimate", path.display()))
}

fn pretty(ns: f64) -> String {
    if ns >= 1e6 {
        format!("{:.2} ms", ns / 1e6)
    } else if ns >= 1e3 {
        format!("{:.2} us", ns / 1e3)
    } else {
        format!("{ns:.1} ns")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_package_name() {
        let text =
            "[package]\nname = \"drafftink-doc\"\nversion.workspace = true\n\n[dependencies]\nname = \"not this\"\n";
        assert_eq!(package_name(text).as_deref(), Some("drafftink-doc"));
        assert_eq!(package_name("[lib]\nname = \"x\"\n"), None);
    }

    #[test]
    fn escapes_regex_metacharacters_but_not_the_path_separator() {
        assert_eq!(regex_escape("a/b_c.d+e"), "a/b_c\\.d\\+e");
    }

    #[test]
    fn every_bench_id_has_group_and_name_and_a_unique_place() {
        let mut seen = std::collections::BTreeSet::new();
        for id in TARGETS.iter().flat_map(|t| t.ids) {
            assert!(seen.insert(*id), "{id} is listed twice");
            assert!(!id.starts_with('/') && !id.ends_with('/'), "{id}");
        }
    }

    #[test]
    fn app_keys_are_the_entries_of_one_platform() {
        let budgets: Value = serde_json::from_str(
            r#"{"scenarios":{"idle":{},"pan@1k":{},"native/idle":{},"native/pan@1k":{},"web/idle":{}}}"#,
        )
        .unwrap();
        assert_eq!(budgeted_keys(&budgets, Platform::Native), ["idle", "pan@1k"]);
        assert_eq!(budgeted_keys(&budgets, Platform::Web), ["idle"]);
    }

    #[test]
    fn the_web_scenario_build_does_not_replace_the_shipped_dist() {
        let root = Path::new("/work/repo");
        let dist = scenario_dist(root);
        assert_ne!(dist, root.join("dist"));
        assert!(dist.starts_with(target_dir(root)), "{}", dist.display());
        assert_eq!(exe("drafftink"), format!("drafftink{}", std::env::consts::EXE_SUFFIX));
    }

    #[test]
    fn pretty_picks_a_unit() {
        assert_eq!(pretty(12.0), "12.0 ns");
        assert_eq!(pretty(12_340.0), "12.34 us");
        assert_eq!(pretty(5_000_000.0), "5.00 ms");
    }
}
