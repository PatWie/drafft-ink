#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The link dialog (`Mod+K`, `ui.open_link_editor`) and the way an opened link reaches the app.

mod common;
mod dialog;

use common::Fixture;
use drafftink_engine::Origin;
use drafftink_engine::testing::{add_rect, select};
use drafftink_ui::{Call, HostRequest, UiAction, Verb};
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

/// A fixture with one selected rectangle (`e1`), the dialog open.
fn open(link: Option<&str>) -> Harness<'static, Fixture> {
    let mut f = Fixture::new();
    let id = add_rect(&mut f.session, 0.0, 0.0, 50.0, 50.0).unwrap();
    select(&mut f.session, std::slice::from_ref(&id));
    if let Some(l) = link {
        f.session.execute("element.set_link", json!({ "link": l })).unwrap();
    }
    f.refresh();
    f.state.apply_verb(Verb::OpenLinkEditor, &mut Vec::new());
    let mut h = dialog::harness(f);
    h.run();
    h
}

fn calls(h: &Harness<'static, Fixture>) -> Vec<Call> {
    h.state().calls().into_iter().cloned().collect()
}

#[test]
fn the_field_starts_with_the_current_link_and_save_sends_the_text() {
    let mut h = open(Some("https://example.com/a"));
    dialog::field(&h, "Link").focus();
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run();
    dialog::field(&h, "Link").type_text("example.org");
    h.run();
    h.get_by_label("Save").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::Ui, "element.set_link", json!({ "link": "example.org" }))]);
}

#[test]
fn save_shows_the_link_it_was_opened_with_when_nothing_is_typed() {
    let mut h = open(Some("https://example.com/a"));
    h.get_by_label("Save").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::Ui, "element.set_link", json!({ "link": "https://example.com/a" }))]);
}

#[test]
fn remove_clears_the_link_and_exists_only_when_there_is_one() {
    let mut h = open(Some("https://example.com/a"));
    h.get_by_label("Remove").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::Ui, "element.set_link", json!({ "link": null }))]);
    let h = open(None);
    assert!(h.query_by_label("Remove").is_none());
}

#[test]
fn the_dialog_closes_after_the_link_was_set_and_stays_after_a_refusal() {
    let mut h = open(None);
    dialog::field(&h, "Link").focus();
    h.run();
    dialog::field(&h, "Link").type_text("javascript:alert(1)");
    h.run();
    h.get_by_label("Save").click();
    h.run();
    let report = h.state_mut().dispatch();
    let refusal = report.refusals().next().expect("the engine refuses an unsafe link");
    assert_eq!(refusal.code, "invalid_link");
    assert!(h.state().state.link_editor.is_some(), "a refusal keeps the dialog open so the text can be fixed");
    assert_eq!(h.state().state.toasts.len(), 1, "and shows its words");

    let f = h.state_mut();
    let acts = vec![UiAction::run("element.set_link", json!({ "link": "https://example.com" }))];
    let report = drafftink_ui::dispatch(&mut f.session, &mut f.state, acts, 0.0);
    assert!(report.refusals().next().is_none());
    assert!(f.state.link_editor.is_none(), "set: closed");
}

#[test]
fn without_exactly_one_selected_element_the_dialog_says_so_and_has_no_save() {
    let mut f = Fixture::new();
    add_rect(&mut f.session, 0.0, 0.0, 50.0, 50.0).unwrap();
    f.refresh();
    f.state.apply_verb(Verb::OpenLinkEditor, &mut Vec::new());
    let mut h = dialog::harness(f);
    h.run();
    h.get_by_label("Select one element to give it a link.");
    assert!(h.query_by_label("Save").is_none());
}

#[test]
fn an_opened_web_link_becomes_a_host_request_and_the_engine_opens_nothing() {
    let mut f = Fixture::new();
    let id = add_rect(&mut f.session, 0.0, 0.0, 50.0, 50.0).unwrap();
    select(&mut f.session, std::slice::from_ref(&id));
    f.session.execute("element.set_link", json!({ "link": "https://example.com/x" })).unwrap();
    f.refresh();
    let acts = vec![UiAction::run("element.open_link", json!({}))];
    let report = drafftink_ui::dispatch(&mut f.session, &mut f.state, acts, 0.0);
    assert_eq!(report.host, vec![HostRequest::OpenUrl("https://example.com/x".to_owned())]);
}

#[test]
fn an_element_link_sends_no_host_request() {
    let mut f = Fixture::new();
    let a = add_rect(&mut f.session, 0.0, 0.0, 50.0, 50.0).unwrap();
    let b = add_rect(&mut f.session, 500.0, 0.0, 50.0, 50.0).unwrap();
    select(&mut f.session, std::slice::from_ref(&a));
    f.session.execute("element.set_link", json!({ "link": format!("#{}", b.as_str()) })).unwrap();
    let acts = vec![UiAction::run("element.open_link", json!({}))];
    let report = drafftink_ui::dispatch(&mut f.session, &mut f.state, acts, 0.0);
    assert!(report.host.is_empty());
    assert!(report.refusals().next().is_none());
}

#[test]
fn mod_k_is_bound_to_the_link_dialog_on_the_canvas() {
    let vb = drafftink_ui::keymap::VERB_BINDINGS.iter().find(|v| v.verb == Verb::OpenLinkEditor).expect("a binding");
    assert_eq!(vb.keys, ["Mod+K"]);
    assert_eq!(vb.scope, drafftink_engine::spec::Scope::Canvas);
}
