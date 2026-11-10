//! `cargo xtask harness`: the harness accounting gate (docs/plan.md §8, docs/testing.md).
//!
//! 1. A quick look at `tests/scenarios` that needs no compiler: every file is JSON with an `about` and a list of
//!    `steps`, sits in a family directory, and has a name that becomes a valid test name. A mistake here is
//!    reported in one line instead of as a failing generated test.
//! 2. `cargo test -p drafftink-testkit --test accounting`: the accounting test. It fails when a harness method
//!    is declared and never called, when a scenario or a Rust test calls an undeclared method, when a harness
//!    method has no `Reason:`, when a registered command family has no scenario, or when a scenario names a
//!    command that does not exist. It prints the scenarios that wait for a command and the commands no scenario
//!    runs yet.
//! 3. `--test scenario_guard`: changes the expected value of every `eq` check and demands that its step fails.
//! 4. `--test scenarios`: runs every scenario file (one generated test each).
//!
//! `cargo xtask test` runs the same tests as part of the whole suite; this gate runs them alone so that the
//! failure names the discipline that broke.

use std::fs;
use std::path::Path;

use crate::{cargo, run};

/// What the quick look found.
#[derive(Debug, PartialEq, Eq)]
pub struct Scan {
    /// Number of scenario files.
    pub files: usize,
    /// Number of family directories that hold at least one.
    pub families: usize,
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Checks the shape of `tests/scenarios` under `root`. Returns every problem at once.
pub fn scan(root: &Path) -> Result<Scan, Vec<String>> {
    let dir = root.join("tests/scenarios");
    let mut problems = Vec::new();
    let Ok(entries) = fs::read_dir(&dir) else {
        return Err(vec![format!("{} does not exist", dir.display())]);
    };
    let mut paths: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    paths.sort();
    let (mut files, mut families) = (0, 0);
    for p in paths {
        let name = p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if !p.is_dir() {
            if p.extension().is_some_and(|e| e == "json") {
                problems.push(format!(
                    "tests/scenarios/{name}: a scenario belongs in a family directory, tests/scenarios/<family>/{name}"
                ));
            }
            continue;
        }
        if !valid_name(&name) {
            problems.push(format!(
                "tests/scenarios/{name}: a directory name may use a-z, 0-9 and `_` only (it becomes a test module)"
            ));
        }
        let Ok(inner) = fs::read_dir(&p) else { continue };
        let mut found = 0;
        let mut inner: Vec<_> = inner.filter_map(Result::ok).map(|e| e.path()).collect();
        inner.sort();
        for f in inner.into_iter().filter(|f| f.extension().is_some_and(|e| e == "json")) {
            let fname = f.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let stem = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let shown = format!("tests/scenarios/{name}/{fname}");
            if !valid_name(&stem) {
                problems.push(format!("{shown}: a file name may use a-z, 0-9 and `_` only (it becomes a test name)"));
            }
            match fs::read_to_string(&f)
                .map_err(|e| e.to_string())
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).map_err(|e| e.to_string()))
            {
                Err(e) => problems.push(format!("{shown}: {e}")),
                Ok(v) => {
                    if !v.get("about").is_some_and(serde_json::Value::is_string) {
                        problems.push(format!("{shown}: missing `about` (one sentence on what the scenario proves)"));
                    }
                    if !v.get("steps").is_some_and(|s| s.as_array().is_some_and(|a| !a.is_empty())) {
                        problems.push(format!("{shown}: `steps` must be a non-empty list"));
                    }
                }
            }
            found += 1;
        }
        files += found;
        families += usize::from(found > 0);
    }
    if problems.is_empty() { Ok(Scan { files, families }) } else { Err(problems) }
}

pub fn run_gate(root: &Path) -> Result<(), String> {
    match scan(root) {
        Ok(s) => println!("harness: {} scenario files in {} families", s.files, s.families),
        Err(problems) => return Err(format!("scenario files:\n  - {}", problems.join("\n  - "))),
    }
    run(cargo(root).args(["test", "-p", "drafftink-testkit", "--test", "accounting", "--", "--nocapture"]))?;
    run(cargo(root).args(["test", "-p", "drafftink-testkit", "--test", "scenario_guard", "--test", "scenarios"]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Tree(std::path::PathBuf);

    impl Tree {
        fn new(files: &[(&str, &str)]) -> Tree {
            static N: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "xtask-harness-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            for (path, text) in files {
                let full = root.join(path);
                fs::create_dir_all(full.parent().unwrap_or(&root)).unwrap();
                fs::write(full, text).unwrap();
            }
            fs::create_dir_all(root.join("tests/scenarios")).unwrap();
            Tree(root)
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const GOOD: &str = r#"{"about": "a", "steps": [{"note": "x"}]}"#;

    #[test]
    fn a_tidy_tree_passes_and_is_counted() {
        let t = Tree::new(&[
            ("tests/scenarios/view/a.json", GOOD),
            ("tests/scenarios/view/b.json", GOOD),
            ("tests/scenarios/tab/c.json", GOOD),
            ("tests/scenarios/tab/snapshots/c__x.txt", "text"),
            ("tests/scenarios/pending-families.txt", "# x"),
        ]);
        assert_eq!(scan(&t.0), Ok(Scan { files: 3, families: 2 }));
    }

    #[test]
    fn every_kind_of_mistake_is_reported_at_once() {
        let t = Tree::new(&[
            ("tests/scenarios/stray.json", GOOD),
            ("tests/scenarios/View/a.json", GOOD),
            ("tests/scenarios/view/Bad-Name.json", GOOD),
            ("tests/scenarios/view/not_json.json", "{"),
            ("tests/scenarios/view/no_about.json", r#"{"steps": [{"note": "x"}]}"#),
            ("tests/scenarios/view/no_steps.json", r#"{"about": "a", "steps": []}"#),
        ]);
        let problems = scan(&t.0).unwrap_err();
        let all = problems.join("\n");
        for needle in [
            "stray.json: a scenario belongs in a family directory",
            "View: a directory name",
            "Bad-Name.json: a file name",
            "not_json.json:",
            "no_about.json: missing `about`",
            "no_steps.json: `steps` must be",
        ] {
            assert!(all.contains(needle), "missing `{needle}` in:\n{all}");
        }
    }

    #[test]
    fn a_missing_directory_is_an_error() {
        let t = Tree::new(&[]);
        fs::remove_dir_all(t.0.join("tests/scenarios")).unwrap();
        assert!(scan(&t.0).unwrap_err()[0].contains("does not exist"));
    }
}
