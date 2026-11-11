//! Checks scenario reports against the budgets file. `cargo xtask perf` runs it; run it by hand to see why a
//! gate failed.
//!
//! ```text
//! cargo run -p drafftink-profile --example perf_check -- validate perf/budgets.json
//! cargo run -p drafftink-profile --example perf_check -- check perf/budgets.json target/perf/work [--timing] [--require-all] [--only=a,b] [--skip=a,b]
//! cargo run -p drafftink-profile --example perf_check -- bench-report target/perf/bench/bench.json <id>=<microseconds>...
//! ```
//!
//! `bench-report` writes the report of the timing tier: scenario `bench`, one `external.<id>_us` number per
//! benchmark (`canvas/hit/10000` becomes `external.canvas.hit.10000_us`).
//!
//! `check` reads every `*.json` ScenarioReport in the directory and prints one line per violation. Without
//! `--timing` only the deterministic metrics are checked (counters, gauges, redraws), which hold on every machine.
//! With `--require-all` a budgeted scenario that has no report is a failure; without it, it is a note. `--only` and
//! `--skip` take scenario keys (`bench`, `idle`, `pan@10k`) and narrow the budgets to or away from them.
//! The exit status is 1 when anything fails and 2 when the files cannot be read.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use drafftink_profile::{Budgets, PerfStats, ScenarioReport};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["validate", budgets] => validate(Path::new(budgets)),
        ["check", budgets, dir, flags @ ..] => check(Path::new(budgets), Path::new(dir), flags),
        ["bench-report", out, medians @ ..] => bench_report(Path::new(out), medians),
        _ => Err(Failure::Usage),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Violations(n)) => {
            eprintln!("perf_check: {n} problem(s)");
            ExitCode::from(1)
        }
        Err(Failure::Usage) => {
            eprintln!(
                "usage: perf_check validate <budgets.json>\n       perf_check check <budgets.json> <reports-dir> [--timing] [--require-all] [--only=a,b] [--skip=a,b]\n       perf_check bench-report <out.json> <id>=<microseconds>..."
            );
            ExitCode::from(2)
        }
        Err(Failure::Read(message)) => {
            eprintln!("perf_check: {message}");
            ExitCode::from(2)
        }
    }
}

enum Failure {
    Usage,
    Read(String),
    Violations(usize),
}

fn load_budgets(path: &Path) -> Result<Budgets, Failure> {
    let text = std::fs::read_to_string(path).map_err(|e| Failure::Read(format!("{}: {e}", path.display())))?;
    Budgets::from_json(&text).map_err(|e| Failure::Read(format!("{}: {e}", path.display())))
}

fn bench_report(out: &Path, medians: &[&str]) -> Result<(), Failure> {
    let mut report = ScenarioReport::new("bench", None, PerfStats::empty());
    report.meta.insert("platform".into(), "native".into());
    report.meta.insert("source".into(), "criterion medians".into());
    for pair in medians {
        let (id, us) =
            pair.split_once('=').ok_or_else(|| Failure::Read(format!("`{pair}` is not <id>=<microseconds>")))?;
        let us: f64 = us.parse().map_err(|_| Failure::Read(format!("`{us}` is not a number")))?;
        report.external.insert(format!("{}_us", id.replace('/', ".")), us);
    }
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Failure::Read(format!("{}: {e}", dir.display())))?;
    }
    std::fs::write(out, report.to_json_pretty()).map_err(|e| Failure::Read(format!("{}: {e}", out.display())))?;
    println!("{}: {} medians", out.display(), report.external.len());
    Ok(())
}

fn validate(path: &Path) -> Result<(), Failure> {
    let budgets = load_budgets(path)?;
    let metrics: usize = budgets.scenarios.values().map(|m| m.len()).sum();
    println!("{}: {} scenarios, {metrics} limits", path.display(), budgets.scenarios.len());
    Ok(())
}

fn read_reports(dir: &Path) -> Result<Vec<(PathBuf, ScenarioReport)>, Failure> {
    let entries = std::fs::read_dir(dir).map_err(|e| Failure::Read(format!("{}: {e}", dir.display())))?;
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path).map_err(|e| Failure::Read(format!("{}: {e}", path.display())))?;
            let report =
                ScenarioReport::from_json(&text).map_err(|e| Failure::Read(format!("{}: {e}", path.display())))?;
            Ok((path, report))
        })
        .collect()
}

fn check(budgets_path: &Path, dir: &Path, flags: &[&str]) -> Result<(), Failure> {
    let timing = flags.contains(&"--timing");
    let require_all = flags.contains(&"--require-all");
    let list = |name: &str| -> Option<Vec<&str>> {
        flags.iter().find_map(|f| f.strip_prefix(name)).map(|keys| keys.split(',').collect())
    };
    let (only, skip) = (list("--only="), list("--skip="));
    let mut budgets = load_budgets(budgets_path)?;
    budgets.scenarios.retain(|key, _| {
        only.as_ref().is_none_or(|keys| keys.contains(&key.as_str()))
            && skip.as_ref().is_none_or(|keys| !keys.contains(&key.as_str()))
    });
    let reports = read_reports(dir)?;
    let mut problems = 0;
    let mut seen = BTreeSet::new();
    for (path, report) in &reports {
        let key = report.key();
        if !budgets.has(&key) {
            println!("note: {key} ({}) has no budget", path.display());
            continue;
        }
        seen.insert(key.clone());
        let violations = budgets.check_filtered(report, timing);
        for v in &violations {
            println!("FAIL {v}");
        }
        problems += violations.len();
        if violations.is_empty() {
            println!("ok   {key}");
        }
    }
    for key in budgets.scenarios.keys().filter(|k| !seen.contains(*k)) {
        if require_all {
            println!("FAIL {key}: no report in {}", dir.display());
            problems += 1;
        } else {
            println!("note: {key}: no report in {}", dir.display());
        }
    }
    if problems == 0 { Ok(()) } else { Err(Failure::Violations(problems)) }
}
