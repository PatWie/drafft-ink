//! Tests of `tool.set_arrow`: availability, refusal text, effect on the tool defaults and the settings, a
//! negative case, and that no undo step or document change comes of it.

use drafftink_edit::arrow_defaults::ArrowDefaults;
use drafftink_scene::{Head, PathType};
use serde_json::{Value, json};

use crate::ports::Ns;
use crate::refusal::Refusal;
use crate::ring::Origin;
use crate::session::Session;
use crate::testing;

fn run(s: &mut Session, id: &str, args: Value) -> Value {
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => v,
        Err(r) => panic!("{id} refused: {} ({})", r.message, r.code),
    }
}

fn refuse(s: &mut Session, id: &str, args: Value) -> Refusal {
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => panic!("{id} should have refused, got {v}"),
        Err(r) => r,
    }
}

fn setting(s: &Session, key: &str) -> Value {
    s.settings.get(key).cloned().unwrap()
}

#[test]
fn a_new_session_draws_sharp_arrows_with_an_end_head() {
    let s = testing::headless_session();
    assert_eq!(s.tool.arrow_default, ArrowDefaults::default());
    assert_eq!(setting(&s, "arrow.type"), json!("sharp"));
    assert_eq!(setting(&s, "arrow.start_head"), json!("none"));
    assert_eq!(setting(&s, "arrow.end_head"), json!("arrow"));
}

#[test]
fn it_is_available_in_view_only_mode_and_during_a_text_edit() {
    let mut s = testing::headless_session();
    s.view_only = true;
    s.text_edit = Some(crate::state::TextEdit { element: None, draft: true });
    assert!(s.availability("tool.set_arrow").is_ok());
}

#[test]
fn it_sets_the_type_and_the_heads_in_the_tool_defaults_and_the_settings() {
    let (mut s, f) = testing::session();
    let out = run(
        &mut s,
        "tool.set_arrow",
        json!({ "arrow_type": "elbow", "start_head": "circle", "end_head": null }),
    );
    assert_eq!(out, json!({ "changed": true, "arrow_type": "elbow", "start_head": "circle", "end_head": "none", "warnings": [] }));
    let d = s.tool.arrow_default;
    assert_eq!((d.path, d.start_head, d.end_head), (PathType::Elbow, Head::Circle, Head::None));
    assert_eq!(setting(&s, "arrow.type"), json!("elbow"));
    assert_eq!(setting(&s, "arrow.start_head"), json!("circle"));
    assert_eq!(setting(&s, "arrow.end_head"), json!("none"));
    let record: Value = serde_json::from_slice(&f.storage.bytes(Ns::Settings, "settings").unwrap()).unwrap();
    assert_eq!(record["values"]["arrow.type"], json!("elbow"), "the choice is stored");
}

#[test]
fn a_part_that_is_left_out_keeps_its_value() {
    let mut s = testing::headless_session();
    run(&mut s, "tool.set_arrow", json!({ "arrow_type": "round" }));
    let out = run(&mut s, "tool.set_arrow", json!({ "end_head": "triangle_outline" }));
    assert_eq!(out["arrow_type"], json!("round"));
    assert_eq!(out["end_head"], json!("triangle_outline"));
    assert_eq!(out["start_head"], json!("none"));
}

#[test]
fn the_same_values_again_change_nothing() {
    let (mut s, f) = testing::session();
    run(&mut s, "tool.set_arrow", json!({ "arrow_type": "round" }));
    let before = (s.fingerprint(), f.storage.bytes(Ns::Settings, "settings"));
    let out = run(&mut s, "tool.set_arrow", json!({ "arrow_type": "round", "end_head": "arrow" }));
    assert_eq!(out["changed"], json!(false));
    assert_eq!((s.fingerprint(), f.storage.bytes(Ns::Settings, "settings")), before);
}

#[test]
fn it_makes_no_undo_step_and_leaves_the_document_alone() {
    let mut s = testing::headless_session();
    let rect = testing::add_rect(&mut s, 0.0, 0.0, 50.0, 50.0).unwrap();
    let doc_before = s.element(&rect).cloned();
    run(&mut s, "tool.set_arrow", json!({ "arrow_type": "elbow" }));
    assert_eq!(s.tab().doc.undo_depth(), 0);
    assert_eq!(s.element(&rect).cloned(), doc_before);
}

#[test]
fn existing_arrows_are_not_changed() {
    let mut s = testing::headless_session();
    let a = testing::add_rect(&mut s, 0.0, 0.0, 50.0, 50.0).unwrap();
    let b = testing::add_rect(&mut s, 300.0, 0.0, 50.0, 50.0).unwrap();
    let arrow = testing::create_arrow_between(&mut s, &a, &b).unwrap();
    let before = s.element(&arrow).cloned();
    run(&mut s, "tool.set_arrow", json!({ "arrow_type": "elbow", "end_head": "bar" }));
    assert_eq!(s.element(&arrow).cloned(), before);
}

#[test]
fn it_refuses_a_call_that_names_nothing() {
    let mut s = testing::headless_session();
    let before = s.fingerprint();
    let r = refuse(&mut s, "tool.set_arrow", json!({}));
    assert_eq!(r.code, "bad_args");
    assert_eq!(
        r.message,
        "Invalid arguments for `tool.set_arrow`: give at least one of `arrow_type`, `start_head` and `end_head`, for example {\"arrow_type\": \"elbow\"}. See `engine.describe` for the schema."
    );
    assert_eq!(s.fingerprint(), before);
}

#[test]
fn it_refuses_unknown_words_and_changes_nothing() {
    let mut s = testing::headless_session();
    let before = s.fingerprint();
    for args in [
        json!({ "arrow_type": "wavy" }),
        json!({ "start_head": "spiky" }),
        json!({ "arrow_type": "round", "end_head": "spiky" }),
        json!({ "colour": "red" }),
    ] {
        let r = refuse(&mut s, "tool.set_arrow", args.clone());
        assert_eq!(r.code, "bad_args", "{args}");
        assert_eq!(s.fingerprint(), before, "{args}");
    }
}

#[test]
fn a_running_gesture_blocks_it() {
    let mut s = testing::headless_session();
    let a = testing::add_rect(&mut s, 0.0, 0.0, 10.0, 10.0).unwrap();
    s.gesture_begin_typed(
        Origin::Test,
        "element.move",
        &serde_json::from_value::<crate::cmd::element::MoveArgs>(json!({ "ids": [a.as_str()], "dx": 1.0, "dy": 1.0 })).unwrap(),
    )
    .unwrap();
    let a = s.availability("tool.set_arrow").unwrap_err();
    assert_eq!(a.code, "gesture_active");
    let before = s.fingerprint();
    let r = refuse(&mut s, "tool.set_arrow", json!({ "arrow_type": "round" }));
    assert_eq!((r.code, r.message.as_str()), (a.code, a.message.as_str()));
    assert_eq!(s.fingerprint(), before);
}

#[test]
fn settings_set_reaches_the_same_defaults_and_a_restart_keeps_them() {
    let (mut s, f) = testing::session();
    run(&mut s, "settings.set", json!({ "key": "arrow.type", "value": "round" }));
    assert_eq!(s.tool.arrow_default.path, PathType::Round);
    run(&mut s, "tool.set_arrow", json!({ "start_head": "diamond" }));
    let mut again = Session::new(f.ports());
    again.load_settings().unwrap();
    let d = again.tool.arrow_default;
    assert_eq!((d.path, d.start_head, d.end_head), (PathType::Round, Head::Diamond, Head::Arrow));
    run(&mut again, "settings.reset", json!({ "key": "arrow.type" }));
    assert_eq!(again.tool.arrow_default.path, PathType::Sharp, "a reset brings the built-in type back");
}

#[test]
fn the_head_words_of_the_settings_are_the_heads_of_the_domain() {
    use crate::cmd::settings::keys::{ARROW_TYPES, HEAD_NAMES};
    use drafftink_edit::arrow_defaults::{HEADS, PATH_TYPES, name_of, path_name};
    assert_eq!(HEAD_NAMES, HEADS.map(name_of));
    assert_eq!(ARROW_TYPES, PATH_TYPES.map(path_name));
}
