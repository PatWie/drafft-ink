//! The built-in welcome document is generated from a command script, and the checked-in JSON must be that script's
//! output. `cargo xtask intro` regenerates it (`DRAFFTINK_REGEN_INTRO=1`); this test fails when it is stale.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// The seed of the element ids and of the hand-drawn jitter. Changing it changes every id in `intro.json`.
const SEED: &str = "7";

fn assets() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/engine/assets/intro")
}

/// Runs the script the way `cargo xtask intro` documents and returns the document it writes.
fn generate() -> String {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("intro.json");
    let o = Command::new(env!("CARGO_BIN_EXE_drafftink-headless"))
        .args(["run", assets().join("intro.jsonl").to_str().unwrap(), "--batch", "--seed", SEED, "--out"])
        .arg(&out)
        // Paths in the script (`image.insert {"path": ...}`) are relative to the repository root.
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .output()
        .unwrap();
    assert!(o.status.success(), "the intro script failed:\n{}", String::from_utf8_lossy(&o.stderr));
    std::fs::read_to_string(&out).unwrap()
}

#[test]
fn intro_json_is_the_output_of_intro_jsonl() {
    let generated = generate();
    let committed = assets().join("intro.json");
    if std::env::var_os("DRAFFTINK_REGEN_INTRO").is_some() {
        std::fs::write(&committed, &generated).unwrap();
        return;
    }
    let current = std::fs::read_to_string(&committed).unwrap_or_default();
    assert!(
        current == generated,
        "crates/engine/assets/intro/intro.json is not the output of intro.jsonl. Run `cargo xtask intro` and commit the result.\n{}",
        first_differences(&current, &generated)
    );
}

/// The first lines where `committed` and `generated` differ, numbered, so a failure on another machine shows what
/// differs without a download.
fn first_differences(committed: &str, generated: &str) -> String {
    const SHOWN: usize = 8;
    let mut report = String::new();
    let mut committed_lines = committed.lines();
    let mut generated_lines = generated.lines();
    let mut shown = 0;
    for number in 1.. {
        let (a, b) = (committed_lines.next(), generated_lines.next());
        if a.is_none() && b.is_none() {
            break;
        }
        if a != b {
            report.push_str(&format!(
                "line {number}:\n  committed: {}\n  generated: {}\n",
                a.unwrap_or("<end>"),
                b.unwrap_or("<end>")
            ));
            shown += 1;
            if shown == SHOWN {
                break;
            }
        }
    }
    if report.is_empty() {
        report = format!(
            "the lines are equal; the files differ in line endings or in the end ({} and {} bytes)\n",
            committed.len(),
            generated.len()
        );
    }
    report
}

#[test]
fn the_script_makes_the_same_document_every_time() {
    assert_eq!(generate(), generate(), "a fixed seed must give the same ids and the same jitter");
}
