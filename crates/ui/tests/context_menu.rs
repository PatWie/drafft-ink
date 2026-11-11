//! The canvas context menu: the rows, the calls they make (origin `ContextMenu`), closing, and the greyed
//! rows with the refusal text.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;
mod support_a;

use common::Fixture;
use drafftink_engine::Origin;
use drafftink_engine::testing::{add_rect, select};
use drafftink_ui::surfaces::context_menu::ALIGN_EDGES;
use drafftink_ui::{Call, UiAction, Verb, args_form, model};
use egui::accesskit::Role;
use egui::pos2;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use support_a::themed_lenient as themed;

type H = egui_kittest::Harness<'static, Fixture>;

/// A fixture with `n` rectangles, all selected, and the menu open at `at`.
fn menu_with(n: usize, at: egui::Pos2) -> H {
    menu_over(Fixture::new(), n, at, None)
}

/// Like [`menu_with`] over a given fixture, with the world point under the click.
fn menu_over(mut f: Fixture, n: usize, at: egui::Pos2, world: Option<[f64; 2]>) -> H {
    let mut ids = Vec::new();
    for i in 0..n {
        ids.push(add_rect(&mut f.session, 40.0 * i as f64, 0.0, 30.0, 30.0).unwrap());
    }
    select(&mut f.session, &ids);
    f.refresh();
    f.state.open_context_menu(at, world);
    let mut h = themed(f, false);
    h.run();
    h
}

/// True when a node lies where the menu opened by `menu_with(_, pos2(300, 200))` is drawn. The chrome (tool column on the
/// left, bottom bar and tab bar at the bottom, right panel) has buttons of the same names ("Undo", "Zoom to fit",
/// "Align left"), so rows are told apart by position.
fn in_menu(n: &egui_kittest::Node<'_>) -> bool {
    let r = n.rect();
    (290.0..640.0).contains(&r.min.x) && (55.0..745.0).contains(&r.min.y)
}

/// The menu row with this text.
fn row<'a>(h: &'a H, text: &'a str) -> egui_kittest::Node<'a> {
    h.get_all_by_role_and_label(Role::Button, text).find(in_menu).unwrap_or_else(|| panic!("no menu row {text}"))
}

fn has_row(h: &H, text: &str) -> bool {
    h.query_all_by_role_and_label(Role::Button, text).any(|n| in_menu(&n))
}

fn click_at(h: &mut H, pos: egui::Pos2) {
    h.hover_at(pos);
    h.run();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    h.run();
}

fn calls(h: &H) -> Vec<Call> {
    h.state().calls().into_iter().cloned().collect()
}

const ROWS: [&str; 15] = [
    "Cut",
    "Copy",
    "Paste",
    "Duplicate",
    "Delete",
    "Group",
    "Ungroup",
    "Bring to front",
    "Bring forward",
    "Send backward",
    "Send to back",
    "Lock or unlock",
    "Select all",
    "Paste style",
    "Zoom to fit",
];

#[test]
fn opening_shows_the_rows() {
    let h = menu_with(1, pos2(300.0, 60.0));
    for text in ROWS {
        assert!(has_row(&h, text), "missing row {text}");
    }
    assert!(has_row(&h, "Copy style"));
    assert!(!has_row(&h, "Undo") && !has_row(&h, "Redo"), "undo and redo are not in this menu");
}

#[test]
fn a_click_on_a_row_makes_the_call_with_origin_context_menu_and_closes_the_menu() {
    let mut h = menu_with(1, pos2(300.0, 60.0));
    row(&h, "Zoom to fit").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::ContextMenu, "view.zoom_to_fit", json!({ "scope": "all" }))]);
    assert!(h.state().state.context_menu.is_none(), "the menu closed");
}

#[test]
fn z_order_rows_carry_their_op() {
    for (text, op) in [
        ("Bring to front", "front"),
        ("Bring forward", "forward"),
        ("Send backward", "backward"),
        ("Send to back", "back"),
    ] {
        let mut h = menu_with(2, pos2(300.0, 60.0));
        row(&h, text).click();
        h.run();
        assert_eq!(calls(&h), vec![Call::new(Origin::ContextMenu, "arrange.z_order", json!({ "op": op }))], "{text}");
    }
}

#[test]
fn group_with_a_selection_makes_the_call() {
    let mut h = menu_with(2, pos2(300.0, 60.0));
    row(&h, "Group").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::ContextMenu, "group.create", json!({}))]);
}

#[test]
fn a_refused_row_is_disabled_and_a_click_does_nothing() {
    let mut h = menu_with(0, pos2(300.0, 60.0));
    let reason = h.state().model.refusal_text("group.create").expect("group.create is refused with nothing selected");
    assert!(row(&h, "Group").accesskit_node().is_disabled());
    h.state_mut().clear();
    row(&h, "Group").click();
    h.run();
    assert!(calls(&h).is_empty(), "{:?}", calls(&h));
    assert!(h.state().state.context_menu.is_some(), "a refused row keeps the menu open");
    // The refusal text is the tooltip.
    row(&h, "Group").hover();
    h.run();
    h.get_by_label(&reason);
}

#[test]
fn a_command_the_registry_lacks_is_greyed_with_the_not_available_text() {
    let omitted = ["clipboard.cut", "selection.all", "element.lock"];
    let mut h = menu_over(Fixture::without(&omitted), 1, pos2(300.0, 60.0), None);
    for (text, id) in [("Cut", "clipboard.cut"), ("Select all", "selection.all"), ("Lock or unlock", "element.lock")] {
        let reason = h.state().model.refusal_text(id).expect("left out of the registry");
        assert_eq!(reason, format!("`{id}` is not available in this build."));
        assert!(row(&h, text).accesskit_node().is_disabled(), "{text} is greyed");
        row(&h, text).hover();
        h.run();
        h.get_by_label(&reason);
    }
    // The rows of the commands that are there can run.
    assert!(!row(&h, "Copy").accesskit_node().is_disabled());
}

#[test]
fn rows_of_unregistered_commands_never_run() {
    let omitted = ["clipboard.cut", "clipboard.copy", "clipboard.paste"];
    let mut h = menu_over(Fixture::without(&omitted), 1, pos2(300.0, 60.0), None);
    for text in ["Cut", "Copy", "Paste"] {
        h.state_mut().clear();
        row(&h, text).click();
        h.run();
        assert!(calls(&h).is_empty(), "{text} must not emit a call for an unregistered command");
        assert!(h.state().state.context_menu.is_some(), "{text}: the menu stays open");
    }
}

#[test]
fn rows_of_the_real_commands_are_enabled_or_refused_by_the_real_engine() {
    // One rectangle selected: the clipboard, delete and lock rows can run.
    let h = menu_with(1, pos2(300.0, 60.0));
    for (text, id) in [
        ("Cut", "clipboard.cut"),
        ("Copy", "clipboard.copy"),
        ("Duplicate", "element.duplicate"),
        ("Delete", "element.delete"),
        ("Lock or unlock", "element.lock"),
        ("Select all", "selection.all"),
    ] {
        assert!(h.state().model.is_available(id), "{id} is registered and available");
        assert!(!row(&h, text).accesskit_node().is_disabled(), "{text}");
    }
    // Nothing selected: the rows that need a selection are refused with the engine's text.
    let h = menu_with(0, pos2(300.0, 60.0));
    for (text, id) in [("Cut", "clipboard.cut"), ("Delete", "element.delete")] {
        assert!(h.state().model.refusal_text(id).is_some_and(|t| !t.contains("not available in this build")), "{id}");
        assert!(row(&h, text).accesskit_node().is_disabled(), "{text}");
    }
}

#[test]
fn delete_removes_the_selected_element_from_the_document() {
    let mut h = menu_with(2, pos2(300.0, 60.0));
    row(&h, "Delete").click();
    h.run();
    let report = h.state_mut().dispatch();
    assert_eq!(report.refusals().count(), 0, "{:?}", report.ran);
    assert_eq!(common_total(h.state_mut()), 0);
}

fn common_total(f: &mut Fixture) -> u64 {
    f.session.execute("element.query", json!({})).unwrap()["total"].as_u64().unwrap()
}

/// A copied rectangle on the clipboard, then the menu opened with the click at `world`.
fn paste_menu(world: Option<[f64; 2]>) -> H {
    let mut f = Fixture::new();
    let r = add_rect(&mut f.session, 0.0, 0.0, 40.0, 20.0).unwrap();
    select(&mut f.session, &[r]);
    f.session.execute("clipboard.copy", json!({})).unwrap();
    menu_over(f, 0, pos2(300.0, 60.0), world)
}

#[test]
fn paste_puts_the_content_where_the_menu_was_opened() {
    let mut h = paste_menu(Some([500.0, 400.0]));
    row(&h, "Paste").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::ContextMenu, "clipboard.paste", json!({ "at": [500.0, 400.0] }))]);
    let report = h.state_mut().dispatch();
    assert_eq!(report.refusals().count(), 0, "{:?}", report.ran);
    // The real command pasted a copy, selected it and centred it on the click.
    let got = h.state_mut().session.execute("selection.get", json!({})).unwrap();
    assert_eq!(got["count"], json!(1));
    let b = &got["bounds"];
    let (cx, cy) = (
        b["x"].as_f64().unwrap() + b["w"].as_f64().unwrap() / 2.0,
        b["y"].as_f64().unwrap() + b["h"].as_f64().unwrap() / 2.0,
    );
    assert!((cx - 500.0).abs() < 0.01 && (cy - 400.0).abs() < 0.01, "centre ({cx}, {cy}): {b}");
    assert_eq!(common_total(h.state_mut()), 2);
}

#[test]
fn paste_without_a_known_world_point_leaves_the_place_to_the_command() {
    let mut h = paste_menu(None);
    row(&h, "Paste").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::ContextMenu, "clipboard.paste", json!({}))]);
    let report = h.state_mut().dispatch();
    assert_eq!(report.refusals().count(), 0, "{:?}", report.ran);
    assert_eq!(common_total(h.state_mut()), 2);
}

#[test]
fn a_cursor_shortcut_pastes_at_the_pointer_with_the_real_command() {
    // The binding `Mod+V` carries `"at": "$cursor"`; the keymap fills it with a world point the command takes.
    let mut f = Fixture::new();
    let r = add_rect(&mut f.session, 0.0, 0.0, 40.0, 20.0).unwrap();
    select(&mut f.session, &[r]);
    f.session.execute("clipboard.copy", json!({})).unwrap();
    f.refresh();
    let args = drafftink_ui::keymap::substitute(
        &json!({ "at": "$cursor" }),
        &drafftink_ui::Pointer { cursor: Some((70.0, 30.0)), viewport_center: None },
    );
    f.actions.push(UiAction::run_from(Origin::Shortcut, "clipboard.paste", args));
    let report = f.dispatch();
    assert_eq!(report.refusals().count(), 0, "{:?}", report.ran);
    let got = f.session.execute("selection.get", json!({})).unwrap();
    let b = &got["bounds"];
    let cx = b["x"].as_f64().unwrap() + b["w"].as_f64().unwrap() / 2.0;
    assert!((cx - 70.0).abs() < 0.01, "{b}");
}

#[test]
fn align_rows_need_two_selected_and_distribute_three() {
    let h = menu_with(1, pos2(300.0, 60.0));
    assert!(!has_row(&h, "Align and distribute…"));

    let mut h = menu_with(2, pos2(300.0, 60.0));
    assert!(has_row(&h, "Align and distribute…"));
    assert!(!has_row(&h, "Align left"), "the block is closed until it is opened");
    row(&h, "Align and distribute…").click();
    h.run();
    for (text, _) in ALIGN_EDGES {
        assert!(has_row(&h, text), "missing {text}");
    }
    assert!(!has_row(&h, "Distribute horizontally"), "distribute needs three");
    h.state_mut().clear();
    row(&h, "Align left").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::ContextMenu, "arrange.align", json!({ "edge": "left" }))]);
    assert!(h.state().state.context_menu.is_none());

    let mut h = menu_with(3, pos2(300.0, 60.0));
    row(&h, "Align and distribute…").click();
    h.run();
    row(&h, "Distribute vertically").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::ContextMenu, "arrange.distribute", json!({ "axis": "vertical" }))]);
}

#[test]
fn the_align_rows_cover_every_edge_of_the_command() {
    let schema = model::args_schema("arrange.align").expect("schema");
    let fields = args_form::fields(&schema);
    let edge = fields.iter().find(|f| f.name == "edge").expect("edge");
    let args_form::FieldKind::Choice(choices) = &edge.kind else {
        panic!("edge is an enum");
    };
    let mut in_schema: Vec<String> = choices.iter().map(|c| c.label.clone()).collect();
    let mut in_menu: Vec<String> = ALIGN_EDGES.iter().map(|(_, e)| (*e).to_owned()).collect();
    in_schema.sort();
    in_menu.sort();
    assert_eq!(in_menu, in_schema);
}

#[test]
fn a_click_elsewhere_closes_the_menu() {
    let mut h = menu_with(1, pos2(300.0, 60.0));
    h.state_mut().clear();
    click_at(&mut h, pos2(1000.0, 700.0));
    assert!(
        h.state().actions.contains(&UiAction::Verb(Verb::CloseOverlays)),
        "a click outside asks to close: {:?}",
        h.state().actions
    );
    assert!(calls(&h).is_empty());
}

#[test]
fn the_menu_stays_inside_the_window() {
    let h = menu_with(1, pos2(1275.0, 795.0));
    let screen = egui::Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(1280.0, 800.0));
    // The menu opened in the bottom right corner is pushed back inside; "Cut" exists only in the menu.
    for text in ["Cut", "Select all"] {
        let rect = h.get_all_by_role_and_label(Role::Button, text).last().unwrap().rect();
        assert!(screen.contains_rect(rect), "{text} at {rect:?} is outside {screen:?}");
    }
}

#[test]
fn the_menu_is_drawn_in_the_dark_theme() {
    let mut f = Fixture::new();
    f.state.open_context_menu(pos2(300.0, 60.0), None);
    let mut h = themed(f, true);
    h.run();
    assert!(has_row(&h, "Zoom to fit"));
}

#[test]
fn closing_the_menu_forgets_the_open_align_block() {
    let mut h = menu_with(2, pos2(300.0, 60.0));
    row(&h, "Align and distribute…").click();
    h.run();
    assert!(has_row(&h, "Align left"));
    h.state_mut().state.context_menu = None;
    h.run();
    h.state_mut().state.open_context_menu(pos2(300.0, 60.0), None);
    h.run();
    assert!(!has_row(&h, "Align left"), "a new menu starts closed");
}

// ---- a locked element ----

/// The menu of a locked element at (300, 200): the fixture has one locked rectangle and nothing selected.
fn locked_menu() -> H {
    let mut f = Fixture::new();
    let id = add_rect(&mut f.session, 40.0, 40.0, 30.0, 30.0).unwrap();
    f.session.execute("element.lock", json!({ "ids": [id.as_str()], "locked": true })).unwrap();
    f.refresh();
    f.state.open_context_menu_on_locked(pos2(300.0, 60.0), Some([55.0, 55.0]), id.as_str());
    let mut h = themed(f, false);
    h.run();
    h
}

#[test]
fn the_menu_of_a_locked_element_offers_unlock_and_runs_it_on_that_element() {
    let mut h = locked_menu();
    for hidden in ["Cut", "Duplicate", "Delete", "Group", "Lock or unlock", "Link…"] {
        assert!(!has_row(&h, hidden), "{hidden} is not for a locked element");
    }
    row(&h, "Unlock").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::ContextMenu, "element.unlock_all", json!({ "ids": ["e1"] }))]);
    assert!(h.state().state.context_menu.is_none());
    let f = h.state_mut();
    f.dispatch();
    let e1 = drafftink_scene::ElementId::new("e1").unwrap();
    assert!(!f.session.element(&e1).unwrap().locked, "the element is unlocked");
    assert_eq!(f.session.selection().len(), 1, "and selected, as Excalidraw leaves it");
}

#[test]
fn the_menu_of_one_selected_element_has_a_link_row_that_opens_the_dialog() {
    let mut h = menu_with(1, pos2(300.0, 60.0));
    row(&h, "Link…").click();
    h.run();
    assert_eq!(
        h.state()
            .actions
            .iter()
            .filter_map(|a| if let UiAction::Verb(v) = a { Some(*v) } else { None })
            .collect::<Vec<_>>(),
        vec![Verb::OpenLinkEditor]
    );
    assert!(!has_row(&h, "Open link"), "no link yet");
}

#[test]
fn an_element_with_a_link_has_an_open_link_row() {
    let mut f = Fixture::new();
    let id = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    select(&mut f.session, std::slice::from_ref(&id));
    f.session.execute("element.set_link", json!({ "link": "https://example.com" })).unwrap();
    f.refresh();
    f.state.open_context_menu(pos2(300.0, 60.0), None);
    let mut h = themed(f, false);
    h.run();
    row(&h, "Open link").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::ContextMenu, "element.open_link", json!({}))]);
}
