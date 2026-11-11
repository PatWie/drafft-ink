#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Naming from the UI: the document title, double click on a tab, the tab menu (rename, duplicate, move,
//! close) and the Save As name. Every control runs the real command with the right arguments.

mod common;
mod dialog;

use common::Fixture;
use drafftink_engine::Origin;
use drafftink_ui::{Call, HostRequest, Verb};
use egui::{Event, Modifiers, PointerButton};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;

type H = Harness<'static, Fixture>;

fn calls(h: &H) -> Vec<Call> {
    h.state().calls().into_iter().cloned().collect()
}

/// A fixture with tabs named after `names` (the first is the existing tab); the last one is active.
fn with_tabs(names: &[&str]) -> Fixture {
    let mut f = dialog::fixture();
    dialog::run(&mut f, "doc.rename", json!({ "name": names[0] }));
    for name in &names[1..] {
        dialog::run(&mut f, "tab.new", json!({ "name": name }));
    }
    f
}

fn tab_id(f: &Fixture, name: &str) -> String {
    f.model.tabs.iter().find(|t| t.name == name).unwrap().id.clone()
}

fn names(f: &mut Fixture) -> Vec<String> {
    f.refresh();
    f.model.tabs.iter().map(|t| t.name.clone()).collect()
}

fn double_click(h: &mut H, label: &str) {
    let pos = h.get_by_label(label).rect().center();
    h.event(Event::PointerMoved(pos));
    h.run();
    // Both clicks inside one frame: `Harness::step` runs one frame per queued event, and egui's double click
    // delay is shorter than two of those.
    for pressed in [true, false, true, false] {
        h.input_mut().events.push(Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    h.step();
    h.run();
}

fn type_name(h: &mut H, field: &str, text: &str) {
    dialog::field(h, field).focus();
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::A);
    h.run();
    if text.is_empty() {
        h.key_press(egui::Key::Backspace);
    } else {
        dialog::field(h, field).type_text(text);
    }
    h.run();
}

/// Dispatches what the harness recorded and draws again.
fn apply(h: &mut H) -> drafftink_ui::Report {
    let r = h.state_mut().dispatch();
    h.run();
    r
}

#[test]
fn a_double_click_on_a_tab_opens_the_rename_field_and_rename_runs_tab_rename() {
    let f = with_tabs(&["First", "Second"]);
    let second = tab_id(&f, "Second");
    let mut h = dialog::harness(f);
    h.run();
    double_click(&mut h, "Second");
    assert!(h.state().state.rename.is_some(), "the dialog is open");
    h.state_mut().clear();
    h.run();
    type_name(&mut h, "Name", "Renamed");
    h.state_mut().clear();
    h.get_by_label("Rename").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::Ui, "tab.rename", json!({ "tab": second, "name": "Renamed" }))]);
    apply(&mut h);
    assert!(h.state().state.rename.is_none(), "taking the name closes the dialog");
    assert_eq!(names(h.state_mut()), ["First", "Renamed"]);
}

#[test]
fn the_document_title_shows_the_name_and_renames_the_document_with_doc_rename() {
    let f = with_tabs(&["Plan"]);
    let mut h = dialog::harness(f);
    h.run();
    h.get_by_label("Document name: Plan").click();
    h.run();
    assert_eq!(dialog::verbs(h.state()), vec![Verb::RenameDocument]);
    apply(&mut h);
    h.run();
    // The field starts with the current name.
    assert_eq!(dialog::field(&h, "Name").value().as_deref(), Some("Plan"));
    type_name(&mut h, "Name", "  Roadmap ");
    h.state_mut().clear();
    h.get_by_label("Rename").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::Ui, "doc.rename", json!({ "name": "Roadmap" }))]);
    apply(&mut h);
    assert_eq!(names(h.state_mut()), ["Roadmap"]);
}

#[test]
fn an_empty_name_is_refused_with_the_engines_words_and_the_dialog_stays() {
    let f = with_tabs(&["Plan"]);
    let mut h = dialog::harness(f);
    h.state_mut().state.apply_verb(Verb::RenameDocument, &mut Vec::new());
    h.run();
    type_name(&mut h, "Name", "");
    h.state_mut().clear();
    h.get_by_label("Rename").click();
    h.run();
    let report = apply(&mut h);
    let refusal = report.refusals().next().expect("an empty name is refused");
    assert_eq!(refusal.code, "empty_name");
    assert!(h.state().state.rename.is_some());
    assert_eq!(h.state().state.toasts.len(), 1);
    assert_eq!(names(h.state_mut()), ["Plan"]);
}

#[test]
fn enter_applies_the_name() {
    let f = with_tabs(&["Plan"]);
    let mut h = dialog::harness(f);
    h.state_mut().state.apply_verb(Verb::RenameDocument, &mut Vec::new());
    h.run();
    type_name(&mut h, "Name", "Done");
    h.state_mut().clear();
    h.key_press(egui::Key::Enter);
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::Ui, "doc.rename", json!({ "name": "Done" }))]);
}

#[test]
fn save_as_with_a_new_name_renames_first_and_then_asks_the_app_for_the_place() {
    let f = with_tabs(&["Plan"]);
    let mut h = dialog::harness(f);
    h.get_by_label("Open or close the main menu").click();
    h.run();
    apply(&mut h);
    h.get_by_label("Save as…").click();
    h.run();
    apply(&mut h);
    h.run();
    assert!(h.state().state.save_as.is_some(), "the verb opens the name dialog, not the picker");
    assert_eq!(dialog::field(&h, "Document name").value().as_deref(), Some("Plan"));
    type_name(&mut h, "Document name", "Final");
    h.state_mut().clear();
    h.get_by_label("Choose a place").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::Ui, "doc.rename", json!({ "name": "Final" }))]);
    let report = apply(&mut h);
    assert_eq!(report.host, vec![HostRequest::PickSaveTarget], "the rename went through, so the picker follows");
    assert!(h.state().state.save_as.is_none());
    assert_eq!(names(h.state_mut()), ["Final"]);
}

#[test]
fn save_as_with_the_same_name_goes_straight_to_the_picker() {
    let f = with_tabs(&["Plan"]);
    let mut h = dialog::harness(f);
    h.state_mut().state.apply_verb(Verb::SaveAs, &mut Vec::new());
    h.run();
    h.state_mut().clear();
    h.get_by_label("Choose a place…").click();
    h.run();
    assert!(calls(&h).is_empty(), "no rename for an unchanged name");
    let report = apply(&mut h);
    assert_eq!(report.host, vec![HostRequest::PickSaveTarget]);
    assert!(h.state().state.save_as.is_none());
}

#[test]
fn a_refused_rename_in_save_as_does_not_ask_for_the_place() {
    let f = with_tabs(&["Plan"]);
    let mut h = dialog::harness(f);
    h.state_mut().state.apply_verb(Verb::SaveAs, &mut Vec::new());
    h.run();
    type_name(&mut h, "Document name", "");
    h.state_mut().clear();
    h.get_by_label("Choose a place").click();
    h.run();
    let report = apply(&mut h);
    assert_eq!(report.refusals().next().map(|r| r.code), Some("empty_name"));
    assert!(report.host.is_empty());
    assert!(h.state().state.save_as.is_some());
}

fn open_tab_menu(h: &mut H, label: &str) {
    h.get_by_label(label).click_secondary();
    h.run();
}

#[test]
fn the_tab_menu_duplicates_the_tab_it_was_opened_on() {
    let f = with_tabs(&["First", "Second"]);
    let first = tab_id(&f, "First");
    let mut h = dialog::harness(f);
    h.run();
    open_tab_menu(&mut h, "First");
    assert!(h.state().state.tab_menu.is_some());
    h.state_mut().clear();
    h.get_by_label("Duplicate tab").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::ContextMenu, "tab.duplicate", json!({ "tab": first }))]);
    apply(&mut h);
    assert_eq!(names(h.state_mut()).len(), 3);
    assert!(h.state().state.tab_menu.is_none(), "choosing a row closes the menu");
}

#[test]
fn the_tab_menu_moves_a_tab_left_and_right_and_at_an_end_asks_for_the_place_the_tab_has() {
    let f = with_tabs(&["A", "B", "C"]);
    let b = tab_id(&f, "B");
    let mut h = dialog::harness(f);
    h.run();
    open_tab_menu(&mut h, "B");
    h.state_mut().clear();
    h.get_by_label("Move right").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::ContextMenu, "tab.move", json!({ "tab": b, "to_index": 2 }))]);
    apply(&mut h);
    assert_eq!(names(h.state_mut()), ["A", "C", "B"]);

    // B is last now. The row is as available as `tab.move` is (the UI adds no reason of its own); it asks for
    // the place the tab already has, which the command accepts and leaves as it is.
    open_tab_menu(&mut h, "B");
    assert!(!h.get_by_label("Move right").accesskit_node().is_disabled());
    h.state_mut().clear();
    h.get_by_label("Move right").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::ContextMenu, "tab.move", json!({ "tab": b, "to_index": 2 }))]);
    apply(&mut h);
    assert_eq!(names(h.state_mut()), ["A", "C", "B"], "nothing moved");
    assert_eq!(h.state().report.refusals().count(), 0);
    h.state_mut().state.close_overlays();
    h.run();

    open_tab_menu(&mut h, "B");
    h.state_mut().clear();
    h.get_by_label("Move left").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::ContextMenu, "tab.move", json!({ "tab": b, "to_index": 1 }))]);
    apply(&mut h);
    assert_eq!(names(h.state_mut()), ["A", "B", "C"]);
}

#[test]
fn the_tab_menu_renames_and_closes_its_tab() {
    let f = with_tabs(&["A", "B"]);
    let a = tab_id(&f, "A");
    let mut h = dialog::harness(f);
    h.run();
    open_tab_menu(&mut h, "A");
    h.get_by_label("Rename tab…").click();
    h.run();
    assert!(h.state().state.tab_menu.is_none());
    assert_eq!(h.state().state.rename.as_ref().map(|r| r.text.as_str()), Some("A"));
    h.state_mut().state.close_overlays();
    h.run();

    open_tab_menu(&mut h, "A");
    h.state_mut().clear();
    h.get_by_label("Close tab").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::ContextMenu, "tab.close", json!({ "tab": a }))]);
    let report = apply(&mut h);
    // The tab has unsaved changes (it was just named), so the usual question comes first.
    assert_eq!(report.refusals().next().map(|r| r.code), Some("unsaved_changes"));
    let ask = h.state().state.confirm.as_ref().expect("the confirm dialog asks");
    assert_eq!((ask.call.id.as_str(), &ask.call.args), ("tab.close", &json!({ "tab": a })));
}

#[test]
fn a_click_elsewhere_closes_the_tab_menu() {
    let f = with_tabs(&["A", "B"]);
    let mut h = dialog::harness(f);
    h.run();
    open_tab_menu(&mut h, "A");
    dialog::click_canvas(&mut h);
    apply(&mut h);
    assert!(h.state().state.tab_menu.is_none());
}
