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
use egui::pos2;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use support_a::menu::{H, choose, has_row, has_row_in, is_open, open_flyout, row, row_in, rows_in, top_rows};
use support_a::themed_lenient as themed;

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

/// What a menu over one selected rectangle holds.
const ONE_RECTANGLE: [&str; 12] = [
    "Cut",
    "Copy",
    "Paste",
    "Duplicate",
    "Delete",
    "Arrange",
    "Flip & convert",
    "Copy as",
    "Lock",
    "Link…",
    "Add to library",
    "Copy style",
];

#[test]
fn a_single_element_gets_at_most_twelve_top_level_rows() {
    let h = menu_with(1, pos2(300.0, 60.0));
    let rows = top_rows(&h);
    assert_eq!(rows, ONE_RECTANGLE, "the rows of the menu over one rectangle");
    assert!(rows.len() <= 12, "{} rows: {rows:?}", rows.len());
    assert!(!has_row(&h, "Undo") && !has_row(&h, "Redo"), "undo and redo are not in this menu");
    for hidden in ["Group", "Ungroup", "Align", "Frame", "Text", "Open link", "Paste style", "Select all"] {
        assert!(!has_row(&h, hidden), "{hidden} does not apply to one rectangle");
    }
}

#[test]
fn a_text_or_an_arrow_gets_at_most_twelve_rows_too() {
    use drafftink_engine::testing::select as pick;
    for kind in ["text", "arrow"] {
        let mut f = Fixture::new();
        let args = match kind {
            "text" => json!({ "kind": "text", "text": "hello", "at": [0, 0] }),
            _ => json!({ "kind": "arrow", "points": [[0, 0], [80, 30]] }),
        };
        let out = f.session.execute("element.create", args).unwrap();
        let id = drafftink_scene::ElementId::new(out["id"].as_str().unwrap()).unwrap();
        pick(&mut f.session, &[id]);
        f.refresh();
        f.state.open_context_menu(pos2(300.0, 60.0), None);
        let mut h = themed(f, false);
        h.run();
        let rows = top_rows(&h);
        assert!(rows.len() <= 12, "{kind}: {} rows: {rows:?}", rows.len());
    }
}

#[test]
fn the_empty_canvas_menu_has_no_element_actions() {
    // Something on the canvas, nothing selected.
    let mut f = Fixture::new();
    add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    let h = menu_over(f, 0, pos2(300.0, 60.0), None);
    assert_eq!(top_rows(&h), ["Paste", "Select all", "Zoom to fit everything", "View"]);
    // On a canvas with nothing on it there is nothing to select.
    let h = menu_with(0, pos2(300.0, 60.0));
    assert_eq!(top_rows(&h), ["Paste", "Zoom to fit everything", "View"]);
    for element_action in [
        "Cut",
        "Copy",
        "Duplicate",
        "Delete",
        "Group",
        "Ungroup",
        "Arrange",
        "Align",
        "Flip & convert",
        "Frame",
        "Text",
        "Copy as",
        "Lock",
        "Link…",
        "Open link",
        "Add to library",
        "Copy style",
        "Paste style",
    ] {
        assert!(!has_row(&h, element_action), "{element_action} is for elements");
    }
}

#[test]
fn a_click_on_a_row_makes_the_call_with_origin_context_menu_and_closes_the_menu() {
    let mut h = menu_with(0, pos2(300.0, 60.0));
    row(&h, "Zoom to fit everything").click();
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
        choose(&mut h, "Arrange", text);
        assert_eq!(calls(&h), vec![Call::new(Origin::ContextMenu, "arrange.z_order", json!({ "op": op }))], "{text}");
        assert!(h.state().state.context_menu.is_none(), "{text} closes the menu");
    }
}

#[test]
fn group_applies_to_a_selection_of_several_and_ungroup_to_a_group() {
    let mut h = menu_with(2, pos2(300.0, 60.0));
    assert!(has_row(&h, "Group") && !has_row(&h, "Ungroup"));
    row(&h, "Group").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::ContextMenu, "group.create", json!({}))]);
    // Dispatched, the two rectangles are a group: now the menu offers Ungroup and no longer Group.
    h.state_mut().dispatch();
    h.state_mut().refresh();
    h.state_mut().state.open_context_menu(pos2(300.0, 60.0), None);
    h.run();
    assert!(has_row(&h, "Ungroup") && !has_row(&h, "Group"), "{:?}", top_rows(&h));
}

#[test]
fn a_refused_clipboard_row_stays_greyed_with_the_engine_text_and_a_click_does_nothing() {
    // View-only mode refuses `clipboard.paste`. Paste is the one row that stays when its command is refused.
    let mut f = Fixture::new();
    f.session.execute("view.set_view_only", json!({ "on": true })).unwrap();
    let mut h = menu_over(f, 0, pos2(300.0, 60.0), None);
    let reason = h.state().model.refusal_text("clipboard.paste").expect("paste is refused in view-only mode");
    assert!(row(&h, "Paste").accesskit_node().is_disabled());
    h.state_mut().clear();
    row(&h, "Paste").click();
    h.run();
    assert!(calls(&h).is_empty(), "{:?}", calls(&h));
    assert!(h.state().state.context_menu.is_some(), "a refused row keeps the menu open");
    // The refusal text is the tooltip.
    row(&h, "Paste").hover();
    h.run();
    h.get_by_label(&reason);
}

#[test]
fn a_command_the_engine_refuses_is_left_out_of_the_menu() {
    // Nothing is selected, so `clipboard.cut` is refused with `no_target`: the row is not there, instead of greyed.
    let h = menu_with(0, pos2(300.0, 60.0));
    assert!(h.state().model.refusal_text("clipboard.cut").is_some());
    assert!(!has_row(&h, "Cut"));
    // One element: group.create needs two, the menu has no Group row.
    let h = menu_with(1, pos2(300.0, 60.0));
    assert!(h.state().model.refusal_text("group.create").is_some());
    assert!(!has_row(&h, "Group"));
}

#[test]
fn a_command_the_registry_lacks_is_greyed_with_the_not_available_text() {
    let omitted = ["clipboard.cut", "element.lock", "selection.all"];
    let mut f = Fixture::without(&omitted);
    add_rect(&mut f.session, 200.0, 200.0, 30.0, 30.0).unwrap();
    let mut h = menu_over(f, 1, pos2(300.0, 60.0), None);
    // A command the registry lacks has no title, so its row is named by its id.
    for id in ["clipboard.cut", "element.lock"] {
        let reason = h.state().model.refusal_text(id).expect("left out of the registry");
        assert_eq!(reason, format!("`{id}` is not available in this build."));
        assert!(row(&h, id).accesskit_node().is_disabled(), "{id} is greyed");
        row(&h, id).hover();
        h.run();
        h.get_by_label(&reason);
    }
    // The rows of the commands that are there can run.
    assert!(!row(&h, "Copy").accesskit_node().is_disabled());
    // `selection.all` belongs to the menu over the empty canvas.
    let mut f = Fixture::without(&omitted);
    add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    let mut h = menu_over(f, 0, pos2(300.0, 60.0), None);
    let reason = h.state().model.refusal_text("selection.all").expect("left out of the registry");
    assert!(row(&h, "selection.all").accesskit_node().is_disabled());
    row(&h, "selection.all").hover();
    h.run();
    h.get_by_label(&reason);
}

#[test]
fn rows_of_unregistered_commands_never_run() {
    let omitted = ["clipboard.cut", "clipboard.copy", "clipboard.paste"];
    let mut h = menu_over(Fixture::without(&omitted), 1, pos2(300.0, 60.0), None);
    for id in omitted {
        h.state_mut().clear();
        row(&h, id).click();
        h.run();
        assert!(calls(&h).is_empty(), "{id} must not emit a call for an unregistered command");
        assert!(h.state().state.context_menu.is_some(), "{id}: the menu stays open");
    }
}

#[test]
fn rows_of_the_real_commands_are_enabled_by_the_real_engine() {
    // One rectangle selected: every row of the menu can run.
    let h = menu_with(1, pos2(300.0, 60.0));
    for (text, id) in [
        ("Cut", "clipboard.cut"),
        ("Copy", "clipboard.copy"),
        ("Paste", "clipboard.paste"),
        ("Duplicate", "element.duplicate"),
        ("Delete", "element.delete"),
        ("Lock", "element.lock"),
        ("Add to library", "library.add"),
        ("Copy style", "clipboard.copy_style"),
    ] {
        assert!(h.state().model.is_available(id), "{id} is registered and available");
        assert!(!row(&h, text).accesskit_node().is_disabled(), "{text}");
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
fn align_needs_two_selected_and_distribute_three() {
    // With one element the engine refuses both, and the menu leaves them out.
    let h = menu_with(1, pos2(300.0, 60.0));
    assert!(h.state().model.refusal_text("arrange.align").is_some());
    assert!(!has_row(&h, "Align"));

    let mut h = menu_with(2, pos2(300.0, 60.0));
    assert!(has_row(&h, "Align"));
    assert!(!is_open(&h, "Align"), "the flyout is closed until its row is used");
    open_flyout(&mut h, "Align");
    for edge in ALIGN_EDGES {
        let text = h.state().model.label("arrange.align", &json!({ "edge": edge }));
        assert!(!row_in(&h, "Align", &text).accesskit_node().is_disabled(), "{text} can run with two");
    }
    assert!(!has_row_in(&h, "Align", "Distribute horizontally"), "distribute needs three");
    choose(&mut h, "Align", "Align left");
    assert_eq!(calls(&h), vec![Call::new(Origin::ContextMenu, "arrange.align", json!({ "edge": "left" }))]);
    assert!(h.state().state.context_menu.is_none());

    let mut h = menu_with(3, pos2(300.0, 60.0));
    choose(&mut h, "Align", "Distribute vertically");
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
    let mut in_menu: Vec<String> = ALIGN_EDGES.iter().map(|e| (*e).to_owned()).collect();
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
    let mut h = menu_with(1, pos2(1275.0, 795.0));
    let screen = egui::Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(1280.0, 800.0));
    // The menu opened in the bottom right corner is pushed back inside.
    for text in ["Cut", "Add to library"] {
        let rect = row(&h, text).rect();
        assert!(screen.contains_rect(rect), "{text} at {rect:?} is outside {screen:?}");
    }
    // Its flyout has no room on the right: it opens on the left of the menu, inside the window too.
    let parent = row(&h, "Copy as").rect();
    open_flyout(&mut h, "Copy as");
    for text in ["Copy as PNG", "Copy as SVG", "Copy as text"] {
        let rect = row_in(&h, "Copy as", text).rect();
        assert!(screen.contains_rect(rect), "{text} at {rect:?} is outside {screen:?}");
        assert!(rect.right() <= parent.left() + 8.0, "{text} at {rect:?} lies over the menu {parent:?}");
    }
}

#[test]
fn the_menu_is_drawn_in_the_dark_theme() {
    let mut f = Fixture::new();
    f.state.open_context_menu(pos2(300.0, 60.0), None);
    let mut h = themed(f, true);
    h.run();
    assert!(has_row(&h, "Zoom to fit everything"));
}

#[test]
fn a_menu_opened_again_starts_with_no_flyout_open() {
    let mut h = menu_with(2, pos2(300.0, 60.0));
    open_flyout(&mut h, "Align");
    h.state_mut().state.context_menu = None;
    h.run();
    // The pointer leaves; a pointer resting on the row would open its flyout again, which is right.
    h.hover_at(pos2(1000.0, 700.0));
    h.state_mut().state.open_context_menu(pos2(300.0, 60.0), None);
    h.run();
    assert!(!is_open(&h, "Align"), "a new menu starts closed");
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
    assert_eq!(top_rows(&h), ["Unlock", "Paste", "Select all", "Zoom to fit everything"]);
    for hidden in ["Cut", "Duplicate", "Delete", "Group", "Lock", "Link…"] {
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

// ---- flyouts ----

#[test]
fn a_flyout_opens_beside_its_row_and_its_items_run_their_commands() {
    let mut h = menu_with(2, pos2(300.0, 60.0));
    assert!(!is_open(&h, "Arrange"));
    let parent = row(&h, "Arrange").rect();
    open_flyout(&mut h, "Arrange");
    assert_eq!(
        rows_in(&h, "Arrange"),
        ["Bring to front", "Bring forward", "Send backward", "Send to back", "Wrap in frame"]
    );
    let first = row_in(&h, "Arrange", "Bring to front").rect();
    assert!(first.left() >= parent.right() - 8.0, "the flyout is on the right of the menu: {first:?} vs {parent:?}");
    assert!((first.center().y - parent.center().y).abs() < 8.0, "its first row is level with the row: {first:?}");
    h.state_mut().clear();
    row_in(&h, "Arrange", "Wrap in frame").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::ContextMenu, "frame.wrap", json!({}))]);
    assert!(h.state().state.context_menu.is_none(), "choosing a row of a flyout closes the whole menu");
}

#[test]
fn a_click_on_the_row_opens_the_flyout_too() {
    let mut h = menu_with(1, pos2(300.0, 60.0));
    row(&h, "Copy as").click();
    h.run();
    assert!(is_open(&h, "Copy as"));
    assert!(h.state().state.context_menu.is_some(), "the row is no command: the menu stays");
    assert!(calls(&h).is_empty());
}

#[test]
fn a_flyout_closes_when_the_pointer_rests_on_another_row_but_not_at_once() {
    let mut h = menu_with(2, pos2(300.0, 60.0));
    open_flyout(&mut h, "Arrange");
    // The pointer crosses a neighbouring row on its way: one frame there does not close the flyout.
    let neighbour = row(&h, "Align").rect().center();
    h.hover_at(neighbour);
    h.step();
    assert!(is_open(&h, "Arrange"), "a short visit to another row keeps the flyout");
    // Then it reaches the flyout and stays.
    let target = row_in(&h, "Arrange", "Bring forward").rect().center();
    h.hover_at(target);
    h.run_steps(8);
    assert!(is_open(&h, "Arrange"), "the pointer in the flyout keeps it open");
    // Resting on another row closes it after the delay, and that row's flyout opens in its place.
    h.hover_at(neighbour);
    h.run_steps(8);
    assert!(!is_open(&h, "Arrange"), "resting on another row closes the flyout");
    assert!(is_open(&h, "Align"), "and the row under the pointer opens its own");
}

#[test]
fn leaving_the_menu_does_not_close_a_flyout_but_a_click_outside_does() {
    let mut h = menu_with(1, pos2(300.0, 60.0));
    open_flyout(&mut h, "Copy as");
    h.hover_at(pos2(1000.0, 700.0));
    h.run_steps(8);
    assert!(is_open(&h, "Copy as"));
    h.state_mut().clear();
    click_at(&mut h, pos2(1000.0, 700.0));
    assert!(h.state().actions.contains(&UiAction::Verb(Verb::CloseOverlays)));
}

#[test]
fn the_keyboard_moves_between_rows_and_into_flyouts() {
    let mut h = menu_with(1, pos2(300.0, 60.0));
    h.hover_at(pos2(1000.0, 700.0));
    h.run();
    // Down focuses the first row, Down again the next one.
    h.key_press(egui::Key::ArrowDown);
    h.run();
    assert!(row(&h, "Cut").is_focused());
    h.key_press(egui::Key::ArrowDown);
    h.run();
    assert!(row(&h, "Copy").is_focused());
    // Up wraps from the first row to the last.
    h.key_press(egui::Key::ArrowUp);
    h.run();
    h.key_press(egui::Key::ArrowUp);
    h.run();
    assert!(row(&h, "Copy style").is_focused(), "up from the first row goes to the last");
    // Down to "Arrange", Right opens its flyout and focuses its first row.
    for _ in 0..6 {
        h.key_press(egui::Key::ArrowDown);
        h.run();
    }
    assert!(row(&h, "Arrange").is_focused(), "{:?}", top_rows(&h));
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(4);
    assert!(is_open(&h, "Arrange"));
    assert!(row_in(&h, "Arrange", "Bring to front").is_focused());
    // Enter runs the focused row.
    h.key_press(egui::Key::ArrowDown);
    h.run();
    h.state_mut().clear();
    h.key_press(egui::Key::Enter);
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::ContextMenu, "arrange.z_order", json!({ "op": "forward" }))]);
    assert!(h.state().state.context_menu.is_none());
}

#[test]
fn left_closes_a_flyout_and_returns_to_its_row() {
    let mut h = menu_with(1, pos2(300.0, 60.0));
    h.hover_at(pos2(1000.0, 700.0));
    h.run();
    row(&h, "Copy as").focus();
    h.run();
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(4);
    assert!(is_open(&h, "Copy as"));
    h.key_press(egui::Key::ArrowLeft);
    h.run_steps(4);
    assert!(!is_open(&h, "Copy as"), "Left closes the flyout");
    assert!(row(&h, "Copy as").is_focused(), "and the focus is back on its row");
}
