#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The library panel (`ui.toggle_library`): add, insert by click and by drag, rename, remove, export and import.

mod common;
mod dialog;

use common::Fixture;
use drafftink_engine::Origin;
use drafftink_engine::ports::FileRef;
use drafftink_engine::testing::{add_rect, select};
use drafftink_ui::{Call, HostRequest, Verb};
use egui::{Event, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

type H = Harness<'static, Fixture>;

/// A fixture whose library holds one item, "Box", made from a selected rectangle, with the panel open.
fn with_item() -> Fixture {
    let mut f = dialog::fixture();
    let id = add_rect(&mut f.session, 0.0, 0.0, 80.0, 40.0).unwrap();
    select(&mut f.session, std::slice::from_ref(&id));
    dialog::run(&mut f, "library.add", json!({ "name": "Box" }));
    open(f)
}

fn open(mut f: Fixture) -> Fixture {
    f.state.apply_verb(Verb::ToggleLibrary, &mut Vec::new());
    f.model.wants_library = true;
    f.refresh();
    f
}

fn calls(h: &H) -> Vec<Call> {
    h.state().calls().into_iter().cloned().collect()
}

fn item_id(f: &Fixture) -> String {
    f.model.library[0].id.clone()
}

fn element_count(h: &mut H) -> u64 {
    dialog::element_count(h.state_mut())
}

#[test]
fn the_panel_lists_the_items_the_model_read_with_library_list() {
    let f = with_item();
    assert_eq!(f.model.library.len(), 1);
    assert_eq!((f.model.library[0].name.as_str(), f.model.library[0].element_count), ("Box", 1));
    let mut h = dialog::harness(f);
    h.run();
    h.get_by_label("Insert Box");
}

#[test]
fn the_panel_is_hidden_until_the_verb_opens_it() {
    let mut f = dialog::fixture();
    f.refresh();
    let mut h = dialog::harness(f);
    h.run();
    assert!(h.query_by_label("Add selection to library").is_none());
    h.get_by_label("Show or hide the library").click();
    h.run();
    assert_eq!(dialog::verbs(h.state()), vec![Verb::ToggleLibrary]);
}

#[test]
fn a_click_on_an_item_inserts_it_with_library_insert() {
    let f = with_item();
    let id = item_id(&f);
    let mut h = dialog::harness(f);
    h.run();
    let before = element_count(&mut h);
    h.state_mut().clear();
    h.get_by_label("Insert Box").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::Ui, "library.insert", json!({ "item": id }))]);
    h.state_mut().dispatch();
    assert_eq!(element_count(&mut h), before + 1, "the real command put a copy on the canvas");
}

#[test]
fn a_drag_onto_the_canvas_inserts_at_the_world_point_under_the_pointer() {
    let mut f = with_item();
    dialog::run(&mut f, "view.set", json!({ "scroll": [0.0, 0.0], "zoom": 1.0 }));
    let id = item_id(&f);
    let view = f.model.view;
    let mut h = dialog::harness(f);
    h.run();
    let from = h.get_by_label("Insert Box").rect().center();
    let to = Pos2::new(700.0, 400.0);
    h.event(Event::PointerMoved(from));
    h.run();
    h.event(Event::PointerButton {
        pos: from,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    h.run();
    h.state_mut().clear();
    h.event(Event::PointerMoved(Pos2::new(400.0, 300.0)));
    h.run();
    h.event(Event::PointerMoved(to));
    h.run();
    h.event(Event::PointerButton {
        pos: to,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run();
    let at = view.screen_to_world(to);
    assert_eq!(calls(&h), vec![Call::new(Origin::Ui, "library.insert", json!({ "item": id, "at": at }))]);
    let before = element_count(&mut h);
    h.state_mut().dispatch();
    assert_eq!(element_count(&mut h), before + 1);
}

#[test]
fn a_drag_that_ends_over_the_panel_inserts_nothing() {
    let f = with_item();
    let mut h = dialog::harness(f);
    h.run();
    let from = h.get_by_label("Insert Box").rect().center();
    h.event(Event::PointerMoved(from));
    h.run();
    h.event(Event::PointerButton {
        pos: from,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    h.run();
    h.state_mut().clear();
    let near = from + egui::vec2(5.0, 3.0);
    h.event(Event::PointerMoved(near));
    h.run();
    h.event(Event::PointerButton {
        pos: near,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run();
    assert!(calls(&h).iter().all(|c| c.args.get("at").is_none()), "{:?}", calls(&h));
}

#[test]
fn add_stores_the_selection_under_the_typed_name_and_clears_the_field() {
    let mut f = dialog::fixture();
    let id = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    select(&mut f.session, std::slice::from_ref(&id));
    let mut h = dialog::harness(open(f));
    h.run();
    dialog::field(&h, "Library item name").focus();
    h.run();
    dialog::field(&h, "Library item name").type_text("Logo");
    h.run();
    h.state_mut().clear();
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Add selection to library").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::Ui, "library.add", json!({ "name": "Logo" }))]);
    h.state_mut().dispatch();
    assert!(h.state().state.library_name.is_empty());
    assert_eq!(h.state().model.library.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["Logo"]);
}

#[test]
fn add_without_a_selection_is_greyed_with_the_refusal() {
    let mut f = dialog::fixture();
    add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    let mut h = dialog::harness(open(f));
    h.run();
    h.state_mut().clear();
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Add selection to library").click();
    h.run();
    assert!(calls(&h).is_empty());
}

#[test]
fn rename_opens_the_dialog_and_runs_library_rename() {
    let f = with_item();
    let id = item_id(&f);
    let mut h = dialog::harness(f);
    h.run();
    h.get_by_label("Rename Box").click();
    h.run();
    assert_eq!(h.state().state.rename.as_ref().map(|r| r.text.as_str()), Some("Box"));
    dialog::field(&h, "Name").focus();
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::A);
    dialog::field(&h, "Name").type_text("Crate");
    h.run();
    h.state_mut().clear();
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Rename").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::Ui, "library.rename", json!({ "item": id, "name": "Crate" }))]);
    h.state_mut().dispatch();
    assert_eq!(h.state().model.library[0].name, "Crate");
}

#[test]
fn remove_runs_library_remove_for_that_item() {
    let f = with_item();
    let id = item_id(&f);
    let mut h = dialog::harness(f);
    h.run();
    h.state_mut().clear();
    h.get_by_label("Remove Box").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::Ui, "library.remove", json!({ "items": [id] }))]);
    h.state_mut().dispatch();
    assert!(h.state().model.library.is_empty());
}

#[test]
fn export_writes_an_excalidrawlib_download_and_import_asks_the_app_for_a_file() {
    let f = with_item();
    let mut h = dialog::harness(f);
    h.run();
    h.state_mut().clear();
    h.get_by_label("Export library").click();
    h.run();
    assert_eq!(
        calls(&h),
        vec![Call::new(Origin::Ui, "library.export", json!({ "to": { "download": "drafftink.excalidrawlib" } }))]
    );
    let report = h.state_mut().dispatch();
    assert!(report.refusals().next().is_none(), "{:?}", report.ran);
    let file = h.state().fakes.files.bytes(&FileRef::Download { download: "drafftink.excalidrawlib".to_owned() });
    assert!(file.is_some_and(|b| !b.is_empty()));

    h.state_mut().clear();
    h.get_by_label("Import…").click();
    h.run();
    assert_eq!(dialog::hosts(h.state()), vec![HostRequest::PickLibraryImport]);
}

#[test]
fn an_empty_library_says_how_to_fill_it() {
    let mut h = dialog::harness(open(dialog::fixture()));
    h.run();
    h.get_by_label("Nothing here yet. Select shapes and press Add.");
}
