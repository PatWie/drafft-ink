//! `cargo xtask perf`: the performance gate (`docs/performance.md` section 5, `perf/README.md`).
//!
//! Two tiers:
//!
//! * **Work counters** (the default, part of `cargo xtask ci`). Runs `tests/perf_work.rs` of every crate that has
//!   one, in release mode, at 1k, 10k and 100k elements. The tests assert on counted work and write a
//!   `ScenarioReport` for each scenario they drove into `target/perf/work`; the gate then checks the reports against
//!   the deterministic limits in `perf/budgets.json`. Nothing here depends on the speed of the machine.
//! * **Timing** (`--bench`, not part of `ci`: timing on a busy machine is noisy). Runs a curated set of criterion
//!   benches with short measurement times, reads their medians and checks them against the `bench` limits in
//!   `perf/budgets.json`, which have generous headroom.
//!
//! `--idle` runs only the idle and hover tests, the quick check that `AGENTS.md` names.
//!
//! xtask uses no workspace crate, so the budget arithmetic lives in the `perf_check` example of
//! `drafftink-profile`, which this gate runs.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

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
    if let Some(unknown) = args.iter().find(|a| !matches!(a.as_str(), "--bench" | "--idle")) {
        return Err(format!("perf: unknown argument `{unknown}`; use --bench or --idle"));
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
    let out = root.join("target/perf/work");
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

// ---- timing tier ------------------------------------------------------------------------------------------------

fn bench(root: &Path) -> Result<(), String> {
    let criterion = root.join("target/criterion");
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
    let out = root.join("target/perf/bench");
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
    fn pretty_picks_a_unit() {
        assert_eq!(pretty(12.0), "12.0 ns");
        assert_eq!(pretty(12_340.0), "12.34 us");
        assert_eq!(pretty(5_000_000.0), "5.00 ms");
    }
}
