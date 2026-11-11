#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The flyouts of the context menu: copy as, flip and convert, frame, text, arrange, view. Each row makes the call
//! with the right arguments (origin `context_menu`) and, dispatched, the real command does its work. The labels are
//! the registry's.

mod common;
mod dialog;
mod support_a;

use common::Fixture;
use drafftink_engine::Origin;
use drafftink_engine::testing::{add_rect, add_text_label, select};
use drafftink_scene::ElementId;
use drafftink_ui::state::RenameTarget;
use drafftink_ui::{Call, Verb};
use egui::pos2;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::{Value, json};
use support_a::menu::{H, choose, has_row, has_row_in, is_open, open_flyout, row, row_in, rows_in, top_rows};
use support_a::themed_lenient;

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
    let mut h = themed_lenient(f, false);
    h.run();
    h
}

/// Opens the menu again over the current selection (after a command changed it).
fn reopen(h: &mut H) {
    h.state_mut().refresh();
    h.state_mut().state.close_overlays();
    h.state_mut().state.open_context_menu(pos2(300.0, 60.0), None);
    h.hover_at(pos2(1000.0, 700.0));
    h.run();
}

fn calls(h: &H) -> Vec<Call> {
    h.state().calls().into_iter().cloned().collect()
}

/// Clicks a row of a flyout and returns the calls it made.
fn pick(h: &mut H, flyout: &str, text: &str) -> Vec<Call> {
    choose(h, flyout, text);
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

/// The title the registry gives a call.
fn title(h: &H, id: &str, args: Value) -> String {
    h.state().model.label(id, &args)
}

fn assert_enabled(h: &H, flyout: &str, text: &str) {
    assert!(!row_in(h, flyout, text).accesskit_node().is_disabled(), "{text} is enabled");
}

#[test]
fn the_flyouts_start_closed_and_open_with_their_row() {
    let mut f = dialog::fixture();
    let a = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    let mut h = open(f, &[a]);
    for hidden in ["Copy as PNG", "Flip horizontally", "Wrap in frame"] {
        assert!(!has_row(&h, hidden), "{hidden} is behind its flyout");
    }
    assert!(!is_open(&h, "Copy as"));
    open_flyout(&mut h, "Copy as");
    assert_eq!(
        rows_in(&h, "Copy as"),
        ["Copy as PNG", "Copy as SVG", "Copy as text"],
        "the row is there for every selection"
    );
}

#[test]
fn copy_as_png_and_svg_run_copy_as_and_svg_reaches_the_clipboard() {
    let mut f = dialog::fixture();
    let a = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    let mut h = open(f, &[a]);
    assert_eq!(pick(&mut h, "Copy as", "Copy as PNG"), vec![ctx_call("clipboard.copy_as", json!({ "format": "png" }))]);
    let mut h = {
        let mut f = dialog::fixture();
        let a = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
        open(f, &[a])
    };
    assert_eq!(pick(&mut h, "Copy as", "Copy as SVG"), vec![ctx_call("clipboard.copy_as", json!({ "format": "svg" }))]);
    let report = h.state_mut().dispatch();
    assert!(report.refusals().next().is_none(), "{:?}", report.ran);
}

#[test]
fn copy_as_text_on_a_selection_without_text_is_refused_by_the_engine_in_words() {
    let mut f = dialog::fixture();
    let a = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    let mut h = open(f, &[a]);
    assert_eq!(
        pick(&mut h, "Copy as", "Copy as text"),
        vec![ctx_call("clipboard.copy_as", json!({ "format": "text" }))]
    );
    let report = h.state_mut().dispatch();
    let refusal = report.refusals().next().expect("the engine refuses: no element holds text");
    assert!(refusal.message.contains("none of the elements holds text"), "{}", refusal.message);
}

#[test]
fn copy_as_text_needs_a_text_in_the_selection() {
    let mut f = dialog::fixture();
    let t = text(&mut f, 0.0);
    let mut h = open(f, &[t]);
    assert_eq!(
        pick(&mut h, "Copy as", "Copy as text"),
        vec![ctx_call("clipboard.copy_as", json!({ "format": "text" }))]
    );
    let report = h.state_mut().dispatch();
    assert!(report.refusals().next().is_none(), "{:?}", report.ran);
}

#[test]
fn flip_rows_carry_their_axis() {
    for (text, axis) in [("Flip horizontally", "horizontal"), ("Flip vertically", "vertical")] {
        let mut f = dialog::fixture();
        let a = arrow(&mut f);
        let mut h = open(f, &[a]);
        assert_eq!(pick(&mut h, "Flip & convert", text), vec![ctx_call("element.flip", json!({ "axis": axis }))]);
        let report = h.state_mut().dispatch();
        assert!(report.refusals().next().is_none(), "{text}: {:?}", report.ran);
    }
}

#[test]
fn convert_offers_the_family_of_the_selection_and_converts_in_place() {
    let mut f = dialog::fixture();
    let a = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    let mut h = open(f, std::slice::from_ref(&a));
    open_flyout(&mut h, "Flip & convert");
    assert!(
        has_row_in(&h, "Flip & convert", "Convert to ellipse")
            && has_row_in(&h, "Flip & convert", "Convert to diamond")
    );
    assert!(!has_row_in(&h, "Flip & convert", "Convert to elbow arrow"), "a rectangle cannot become an arrow");
    assert_eq!(
        pick(&mut h, "Flip & convert", "Convert to ellipse"),
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
        assert_eq!(pick(&mut h, "Flip & convert", text), vec![ctx_call("element.convert", args)], "{text}");
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
    let mut h = open(f, &[a, b]);
    open_flyout(&mut h, "Flip & convert");
    assert!(
        !has_row_in(&h, "Flip & convert", "Convert to line") && !has_row_in(&h, "Flip & convert", "Convert to ellipse")
    );
}

#[test]
fn a_line_has_an_edit_points_row_a_shape_does_not() {
    let mut f = dialog::fixture();
    let a = arrow(&mut f);
    let mut h = open(f, &[a]);
    assert_eq!(pick(&mut h, "Flip & convert", "Edit points"), vec![ctx_call("arrow.edit_points", json!({}))]);
    // The engine refuses it for a rectangle (`wrong_kind`), so the row is not there.
    let mut f = dialog::fixture();
    let r = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    let mut h = open(f, &[r]);
    assert!(h.state().model.refusal_text("arrow.edit_points").is_some());
    open_flyout(&mut h, "Flip & convert");
    assert!(!has_row_in(&h, "Flip & convert", "Edit points"));
}

#[test]
fn add_to_library_stores_the_selection() {
    let mut f = dialog::fixture();
    let r = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    let mut h = open(f, &[r]);
    h.state_mut().clear();
    row(&h, "Add to library").click();
    h.run();
    assert_eq!(calls(&h), vec![ctx_call("library.add", json!({}))]);
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
    assert!(!has_row(&h, "Frame"), "a rectangle on its own has nothing to do with a frame but wrapping it");
    assert_eq!(pick(&mut h, "Arrange", "Wrap in frame"), vec![ctx_call("frame.wrap", json!({}))]);
    h.state_mut().dispatch();
    // The wrap selects the new frame.
    let frame = h.state().model.selection.iter().find(|e| e.kind == "frame").map(|e| e.id.clone()).expect("a frame");
    reopen(&mut h);
    assert_eq!(
        pick(&mut h, "Frame", "Select frame contents"),
        vec![ctx_call("frame.select_children", json!({ "frame": frame }))]
    );
    h.state_mut().dispatch();
    assert!(h.state().model.selection.iter().all(|e| e.kind != "frame"), "the children are selected now");

    // Back to the frame: Unwrap releases the children.
    select(&mut h.state_mut().session, &[ElementId::new(&frame).unwrap()]);
    reopen(&mut h);
    assert_eq!(pick(&mut h, "Frame", "Unwrap frame"), vec![ctx_call("frame.unwrap", json!({ "frame": frame }))]);
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
    let mut h = themed_lenient(f, false);
    h.state_mut().state.open_context_menu(pos2(300.0, 60.0), None);
    h.run();
    open_flyout(&mut h, "Frame");
    row_in(&h, "Frame", "Rename frame…").click();
    h.run();
    assert_eq!(h.state().state.rename.as_ref().map(|r| r.target.clone()), Some(RenameTarget::Frame(frame.clone())));
    let mut h = h;
    dialog::field(&h, "Name").focus();
    h.run();
    dialog::field(&h, "Name").type_text("Page");
    h.run();
    h.state_mut().clear();
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Rename").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::Ui, "frame.rename", json!({ "frame": frame, "name": "Page" }))]);
    let report = h.state_mut().dispatch();
    assert!(report.refusals().next().is_none(), "{:?}", report.ran);
}

#[test]
fn remove_from_frame_is_offered_for_the_members_of_a_frame() {
    let mut f = dialog::fixture();
    let a = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    select(&mut f.session, std::slice::from_ref(&a));
    dialog::run(&mut f, "frame.wrap", json!({}));
    select(&mut f.session, std::slice::from_ref(&a));
    f.refresh();
    let mut h = open(f, std::slice::from_ref(&a));
    assert_eq!(
        pick(&mut h, "Frame", "Remove from frame"),
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
        pick(&mut h, "Text", "Bind text to container"),
        vec![ctx_call("text.bind", json!({ "text": t.as_str(), "container": r.as_str() }))]
    );
    let report = h.state_mut().dispatch();
    assert!(report.refusals().next().is_none(), "{:?}", report.ran);

    // Unbind: the container with a label.
    let mut f = dialog::fixture();
    let r = add_rect(&mut f.session, 0.0, 0.0, 200.0, 100.0).unwrap();
    let label = add_text_label(&mut f.session, &r, "Hi").unwrap();
    let mut h = open(f, std::slice::from_ref(&r));
    assert_eq!(pick(&mut h, "Text", "Unbind text"), vec![ctx_call("text.unbind", json!({ "text": label.as_str() }))]);
    let report = h.state_mut().dispatch();
    assert!(report.refusals().next().is_none(), "{:?}", report.ran);

    // Wrap: a free text has no container to bind to and no label to unbind, so it has no Text flyout; wrapping it
    // in a shape is in Flip & convert.
    let mut f = dialog::fixture();
    let t = text(&mut f, 0.0);
    let mut h = open(f, &[t]);
    assert!(!has_row(&h, "Text"));
    assert_eq!(
        pick(&mut h, "Flip & convert", "Wrap text in shape"),
        vec![ctx_call("text.wrap_in_container", json!({}))]
    );
    let report = h.state_mut().dispatch();
    assert!(report.refusals().next().is_none(), "{:?}", report.ran);
}

#[test]
fn a_rectangle_has_no_text_flyout() {
    let mut f = dialog::fixture();
    let a = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    let h = open(f, &[a]);
    // The engine refuses `text.wrap_in_container` for a shape, and binding names a text and a container.
    assert!(h.state().model.refusal_text("text.wrap_in_container").is_some());
    assert!(!has_row(&h, "Text"));
}

#[test]
fn the_frame_flyout_of_a_frame_holds_its_rows_and_names_the_frame() {
    let mut f = dialog::fixture();
    let a = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    select(&mut f.session, &[a]);
    dialog::run(&mut f, "frame.wrap", json!({}));
    let mut h = themed_lenient(f, false);
    h.state_mut().state.open_context_menu(pos2(300.0, 60.0), None);
    h.run();
    open_flyout(&mut h, "Frame");
    assert_eq!(rows_in(&h, "Frame"), ["Unwrap frame", "Select frame contents", "Rename frame…"]);
    for text in ["Unwrap frame", "Select frame contents", "Rename frame…"] {
        assert_enabled(&h, "Frame", text);
    }
    assert!(h.state().model.refusal_text("frame.remove").is_some(), "the frame is in no frame: the row is left out");
}

#[test]
fn the_view_flyout_has_grid_snap_zen_view_only_stats_and_unlock_all() {
    let fresh = || {
        let mut f = dialog::fixture();
        add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
        open(f, &[])
    };
    let commands = [
        ("Cycle grid style", "settings.cycle", json!({ "key": "view.grid_style" })),
        ("Snap to grid", "settings.toggle", json!({ "key": "snap.grid" })),
        ("View-only mode", "view.set_view_only", json!({})),
    ];
    for (text, id, args) in commands {
        let mut h = fresh();
        assert_eq!(pick(&mut h, "View", text), vec![ctx_call(id, args)], "{text}");
        let report = h.state_mut().dispatch();
        assert!(report.refusals().next().is_none(), "{text}: {:?}", report.ran);
    }
    for (text, verb) in [("Zen mode", Verb::ToggleZen), ("Stats panel", Verb::ToggleStats)] {
        let mut h = fresh();
        assert!(pick(&mut h, "View", text).is_empty(), "{text} is a verb, not a command");
        assert!(dialog::verbs(h.state()).contains(&verb), "{text}");
    }
    // Nothing is locked yet: the engine refuses Unlock all, and the row is not there.
    let mut h = fresh();
    open_flyout(&mut h, "View");
    assert!(!has_row_in(&h, "View", "Unlock all"));
    // Lock the element first, so the real command has something to do.
    let mut f = dialog::fixture();
    let a = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    dialog::run(&mut f, "element.lock", json!({ "ids": [a.as_str()] }));
    let mut h = open(f, &[]);
    assert_eq!(pick(&mut h, "View", "Unlock all"), vec![ctx_call("element.unlock_all", json!({}))]);
    let report = h.state_mut().dispatch();
    assert!(report.refusals().next().is_none(), "{:?}", report.ran);
    assert!(!locked(&h, &a));
}

#[test]
fn toggles_in_the_view_flyout_show_their_state() {
    let mut f = dialog::fixture();
    add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    dialog::run(&mut f, "settings.set", json!({ "key": "snap.grid", "value": true }));
    let mut h = open(f, &[]);
    open_flyout(&mut h, "View");
    assert_eq!(row_in(&h, "View", "Snap to grid").accesskit_node().toggled(), Some(egui::accesskit::Toggled::True));
    assert_eq!(row_in(&h, "View", "View-only mode").accesskit_node().toggled(), Some(egui::accesskit::Toggled::False));
}

#[test]
fn lock_is_the_lock_command_with_no_arguments() {
    let mut f = dialog::fixture();
    let a = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    let mut h = open(f, std::slice::from_ref(&a));
    h.state_mut().clear();
    row(&h, "Lock").click();
    h.run();
    assert_eq!(calls(&h), vec![ctx_call("element.lock", json!({}))]);
    h.state_mut().dispatch();
    assert!(locked(&h, &a));
}

#[test]
fn paste_style_appears_once_a_style_was_copied() {
    let mut f = dialog::fixture();
    let a = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    let mut h = open(f, std::slice::from_ref(&a));
    assert!(!has_row(&h, "Paste style"), "no style copied yet");
    row(&h, "Copy style").click();
    h.run();
    h.state_mut().dispatch();
    reopen(&mut h);
    assert!(has_row(&h, "Paste style"));
}

// ---- labels ----

/// Every call the menus make by arguments that a registry title names, with the menu that holds it.
#[test]
fn every_label_is_the_title_the_registry_gives_the_call() {
    let mut f = dialog::fixture();
    let a = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    let b = add_rect(&mut f.session, 80.0, 0.0, 30.0, 30.0).unwrap();
    let c = add_rect(&mut f.session, 160.0, 0.0, 30.0, 30.0).unwrap();
    let mut h = open(f, &[a, b, c]);
    let calls: [(&str, &str, Value); 26] = [
        ("", "clipboard.cut", json!({})),
        ("", "clipboard.copy", json!({})),
        ("", "element.duplicate", json!({})),
        ("", "element.delete", json!({})),
        ("", "group.create", json!({})),
        ("", "element.lock", json!({})),
        ("", "library.add", json!({})),
        ("", "clipboard.copy_style", json!({})),
        ("Arrange", "arrange.z_order", json!({ "op": "front" })),
        ("Arrange", "arrange.z_order", json!({ "op": "back" })),
        ("Arrange", "frame.wrap", json!({})),
        ("Align", "arrange.align", json!({ "edge": "left" })),
        ("Align", "arrange.align", json!({ "edge": "center_x" })),
        ("Align", "arrange.align", json!({ "edge": "center_y" })),
        ("Align", "arrange.distribute", json!({ "axis": "horizontal" })),
        ("Flip & convert", "element.flip", json!({ "axis": "horizontal" })),
        ("Flip & convert", "element.flip", json!({ "axis": "vertical" })),
        ("Flip & convert", "element.convert", json!({ "to": "rectangle" })),
        ("Flip & convert", "element.convert", json!({ "to": "diamond" })),
        ("Flip & convert", "element.convert", json!({ "to": "ellipse" })),
        ("Copy as", "clipboard.copy_as", json!({ "format": "png" })),
        ("Copy as", "clipboard.copy_as", json!({ "format": "svg" })),
        ("Copy as", "clipboard.copy_as", json!({ "format": "text" })),
        ("", "clipboard.paste", json!({})),
        ("Align", "arrange.distribute", json!({ "axis": "vertical" })),
        ("Align", "arrange.align", json!({ "edge": "bottom" })),
    ];
    for (flyout, id, args) in calls {
        let want = title(&h, id, args);
        assert_ne!(want, id, "{id} has a title");
        if flyout.is_empty() {
            assert!(has_row(&h, &want), "the menu has a row {want:?} for {id}: {:?}", top_rows(&h));
        } else {
            if !is_open(&h, flyout) {
                open_flyout(&mut h, flyout);
            }
            assert!(has_row_in(&h, flyout, &want), "{flyout} has a row {want:?} for {id}: {:?}", rows_in(&h, flyout));
        }
    }
    // The titles that used to differ between surfaces.
    assert_eq!(title(&h, "arrange.align", json!({ "edge": "center_x" })), "Align centres horizontally");
    assert_eq!(title(&h, "element.flip", json!({ "axis": "horizontal" })), "Flip horizontally");
    assert_eq!(title(&h, "clipboard.copy_as", json!({ "format": "png" })), "Copy as PNG");
    assert_eq!(title(&h, "element.lock", json!({})), "Lock");
}

#[test]
fn the_panel_the_palette_and_the_hamburger_menu_use_the_same_titles() {
    let mut f = dialog::fixture();
    let a = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    let b = add_rect(&mut f.session, 80.0, 0.0, 30.0, 30.0).unwrap();
    let c = add_rect(&mut f.session, 160.0, 0.0, 30.0, 30.0).unwrap();
    select(&mut f.session, &[a, b, c]);
    f.refresh();
    let mut h = themed_lenient(f, false);
    h.run();
    let shared: [(&str, Value); 12] = [
        ("element.duplicate", json!({})),
        ("group.create", json!({})),
        ("element.lock", json!({})),
        ("element.delete", json!({})),
        ("element.flip", json!({ "axis": "horizontal" })),
        ("element.flip", json!({ "axis": "vertical" })),
        ("arrange.z_order", json!({ "op": "forward" })),
        ("arrange.align", json!({ "edge": "center_x" })),
        ("arrange.align", json!({ "edge": "center_y" })),
        ("arrange.distribute", json!({ "axis": "horizontal" })),
        ("arrange.distribute", json!({ "axis": "vertical" })),
        ("arrange.z_order", json!({ "op": "back" })),
    ];
    let palette: Vec<String> =
        drafftink_ui::surfaces::palette::rows(&h.state().model).into_iter().map(|r| r.title).collect();
    for (id, args) in shared {
        let want = title(&h, id, args);
        h.get_by_label(&want);
        assert!(palette.contains(&want), "the palette has a row {want:?}");
    }
    // The hamburger menu.
    h.state_mut().state.menu_open = true;
    h.run();
    let menu = |h: &H| rows_in(h, "Main menu");
    let rows = menu(&h);
    for (id, args) in [
        ("tab.new", json!({})),
        ("doc.save", json!({})),
        ("doc.clear", json!({})),
        ("doc.export", json!({ "format": "png" })),
        ("clipboard.copy_as", json!({ "format": "png" })),
        ("diagram.import_mermaid", json!({ "source": "clipboard" })),
        ("view.set_view_only", json!({})),
        ("doc.show_intro", json!({})),
    ] {
        let want = title(&h, id, args.clone());
        assert!(rows.contains(&want), "the hamburger menu has a row {want:?} for {id}: {rows:?}");
        assert!(
            palette.contains(&want) || id == "diagram.import_mermaid" || id == "clipboard.copy_as",
            "{want:?} is in the palette"
        );
    }
}
