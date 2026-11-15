#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Naming from the UI: the document title, double click on a tab, the tab menu (rename, duplicate, move,
//! close) and the Save As name. Every control runs the real command with the right arguments.

mod common;
mod dialog;
mod support_a;

use common::Fixture;
use drafftink_engine::Origin;
use drafftink_engine::testing::{add_rect, select};
use drafftink_ui::state::RenameTarget;
use drafftink_ui::{Call, HostRequest, Verb};
use egui::{Event, Modifiers, PointerButton, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use support_a::menu::{open_flyout, row_in};
use support_a::{themed, themed_lenient};

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
    assert_eq!(h.state().state.rename.as_ref().and_then(|r| r.text.as_deref()), Some("A"));
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

// ---- the field of a rename dialog opens with the current name, all of it selected ----

/// The value of the rename field.
fn name_field(h: &H) -> Option<String> {
    dialog::field(h, "Name").value()
}

fn press(h: &mut H, key: egui::Key) {
    h.key_press(key);
    h.run();
}

/// A fixture with one frame, called `name` unless that is `None`, selected. Returns the fixture and the frame's id.
fn with_frame(name: Option<&str>) -> (Fixture, String) {
    let mut f = dialog::fixture();
    let rect = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    select(&mut f.session, &[rect]);
    dialog::run(&mut f, "frame.wrap", json!({}));
    let frame = f.model.selection[0].id.clone();
    if let Some(name) = name {
        dialog::run(&mut f, "frame.rename", json!({ "frame": frame, "name": name }));
    }
    (f, frame)
}

/// Opens the rename dialog of the selected frame the way the context menu does.
fn open_frame_rename_from_the_context_menu(f: Fixture) -> H {
    let mut h = themed_lenient(f, false);
    h.state_mut().state.open_context_menu(pos2(300.0, 60.0), None);
    h.run();
    open_flyout(&mut h, "Frame");
    row_in(&h, "Frame", "Rename frame…").click();
    h.run();
    h
}

#[test]
fn renaming_a_frame_from_the_context_menu_shows_its_name_with_all_of_it_selected() {
    let (f, frame) = with_frame(Some("Login"));
    let mut h = open_frame_rename_from_the_context_menu(f);
    assert_eq!(h.state().state.rename.as_ref().map(|r| r.target.clone()), Some(RenameTarget::Frame(frame.clone())));
    assert_eq!(name_field(&h).as_deref(), Some("Login"), "the field opens with the name");
    assert!(dialog::field(&h, "Name").is_focused(), "and has the focus, with no click");
    // Typing replaces it: the whole text is selected.
    dialog::field(&h, "Name").type_text("Signup");
    h.run();
    assert_eq!(name_field(&h).as_deref(), Some("Signup"));
    h.state_mut().clear();
    press(&mut h, egui::Key::Enter);
    assert_eq!(calls(&h), vec![Call::new(Origin::Ui, "frame.rename", json!({ "frame": frame, "name": "Signup" }))]);
    let report = apply(&mut h);
    assert_eq!(report.refusals().count(), 0, "{:?}", report.ran);
    assert!(h.state().state.rename.is_none(), "Enter closes the dialog");
}

#[test]
fn an_arrow_key_keeps_the_name_and_puts_the_caret_at_one_end() {
    for (key, typed, expected) in [(egui::Key::ArrowRight, "!", "Login!"), (egui::Key::ArrowLeft, ">", ">Login")] {
        let (f, _) = with_frame(Some("Login"));
        let mut h = open_frame_rename_from_the_context_menu(f);
        press(&mut h, key);
        assert_eq!(name_field(&h).as_deref(), Some("Login"), "{key:?} keeps the text");
        dialog::field(&h, "Name").type_text(typed);
        h.run();
        assert_eq!(name_field(&h).as_deref(), Some(expected), "{key:?}");
    }
}

#[test]
fn cancel_closes_the_frame_rename_and_changes_nothing() {
    let (f, frame) = with_frame(Some("Login"));
    let mut h = open_frame_rename_from_the_context_menu(f);
    dialog::field(&h, "Name").type_text("Other");
    h.run();
    h.state_mut().clear();
    h.get_by_label("Cancel").click();
    h.run();
    apply(&mut h);
    assert!(h.state().state.rename.is_none(), "Cancel closes the dialog");
    assert!(calls(&h).is_empty(), "and runs no command");
    let label = h
        .state_mut()
        .session
        .execute("element.query", json!({ "filter": { "ids": [frame] }, "fields": ["text"] }))
        .unwrap();
    assert_eq!(label["items"][0]["text"], json!("Login"));
}

#[test]
fn a_frame_without_a_name_opens_with_the_label_the_canvas_shows() {
    let (f, frame) = with_frame(None);
    let mut h = open_frame_rename_from_the_context_menu(f);
    assert_eq!(name_field(&h).as_deref(), Some("Frame"));
    // Enter on the default label restores the default, it does not store the word as a name.
    h.state_mut().clear();
    press(&mut h, egui::Key::Enter);
    assert_eq!(calls(&h), vec![Call::new(Origin::Ui, "frame.rename", json!({ "frame": frame, "name": null }))]);
}

#[test]
fn the_palette_rename_rows_open_the_dialog_with_the_current_name() {
    use drafftink_ui::state::PaletteState;
    // A selected frame.
    let (mut f, frame) = with_frame(Some("Login"));
    f.state.palette = Some(PaletteState { query: "rename frame".to_owned(), selected: 0 });
    let mut h = themed(f, false);
    h.run();
    h.get_all_by_role_and_label(egui::accesskit::Role::Button, "Rename frame").last().unwrap().click();
    h.run();
    assert_eq!(h.state().state.rename.as_ref().map(|r| r.target.clone()), Some(RenameTarget::Frame(frame)));
    assert_eq!(name_field(&h).as_deref(), Some("Login"));
    assert!(h.state().state.palette.is_none(), "the dialog replaces the palette");

    // The active tab and the document.
    for (query, title, target) in [("rename tab", "Rename tab", "tab"), ("rename document", "Rename document", "doc")] {
        let mut f = with_tabs(&["First", "Second"]);
        f.state.palette = Some(PaletteState { query: query.to_owned(), selected: 0 });
        let mut h = themed(f, false);
        h.run();
        h.get_all_by_role_and_label(egui::accesskit::Role::Button, title).last().unwrap().click();
        h.run();
        assert!(h.state().state.rename.is_some(), "{target}: the row opens the dialog, not a form");
        assert_eq!(name_field(&h).as_deref(), Some("Second"), "{target}");
    }
}

#[test]
fn every_rename_opens_with_the_current_name_selected_so_that_typing_replaces_it() {
    // A tab, from the tab bar and from the menu; the document, from its title and the verb.
    let f = with_tabs(&["First", "Second"]);
    let mut h = dialog::harness(f);
    h.run();
    double_click(&mut h, "First");
    assert_eq!(name_field(&h).as_deref(), Some("First"));
    dialog::field(&h, "Name").type_text("Alpha");
    h.run();
    assert_eq!(name_field(&h).as_deref(), Some("Alpha"));
    h.state_mut().state.close_overlays();
    h.run();

    open_tab_menu(&mut h, "Second");
    h.get_by_label("Rename tab…").click();
    h.run();
    assert_eq!(name_field(&h).as_deref(), Some("Second"));
    h.state_mut().state.close_overlays();
    h.run();

    h.state_mut().clear(); // the clicks on the tabs also asked for a switch; this test is about the names
    h.get_by_label("Document name: Second").click();
    h.run();
    apply(&mut h);
    assert_eq!(name_field(&h).as_deref(), Some("Second"));
    dialog::field(&h, "Name").type_text("Omega");
    h.run();
    assert_eq!(name_field(&h).as_deref(), Some("Omega"));
}

#[test]
fn save_as_opens_with_the_name_selected_too() {
    let f = with_tabs(&["Plan"]);
    let mut h = dialog::harness(f);
    h.state_mut().state.apply_verb(Verb::SaveAs, &mut Vec::new());
    h.run();
    assert_eq!(dialog::field(&h, "Document name").value().as_deref(), Some("Plan"));
    dialog::field(&h, "Document name").type_text("Final");
    h.run();
    assert_eq!(dialog::field(&h, "Document name").value().as_deref(), Some("Final"));
}

// ---- the field over the name label of a frame (a double-click on the label asks for it) ----

/// The harness with the field open the way the app opens it for `UiRequest::OpenFrameNameEditor`: over a frame whose
/// top-left corner is at world (40, 60).
fn with_the_field_open(name: &str) -> (H, String) {
    let (mut f, frame) = with_frame(Some(name));
    f.state.open_frame_name(frame.clone(), name, [40.0, 60.0], 200.0);
    let mut h = themed(f, false);
    h.run();
    (h, frame)
}

fn frame_field(h: &H) -> egui_kittest::Node<'_> {
    dialog::field(h, "Frame name")
}

#[test]
fn the_field_over_a_frames_name_opens_with_the_name_selected_and_sits_on_the_label() {
    let (h, _) = with_the_field_open("Login");
    let field = frame_field(&h);
    assert_eq!(field.value().as_deref(), Some("Login"));
    assert!(field.is_focused(), "it has the keyboard with no click");
    // The label of a frame starts at its top-left corner and ends 4 points above it: the field is on that place.
    let corner = h.state().model.view.world_to_screen([40.0, 60.0]);
    let rect = field.rect();
    assert!((rect.left() - corner.x).abs() < 6.0, "left {} against the corner {}", rect.left(), corner.x);
    assert!(
        rect.bottom() <= corner.y && rect.bottom() > corner.y - 8.0,
        "bottom {} over the corner {}",
        rect.bottom(),
        corner.y
    );
    assert!(rect.height() < 30.0, "it is one line of the label's size, not a dialog: {}", rect.height());
    assert!(!h.state().state.modal_open(), "the canvas keeps its pointer");
}

#[test]
fn typing_replaces_the_name_and_enter_runs_frame_rename() {
    let (mut h, frame) = with_the_field_open("Login");
    frame_field(&h).type_text("Signup");
    h.run();
    assert_eq!(frame_field(&h).value().as_deref(), Some("Signup"));
    h.state_mut().clear();
    press(&mut h, egui::Key::Enter);
    assert_eq!(calls(&h), vec![Call::new(Origin::Ui, "frame.rename", json!({ "frame": frame, "name": "Signup" }))]);
    let report = apply(&mut h);
    assert_eq!(report.refusals().count(), 0, "{:?}", report.ran);
    assert!(h.state().state.frame_name.is_none(), "the field closes when the name is taken");
    let label = h
        .state_mut()
        .session
        .execute("element.query", json!({ "filter": { "ids": [frame] }, "fields": ["text"] }))
        .unwrap();
    assert_eq!(label["items"][0]["text"], json!("Signup"));
}

#[test]
fn an_arrow_key_in_the_field_keeps_the_name() {
    let (mut h, _) = with_the_field_open("Login");
    press(&mut h, egui::Key::ArrowRight);
    assert_eq!(frame_field(&h).value().as_deref(), Some("Login"));
    frame_field(&h).type_text("!");
    h.run();
    assert_eq!(frame_field(&h).value().as_deref(), Some("Login!"));
}

#[test]
fn escape_in_the_field_cancels_with_no_call() {
    let (mut h, frame) = with_the_field_open("Login");
    frame_field(&h).type_text("Other");
    h.run();
    h.state_mut().clear();
    press(&mut h, egui::Key::Escape);
    assert!(calls(&h).iter().all(|c| c.id != "frame.rename"), "Esc runs no rename: {:?}", calls(&h));
    apply(&mut h);
    assert!(h.state().state.frame_name.is_none(), "Esc closes the field");
    let label = h
        .state_mut()
        .session
        .execute("element.query", json!({ "filter": { "ids": [frame] }, "fields": ["text"] }))
        .unwrap();
    assert_eq!(label["items"][0]["text"], json!("Login"));
}

#[test]
fn enter_on_an_unchanged_name_closes_the_field_without_an_undo_step() {
    let (mut h, _) = with_the_field_open("Login");
    h.state_mut().clear();
    press(&mut h, egui::Key::Enter);
    assert!(calls(&h).is_empty(), "nothing changed, so nothing runs: {:?}", calls(&h));
    assert!(h.state().state.frame_name.is_none());
}

#[test]
fn a_click_away_commits_the_name() {
    let (mut h, frame) = with_the_field_open("Login");
    frame_field(&h).type_text("Moved");
    h.run();
    h.state_mut().clear();
    dialog::click_canvas(&mut h);
    assert_eq!(calls(&h), vec![Call::new(Origin::Ui, "frame.rename", json!({ "frame": frame, "name": "Moved" }))]);
}

#[test]
fn escape_closes_a_dialog_whose_field_has_the_keyboard() {
    let f = with_tabs(&["Plan"]);
    let mut h = dialog::harness(f);
    h.state_mut().state.apply_verb(Verb::RenameDocument, &mut Vec::new());
    h.run();
    assert!(dialog::field(&h, "Name").is_focused());
    h.state_mut().clear();
    press(&mut h, egui::Key::Escape);
    apply(&mut h);
    assert!(h.state().state.rename.is_none(), "Esc closes the dialog");
    assert!(calls(&h).is_empty(), "and renames nothing");
}
