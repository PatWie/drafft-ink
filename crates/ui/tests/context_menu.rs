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
    let mut f = Fixture::new();
    let mut ids = Vec::new();
    for i in 0..n {
        ids.push(add_rect(&mut f.session, 40.0 * i as f64, 0.0, 30.0, 30.0).unwrap());
    }
    select(&mut f.session, &ids);
    f.refresh();
    f.state.open_context_menu(at);
    let mut h = themed(f, false);
    h.run();
    h
}

/// The menu row with this text. The toolbar can have a button of the same name, so take the last one.
fn row<'a>(h: &'a H, text: &'a str) -> egui_kittest::Node<'a> {
    h.get_all_by_role_and_label(Role::Button, text).last().unwrap_or_else(|| panic!("no menu row {text}"))
}

fn has_row(h: &H, text: &str) -> bool {
    h.query_all_by_role_and_label(Role::Button, text).next().is_some()
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
    let h = menu_with(1, pos2(300.0, 200.0));
    for text in ROWS {
        assert!(has_row(&h, text), "missing row {text}");
    }
    assert!(has_row(&h, "Copy style"));
    assert!(!has_row(&h, "Undo") && !has_row(&h, "Redo"), "undo and redo are not in this menu");
}

#[test]
fn a_click_on_a_row_makes_the_call_with_origin_context_menu_and_closes_the_menu() {
    let mut h = menu_with(1, pos2(300.0, 200.0));
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
        let mut h = menu_with(2, pos2(300.0, 200.0));
        row(&h, text).click();
        h.run();
        assert_eq!(calls(&h), vec![Call::new(Origin::ContextMenu, "arrange.z_order", json!({ "op": op }))], "{text}");
    }
}

#[test]
fn group_with_a_selection_makes_the_call() {
    let mut h = menu_with(2, pos2(300.0, 200.0));
    row(&h, "Group").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::ContextMenu, "group.create", json!({}))]);
}

#[test]
fn a_refused_row_is_disabled_and_a_click_does_nothing() {
    let mut h = menu_with(0, pos2(300.0, 200.0));
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
    let mut h = menu_with(1, pos2(300.0, 200.0));
    for (text, id) in [("Cut", "clipboard.cut"), ("Select all", "selection.all"), ("Lock or unlock", "element.lock")] {
        let reason = h.state().model.refusal_text(id);
        let registered = h.state().model.command(id).is_some();
        match &reason {
            Some(r) if !registered => {
                assert_eq!(r, &format!("`{id}` is not available in this build."));
                assert!(row(&h, text).accesskit_node().is_disabled(), "{text} is greyed");
                row(&h, text).hover();
                h.run();
                h.get_by_label(r);
            }
            Some(_) => assert!(row(&h, text).accesskit_node().is_disabled(), "{text} is refused"),
            None => assert!(!row(&h, text).accesskit_node().is_disabled(), "{text} can run"),
        }
    }
}

#[test]
fn rows_of_unregistered_commands_never_run() {
    let mut h = menu_with(1, pos2(300.0, 200.0));
    let missing: Vec<&str> =
        ["clipboard.cut", "clipboard.copy", "clipboard.paste", "element.duplicate", "element.delete"]
            .into_iter()
            .filter(|id| h.state().model.command(id).is_none())
            .collect();
    for (text, id) in [("Cut", "clipboard.cut"), ("Copy", "clipboard.copy"), ("Paste", "clipboard.paste")] {
        if missing.contains(&id) {
            h.state_mut().clear();
            row(&h, text).click();
            h.run();
            assert!(calls(&h).is_empty(), "{text} must not emit a call for an unregistered command");
        }
    }
}

#[test]
fn align_rows_need_two_selected_and_distribute_three() {
    let h = menu_with(1, pos2(300.0, 200.0));
    assert!(!has_row(&h, "Align and distribute…"));

    let mut h = menu_with(2, pos2(300.0, 200.0));
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

    let mut h = menu_with(3, pos2(300.0, 200.0));
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
    let mut h = menu_with(1, pos2(300.0, 200.0));
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
    for text in ["Cut", "Zoom to fit"] {
        let rect = row(&h, text).rect();
        assert!(screen.contains_rect(rect), "{text} at {rect:?} is outside {screen:?}");
    }
}

#[test]
fn the_menu_is_drawn_in_the_dark_theme() {
    let mut f = Fixture::new();
    f.state.open_context_menu(pos2(300.0, 200.0));
    let mut h = themed(f, true);
    h.run();
    assert!(has_row(&h, "Zoom to fit"));
}

#[test]
fn closing_the_menu_forgets_the_open_align_block() {
    let mut h = menu_with(2, pos2(300.0, 200.0));
    row(&h, "Align and distribute…").click();
    h.run();
    assert!(has_row(&h, "Align left"));
    h.state_mut().state.context_menu = None;
    h.run();
    h.state_mut().state.open_context_menu(pos2(300.0, 200.0));
    h.run();
    assert!(!has_row(&h, "Align left"), "a new menu starts closed");
}
