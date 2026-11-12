//! `cargo xtask intro [--check]`: the welcome document is generated from `intro.jsonl`.
//!
//! The script is a list of engine commands (`crates/engine/assets/intro/intro.jsonl`); `drafftink-headless run --batch
//! --seed N` runs it and writes `intro.json`, which the engine embeds. The test `apps/drafftink-headless/tests/intro.rs`
//! owns the exact command line and compares the committed file with the script's output, so `ci` fails on a stale
//! file. This command is the front door to that test: without `--check` it rewrites the file, with `--check` it only
//! reports (what `cargo xtask test` does for the whole workspace).

use std::path::Path;

use crate::{cargo, run};

const USAGE: &str = "usage: cargo xtask intro [--check]";

/// Runs the generator test: `regenerate` makes it write the file instead of comparing.
fn generator_test(root: &Path, regenerate: bool) -> Result<(), String> {
    let mut command = cargo(root);
    command.args(["test", "-p", "drafftink-headless", "--test", "intro", "intro_json_is_the_output_of_intro_jsonl"]);
    if regenerate {
        command.env("DRAFFTINK_REGEN_INTRO", "1");
    }
    run(&mut command)
}

/// Entry point of `cargo xtask intro`.
pub fn run_intro(root: &Path, args: &[String]) -> Result<(), String> {
    match args {
        [] => {
            generator_test(root, true)?;
            println!("intro: wrote crates/engine/assets/intro/intro.json from intro.jsonl");
            Ok(())
        }
        [flag] if flag == "--check" => generator_test(root, false),
        _ => Err(USAGE.to_owned()),
    }
}
