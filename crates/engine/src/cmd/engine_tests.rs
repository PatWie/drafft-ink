//! Tests of the `engine` family: what an agent or a palette sees of the registry and the session.

use serde_json::{Value, json};

use crate::ring::Origin;
use crate::session::Session;
use crate::testing;

fn run(s: &mut Session, id: &str, args: Value) -> Value {
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => v,
        Err(r) => panic!("{id} refused: {} ({})", r.message, r.code),
    }
}

fn find<'a>(list: &'a Value, id: &str) -> &'a Value {
    list["commands"].as_array().unwrap().iter().find(|c| c["id"] == id).unwrap_or_else(|| panic!("{id} not listed"))
}

#[test]
fn commands_lists_titles_shortcuts_facts_and_live_availability() {
    let mut s = testing::headless_session();
    let all = run(&mut s, "engine.commands", json!({}));
    assert_eq!(all["count"].as_u64().unwrap() as usize, s.registry().len());
    let zoom = find(&all, "view.zoom_in");
    assert_eq!(zoom["title"], "Zoom in");
    assert_eq!(zoom["family"], "view");
    assert_eq!(zoom["available"], true);
    assert_eq!(zoom["shortcuts"]["mac"][0]["keys"], "Cmd+=");
    assert_eq!(zoom["shortcuts"]["other"][0]["keys"], "Ctrl+=");
    assert_eq!(zoom["facts"]["redraw"], "camera");
    assert_eq!(zoom["undo"], "NoUndo");
    assert!(zoom.get("args_schema").is_none(), "schemas only on request");
}

#[test]
fn commands_filters_by_family_and_search_and_can_include_schemas() {
    let mut s = testing::headless_session();
    let v = run(&mut s, "engine.commands", json!({"family": "perf", "include_schemas": true}));
    assert_eq!(v["count"], 1);
    let stats = find(&v, "perf.stats");
    assert_eq!(stats["args_schema"]["additionalProperties"], false);
    assert!(stats["output_schema"]["properties"]["changed"].is_object(), "the common output keys are merged in");

    let v = run(&mut s, "engine.commands", json!({"search": "SCREENSHOT"}));
    assert_eq!(v["count"], 1);
    assert_eq!(find(&v, "view.screenshot")["id"], "view.screenshot");
}

#[test]
fn commands_available_only_drops_what_refuses_now() {
    let mut s = testing::headless_session();
    s.view_only = true;
    let all = run(&mut s, "engine.commands", json!({}));
    assert_eq!(find(&all, "view.pan")["available"], true);
    let before = all["count"].as_u64().unwrap();
    let avail = run(&mut s, "engine.commands", json!({"available_only": true}));
    assert!(avail["count"].as_u64().unwrap() <= before);
    for c in avail["commands"].as_array().unwrap() {
        assert_eq!(c["available"], true, "{}", c["id"]);
    }
}

#[test]
fn commands_with_an_unknown_family_names_the_real_ones() {
    let mut s = testing::headless_session();
    let r = s.execute("engine.commands", json!({"family": "viw"})).unwrap_err();
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("`family`") && r.message.contains("view"), "{}", r.message);
}

#[test]
fn describe_gives_schemas_facts_refusals_and_a_worked_example() {
    let mut s = testing::headless_session();
    let d = run(&mut s, "engine.describe", json!({"id": "view.zoom_to_fit"}));
    assert_eq!(d["summary"].as_str().unwrap().chars().last(), Some('.'));
    assert!(d["args_schema"]["properties"]["padding"].is_object());
    assert_eq!(d["example"]["id"], "view.zoom_to_fit");
    assert_eq!(d["example"]["args"]["scope"], "all");
    let codes: Vec<&str> = d["refusals"].as_array().unwrap().iter().map(|r| r["code"].as_str().unwrap()).collect();
    assert!(codes.contains(&"nothing_to_do") && codes.contains(&"bad_args") && codes.contains(&"internal"), "{codes:?}");
    let bad = d["refusals"].as_array().unwrap().iter().find(|r| r["code"] == "bad_args").unwrap();
    assert!(bad["message"].as_str().unwrap().contains("engine.describe"));
}

#[test]
fn describe_of_an_unknown_id_suggests_and_names_itself_as_the_refuser() {
    let mut s = testing::headless_session();
    let r = s.execute("engine.describe", json!({"id": "view.zoom_fit"})).unwrap_err();
    assert_eq!(r.code, "unknown_command");
    assert_eq!(r.command, "engine.describe");
    assert!(r.message.contains("`view.zoom_to_fit`"), "{}", r.message);
    assert_eq!(r.suggest.first().map(|x| x.command.as_str()), Some("view.zoom_to_fit"));
    assert_eq!(s.execute("engine.describe", json!({})).unwrap_err().code, "bad_args");
}

#[test]
fn recent_returns_the_ring_oldest_first_and_filters() {
    let mut s = testing::headless_session();
    run(&mut s, "view.zoom_in", json!({}));
    run(&mut s, "view.get", json!({}));
    run(&mut s, "view.zoom_out", json!({}));
    let all = run(&mut s, "engine.recent", json!({}));
    let ids: Vec<&str> = all["entries"].as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["view.zoom_in", "view.get", "view.zoom_out"], "engine.recent does not list itself");
    let first = &all["entries"][0];
    assert_eq!(first["source"], "test");
    assert_eq!(first["outcome"]["ok"], true);
    assert!(first["args"].is_string() && first["seq"] == 1);

    let m = run(&mut s, "engine.recent", json!({"mutating_only": true}));
    assert!(m["entries"].as_array().unwrap().iter().all(|e| e["id"] != "view.get" && e["id"] != "engine.recent"));
    let since = run(&mut s, "engine.recent", json!({"since_seq": 2}));
    assert!(since["entries"].as_array().unwrap().iter().all(|e| e["seq"].as_u64().unwrap() > 2));
    let one = run(&mut s, "engine.recent", json!({"limit": 1}));
    assert_eq!(one["entries"].as_array().unwrap().len(), 1);
}

#[test]
fn recent_shows_refusals_with_code_and_text_and_checks_its_limit() {
    let mut s = testing::headless_session();
    let _ = s.execute("view.pan", json!({"dx": "x"}));
    let v = run(&mut s, "engine.recent", json!({}));
    let e = &v["entries"][0];
    assert_eq!(e["outcome"]["ok"], false);
    assert_eq!(e["outcome"]["code"], "bad_args");
    assert!(e["outcome"]["message"].as_str().unwrap().contains("`dx`"));
    for bad in [json!({"limit": 0}), json!({"limit": 257}), json!({"limit": -3}), json!({"limit": "ten"})] {
        let r = s.execute("engine.recent", bad).unwrap_err();
        assert_eq!(r.code, "bad_args");
    }
    run(&mut s, "engine.recent", json!({"limit": 256}));
}

#[test]
fn state_summarises_tabs_modes_selection_and_revision() {
    let (mut s, _) = testing::session_with_shapes().unwrap();
    s.tool.lock = true;
    let v = run(&mut s, "engine.state", json!({}));
    assert_eq!(v["tabs"][0]["id"], "t1");
    assert_eq!(v["tabs"][0]["active"], true);
    assert_eq!(v["tool"], "select");
    assert_eq!(v["tool_lock"], true);
    assert_eq!(v["view_only"], false);
    assert_eq!(v["selection"]["count"], 1);
    assert_eq!(v["gesture"], Value::Null);
    assert_eq!(v["text_edit"], Value::Null);
    assert_eq!(v["changed"], false);
    assert_eq!(s.execute("engine.state", json!({"x": 1})).unwrap_err().code, "bad_args");
}

#[test]
fn introspection_is_allowed_in_every_state_and_changes_nothing() {
    let mut s = testing::headless_session();
    s.view_only = true;
    s.text_edit = Some(crate::state::TextEdit { element: None, draft: true });
    let before = s.fingerprint();
    for (id, args) in [
        ("engine.commands", json!({})),
        ("engine.describe", json!({"id": "view.pan"})),
        ("engine.recent", json!({})),
        ("engine.state", json!({})),
        ("perf.stats", json!({})),
    ] {
        run(&mut s, id, args);
    }
    assert_eq!(s.fingerprint(), before);
    assert_eq!(s.take_dirty(), None);
}
