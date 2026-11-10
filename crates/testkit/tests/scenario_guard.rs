#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Guards against scenarios that pass for the wrong reason.
//!
//! For every `eq` check in every committed scenario, this test changes the expected value and runs the scenario
//! again. The run must fail *at the step that holds the check*. A check that still passes with a wrong value, or a
//! step that is never reached, is a check that proves nothing. Scenarios that are skipped here (waiting for a
//! command, no GPU) and the scale scenarios (too slow to run once per check) are not mutated.

use std::path::{Path, PathBuf};

use drafftink_testkit::scenario::{Options, Outcome, Scenario, run};
use serde_json::{Value, json};

fn scenario_files() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/scenarios");
    let mut out = Vec::new();
    for fam in std::fs::read_dir(&root).unwrap().filter_map(Result::ok) {
        let p = fam.path();
        if !p.is_dir() || fam.file_name() == "scale" {
            continue;
        }
        for f in std::fs::read_dir(&p).unwrap().filter_map(Result::ok) {
            if f.path().extension().is_some_and(|x| x == "json") {
                out.push(f.path());
            }
        }
    }
    out.sort();
    out
}

/// A different value of the same rough kind.
fn wrong(v: &Value) -> Value {
    match v {
        Value::Number(n) => json!(n.as_f64().unwrap_or(0.0) + 1.0),
        Value::String(s) => json!(format!("{s}-wrong")),
        Value::Bool(b) => json!(!b),
        Value::Null => json!("not null"),
        Value::Array(a) => {
            let mut a = a.clone();
            a.push(json!("extra"));
            Value::Array(a)
        }
        Value::Object(_) => Value::Null,
    }
}

/// Where the `eq` checks are: (step index, index in `expect`).
fn eq_checks(doc: &Value) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (i, step) in doc["steps"].as_array().into_iter().flatten().enumerate() {
        for (j, check) in step["expect"].as_array().into_iter().flatten().enumerate() {
            if check.get("eq").is_some() {
                out.push((i, j));
            }
        }
    }
    out
}

#[test]
fn changing_the_expected_value_of_any_eq_check_makes_its_step_fail() {
    let (mut mutated, mut files) = (0, 0);
    for file in scenario_files() {
        let text = std::fs::read_to_string(&file).unwrap();
        let name = file
            .strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/scenarios"))
            .unwrap_or(&file)
            .display()
            .to_string();
        // The real snapshot directory, so a scenario with snapshots still passes unmodified.
        let real_snaps = file.parent().unwrap().join("snapshots");
        let stem = file.file_stem().unwrap().to_string_lossy().into_owned();
        let sc = Scenario::parse(&text).unwrap();
        match run(&sc, &real_snaps, &stem, Options::default()) {
            Ok(Outcome::Passed { .. }) => {}
            Ok(Outcome::Skipped { reason }) => {
                eprintln!("not mutated, skipped: {name}: {reason}");
                continue;
            }
            Err(e) => panic!("{name} fails unmodified: {e}"),
        }
        files += 1;
        let doc: Value = serde_json::from_str(&text).unwrap();
        for (i, j) in eq_checks(&doc) {
            let mut bad = doc.clone();
            let original = bad["steps"][i]["expect"][j]["eq"].clone();
            let changed = wrong(&original);
            bad["steps"][i]["expect"][j]["eq"] = changed.clone();
            let result = match Scenario::parse(&bad.to_string()) {
                Ok(sc) => run(&sc, &real_snaps, &stem, Options::default()),
                Err(e) => panic!("{name}: the mutated scenario does not parse: {e}"),
            };
            match result {
                Err(msg) => assert!(
                    msg.starts_with(&format!("step {i} ")),
                    "{name}: changing eq {original} in step {i} expect[{j}] failed the run, but at another place:\n{msg}"
                ),
                Ok(o) => panic!(
                    "{name}: step {i} expect[{j}]: `eq {original}` still passes when changed to {changed} ({o:?}); the check proves nothing"
                ),
            }
            mutated += 1;
        }
    }
    assert!(
        files >= 15 && mutated >= 100,
        "the guard looked at {files} files and {mutated} checks; did the scenarios move?"
    );
    println!("mutated {mutated} eq checks in {files} scenarios; every one failed at its own step");
}
