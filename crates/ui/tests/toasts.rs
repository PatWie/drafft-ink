#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Error toasts: the text stays readable, and the Copy button asks the app to copy the whole error.

mod common;
mod dialog;

use common::Fixture;
use drafftink_ui::{HostRequest, Toast, UiAction};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

type H = Harness<'static, Fixture>;

/// A harness over a fixture that has run `id` and been refused.
fn after_refusal(id: &str, args: serde_json::Value) -> H {
    let mut f = dialog::fixture();
    f.actions = vec![UiAction::run(id, args)];
    let report = f.dispatch();
    assert_eq!(report.refusals().count(), 1, "{:?}", report.ran);
    let mut h = dialog::harness(f);
    h.run();
    h
}

/// The texts of the `CopyText` requests among the actions of the harness.
fn copied(h: &H) -> Vec<String> {
    dialog::hosts(h.state())
        .into_iter()
        .filter_map(|r| if let HostRequest::CopyText(t) = r { Some(t) } else { None })
        .collect()
}

#[test]
fn the_copy_button_copies_code_command_arguments_message_version_and_platform() {
    let mut h = after_refusal("history.undo", json!({}));
    let toast = h.state().state.toasts[0].clone();
    h.get_by_label("Copy").click();
    h.run();

    let texts = copied(&h);
    assert_eq!(texts.len(), 1, "{texts:?}");
    let text = &texts[0];
    for part in [
        format!("code: {}", toast.code),
        "command: history.undo".to_owned(),
        "args: {}".to_owned(),
        format!("message: {}", toast.text),
        format!("version: {}", env!("CARGO_PKG_VERSION")),
        "platform: ".to_owned(),
    ] {
        assert!(text.contains(&part), "{part:?} in {text}");
    }
}

#[test]
fn the_copied_text_has_no_large_values() {
    let blob = "QUJD".repeat(5000);
    let mut h = after_refusal("element.create", json!({ "kind": "no_such_kind", "text": blob, "token": "hunter2" }));
    h.get_by_label("Copy").click();
    h.run();
    let texts = copied(&h);
    assert_eq!(texts.len(), 1, "{texts:?}");
    assert!(texts[0].len() < 1500, "{} bytes", texts[0].len());
    assert!(!texts[0].contains(&blob) && !texts[0].contains("hunter2"), "{}", texts[0]);
    assert!(texts[0].contains("command: element.create"), "{}", texts[0]);
}

#[test]
fn a_long_message_is_cut_with_show_more_and_copy_still_has_all_of_it() {
    let words: Vec<String> = (0..120).map(|i| format!("word{i}")).collect();
    let long = format!("{}.", words.join(" "));
    let mut f = dialog::fixture();
    f.state.push_toast(Toast {
        text: long.clone(),
        code: "invalid_args".to_owned(),
        born: 0.0,
        suggestion: None,
        command: "element.create".to_owned(),
        args: "{}".to_owned(),
    });
    let mut h = dialog::harness(f);
    h.run();

    assert!(h.query_by_label(&long).is_none(), "the full text is not shown at first");
    assert!(h.query_by_label_contains("word0 word1").is_some());
    assert!(h.query_by_label_contains("word119").is_none());
    h.get_by_label("Show more").click();
    h.run();
    assert!(h.query_by_label(&long).is_some(), "Show more shows all of it");
    h.get_by_label("Show less").click();
    h.run();
    assert!(h.query_by_label("Show more").is_some());

    h.get_by_label("Copy").click();
    h.run();
    assert!(copied(&h)[0].contains(&long));
}

#[test]
fn a_short_message_has_no_show_more_button() {
    let h = after_refusal("history.undo", json!({}));
    assert!(h.query_by_label("Show more").is_none());
    assert!(h.query_by_label("Copy").is_some());
}

#[test]
fn a_suggestion_is_still_a_button_next_to_copy() {
    let mut f = dialog::fixture();
    f.actions = vec![UiAction::run(
        "gesture.begin",
        json!({ "command": "style.set", "args": { "props": { "opacity": 0.5 } } }),
    )];
    f.dispatch();
    f.actions = vec![UiAction::run("tool.set", json!({ "tool": "ellipse" }))];
    f.dispatch();
    let toast = f.state.toasts.last().cloned().expect("a refusal toast");
    assert_eq!(toast.code, "gesture_active");
    let mut h = dialog::harness(f);
    h.run();
    assert!(h.query_by_label("Copy").is_some());
    assert!(toast.suggestion.is_some() && h.query_by_label(h.state().model.title("gesture.cancel")).is_some());
}
