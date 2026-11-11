//! Prints the command registry with its JSON schemas, for `e2e/web/scripts/gen-command-types.ts`.
//!
//! ```text
//! cargo run -p drafftink-e2e --example command_schemas > e2e/web/generated/commands.json
//! ```
//!
//! The output is `engine.commands {include_schemas: true}` of a fresh session, so the browser specs are
//! typed from the registry that the wasm build compiles in, not from a hand-written list.

use std::process::ExitCode;

use drafftink_testkit::Harness;
use serde_json::json;

fn main() -> ExitCode {
    let mut harness = Harness::new();
    match harness.session.execute("engine.commands", json!({ "include_schemas": true })) {
        Ok(output) => match serde_json::to_string_pretty(&output) {
            Ok(text) => {
                println!("{text}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("cannot serialise the registry: {e}");
                ExitCode::FAILURE
            }
        },
        Err(refusal) => {
            eprintln!("engine.commands refused: {refusal:?}");
            ExitCode::FAILURE
        }
    }
}
