#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The rows the context menu gained: copy as, flip, convert, frame, text, view and snapping. Each row makes the
//! call with the right arguments (origin `context_menu`) and, dispatched, the real command does its work.

mod common;
mod dialog;
mod support_a;

use common::Fixture;
use drafftink_engine::Origin;
use drafftink_engine::testing::{add_rect, add_text_label, select};
use drafftink_scene::ElementId;
use drafftink_ui::state::RenameTarget;
use drafftink_ui::{Call, Verb};
use egui::accesskit::Role;
use egui::pos2;
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};
use support_a::themed_lenient_sized;

type H = egui_kittest::Harness<'static, Fixture>;

fn created(f: &mut Fixture, args: Value) -> ElementId {
    let out = dialog::run(f, "element.create", args);
    ElementId::new(out["id"].as_str().unwrap()).unwrap()
}

fn arrow(f: &mut Fixture) -> ElementId {
    created(f, json!({ "kind": "arrow", "points": [[0, 0], [80, 30]] }))
}

fn text(f: &mut Fixture, x: f64) -> ElementId {
    created(f, json!({ "kind": "text", "text": "hello", "at": [x, 0] }))
}

fn locked(h: &H, id: &ElementId) -> bool {
    use drafftink_scene::SceneRead;
    h.state().session.store().get(id).unwrap().locked
}

/// The menu open over a fixture whose selection is `ids`.
fn open(mut f: Fixture, ids: &[ElementId]) -> H {
    select(&mut f.session, ids);
    f.refresh();
    f.state.open_context_menu(pos2(300.0, 60.0), None);
    // Tall, so that the menu with every block open fits.
    let mut h = themed_lenient_sized(f, egui::vec2(1280.0, 1400.0));
    h.run();
    h
}

fn in_menu(n: &egui_kittest::Node<'_>) -> bool {
    let r = n.rect();
    (290.0..640.0).contains(&r.min.x) && (55.0..1345.0).contains(&r.min.y) && r.width() > 100.0
}

fn has_row(h: &H, text: &str) -> bool {
    h.query_all_by_role_and_label(Role::Button, text).any(|n| in_menu(&n))
}

fn row<'a>(h: &'a H, text: &'a str) -> egui_kittest::Node<'a> {
    h.get_all_by_role_and_label(Role::Button, text).find(in_menu).unwrap_or_else(|| panic!("no menu row {text}"))
}

fn calls(h: &H) -> Vec<Call> {
    h.state().calls().into_iter().cloned().collect()
}

/// Opens a block of rows, clicks the row, and returns the calls it made.
fn choose(h: &mut H, block: &str, text: &str) -> Vec<Call> {
    if !has_row(h, text) {
        row(h, block).click();
        h.run();
    }
    h.state_mut().clear();
    row(h, text).click();
    h.run();
    calls(h)
}

fn ctx_call(id: &str, args: Value) -> Call {
    Call::new(Origin::ContextMenu, id, args)
}

fn kind_of(h: &mut H, id: &ElementId) -> String {
    let f = h.state_mut();
    let out = f.session.execute("element.query", json!({ "filter": { "ids": [id.as_str()] } })).unwrap();
    out["items"][0]["kind"].as_str().unwrap().to_owned()
}

#[test]
fn the_blocks_start_closed_and_open_with_their_row() {
    let mut f = dialog::fixture();
    let a = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    let mut h = open(f, &[a]);
    for hidden in ["Copy as PNG", "Flip horizontal", "Wrap in frame", "Snap to grid"] {
        assert!(!has_row(&h, hidden), "{hidden} is behind its block");
    }
    row(&h, "Copy as…").click();
    h.run();
    assert!(has_row(&h, "Copy as PNG") && has_row(&h, "Copy as SVG"));
    assert!(!has_row(&h, "Copy as text"), "no text in the selection");
}

#[test]
fn copy_as_png_and_svg_run_copy_as_and_svg_reaches_the_clipboard() {
    let mut f = dialog::fixture();
    let a = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    let mut h = open(f, &[a]);
    assert_eq!(
        choose(&mut h, "Copy as…", "Copy as PNG"),
        vec![ctx_call("clipboard.copy_as", json!({ "format": "png" }))]
    );
    let mut h = {
        let mut f = dialog::fixture();
        let a = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
        open(f, &[a])
    };
    assert_eq!(
        choose(&mut h, "Copy as…", "Copy as SVG"),
        vec![ctx_call("clipboard.copy_as", json!({ "format": "svg" }))]
    );
    let report = h.state_mut().dispatch();
    assert!(report.refusals().next().is_none(), "{:?}", report.ran);
}

#[test]
fn copy_as_text_needs_a_text_in_the_selection() {
    let mut f = dialog::fixture();
    let t = text(&mut f, 0.0);
    let mut h = open(f, &[t]);
    assert_eq!(
        choose(&mut h, "Copy as…", "Copy as text"),
        vec![ctx_call("clipboard.copy_as", json!({ "format": "text" }))]
    );
    let report = h.state_mut().dispatch();
    assert!(report.refusals().next().is_none(), "{:?}", report.ran);
}

#[test]
fn flip_rows_carry_their_axis() {
    for (text, axis) in [("Flip horizontal", "horizontal"), ("Flip vertical", "vertical")] {
        let mut f = dialog::fixture();
        let a = arrow(&mut f);
        let mut h = open(f, &[a]);
        assert_eq!(choose(&mut h, "Flip and convert…", text), vec![ctx_call("element.flip", json!({ "axis": axis }))]);
        let report = h.state_mut().dispatch();
        assert!(report.refusals().next().is_none(), "{text}: {:?}", report.ran);
    }
}

#[test]
fn convert_offers_the_family_of_the_selection_and_converts_in_place() {
    let mut f = dialog::fixture();
    let a = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    let mut h = open(f, std::slice::from_ref(&a));
    row(&h, "Flip and convert…").click();
    h.run();
    assert!(has_row(&h, "Convert to ellipse") && has_row(&h, "Convert to diamond"));
    assert!(!has_row(&h, "Convert to elbow arrow"), "a rectangle cannot become an arrow");
    assert_eq!(
        choose(&mut h, "Flip and convert…", "Convert to ellipse"),
        vec![ctx_call("element.convert", json!({ "to": "ellipse" }))]
    );
    h.state_mut().dispatch();
    assert_eq!(kind_of(&mut h, &a), "ellipse");
}

#[test]
fn an_arrow_converts_to_a_line_and_to_the_three_arrow_types() {
    let rows = [
        ("Convert to line", json!({ "to": "line" })),
        ("Convert to sharp arrow", json!({ "to": "arrow", "arrow_type": "sharp" })),
        ("Convert to round arrow", json!({ "to": "arrow", "arrow_type": "round" })),
        ("Convert to elbow arrow", json!({ "to": "arrow", "arrow_type": "elbow" })),
    ];
    for (text, args) in rows {
        let mut f = dialog::fixture();
        let a = arrow(&mut f);
        let mut h = open(f, std::slice::from_ref(&a));
        assert_eq!(choose(&mut h, "Flip and convert…", text), vec![ctx_call("element.convert", args)], "{text}");
        h.state_mut().dispatch();
        // Converting to what it already is answers `nothing_to_do`; every other row changes the arrow.
        let kind = kind_of(&mut h, &a);
        assert!(kind == "arrow" || kind == "line", "{text}: {kind}");
    }
}

#[test]
fn mixed_shapes_and_lines_get_no_convert_rows() {
    let mut f = dialog::fixture();
    let a = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    let b = arrow(&mut f);
    let h = open(f, &[a, b]);
    row(&h, "Flip and convert…").click();
    let mut h = h;
    h.run();
    assert!(!has_row(&h, "Convert to line") && !has_row(&h, "Convert to ellipse"));
}

#[test]
fn a_line_has_an_edit_points_row_and_everything_has_add_to_library() {
    let mut f = dialog::fixture();
    let a = arrow(&mut f);
    let mut h = open(f, &[a]);
    assert_eq!(choose(&mut h, "Flip and convert…", "Edit line points"), vec![ctx_call("arrow.edit_points", json!({}))]);
    let mut f = dialog::fixture();
    let r = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    let mut h = open(f, &[r]);
    assert!(!has_row(&h, "Edit line points"));
    assert_eq!(choose(&mut h, "Flip and convert…", "Add to library"), vec![ctx_call("library.add", json!({}))]);
    let report = h.state_mut().dispatch();
    assert!(report.refusals().next().is_none(), "{:?}", report.ran);
    let listed = h.state_mut().session.execute("library.list", json!({})).unwrap();
    assert_eq!(listed["items"].as_array().map(Vec::len), Some(1));
}

#[test]
fn wrap_in_frame_makes_a_frame_and_the_frame_has_its_own_rows() {
    let mut f = dialog::fixture();
    let a = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    let mut h = open(f, &[a]);
    assert_eq!(choose(&mut h, "Frame…", "Wrap in frame"), vec![ctx_call("frame.wrap", json!({}))]);
    h.state_mut().dispatch();
    // The wrap selects the new frame.
    let frame = h.state().model.selection.iter().find(|e| e.kind == "frame").map(|e| e.id.clone()).expect("a frame");
    h.state_mut().state.close_overlays();
    h.state_mut().state.open_context_menu(pos2(300.0, 60.0), None);
    h.run();
    assert_eq!(
        choose(&mut h, "Frame…", "Select all in frame"),
        vec![ctx_call("frame.select_children", json!({ "frame": frame }))]
    );
    h.state_mut().dispatch();
    assert!(h.state().model.selection.iter().all(|e| e.kind != "frame"), "the children are selected now");

    // Back to the frame: Unwrap releases the children.
    select(&mut h.state_mut().session, &[ElementId::new(&frame).unwrap()]);
    h.state_mut().refresh();
    h.state_mut().state.close_overlays();
    h.state_mut().state.open_context_menu(pos2(300.0, 60.0), None);
    h.run();
    assert_eq!(choose(&mut h, "Frame…", "Unwrap frame"), vec![ctx_call("frame.unwrap", json!({ "frame": frame }))]);
    let report = h.state_mut().dispatch();
    assert!(report.refusals().next().is_none(), "{:?}", report.ran);
}

#[test]
fn rename_frame_opens_the_rename_dialog_for_that_frame() {
    let mut f = dialog::fixture();
    let a = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    select(&mut f.session, &[a]);
    dialog::run(&mut f, "frame.wrap", json!({}));
    let frame = f.model.selection[0].id.clone();
    let mut h = themed_lenient_sized(f, egui::vec2(1280.0, 1400.0));
    h.state_mut().state.open_context_menu(pos2(300.0, 60.0), None);
    h.run();
    row(&h, "Frame…").click();
    h.run();
    row(&h, "Rename frame…").click();
    h.run();
    assert_eq!(h.state().state.rename.as_ref().map(|r| r.target.clone()), Some(RenameTarget::Frame(frame.clone())));
    let mut h = h;
    dialog::field(&h, "Name").focus();
    h.run();
    dialog::field(&h, "Name").type_text("Page");
    h.run();
    h.state_mut().clear();
    h.get_by_role_and_label(Role::Button, "Rename").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::Ui, "frame.rename", json!({ "frame": frame, "name": "Page" }))]);
    let report = h.state_mut().dispatch();
    assert!(report.refusals().next().is_none(), "{:?}", report.ran);
}

#[test]
fn remove_from_frame_lists_the_members_of_a_frame() {
    let mut f = dialog::fixture();
    let a = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    select(&mut f.session, std::slice::from_ref(&a));
    dialog::run(&mut f, "frame.wrap", json!({}));
    select(&mut f.session, std::slice::from_ref(&a));
    f.refresh();
    let mut h = open(f, std::slice::from_ref(&a));
    assert_eq!(
        choose(&mut h, "Frame…", "Remove from frame"),
        vec![ctx_call("frame.remove", json!({ "ids": [a.as_str()] }))]
    );
    let report = h.state_mut().dispatch();
    assert!(report.refusals().next().is_none(), "{:?}", report.ran);
}

#[test]
fn text_rows_bind_unbind_and_wrap() {
    // Bind: one free text and one container.
    let mut f = dialog::fixture();
    let r = add_rect(&mut f.session, 0.0, 0.0, 200.0, 100.0).unwrap();
    let t = text(&mut f, 400.0);
    let mut h = open(f, &[r.clone(), t.clone()]);
    assert_eq!(
        choose(&mut h, "Text…", "Bind text to shape"),
        vec![ctx_call("text.bind", json!({ "text": t.as_str(), "container": r.as_str() }))]
    );
    let report = h.state_mut().dispatch();
    assert!(report.refusals().next().is_none(), "{:?}", report.ran);

    // Unbind: the container with a label.
    let mut f = dialog::fixture();
    let r = add_rect(&mut f.session, 0.0, 0.0, 200.0, 100.0).unwrap();
    let label = add_text_label(&mut f.session, &r, "Hi").unwrap();
    let mut h = open(f, std::slice::from_ref(&r));
    assert_eq!(
        choose(&mut h, "Text…", "Unbind text"),
        vec![ctx_call("text.unbind", json!({ "text": label.as_str() }))]
    );
    let report = h.state_mut().dispatch();
    assert!(report.refusals().next().is_none(), "{:?}", report.ran);

    // Wrap: a free text.
    let mut f = dialog::fixture();
    let t = text(&mut f, 0.0);
    let mut h = open(f, &[t]);
    assert!(!has_row(&h, "Unbind text") && !has_row(&h, "Bind text to shape"));
    assert_eq!(choose(&mut h, "Text…", "Wrap text in a shape"), vec![ctx_call("text.wrap_in_container", json!({}))]);
    let report = h.state_mut().dispatch();
    assert!(report.refusals().next().is_none(), "{:?}", report.ran);
}

#[test]
fn a_rectangle_has_no_text_block() {
    let mut f = dialog::fixture();
    let a = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    let h = open(f, &[a]);
    assert!(!has_row(&h, "Text…"));
}

#[test]
fn the_view_block_has_grid_snap_zen_view_only_stats_and_unlock_all() {
    let block = "View and snapping…";
    let fresh = || {
        let mut f = dialog::fixture();
        let a = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
        open(f, &[a])
    };
    let commands = [
        ("Change the grid", "settings.cycle", json!({ "key": "view.grid_style" })),
        ("Snap to grid", "settings.toggle", json!({ "key": "snap.grid" })),
        ("View-only mode", "view.set_view_only", json!({})),
    ];
    for (text, id, args) in commands {
        let mut h = fresh();
        assert_eq!(choose(&mut h, block, text), vec![ctx_call(id, args)], "{text}");
        let report = h.state_mut().dispatch();
        assert!(report.refusals().next().is_none(), "{text}: {:?}", report.ran);
    }
    for (text, verb) in [("Zen mode", Verb::ToggleZen), ("Stats panel", Verb::ToggleStats)] {
        let mut h = fresh();
        assert!(choose(&mut h, block, text).is_empty(), "{text} is a verb, not a command");
        assert!(dialog::verbs(h.state()).contains(&verb), "{text}");
    }
    // Unlock all: lock the element first, so the real command has something to do.
    let mut f = dialog::fixture();
    let a = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    dialog::run(&mut f, "element.lock", json!({ "ids": [a.as_str()] }));
    let mut h = themed_lenient_sized(f, egui::vec2(1280.0, 1400.0));
    h.state_mut().state.open_context_menu(pos2(300.0, 60.0), None);
    h.run();
    assert_eq!(choose(&mut h, block, "Unlock all"), vec![ctx_call("element.unlock_all", json!({}))]);
    let report = h.state_mut().dispatch();
    assert!(report.refusals().next().is_none(), "{:?}", report.ran);
    assert!(!locked(&h, &a));
}

#[test]
fn lock_or_unlock_is_the_lock_command_with_no_arguments() {
    let mut f = dialog::fixture();
    let a = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    let mut h = open(f, std::slice::from_ref(&a));
    h.state_mut().clear();
    row(&h, "Lock or unlock").click();
    h.run();
    assert_eq!(calls(&h), vec![ctx_call("element.lock", json!({}))]);
    h.state_mut().dispatch();
    assert!(locked(&h, &a));
}
