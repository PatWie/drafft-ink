//! The engine as the web build has it: no `schemas` feature, so no schema is generated from the types. Run with
//! `cargo test -p drafftink-engine --no-default-features --test without_schemas` (`cargo xtask test` does);
//! with the feature on this file is empty.
//!
//! The table is a process-wide once-only value, so the steps are one test, in order: before the table, the table,
//! after the table.
#![cfg(not(feature = "schemas"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use drafftink_engine::schemas::{self, SchemaState};
use drafftink_engine::{Origin, Registry, testing};
use serde_json::{Value, json};

/// A table with the one command the test looks at, in the shape `drafftink-cli schemas` writes.
fn table() -> String {
    json!({
        "version": 1,
        "commands": {
            "view.pan": {
                "args": {
                    "type": "object",
                    "properties": { "dx": { "type": "number" }, "dy": { "type": "number" } },
                    "required": ["dx", "dy"],
                    "additionalProperties": false
                },
                "output": { "type": "object", "properties": { "zoom": { "type": "number" } } }
            }
        }
    })
    .to_string()
}

fn run(s: &mut drafftink_engine::Session, id: &str, args: Value) -> Result<Value, drafftink_engine::Refusal> {
    s.execute_from(Origin::Test, id, args)
}

#[test]
fn schemas_are_empty_and_refused_until_the_table_is_installed_and_then_they_are_the_tables() {
    let spec = Registry::global().find("view.pan").unwrap();
    let (mut s, _fakes) = testing::session();

    // Before: nothing is generated, the state says why, and a reader that needs schemas is told to wait.
    assert_eq!(schemas::state(), SchemaState::Loading);
    assert!(!schemas::available());
    assert_eq!(spec.args_schema().to_value(), json!(true), "an empty schema says nothing");
    assert!(spec.arg_keys().is_empty());
    let r = run(&mut s, "engine.describe", json!({ "id": "view.pan" })).unwrap_err();
    assert_eq!(r.code, "schemas_loading");
    assert!(r.message.contains("Try again"), "{}", r.message);
    assert_eq!(run(&mut s, "engine.commands", json!({ "include_schemas": true })).unwrap_err().code, "schemas_loading");
    let list = run(&mut s, "engine.commands", json!({ "family": "view" })).unwrap();
    assert!(list["commands"][0].get("args_schema").is_none(), "facts need no schemas");
    // Commands run, and a wrong key is still refused (serde knows the fields); only the suggestion is missing.
    assert_eq!(run(&mut s, "view.pan", json!({ "dxx": 1, "dy": 2 })).unwrap_err().code, "bad_args");

    // A bad table is refused in words and leaves the state alone.
    assert!(schemas::install("not json").is_err());
    assert!(schemas::install(r#"{"commands": {"view.pan": {"args": true}}}"#).is_err());
    assert_eq!(schemas::state(), SchemaState::Loading);

    // A host that could not fetch the table says why; a reader is told to reload, and the facts still work.
    schemas::fail("the server answered 404");
    assert_eq!(schemas::state(), SchemaState::Failed("the server answered 404".to_owned()));
    let r = run(&mut s, "engine.describe", json!({ "id": "view.pan" })).unwrap_err();
    assert_eq!(r.code, "schemas_unavailable");
    assert!(r.message.contains("404") && r.message.contains("Reload"), "{}", r.message);
    assert!(run(&mut s, "engine.commands", json!({})).is_ok());

    // After: the table answers (and a table that comes later still wins over the failure).
    assert_eq!(schemas::install(&table()).unwrap(), 1);
    assert!(schemas::available());
    assert_eq!(spec.args_schema().get("required").unwrap(), &json!(["dx", "dy"]));
    assert_eq!(spec.arg_keys(), ["dx", "dy"]);
    let described = run(&mut s, "engine.describe", json!({ "id": "view.pan" })).unwrap();
    assert_eq!(described["args_schema"]["properties"]["dx"]["type"], "number");
    assert!(described["output_schema"]["properties"].get("zoom").is_some());
    assert!(described["output_schema"]["properties"].get("changed").is_some(), "the common keys are added on top");
    let listed = run(&mut s, "engine.commands", json!({ "family": "view", "include_schemas": true })).unwrap();
    assert!(listed["commands"][0].get("args_schema").is_some());

    // The keys of the table give the "did you mean" of a misspelt key, in a single call and in a batch.
    let r = run(&mut s, "view.pan", json!({ "dxx": 1, "dy": 2 })).unwrap_err();
    assert!(r.message.contains("did you mean `dx`"), "{}", r.message);
    let steps = [drafftink_engine::BatchStep::new("view.pan", json!({ "dxx": 1, "dy": 2 }))];
    let out = s.execute_batch(Origin::Test, &steps, &drafftink_engine::BatchOpts::default());
    let failed = out.failed.expect("the batch is refused before it runs");
    assert!(failed.refusal.message.contains("did you mean `dx`"), "{}", failed.refusal.message);
    assert_eq!(out.note.as_deref().map(|n| n.contains("nothing was run")), Some(true), "{:?}", out.note);

    // Installing again keeps the first table.
    assert_eq!(schemas::install(&table()).unwrap(), 1);
}
