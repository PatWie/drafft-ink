#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The library drawer (`ui.toggle_library`): where it sits and how tall it is, its pictures, the search, add, insert by
//! click and by drag, rename, remove, export and import.

mod common;
mod dialog;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use common::Fixture;
use drafftink_engine::ports::fake::Fakes;
use drafftink_engine::ports::{FileRef, PortError, RenderRequest, Renderer, SceneView};
use drafftink_engine::services::Services;
use drafftink_engine::testing::{add_rect, select};
use drafftink_engine::{Origin, Session};
use drafftink_scene::SceneRead;
use drafftink_ui::thumbs::{PER_FRAME, Thumb, ThumbImage, ThumbSize};
use drafftink_ui::{Call, FrameInput, HostRequest, Toast, UiShell, Verb};
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

/// Puts the pointer on the tile, which shows its rename and remove buttons.
fn show_tile_buttons(h: &mut H) {
    h.get_by_label("Insert Box").hover();
    h.run();
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
    // The outline of a tile that has no picture yet has the item's proportions: `library.list` gives `[x, y, w, h]`.
    assert_eq!((f.model.library[0].width, f.model.library[0].height), (80.0, 40.0));
    assert_eq!(f.model.library[0].content.len(), 16);
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
    show_tile_buttons(&mut h);
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
    show_tile_buttons(&mut h);
    h.state_mut().clear();
    h.get_by_label("Remove Box").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::Ui, "library.remove", json!({ "items": [id] }))]);
    h.state_mut().dispatch();
    assert!(h.state().model.library.is_empty());
}

/// The menu a right click on the tile "Box" opens.
const TILE_MENU: &str = "Library item menu";

fn open_tile_menu(h: &mut H) {
    h.get_by_label("Insert Box").click_secondary();
    h.run();
}

fn menu_row<'a>(h: &'a H, text: &'a str) -> egui_kittest::Node<'a> {
    h.get_by_role_and_label(egui::accesskit::Role::Menu, TILE_MENU)
        .query_by_role_and_label(egui::accesskit::Role::Button, text)
        .unwrap_or_else(|| panic!("no row {text:?} in the tile menu"))
}

#[test]
fn a_right_click_on_a_tile_opens_a_menu_with_insert_rename_and_remove() {
    let mut h = dialog::harness(with_item());
    h.run();
    assert!(h.query_by_role_and_label(egui::accesskit::Role::Menu, TILE_MENU).is_none());
    open_tile_menu(&mut h);
    for row in ["Insert", "Rename…", "Remove"] {
        menu_row(&h, row);
    }
    assert!(h.state().calls().is_empty(), "opening the menu runs nothing");
}

#[test]
fn the_tile_menu_inserts_and_removes_the_item_it_was_opened_on() {
    let f = with_item();
    let id = item_id(&f);
    let mut h = dialog::harness(f);
    h.run();
    open_tile_menu(&mut h);
    h.state_mut().clear();
    menu_row(&h, "Insert").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::ContextMenu, "library.insert", json!({ "item": id }))]);
    assert!(h.state().state.library_menu.is_none(), "choosing a row closes the menu");
    h.state_mut().dispatch();
    assert_eq!(element_count(&mut h), 2, "the selected rectangle and its copy");

    open_tile_menu(&mut h);
    h.state_mut().clear();
    menu_row(&h, "Remove").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::ContextMenu, "library.remove", json!({ "items": [id] }))]);
    h.state_mut().dispatch();
    assert!(h.state().model.library.is_empty());
}

#[test]
fn the_tile_menu_renames_through_the_dialog_and_a_click_elsewhere_closes_it() {
    let mut h = dialog::harness(with_item());
    h.run();
    open_tile_menu(&mut h);
    h.state_mut().clear();
    menu_row(&h, "Rename…").click();
    h.run();
    assert!(h.state().calls().is_empty(), "the dialog makes the call, not the row");
    assert_eq!(h.state().state.rename.as_ref().map(|r| r.text.as_str()), Some("Box"));
    assert!(h.state().state.library_menu.is_none());

    let mut h = dialog::harness(with_item());
    h.run();
    open_tile_menu(&mut h);
    dialog::click_canvas(&mut h);
    h.state_mut().dispatch();
    h.run();
    assert!(h.state().state.library_menu.is_none());
    assert!(h.query_by_role_and_label(egui::accesskit::Role::Menu, TILE_MENU).is_none());
}

#[test]
fn the_tile_keeps_its_hover_buttons_next_to_the_menu() {
    let mut h = dialog::harness(with_item());
    h.run();
    show_tile_buttons(&mut h);
    h.get_by_label("Rename Box");
    h.get_by_label("Remove Box");
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

// ---- the place of the drawer ----

/// The rectangle the drawer recorded in `ChromeRects` this frame.
fn drawer_rect(h: &H) -> egui::Rect {
    h.state().state.chrome.library.expect("the drawer is on screen")
}

fn at(size: egui::Vec2, f: Fixture) -> H {
    dialog::sized(f, size, egui::Theme::Light)
}

/// The control with this label inside the drawer (the toolbar has a button with the label of the Close button).
fn in_drawer(h: &H, label: &str) -> egui::Rect {
    let drawer = drawer_rect(h);
    let found: Vec<egui::Rect> =
        h.get_all_by_label(label).map(|n| n.rect()).filter(|r| drawer.contains_rect(*r)).collect();
    assert_eq!(found.len(), 1, "{label} in the drawer: {found:?}");
    found[0]
}

/// The rectangles of the two bars the drawer must stay clear of: the first button of the bottom bar and the
/// tab bar's tab.
fn bars(h: &H) -> [egui::Rect; 2] {
    [h.get_by_label("Undo").rect(), h.get_all_by_label("Untitled").last().expect("a tab").rect()]
}

/// The vertical extent of the bars: a tab is wider once the document has changes, so only the heights compare.
fn bar_heights(h: &H) -> [(f32, f32); 2] {
    bars(h).map(|r| (r.top(), r.bottom()))
}

#[test]
fn the_drawer_opens_at_the_bottom_with_30_percent_of_the_window_height() {
    let mut h = at(egui::vec2(1280.0, 800.0), with_item());
    h.run();
    let drawer = drawer_rect(&h);
    let drawn = h.ctx.memory(|m| m.area_rect(egui::Id::unique("library_panel"))).expect("the area is on screen");
    assert_eq!(drawer, drawn, "the rectangle it records is the one it drew");
    assert!((drawer.height() - 240.0).abs() <= 1.0, "30 percent of 800 points: {drawer:?}");
    assert!(drawer.width() > 900.0 && drawer.left() >= 12.0 && drawer.right() <= 1280.0 - 12.0, "{drawer:?}");
    assert!(drawer.top() > 400.0, "in the lower half: {drawer:?}");
    // Everything it shows is inside it.
    let tile = h.get_by_label("Insert Box").rect();
    assert!(drawer.contains_rect(tile), "{tile:?} in {drawer:?}");
    for label in ["Add selection to library", "Export library", "Show or hide the library", "Import…"] {
        in_drawer(&h, label);
    }
}

#[test]
fn the_drawer_leaves_the_tool_column_and_the_properties_panel_free() {
    let mut h = at(egui::vec2(1280.0, 800.0), with_item());
    h.run();
    let drawer = drawer_rect(&h);
    // The toolbar's own button for the library is its lowest control; it stays where it was and works.
    let toggles: Vec<egui::Rect> = h
        .get_all_by_label("Show or hide the library")
        .map(|n| n.rect())
        .filter(|r| !drawer.contains_rect(*r))
        .collect();
    assert_eq!(toggles.len(), 1, "{toggles:?}");
    assert!(!drawer.intersects(toggles[0]), "{toggles:?} under {drawer:?}");
    for label in ["Rectangle tool", "Undo"] {
        assert!(!drawer.intersects(h.get_by_label(label).rect()), "{label} under {drawer:?}");
    }
    h.state_mut().clear();
    dialog::click_at(&mut h, toggles[0].center());
    assert_eq!(dialog::verbs(h.state()), vec![Verb::ToggleLibrary], "the toolbar button still closes the drawer");
}

#[test]
fn the_drawer_does_not_overlap_the_bottom_bar_or_the_tabs() {
    for size in [egui::vec2(1280.0, 800.0), egui::vec2(1280.0, 500.0), egui::vec2(390.0, 800.0)] {
        let mut h = at(size, with_item());
        h.run();
        let drawer = drawer_rect(&h);
        for bar in bars(&h) {
            assert!(drawer.bottom() <= bar.top(), "{size:?}: the drawer {drawer:?} reaches into the bar at {bar:?}");
        }
        let chrome = h.state().state.chrome;
        for (name, rect) in [("bottom bar", chrome.bottom_bar), ("tab bar", chrome.tabs)] {
            let rect = rect.unwrap_or_else(|| panic!("{name} recorded"));
            assert!(drawer.bottom() <= rect.top(), "{size:?}: the drawer {drawer:?} reaches into the {name} {rect:?}");
        }
        let screen = egui::Rect::from_min_size(Pos2::ZERO, size);
        assert!(screen.contains_rect(drawer), "{size:?}: {drawer:?}");
    }
}

#[test]
fn the_drawer_leaves_the_recorded_side_panels_free_and_the_chrome_above_it_clears_it() {
    let mut f = with_item();
    f.state.stats_open = true;
    f.state.push_toast(Toast {
        text: "Nothing to undo.".to_owned(),
        code: "nothing_to_do".to_owned(),
        born: 0.0,
        suggestion: None,
    });
    let h = at(egui::vec2(1280.0, 800.0), f);
    let chrome = h.state().state.chrome;
    let drawer = drawer_rect(&h);
    for (name, panel) in [("tool column", chrome.tools), ("ribbon card", chrome.ribbon)] {
        if let Some(panel) = panel {
            assert!(!drawer.intersects(panel), "the drawer {drawer:?} covers the {name} {panel:?}");
        }
    }
    let toast = h.get_all_by_label("Nothing to undo.").next().expect("the toast").rect();
    assert!(toast.max.y <= drawer.top(), "the toast {toast:?} sits above the drawer {drawer:?}");
    let stats = h.get_all_by_label("Stats").next().expect("the stats panel").rect();
    assert!(stats.max.y <= drawer.top(), "the stats {stats:?} sit above the drawer {drawer:?}");
    let closed = at(egui::vec2(1280.0, 800.0), dialog::fixture());
    assert!(closed.state().state.chrome.library.is_none(), "a closed drawer records nothing");
}

#[test]
fn the_bars_stay_where_they_were_and_are_still_reachable_with_the_drawer_open() {
    let closed = {
        let mut f = dialog::fixture();
        f.refresh();
        let mut h = at(egui::vec2(1280.0, 800.0), f);
        h.run();
        bar_heights(&h)
    };
    let mut h = at(egui::vec2(1280.0, 800.0), with_item());
    h.run();
    assert_eq!(bar_heights(&h), closed, "the drawer sits above the bars and moves nothing");
}

/// Drags the pointer from one point to another with the primary button down.
fn drag(h: &mut H, from: Pos2, to: Pos2) {
    h.event(Event::PointerMoved(from));
    h.run();
    h.event(Event::PointerButton {
        pos: from,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    h.run();
    h.event(Event::PointerMoved(from.lerp(to, 0.5)));
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
}

fn grip(h: &H) -> Pos2 {
    h.get_by_label("Resize the library").rect().center()
}

#[test]
fn dragging_the_top_edge_sets_the_height_within_the_limits() {
    let mut h = at(egui::vec2(1280.0, 800.0), with_item());
    h.run();
    let bottom = drawer_rect(&h).bottom();
    // Up by 100 points: 100 taller.
    let from = grip(&h);
    drag(&mut h, from, from - egui::vec2(0.0, 100.0));
    let drawer = drawer_rect(&h);
    assert!((drawer.height() - 340.0).abs() <= 1.0, "{drawer:?}");
    assert_eq!(drawer.bottom(), bottom, "the bottom edge stays above the bars");
    assert_eq!(h.state().state.library_height.map(f32::round), Some(drawer.height().round()), "the height is kept");
    // Far past the top of the window: seventy percent of it at most.
    let from = grip(&h);
    drag(&mut h, from, Pos2::new(from.x, 0.0));
    assert_eq!(drawer_rect(&h).height(), 560.0);
    // Down to the bottom of the window: the least height, which still shows the header, the header of the section
    // and a whole row of tiles.
    let from = grip(&h);
    drag(&mut h, from, Pos2::new(from.x, 799.0));
    let drawer = drawer_rect(&h);
    assert_eq!(drawer.height(), 224.0);
    let tile = h.get_by_label("Insert Box").rect();
    assert!(drawer.contains_rect(tile), "{tile:?} in {drawer:?}");
}

#[test]
fn the_height_the_person_chose_comes_back_when_the_drawer_is_opened_again() {
    let mut h = at(egui::vec2(1280.0, 800.0), with_item());
    h.run();
    let from = grip(&h);
    drag(&mut h, from, from - egui::vec2(0.0, 60.0));
    let chosen = drawer_rect(&h).height();
    assert!((chosen - 300.0).abs() <= 1.0, "{chosen}");
    for _ in 0..2 {
        h.state_mut().state.apply_verb(Verb::ToggleLibrary, &mut Vec::new());
        h.run();
    }
    assert_eq!(drawer_rect(&h).height(), chosen);
}

#[test]
fn a_height_kept_from_a_taller_window_is_brought_inside_the_limits() {
    let mut f = with_item();
    // Nothing selected: with a selection the ribbon card is under the strip and the drawer stays clear of it.
    select(&mut f.session, &[]);
    f.refresh();
    f.state.library_height = Some(5000.0);
    let mut h = at(egui::vec2(1280.0, 500.0), f);
    h.run();
    assert_eq!(drawer_rect(&h).height(), 350.0, "seventy percent of 500");
    let mut f = with_item();
    f.state.library_height = Some(1.0);
    let mut h = at(egui::vec2(1280.0, 800.0), f);
    h.run();
    assert_eq!(drawer_rect(&h).height(), 224.0);
}

#[test]
fn the_drawer_works_at_400_points_wide_with_larger_targets() {
    let mut h = at(egui::vec2(400.0, 800.0), with_item());
    h.run();
    let screen = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 800.0));
    let drawer = drawer_rect(&h);
    assert!(drawer.height() >= 320.0, "room for two rows of header and a row of tiles: {drawer:?}");
    for label in ["Add selection to library", "Export library", "Show or hide the library", "Import…", "Insert Box"] {
        let r = in_drawer(&h, label);
        assert!(screen.contains_rect(r), "{label} at {r:?}");
    }
    assert!(in_drawer(&h, "Show or hide the library").height() >= 34.0);
    assert!(h.get_by_label("Add selection to library").rect().height() >= 44.0);
    for label in ["Library item name", "Search the library"] {
        let r = h.get_by_label(label).rect();
        assert!(drawer.contains_rect(r), "{label} at {r:?}");
    }
    // There is no pointer to hover with: the buttons of a tile are there.
    assert!(drawer.contains_rect(h.get_by_label("Remove Box").rect()));
    assert!(h.get_by_label("Rename Box").rect().width() >= 34.0);
    h.state_mut().clear();
    h.get_by_label("Insert Box").click();
    h.run();
    assert_eq!(calls(&h).len(), 1);
    h.state_mut().clear();
    let close = in_drawer(&h, "Show or hide the library");
    dialog::click_at(&mut h, close.center());
    assert_eq!(dialog::verbs(h.state()), vec![Verb::ToggleLibrary]);
}

// ---- search ----

/// A fixture with three items named as given, each from its own rectangle.
fn with_named(names: &[&str]) -> Fixture {
    let mut f = dialog::fixture();
    for (n, name) in names.iter().enumerate() {
        let id = add_rect(&mut f.session, 0.0, 100.0 * n as f64, 60.0 + 20.0 * n as f64, 40.0).unwrap();
        select(&mut f.session, std::slice::from_ref(&id));
        dialog::run(&mut f, "library.add", json!({ "name": name }));
    }
    open(f)
}

#[test]
fn the_search_field_filters_the_tiles_by_name_whatever_the_case() {
    let mut h = dialog::harness(with_named(&["Box", "Crate", "Arrow"]));
    h.run();
    h.get_by_label("3 items");
    for name in ["Insert Box", "Insert Crate", "Insert Arrow"] {
        h.get_by_label(name);
    }
    dialog::field(&h, "Search the library").focus();
    h.run();
    dialog::field(&h, "Search the library").type_text("CRA");
    h.run();
    h.get_by_label("Insert Crate");
    assert!(h.query_by_label("Insert Box").is_none() && h.query_by_label("Insert Arrow").is_none());
    h.get_by_label("1 of 3 items");
    // No match says so, and what was typed is kept.
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::A);
    dialog::field(&h, "Search the library").type_text("zzz");
    h.run();
    h.get_by_label("No item has “zzz” in its name.");
    assert!(h.query_by_label("Insert Crate").is_none());
    // Emptying the field brings every tile back.
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::A);
    h.key_press(egui::Key::Backspace);
    h.run();
    for name in ["Insert Box", "Insert Crate", "Insert Arrow"] {
        h.get_by_label(name);
    }
}

#[test]
fn a_filtered_tile_still_inserts_the_item_it_shows() {
    let f = with_named(&["Box", "Crate"]);
    let crate_id = f.model.library[1].id.clone();
    let mut h = dialog::harness(f);
    h.run();
    dialog::field(&h, "Search the library").focus();
    h.run();
    dialog::field(&h, "Search the library").type_text("crate");
    h.run();
    h.state_mut().clear();
    h.get_by_label("Insert Crate").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::Ui, "library.insert", json!({ "item": crate_id }))]);
}

// ---- the pictures of the tiles ----

/// A renderer that draws what the engine asks for as a real picture: the outline of the box of every element,
/// one pixel wide, in black, or in light grey when the dark palette is asked for. The rest is transparent.
#[derive(Clone, Default)]
struct OutlineRenderer {
    /// How many pictures were asked for.
    asked: Arc<AtomicUsize>,
    /// The palette of each request.
    dark: Arc<Mutex<Vec<bool>>>,
    /// The renderer is still drawing, as the browser's is for a while: it answers `NotReady`.
    busy: Arc<AtomicBool>,
}

const LIGHT_INK: [u8; 4] = [0, 0, 0, 255];
const DARK_INK: [u8; 4] = [230, 230, 230, 255];

impl Renderer for OutlineRenderer {
    fn render_png(&self, scene: &SceneView, req: &RenderRequest) -> Result<Vec<u8>, PortError> {
        if self.busy.load(Ordering::SeqCst) {
            return Err(PortError::NotReady);
        }
        self.asked.fetch_add(1, Ordering::SeqCst);
        self.dark.lock().unwrap_or_else(PoisonError::into_inner).push(req.dark);
        let (w, h) = req.size_px;
        let (sx, sy) = (f64::from(w) / req.region.2, f64::from(h) / req.region.3);
        let ink = if req.dark { DARK_INK } else { LIGHT_INK };
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        let mut put = |x: i64, y: i64| {
            if (0..i64::from(w)).contains(&x) && (0..i64::from(h)).contains(&y) {
                let at = ((y as u32 * w + x as u32) * 4) as usize;
                rgba[at..at + 4].copy_from_slice(&ink);
            }
        };
        for e in scene.elements.iter().filter(|e| !e.deleted) {
            let t = &e.transform;
            let (x0, y0) = (((t.x - req.region.0) * sx).round() as i64, ((t.y - req.region.1) * sy).round() as i64);
            let (x1, y1) = (
                ((t.x + t.w - req.region.0) * sx).round() as i64 - 1,
                ((t.y + t.h - req.region.1) * sy).round() as i64 - 1,
            );
            for x in x0..=x1 {
                put(x, y0);
                put(x, y1);
            }
            for y in y0..=y1 {
                put(x0, y);
                put(x1, y);
            }
        }
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(|e| PortError::Failed(e.to_string()))?;
        writer.write_image_data(&rgba).map_err(|e| PortError::Failed(e.to_string()))?;
        writer.finish().map_err(|e| PortError::Failed(e.to_string()))?;
        Ok(out)
    }
}

/// The real shell over a session whose renderer is the [`OutlineRenderer`], with the drawer open.
struct App {
    shell: UiShell,
    session: Session,
}

fn app_with(renderer: &OutlineRenderer, items: &[(&str, f64, f64)], theme: &str) -> App {
    let fakes = Fakes::new();
    let mut ports = fakes.ports();
    ports.renderer = Some(Box::new(renderer.clone()));
    let mut session = Session::with_services(ports, Services::fake());
    for (name, w, h) in items {
        let id = add_rect(&mut session, 0.0, 0.0, *w, *h).unwrap();
        select(&mut session, std::slice::from_ref(&id));
        session.execute("library.add", json!({ "name": name })).unwrap();
    }
    // The person's choice of theme, which the shell follows (the test window has no system theme to follow).
    session.execute("settings.set", json!({ "key": "theme", "value": theme })).unwrap();
    let mut shell = UiShell::new(false);
    shell.state.apply_verb(Verb::ToggleLibrary, &mut Vec::new());
    App { shell, session }
}

fn app_harness(app: App) -> Harness<'static, App> {
    Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .with_max_steps(40)
        .with_context_setup(drafftink_ui::theme::install_fonts)
        .build_ui_state(
            |ui, app: &mut App| {
                app.shell.frame(ui, &mut app.session, FrameInput::default());
            },
            app,
        )
}

/// The picture of the item with this name, if the model has one that is drawn.
fn drawn(app: &App, name: &str, size: ThumbSize, dark: bool) -> Option<Arc<ThumbImage>> {
    let item = app.shell.model.library.iter().find(|i| i.name == name)?;
    match app.shell.model.thumbnails.picture(item, size, dark)? {
        Thumb::Ready(image) => Some(Arc::clone(image)),
        Thumb::Pending | Thumb::Failed(_) => None,
    }
}

/// The rectangles of the pictures (meshes with a texture of their own) in what the last frame painted.
fn textured(h: &Harness<'_, App>) -> Vec<egui::Rect> {
    fn walk(shape: &egui::Shape, out: &mut Vec<egui::Rect>) {
        match shape {
            egui::Shape::Vec(inner) => inner.iter().for_each(|s| walk(s, out)),
            egui::Shape::Mesh(m) if m.texture_id != egui::TextureId::default() => out.push(m.calc_bounds()),
            _ => {}
        }
    }
    let mut out = Vec::new();
    h.output().shapes.iter().for_each(|clipped| walk(&clipped.shape, &mut out));
    out
}

fn opaque_pixels(image: &ThumbImage) -> usize {
    image.rgba.chunks_exact(4).filter(|p| p[3] == 255).count()
}

#[test]
fn a_tile_shows_an_outline_until_its_picture_arrives_and_then_the_picture() {
    let renderer = OutlineRenderer::default();
    // The renderer is busy, as the browser's is for a while: the tile keeps its outline.
    renderer.busy.store(true, Ordering::SeqCst);
    let mut h = app_harness(app_with(&renderer, &[("Box", 80.0, 40.0)], "light"));
    h.run();
    let tile = h.get_by_label("Insert Box").rect();
    let before = (drawn(h.state(), "Box", ThumbSize::Tile, false), textured(&h));
    assert!(before.0.is_none() && before.1.is_empty(), "no picture yet, so the outline: {before:?}");
    // The renderer wakes the app, and the frame draws the picture.
    renderer.busy.store(false, Ordering::SeqCst);
    h.step();
    h.run();
    let picture = drawn(h.state(), "Box", ThumbSize::Tile, false).expect("the picture arrived");
    let shown = textured(&h);
    assert_eq!(shown.len(), 1, "one picture on the screen: {shown:?}");
    assert!(tile.contains_rect(shown[0]), "the picture is inside its tile: {:?} in {tile:?}", shown[0]);

    // 80 by 40 units and 4 units of air around: 88 by 48, drawn 224 pixels wide.
    assert_eq!(picture.size, [224, 122]);
    let aspect = |w: f32, h: f32| w / h;
    assert!(
        (aspect(shown[0].width(), shown[0].height()) - aspect(224.0, 122.0)).abs() < 0.02,
        "the picture keeps its proportions: {:?}",
        shown[0]
    );
    // The pixels are the item and not the outline of a placeholder: the box's border in ink, the inside and the
    // corners of the picture transparent.
    assert_eq!(picture.pixel(0, 0), Some([0, 0, 0, 0]), "the air is transparent");
    assert_eq!(picture.pixel(112, 61), Some([0, 0, 0, 0]), "the inside of the box is transparent");
    assert_eq!(
        picture.pixel(10, 61),
        Some(LIGHT_INK),
        "the left side of the box: 4 units of air at 224/88 pixels each"
    );
    assert_eq!(picture.pixel(112, 10), Some(LIGHT_INK), "the top side");
    let perimeter = 2 * (205 + 102);
    assert!(opaque_pixels(&picture).abs_diff(perimeter) < 20, "{} opaque pixels", opaque_pixels(&picture));
}

#[test]
fn an_item_is_drawn_once_and_the_frames_after_it_are_idle() {
    let renderer = OutlineRenderer::default();
    let mut h = app_harness(app_with(&renderer, &[("Box", 80.0, 40.0)], "light"));
    h.run();
    assert_eq!(renderer.asked.load(Ordering::SeqCst), 1);
    // Nothing changed: no repaint is wanted and nothing is asked for again.
    assert!(h.try_run().is_ok(), "idle means idle");
    for _ in 0..3 {
        h.step();
    }
    assert_eq!(renderer.asked.load(Ordering::SeqCst), 1);
}

#[test]
fn a_full_drawer_asks_for_a_few_pictures_a_frame() {
    let renderer = OutlineRenderer::default();
    let items: Vec<(String, f64, f64)> =
        (0..7).map(|n| (format!("Item {n}"), 40.0 + 10.0 * f64::from(n), 30.0)).collect();
    let items: Vec<(&str, f64, f64)> = items.iter().map(|(n, w, h)| (n.as_str(), *w, *h)).collect();
    let mut app = app_with(&renderer, &items, "light");
    app.shell.state.library_open = false;
    let mut h = app_harness(app);
    h.run();
    assert_eq!(renderer.asked.load(Ordering::SeqCst), 0, "a closed drawer asks for nothing");
    h.state_mut().shell.state.apply_verb(Verb::ToggleLibrary, &mut Vec::new());
    let mut asked = vec![0];
    for _ in 0..8 {
        h.step();
        asked.push(renderer.asked.load(Ordering::SeqCst));
    }
    let per_frame: Vec<usize> = asked.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(per_frame.iter().all(|n| *n <= PER_FRAME), "at most {PER_FRAME} pictures a frame: {per_frame:?}");
    assert!(per_frame.contains(&PER_FRAME), "and the budget is used: {per_frame:?}");
    assert_eq!(asked.last(), Some(&7), "every tile has its picture in the end");
    for item in &items {
        assert!(drawn(h.state(), item.0, ThumbSize::Tile, false).is_some(), "{}", item.0);
    }
    assert!(h.try_run().is_ok(), "and the drawer is idle again");
}

#[test]
fn tiles_that_are_scrolled_out_of_the_drawer_are_not_drawn_until_they_come_into_view() {
    let renderer = OutlineRenderer::default();
    let names: Vec<String> = (0..60).map(|n| format!("Item {n:02}")).collect();
    let items: Vec<(&str, f64, f64)> = names.iter().map(|n| (n.as_str(), 50.0, 30.0)).collect();
    let mut h = app_harness(app_with(&renderer, &items, "light"));
    h.run();
    let asked = renderer.asked.load(Ordering::SeqCst);
    assert!(asked > 0 && asked < 60, "only the tiles in the drawer: {asked}");
    assert!(drawn(h.state(), "Item 59", ThumbSize::Tile, false).is_none());
    // The mouse wheel over the tiles brings more into view.
    let tile = h.get_by_label("Insert Item 00").rect().center();
    h.event(Event::PointerMoved(tile));
    h.event(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -3000.0),
        modifiers: Modifiers::NONE,
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
    });
    h.run();
    assert!(renderer.asked.load(Ordering::SeqCst) > asked);
    assert!(drawn(h.state(), "Item 59", ThumbSize::Tile, false).is_some());
}

#[test]
fn the_dark_theme_draws_the_pictures_with_the_dark_palette_and_the_light_one_draws_them_again() {
    let renderer = OutlineRenderer::default();
    let mut h = app_harness(app_with(&renderer, &[("Box", 80.0, 40.0)], "dark"));
    h.run();
    let dark = drawn(h.state(), "Box", ThumbSize::Tile, true).expect("drawn for the dark canvas");
    assert_eq!(dark.pixel(10, 61), Some(DARK_INK), "light strokes on the dark panel");
    assert_eq!(*renderer.dark.lock().unwrap(), vec![true]);
    assert!(drawn(h.state(), "Box", ThumbSize::Tile, false).is_none(), "and nothing is kept for the other theme");
    // The setting follows the person's choice.
    h.state_mut().session.execute("settings.set", json!({ "key": "theme", "value": "light" })).unwrap();
    h.run();
    let light = drawn(h.state(), "Box", ThumbSize::Tile, false).expect("drawn again for the light canvas");
    assert_eq!(light.pixel(10, 61), Some(LIGHT_INK));
    assert_eq!(*renderer.dark.lock().unwrap(), vec![true, false]);
    assert_ne!(light.pixel(10, 61), dark.pixel(10, 61));
}

#[test]
fn a_new_item_gets_a_picture_and_the_others_keep_theirs() {
    let renderer = OutlineRenderer::default();
    let mut h = app_harness(app_with(&renderer, &[("Box", 80.0, 40.0)], "light"));
    h.run();
    let before = drawn(h.state(), "Box", ThumbSize::Tile, false).unwrap();
    let id = add_rect(&mut h.state_mut().session, 0.0, 200.0, 30.0, 60.0).unwrap();
    select(&mut h.state_mut().session, std::slice::from_ref(&id));
    h.state_mut().session.execute("library.add", json!({ "name": "Tall" })).unwrap();
    h.state_mut().shell.model.invalidate();
    h.run();
    let tall = drawn(h.state(), "Tall", ThumbSize::Tile, false).expect("the new item has a picture");
    assert!(tall.size[1] > tall.size[0], "tall: {:?}", tall.size);
    assert!(Arc::ptr_eq(&before, &drawn(h.state(), "Box", ThumbSize::Tile, false).unwrap()), "Box kept its picture");
    assert_eq!(renderer.asked.load(Ordering::SeqCst), 2);
}

#[test]
fn renaming_an_item_keeps_its_picture() {
    let renderer = OutlineRenderer::default();
    let mut h = app_harness(app_with(&renderer, &[("Box", 80.0, 40.0)], "light"));
    h.run();
    h.state_mut().session.execute("library.rename", json!({ "item": "Box", "name": "Crate" })).unwrap();
    h.state_mut().shell.model.invalidate();
    h.run();
    assert!(drawn(h.state(), "Crate", ThumbSize::Tile, false).is_some());
    assert_eq!(renderer.asked.load(Ordering::SeqCst), 1, "the drawing did not change, so nothing is drawn again");
}

#[test]
fn a_picture_the_browser_is_still_drawing_is_asked_for_again_when_it_wakes_the_app() {
    let renderer = OutlineRenderer::default();
    renderer.busy.store(true, Ordering::SeqCst);
    let mut h = app_harness(app_with(&renderer, &[("Box", 80.0, 40.0)], "light"));
    // Busy: the tile keeps its outline, and the app does not spin asking again (the renderer wakes it).
    assert!(h.try_run().is_ok(), "no repaint is requested while the picture is being drawn");
    assert!(drawn(h.state(), "Box", ThumbSize::Tile, false).is_none());
    h.get_by_label("Insert Box");
    renderer.busy.store(false, Ordering::SeqCst);
    // The wake-up is a frame.
    h.run();
    assert!(drawn(h.state(), "Box", ThumbSize::Tile, false).is_some());
}

#[test]
fn a_renderer_that_cannot_draw_leaves_the_outline_and_the_tile_works() {
    // The fake renderer of the other tests answers bytes that are not a PNG; a session may have no renderer at all.
    for no_renderer in [false, true] {
        let fakes = Fakes::new();
        let mut ports = fakes.ports();
        if no_renderer {
            ports.renderer = None;
        }
        let mut session = Session::with_services(ports, Services::fake());
        let id = add_rect(&mut session, 0.0, 0.0, 80.0, 40.0).unwrap();
        select(&mut session, std::slice::from_ref(&id));
        session.execute("library.add", json!({ "name": "Box" })).unwrap();
        session.execute("settings.set", json!({ "key": "theme", "value": "light" })).unwrap();
        let mut shell = UiShell::new(false);
        shell.state.apply_verb(Verb::ToggleLibrary, &mut Vec::new());
        let mut h = app_harness(App { shell, session });
        h.run();
        assert!(drawn(h.state(), "Box", ThumbSize::Tile, false).is_none());
        assert!(textured(&h).is_empty());
        h.get_by_label("Insert Box").click();
        h.run();
        assert_eq!(h.state().session.store().len(), 2, "the click inserted a copy (no_renderer: {no_renderer})");
    }
}

#[test]
fn hovering_a_tile_shows_a_larger_picture_and_the_element_count() {
    let renderer = OutlineRenderer::default();
    let mut h = app_harness(app_with(&renderer, &[("Box", 80.0, 40.0)], "light"));
    h.run();
    let tile = h.get_by_label("Insert Box").rect();
    h.event(Event::PointerMoved(tile.center()));
    // The tooltip opens after the pointer rests on the tile for a moment.
    for _ in 0..60 {
        h.step();
        if h.query_by_label("1 element").is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    h.get_by_label("1 element");
    h.run();
    let preview =
        drawn(h.state(), "Box", ThumbSize::Preview, false).expect("the larger picture was drawn for the tooltip");
    assert_eq!(preview.size[0], 480);
    assert!(opaque_pixels(&preview) > opaque_pixels(&drawn(h.state(), "Box", ThumbSize::Tile, false).unwrap()));
}

// ---- import ----

/// An `.excalidrawlib` file with one item per `(name, w, h)`, as `library.export` of another session writes it.
fn library_file(items: &[(&str, f64, f64)]) -> String {
    let mut other = Session::with_services(Fakes::new().ports(), Services::fake());
    for (name, w, h) in items {
        let id = add_rect(&mut other, 0.0, 0.0, *w, *h).unwrap();
        select(&mut other, std::slice::from_ref(&id));
        other.execute("library.add", json!({ "name": name })).unwrap();
    }
    let out = other.execute("library.export", json!({})).unwrap();
    out["bytes"]["base64"].as_str().unwrap().to_owned()
}

#[test]
fn an_imported_library_gets_real_pictures_without_any_other_interaction() {
    let renderer = OutlineRenderer::default();
    let mut h = app_harness(app_with(&renderer, &[("Box", 80.0, 40.0)], "light"));
    h.run();
    let kept = drawn(h.state(), "Box", ThumbSize::Tile, false).expect("Box is drawn");
    // The app runs the import from its inbox, between frames and not through the shell's dispatch.
    let file = library_file(&[("Wide", 120.0, 30.0), ("Tall", 30.0, 90.0)]);
    let source = json!({ "base64": file, "name": "shapes.excalidrawlib" });
    h.state_mut().session.execute_from(Origin::Ui, "library.import", json!({ "source": source })).unwrap();
    h.run();
    let wide = drawn(h.state(), "Wide", ThumbSize::Tile, false).expect("the imported item has a picture");
    let tall = drawn(h.state(), "Tall", ThumbSize::Tile, false).expect("the imported item has a picture");
    assert!(wide.size[0] > wide.size[1] && tall.size[1] > tall.size[0], "{:?} {:?}", wide.size, tall.size);
    assert!(opaque_pixels(&wide) > 0 && opaque_pixels(&tall) > 0, "real pictures, not empty ones");
    assert!(Arc::ptr_eq(&kept, &drawn(h.state(), "Box", ThumbSize::Tile, false).unwrap()), "Box kept its picture");
    // The new section is brought into view, and its pictures are on the screen.
    let drawer = h.state().shell.state.chrome.library.expect("the drawer");
    for name in ["Wide", "Tall"] {
        let tile = h.get_by_label(&format!("Insert {name}")).rect();
        assert!(drawer.contains_rect(tile), "{name} at {tile:?} in {drawer:?}");
        assert!(textured(&h).iter().any(|pic| tile.contains_rect(*pic)), "{name} shows its picture");
    }
    assert_eq!(renderer.asked.load(Ordering::SeqCst), 3);
}

// ---- hover ----

/// The rectangle of every tile, in order.
fn tile_rects(h: &H, names: &[&str]) -> Vec<egui::Rect> {
    names.iter().map(|n| h.get_by_label(&format!("Insert {n}")).rect()).collect()
}

#[test]
fn hovering_a_tile_moves_and_resizes_no_tile() {
    let names = ["Box", "Crate", "Arrow", "Frame", "Logo", "Star"];
    for size in [egui::vec2(1280.0, 800.0), egui::vec2(520.0, 800.0)] {
        let mut h = at(size, with_named(&names));
        h.run();
        let calm = tile_rects(&h, &names);
        // Far from every tile: the first frame with the pointer in the window changes nothing either.
        for hovered in [0, 1, names.len() - 1] {
            h.event(Event::PointerMoved(calm[hovered].center()));
            h.run();
            h.get_by_label(&format!("Rename {}", names[hovered]));
            assert_eq!(tile_rects(&h, &names), calm, "{size:?}: hovering {} moved a tile", names[hovered]);
        }
        h.event(Event::PointerMoved(Pos2::new(2.0, 2.0)));
        h.run();
        assert_eq!(tile_rects(&h, &names), calm, "{size:?}: the pointer leaving moved a tile");
    }
}

// ---- sections ----

/// A fixture with one item of its own, "Box", and a library file "shapes.excalidrawlib" with "Wide" and "Tall"
/// imported, with the drawer open.
fn with_sections() -> Fixture {
    let mut f = dialog::fixture();
    let id = add_rect(&mut f.session, 0.0, 0.0, 80.0, 40.0).unwrap();
    select(&mut f.session, std::slice::from_ref(&id));
    dialog::run(&mut f, "library.add", json!({ "name": "Box" }));
    let file = library_file(&[("Wide", 120.0, 30.0), ("Tall", 30.0, 90.0)]);
    dialog::run(&mut f, "library.import", json!({ "source": { "base64": file, "name": "shapes.excalidrawlib" } }));
    // Tall enough for both sections.
    f.state.library_height = Some(480.0);
    open(f)
}

fn section_id(f: &Fixture, name: &str) -> String {
    f.model.library_sections.iter().find(|s| s.name == name).unwrap().id.clone()
}

fn button<'a>(h: &'a H, label: &'a str) -> egui_kittest::Node<'a> {
    h.get_by_role_and_label(egui::accesskit::Role::Button, label)
}

#[test]
fn the_items_are_grouped_in_sections_with_a_header_each() {
    let f = with_sections();
    assert_eq!(
        f.model.library_sections.iter().map(|s| (s.name.as_str(), s.items, s.mine)).collect::<Vec<_>>(),
        [("My items", 1, true), ("shapes", 2, false)]
    );
    let mut h = dialog::harness(f);
    h.run();
    let (mine, shapes) = (button(&h, "Collapse My items").rect(), button(&h, "Collapse shapes").rect());
    assert!(mine.top() < shapes.top(), "the person's own items come first");
    let (boxed, wide, tall) = (
        h.get_by_label("Insert Box").rect(),
        h.get_by_label("Insert Wide").rect(),
        h.get_by_label("Insert Tall").rect(),
    );
    assert!(
        mine.bottom() <= boxed.top() && boxed.bottom() <= shapes.top(),
        "Box sits under its header: {mine:?} {boxed:?} {shapes:?}"
    );
    assert!(shapes.bottom() <= wide.top() && wide.top() == tall.top(), "{shapes:?} {wide:?} {tall:?}");
    h.get_by_label("3 items");
}

#[test]
fn a_click_on_a_header_folds_and_unfolds_its_section() {
    let mut h = dialog::harness(with_sections());
    h.run();
    h.state_mut().clear();
    button(&h, "Collapse shapes").click();
    h.run();
    assert!(h.state().calls().is_empty(), "folding is the UI's own");
    assert!(h.query_by_label("Insert Wide").is_none() && h.query_by_label("Insert Tall").is_none());
    h.get_by_label("Insert Box");
    button(&h, "Expand shapes").click();
    h.run();
    h.get_by_label("Insert Wide");
}

#[test]
fn a_folded_section_asks_for_no_pictures() {
    let renderer = OutlineRenderer::default();
    let mut app = app_with(&renderer, &[("Box", 80.0, 40.0)], "light");
    let file = library_file(&[("Wide", 120.0, 30.0)]);
    let source = json!({ "base64": file, "name": "shapes.excalidrawlib" });
    app.session.execute("library.import", json!({ "source": source })).unwrap();
    let id = app.session.execute("library.list", json!({})).unwrap()["sections"][1]["id"].as_str().unwrap().to_owned();
    app.shell.state.library_collapsed.insert(id);
    let mut h = app_harness(app);
    h.run();
    assert!(drawn(h.state(), "Box", ThumbSize::Tile, false).is_some());
    assert!(
        drawn(h.state(), "Wide", ThumbSize::Tile, false).is_none(),
        "its tile is not drawn, so its picture is not asked for"
    );
    assert_eq!(renderer.asked.load(Ordering::SeqCst), 1);
}

#[test]
fn the_search_filters_within_sections_hides_empty_ones_and_shows_matches_of_folded_ones() {
    let mut f = with_sections();
    f.state.library_collapsed.insert(section_id(&f, "shapes"));
    let mut h = dialog::harness(f);
    h.run();
    dialog::field(&h, "Search the library").focus();
    h.run();
    dialog::field(&h, "Search the library").type_text("wid");
    h.run();
    h.get_by_label("Insert Wide");
    assert!(h.query_by_label("Insert Tall").is_none() && h.query_by_label("Insert Box").is_none());
    assert!(
        h.query_by_role_and_label(egui::accesskit::Role::Button, "Collapse My items").is_none(),
        "no match, no header"
    );
    assert!(h.query_by_role_and_label(egui::accesskit::Role::Button, "Expand My items").is_none());
    h.get_by_label("1 of 3 items");
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::A);
    dialog::field(&h, "Search the library").type_text("zzz");
    h.run();
    h.get_by_label("No item has “zzz” in its name.");
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::A);
    h.key_press(egui::Key::Backspace);
    h.run();
    assert!(h.query_by_label("Insert Wide").is_none(), "folded again once the search is empty");
    h.get_by_label("Insert Box");
}

#[test]
fn removing_a_section_asks_first_and_then_runs_library_remove_section() {
    let f = with_sections();
    let id = section_id(&f, "shapes");
    let mut h = dialog::harness(f);
    h.run();
    h.state_mut().clear();
    button(&h, "Remove section shapes").click();
    h.run();
    assert!(h.state().calls().is_empty(), "nothing runs before the answer");
    h.get_by_label("Remove “shapes” and its 2 items from the library? This cannot be undone.");
    // Cancel leaves everything.
    h.state_mut().clear();
    button(&h, "Cancel").click();
    h.run();
    assert!(h.state().calls().is_empty());
    h.state_mut().dispatch();
    assert!(h.state().state.library_remove.is_none());
    assert_eq!(h.state().model.library.len(), 3);
    // Ask again and confirm.
    button(&h, "Remove section shapes").click();
    h.run();
    h.state_mut().clear();
    button(&h, "Remove the section").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::Ui, "library.remove_section", json!({ "section": id }))]);
    h.state_mut().dispatch();
    assert!(h.state().state.library_remove.is_none(), "the dialog closes");
    let names: Vec<_> = h.state().model.library.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["Box"]);
    assert_eq!(h.state().model.library_sections.len(), 1);
    h.run();
    assert!(h.query_by_role_and_label(egui::accesskit::Role::Button, "Collapse shapes").is_none());
}

#[test]
fn my_items_has_a_remove_button_and_no_rename_button() {
    let mut h = dialog::harness(with_sections());
    h.run();
    button(&h, "Remove section My items");
    assert!(h.query_by_label("Rename section My items").is_none());
    button(&h, "Rename section shapes");
}

#[test]
fn the_rename_button_of_a_section_renames_it_with_library_rename_section() {
    let f = with_sections();
    let id = section_id(&f, "shapes");
    let mut h = dialog::harness(f);
    h.run();
    button(&h, "Rename section shapes").click();
    h.run();
    assert_eq!(h.state().state.rename.as_ref().map(|r| r.text.as_str()), Some("shapes"));
    dialog::field(&h, "Name").focus();
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::A);
    dialog::field(&h, "Name").type_text("Flowchart");
    h.run();
    h.state_mut().clear();
    button(&h, "Rename").click();
    h.run();
    assert_eq!(
        calls(&h),
        vec![Call::new(Origin::Ui, "library.rename_section", json!({ "section": id, "name": "Flowchart" }))]
    );
    h.state_mut().dispatch();
    h.run();
    assert!(h.state().state.rename.is_none());
    button(&h, "Collapse Flowchart");
}

const SECTION_MENU: &str = "Library section menu";

fn section_menu_row<'a>(h: &'a H, text: &'a str) -> egui_kittest::Node<'a> {
    h.get_by_role_and_label(egui::accesskit::Role::Menu, SECTION_MENU)
        .query_by_role_and_label(egui::accesskit::Role::Button, text)
        .unwrap_or_else(|| panic!("no row {text:?} in the section menu"))
}

#[test]
fn a_right_click_on_a_header_opens_a_menu_with_collapse_rename_and_remove() {
    let mut h = dialog::harness(with_sections());
    h.run();
    button(&h, "Collapse shapes").click_secondary();
    h.run();
    for row in ["Collapse", "Rename…", "Remove section…"] {
        section_menu_row(&h, row);
    }
    assert!(h.state().calls().is_empty(), "opening the menu runs nothing");
    section_menu_row(&h, "Collapse").click();
    h.run();
    assert!(h.query_by_label("Insert Wide").is_none(), "folded");
    assert!(h.state().state.library_menu.is_none(), "choosing a row closes the menu");

    button(&h, "Expand shapes").click_secondary();
    h.run();
    section_menu_row(&h, "Rename…").click();
    h.run();
    assert!(h.state().state.rename.is_some());

    let mut h = dialog::harness(with_sections());
    h.run();
    button(&h, "Collapse shapes").click_secondary();
    h.run();
    section_menu_row(&h, "Remove section…").click();
    h.run();
    h.get_by_label("Remove “shapes” and its 2 items from the library? This cannot be undone.");
    assert!(h.state().calls().is_empty());
}

#[test]
fn the_menu_of_my_items_has_no_rename_row() {
    let mut h = dialog::harness(with_sections());
    h.run();
    button(&h, "Collapse My items").click_secondary();
    h.run();
    section_menu_row(&h, "Remove section…");
    let menu = h.get_by_role_and_label(egui::accesskit::Role::Menu, SECTION_MENU);
    assert!(menu.query_by_role_and_label(egui::accesskit::Role::Button, "Rename…").is_none());
}

#[test]
fn the_headers_and_their_buttons_fit_a_narrow_drawer_with_touch_sized_targets() {
    let mut h = at(egui::vec2(400.0, 800.0), with_sections());
    h.run();
    let drawer = drawer_rect(&h);
    for label in ["Collapse My items", "Collapse shapes", "Remove section shapes", "Rename section shapes"] {
        let r = h.get_all_by_label(label).map(|n| n.rect()).find(|r| drawer.contains_rect(*r));
        assert!(r.is_some(), "{label} inside {drawer:?}");
    }
    assert!(h.get_by_label("Remove section shapes").rect().height() >= 34.0, "a touch target");
}

#[test]
fn a_section_the_person_folded_stays_folded_when_the_library_changes() {
    let mut f = with_sections();
    f.state.library_collapsed.insert(section_id(&f, "shapes"));
    let mut h = dialog::harness(f);
    h.run();
    let id = h.state().model.library[0].id.clone();
    dialog::run(h.state_mut(), "library.rename", json!({ "item": id, "name": "Boxy" }));
    h.run();
    assert!(h.query_by_label("Insert Wide").is_none());
    h.get_by_label("Insert Boxy");
}
