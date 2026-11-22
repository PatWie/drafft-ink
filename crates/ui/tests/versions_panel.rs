#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The versions panel (`ui.toggle_versions`): it opens from the menu and the palette, naming a version adds a row,
//! the row menu has the four actions, restore asks first and then changes the canvas, and it works in the touch
//! layout. Every control runs the real command with the right arguments.

mod common;
mod dialog;
mod support_a;

use common::Fixture;
use drafftink_engine::testing::add_rect;
use drafftink_ui::{Call, Verb};
use egui::Key;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use support_a::menu::{has_row_in, is_open, row_in, rows_in};

type H = Harness<'static, Fixture>;

const MENU: &str = "Version menu";

/// Two rectangles in the document and the panel open.
fn open_panel() -> Fixture {
    let mut f = dialog::fixture();
    add_rect(&mut f.session, 0.0, 0.0, 50.0, 50.0).unwrap();
    add_rect(&mut f.session, 100.0, 0.0, 50.0, 50.0).unwrap();
    f.state.apply_verb(Verb::ToggleVersions, &mut Vec::new());
    f.model.wants_versions = true;
    f.refresh();
    f
}

fn harness(f: Fixture) -> H {
    let mut h = support_a::themed(f, false);
    h.run();
    h
}

fn calls(h: &H) -> Vec<Call> {
    h.state().calls().into_iter().cloned().collect()
}

/// Dispatches what the harness recorded, as the app does, and draws again.
fn apply(h: &mut H) -> drafftink_ui::Report {
    let r = h.state_mut().dispatch();
    h.run();
    r
}

fn element_count(h: &mut H) -> u64 {
    dialog::element_count(h.state_mut())
}

fn type_name(h: &mut H, text: &str) {
    dialog::field(h, "Name").focus();
    h.run();
    dialog::field(h, "Name").type_text(text);
    h.run();
}

/// Names a version through the dialog of the panel, the way a person does.
fn name_one(h: &mut H, name: &str) {
    h.get_by_label("Name this version…").click();
    h.run();
    type_name(h, name);
    h.state_mut().clear();
    h.get_by_label("Save").click();
    h.run();
    apply(h);
}

fn row_click(h: &mut H, name: &str) {
    h.get_by_label(&format!("Version {name}")).click();
    h.run();
}

#[test]
fn the_menu_has_a_versions_row_in_the_file_section_and_it_opens_the_panel() {
    let mut f = dialog::fixture();
    f.state.menu_open = true;
    let mut h = support_a::themed_lenient(f, false);
    h.run();
    let rows = rows_in(&h, "Main menu");
    let at = |text: &str| rows.iter().position(|r| r == text).unwrap_or_else(|| panic!("no row {text}: {rows:?}"));
    assert!(
        at("Rename document…") < at("Versions") && at("Versions") < at("Clear document"),
        "in the File section: {rows:?}"
    );
    row_in(&h, "Main menu", "Versions").click();
    h.run();
    assert_eq!(dialog::verbs(h.state()), vec![Verb::ToggleVersions]);
    apply(&mut h);
    assert!(h.state().state.versions_open);
    h.get_by_label("Show or hide the versions");
}

#[test]
fn the_palette_lists_the_panel_and_the_naming_command_and_the_panel_row_opens_it() {
    let mut f = dialog::fixture();
    f.state.palette = Some(drafftink_ui::state::PaletteState { query: "version".to_owned(), selected: 0 });
    let mut h = support_a::themed(f, false);
    h.run();
    let named =
        |h: &H, title: &str| h.query_all_by_role_and_label(egui::accesskit::Role::Button, title).last().is_some();
    assert!(named(&h, "Show or hide the versions"), "the verb has a row");
    assert!(named(&h, "Name this version"), "the command has a row");
    assert!(!named(&h, "Restore version"), "a command that needs a version id is not offered");
    h.get_all_by_role_and_label(egui::accesskit::Role::Button, "Show or hide the versions").last().unwrap().click();
    h.run();
    assert_eq!(dialog::verbs(h.state()).last().copied(), Some(Verb::ToggleVersions));
}

#[test]
fn naming_a_version_adds_a_row_newest_first_with_its_age_and_author() {
    let mut f = open_panel();
    dialog::run(&mut f, "settings.set", json!({ "key": "collab.user_name", "value": "Ana" }));
    let mut h = harness(f);
    h.get_by_label("Versions");
    assert!(h.query_by_label_contains("No versions yet").is_some());
    h.get_by_label("Name this version…").click();
    h.run();
    assert!(h.state().state.rename.is_some(), "the name dialog is open");
    type_name(&mut h, "  Before review ");
    h.state_mut().clear();
    h.get_by_label("Save").click();
    h.run();
    assert_eq!(
        calls(&h),
        vec![Call::new(drafftink_engine::Origin::Ui, "version.save", json!({ "name": "Before review" }))]
    );
    apply(&mut h);
    assert!(h.state().state.rename.is_none(), "taking the name closes the dialog");
    h.get_by_label("Versions (1)");
    h.get_by_label("Version Before review");
    assert_eq!(h.state().model.versions[0].author.as_deref(), Some("Ana"));
    h.state_mut().fakes.clock.advance_ms(90_000);
    h.state_mut().refresh();
    name_one(&mut h, "Second");
    let names: Vec<_> = h.state().model.versions.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, ["Second", "Before review"], "newest first");
    h.get_by_label("Versions (2)");
}

#[test]
fn an_empty_name_keeps_the_dialog_open_next_to_the_refusal() {
    let mut h = harness(open_panel());
    h.get_by_label("Name this version…").click();
    h.run();
    h.state_mut().clear();
    h.get_by_label("Save").click();
    h.run();
    let r = apply(&mut h);
    let refusal = r.refusals().next().expect("refused");
    assert_eq!(refusal.code, "empty_name");
    assert!(h.state().state.rename.is_some(), "the dialog stays");
    assert!(h.state().model.versions.is_empty());
}

#[test]
fn the_name_button_is_greyed_with_the_refusal_while_the_document_is_view_only() {
    let mut f = open_panel();
    dialog::run(&mut f, "view.set_view_only", json!({ "on": true }));
    let h = harness(f);
    let button = h.get_by_label("Name this version…");
    assert!(button.accesskit_node().is_disabled(), "greyed");
}

#[test]
fn a_row_opens_a_menu_with_the_four_actions_named_by_the_registry() {
    let mut f = open_panel();
    dialog::run(&mut f, "version.save", json!({ "name": "Start" }));
    let mut h = harness(f);
    row_click(&mut h, "Start");
    assert!(is_open(&h, MENU));
    assert_eq!(rows_in(&h, MENU), ["Open version as copy", "Restore version…", "Rename version…", "Delete version…"]);
}

#[test]
fn open_as_copy_runs_version_open_and_a_new_tab_shows_the_version() {
    let mut f = open_panel();
    dialog::run(&mut f, "doc.rename", json!({ "name": "Plan" }));
    let id =
        dialog::run(&mut f, "version.save", json!({ "name": "Start" }))["version"]["id"].as_str().unwrap().to_owned();
    dialog::run(&mut f, "element.delete", json!({ "ids": ["e1", "e2"] }));
    let mut h = harness(f);
    row_click(&mut h, "Start");
    h.state_mut().clear();
    row_in(&h, MENU, "Open version as copy").click();
    h.run();
    assert_eq!(
        calls(&h),
        vec![Call::new(drafftink_engine::Origin::ContextMenu, "version.open", json!({ "id": id, "mode": "new_tab" }))]
    );
    apply(&mut h);
    assert!(h.state().state.version_menu.is_none(), "the menu closes");
    assert_eq!(h.state().model.tabs.len(), 2);
    assert_eq!(h.state().model.tabs.iter().find(|t| t.active).unwrap().name, "Plan — Start");
    assert_eq!(element_count(&mut h), 2, "the copy has the two rectangles");
}

#[test]
fn restore_asks_first_and_then_changes_the_canvas() {
    let mut f = open_panel();
    let id = dialog::run(&mut f, "version.save", json!({ "name": "Two shapes" }))["version"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    dialog::run(&mut f, "element.delete", json!({ "ids": ["e1"] }));
    let mut h = harness(f);
    assert_eq!(element_count(&mut h), 1);
    row_click(&mut h, "Two shapes");
    h.state_mut().clear();
    row_in(&h, MENU, "Restore version…").click();
    h.run();
    assert!(calls(&h).is_empty(), "choosing the row only asks");
    apply(&mut h);
    assert!(h.state().state.version_ask.is_some());
    h.get_by_label("Restore this version?");
    assert!(
        h.query_by_label_contains("Undo (Ctrl+Z) takes it back").is_some(),
        "the question says how to take it back"
    );
    assert_eq!(element_count(&mut h), 1, "nothing changed yet");

    h.state_mut().clear();
    h.get_by_label("Restore version").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(drafftink_engine::Origin::Ui, "version.restore", json!({ "id": id }))]);
    apply(&mut h);
    assert_eq!(element_count(&mut h), 2, "the canvas has both rectangles again");
    assert!(h.state().state.version_ask.is_none());
    let undo = h.state().model.history.undo_label.clone();
    assert_eq!(undo.as_deref(), Some("Restore version"), "one undo step");
}

#[test]
fn cancel_in_the_question_runs_nothing() {
    let mut f = open_panel();
    dialog::run(&mut f, "version.save", json!({ "name": "A" }));
    let mut h = harness(f);
    row_click(&mut h, "A");
    row_in(&h, MENU, "Delete version…").click();
    h.run();
    apply(&mut h);
    h.get_by_label("Delete this version?");
    h.state_mut().clear();
    h.get_by_label("Cancel").click();
    h.run();
    apply(&mut h);
    assert!(calls(&h).is_empty());
    assert_eq!(h.state().model.versions.len(), 1);
    assert!(h.state().state.version_ask.is_none());
}

#[test]
fn rename_opens_the_dialog_with_the_current_name_and_runs_version_rename() {
    let mut f = open_panel();
    let id = dialog::run(&mut f, "version.save", json!({ "name": "Old name" }))["version"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut h = harness(f);
    row_click(&mut h, "Old name");
    row_in(&h, MENU, "Rename version…").click();
    h.run();
    apply(&mut h);
    h.get_by_label("Rename the version");
    assert_eq!(dialog::field(&h, "Name").value().as_deref(), Some("Old name"));
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
    h.run();
    dialog::field(&h, "Name").type_text("New name");
    h.run();
    h.state_mut().clear();
    h.get_by_label("Rename").click();
    h.run();
    assert_eq!(
        calls(&h),
        vec![Call::new(drafftink_engine::Origin::Ui, "version.rename", json!({ "id": id, "name": "New name" }))]
    );
    apply(&mut h);
    h.get_by_label("Version New name");
}

#[test]
fn delete_asks_and_then_removes_the_row() {
    let mut f = open_panel();
    let id =
        dialog::run(&mut f, "version.save", json!({ "name": "Doomed" }))["version"]["id"].as_str().unwrap().to_owned();
    let mut h = harness(f);
    row_click(&mut h, "Doomed");
    row_in(&h, MENU, "Delete version…").click();
    h.run();
    apply(&mut h);
    h.state_mut().clear();
    h.get_by_label("Delete version").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(drafftink_engine::Origin::Ui, "version.delete", json!({ "id": id }))]);
    apply(&mut h);
    assert!(h.query_by_label("Version Doomed").is_none());
    assert!(h.query_by_label_contains("No versions yet").is_some());
}

#[test]
fn restore_and_delete_are_greyed_in_the_menu_while_the_document_is_view_only() {
    let mut f = open_panel();
    dialog::run(&mut f, "version.save", json!({ "name": "A" }));
    dialog::run(&mut f, "view.set_view_only", json!({ "on": true }));
    let mut h = harness(f);
    row_click(&mut h, "A");
    assert!(row_in(&h, MENU, "Restore version…").accesskit_node().is_disabled());
    assert!(row_in(&h, MENU, "Delete version…").accesskit_node().is_disabled());
    assert!(!row_in(&h, MENU, "Open version as copy").accesskit_node().is_disabled(), "a copy is not an edit");
    assert!(has_row_in(&h, MENU, "Rename version…"));
}

#[test]
fn the_panel_works_in_the_touch_layout_with_large_targets_inside_the_window() {
    let mut f = open_panel();
    for name in ["One", "Two", "Three"] {
        dialog::run(&mut f, "version.save", json!({ "name": name }));
    }
    f.state.layout = drafftink_ui::state::LayoutPreference::Touch;
    let size = egui::vec2(390.0, 700.0);
    let mut h = dialog::sized(f, size, egui::Theme::Light);
    h.run();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
    for name in ["One", "Two", "Three"] {
        let row = h.get_by_label(&format!("Version {name}")).rect();
        assert!(screen.contains_rect(row), "{name} lies inside the window: {row:?}");
        assert!(row.height() >= 44.0, "{name} is a touch target: {}", row.height());
    }
    let button = h.get_by_label("Name this version…").rect();
    assert!(screen.contains_rect(button) && button.height() >= 32.0, "{button:?}");
    // The whole flow works with touch-sized rows too.
    h.get_by_label("Version Two").click();
    h.run();
    assert_eq!(rows_in(&h, MENU).len(), 4);
}
