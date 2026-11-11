//! One test per surface: clicking a control makes the expected command call, a refused control is
//! greyed with the refusal text.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{Fixture, click, harness};
use drafftink_engine::Origin;
use serde_json::json;

#[test]
fn toolbar_click_runs_tool_set_with_the_tool() {
    let mut h = harness(Fixture::new());
    h.run();
    let calls = click(&mut h, "Rectangle tool");
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].id, "tool.set");
    assert_eq!(calls[0].args, json!({"tool": "rectangle"}));
    assert_eq!(calls[0].origin, Origin::Ui);
}

use drafftink_engine::Session;
use drafftink_engine::testing::{add_rect, select};
use drafftink_ui::{Model, UiAction, UiState, Verb};
use egui_kittest::kittest::Queryable;

fn with_two_rects() -> Fixture {
    let mut f = Fixture::new();
    let a = add_rect(&mut f.session, 0.0, 0.0, 100.0, 60.0).unwrap();
    let b = add_rect(&mut f.session, 200.0, 40.0, 100.0, 60.0).unwrap();
    select(&mut f.session, &[a, b]);
    f.refresh();
    f
}

#[test]
fn every_tool_button_runs_tool_set_with_its_tool() {
    let mut h = harness(Fixture::new());
    h.run();
    for (label, tool) in [
        ("Select tool", "select"),
        ("Hand tool", "hand"),
        ("Rectangle tool", "rectangle"),
        ("Diamond tool", "diamond"),
        ("Ellipse tool", "ellipse"),
        ("Arrow tool", "arrow"),
        ("Line tool", "line"),
        ("Freehand tool", "freehand"),
        ("Text tool", "text"),
        ("Math tool", "math"),
        ("Frame tool", "frame"),
        ("Eraser tool", "eraser"),
        ("Laser tool", "laser"),
    ] {
        let calls = click(&mut h, label);
        assert_eq!(calls.len(), 1, "{label}: {calls:?}");
        assert_eq!((calls[0].id.as_str(), &calls[0].args), ("tool.set", &json!({ "tool": tool })), "{label}");
    }
}

#[test]
fn tool_lock_button_runs_tool_lock() {
    let mut h = harness(Fixture::new());
    h.run();
    let calls = click(&mut h, "Keep tool active");
    assert_eq!(calls.len(), 1);
    assert_eq!((calls[0].id.as_str(), &calls[0].args), ("tool.lock", &json!({})));
}

#[test]
fn top_bar_swatches_and_widths_run_style_set() {
    let mut h = harness(Fixture::new());
    h.run();
    let c = click(&mut h, "Stroke blue");
    assert_eq!(c[0].id, "style.set");
    assert_eq!(c[0].args, json!({"props": {"stroke_color": "blue-500"}}));
    let c = click(&mut h, "Fill red");
    assert_eq!(c[0].args, json!({"props": {"fill_color": "red-100"}}));
    // "No fill" removes the fill: null, not a transparent colour (a bug of the old app).
    let c = click(&mut h, "No fill");
    assert_eq!(c[0].args, json!({"props": {"fill_color": null}}));
    let c = click(&mut h, "Bold");
    assert_eq!(c[0].args, json!({"props": {"stroke_width": 4.0}}));
}

#[test]
fn top_bar_wheel_opens_the_colour_popover_by_verb_and_runs_no_command() {
    let mut h = harness(Fixture::new());
    h.run();
    h.state_mut().clear();
    h.get_by_label("Pick the stroke colour").click();
    h.run();
    assert!(h.state().calls().is_empty());
    assert_eq!(h.state().actions, vec![UiAction::Verb(Verb::OpenColorStroke)]);
}

#[test]
fn right_panel_shows_for_a_selection_and_its_buttons_run_style_set() {
    let mut h = harness(with_two_rects());
    h.run();
    let c = click(&mut h, "Cartoonist");
    assert_eq!(c[0].args, json!({"props": {"sloppiness": "cartoonist"}}));
    let c = click(&mut h, "Dashed");
    assert_eq!(c[0].args, json!({"props": {"stroke_style": "dashed"}}));
    let c = click(&mut h, "Round");
    assert_eq!(c[0].args, json!({"props": {"corner_radius": 32.0}}));
    let c = click(&mut h, "Bring to front");
    assert_eq!((c[0].id.as_str(), &c[0].args), ("arrange.z_order", &json!({"op": "front"})));
    // Two elements are selected, so the align row is there.
    let c = click(&mut h, "Align left");
    assert_eq!((c[0].id.as_str(), &c[0].args), ("arrange.align", &json!({"edge": "left"})));
    let c = click(&mut h, "Align centres horizontally");
    assert_eq!(c[0].args, json!({"edge": "center_x"}));
}

#[test]
fn right_panel_of_the_arrow_tool_sets_the_type_and_heads_of_new_arrows() {
    let mut f = Fixture::new();
    f.session.execute("tool.set", json!({ "tool": "arrow" })).unwrap();
    f.refresh();
    let mut h = harness(f);
    h.run();
    let c = click(&mut h, "New arrows: Elbow");
    assert_eq!((c[0].id.as_str(), &c[0].args), ("tool.set_arrow", &json!({ "arrow_type": "elbow" })));
    let c = click(&mut h, "New arrows, End head: Triangle");
    assert_eq!((c[0].id.as_str(), &c[0].args), ("tool.set_arrow", &json!({ "end_head": "triangle" })));
    let c = click(&mut h, "New arrows, Start head: None");
    assert_eq!((c[0].id.as_str(), &c[0].args), ("tool.set_arrow", &json!({ "start_head": "none" })));
    assert!(h.query_by_label("Swap heads").is_none(), "the swap button is for arrows that exist");
}

#[test]
fn right_panel_has_no_text_section_for_rectangles_and_no_align_for_one_element() {
    let mut f = Fixture::new();
    let a = add_rect(&mut f.session, 0.0, 0.0, 100.0, 60.0).unwrap();
    select(&mut f.session, &[a]);
    f.refresh();
    let mut h = harness(f);
    h.run();
    assert!(h.query_by_label("Light").is_none(), "font weight is a text property");
    assert!(h.query_by_label("Align left").is_none(), "align needs two elements");
    assert!(h.query_by_label("Group").is_some());
}

#[test]
fn right_panel_is_hidden_without_selection_for_the_select_tool_and_shows_for_a_drawing_tool() {
    let mut h = harness(Fixture::new());
    h.run();
    assert!(h.query_by_label("Sloppiness").is_none());
    assert!(h.query_by_label("Cartoonist").is_none());
    h.state_mut().session.execute("tool.set", json!({"tool": "rectangle"})).unwrap();
    h.state_mut().refresh();
    h.run();
    assert!(h.query_by_label("Cartoonist").is_some(), "drawing with the rectangle tool edits the defaults");
}

#[test]
fn bottom_bar_buttons_run_their_commands() {
    let mut h = harness(Fixture::new());
    h.run();
    let c = click(&mut h, "Zoom in");
    assert_eq!(c[0].id, "view.zoom_in");
    let c = click(&mut h, "Zoom out");
    assert_eq!(c[0].id, "view.zoom_out");
    let c = click(&mut h, "Reset zoom to 100%");
    assert_eq!(c[0].id, "view.zoom_reset");
    let c = click(&mut h, "Smart guides");
    assert_eq!((c[0].id.as_str(), &c[0].args), ("settings.toggle", &json!({"key": "snap.smart_guides"})));
    let c = click(&mut h, "Snap to grid");
    assert_eq!(c[0].args, json!({"key": "snap.grid"}));
    let c = click(&mut h, "Angle snap");
    assert_eq!(c[0].args, json!({"key": "snap.angle"}));
    let c = click(&mut h, "Zoom to fit");
    assert_eq!(c[0].id, "view.zoom_to_fit");
    let c = click(&mut h, "Centre the canvas at the origin");
    assert_eq!((c[0].id.as_str(), &c[0].args), ("view.center", &json!({"on": "origin"})));
}

#[test]
fn a_refused_command_greys_its_control_and_the_tooltip_is_the_refusal_text() {
    let f = Fixture::new();
    let expected = f.model.refusal_text("history.undo").expect("a fresh session has nothing to undo");
    assert!(expected.contains("nothing to undo"), "{expected}");
    let mut h = harness(f);
    h.run();
    let calls = click(&mut h, "Undo");
    assert!(calls.is_empty(), "a disabled button runs nothing: {calls:?}");
    h.get_by_label("Undo").hover();
    // egui shows a tooltip after a short delay; step time forward.
    for _ in 0..8 {
        h.step();
    }
    h.get_by_label_contains(&expected[..expected.len().min(30)]);
}

#[test]
fn undo_button_is_enabled_after_a_change() {
    let mut f = Fixture::new();
    f.session.execute("tool.set", json!({"tool": "rectangle"})).unwrap();
    f.session.execute("style.set", json!({"props": {"stroke_color": "red-500"}, "scope": "defaults"})).unwrap();
    // `style.set` on the defaults only changes the tool state, which has no undo step.
    f.refresh();
    assert!(f.model.refusal_text("history.undo").is_some());
    let a = add_rect(&mut f.session, 0.0, 0.0, 10.0, 10.0).unwrap();
    select(&mut f.session, std::slice::from_ref(&a));
    f.session.execute("style.set", json!({"props": {"stroke_color": "blue-500"}})).unwrap();
    f.refresh();
    assert!(f.model.refusal_text("history.undo").is_none());
    let mut h = harness(f);
    h.run();
    let c = click(&mut h, "Undo");
    assert_eq!(c[0].id, "history.undo");
}

fn two_tabs() -> Fixture {
    let mut f = Fixture::new();
    f.session.execute("tab.new", json!({"name": "Second"})).unwrap();
    f.session.execute("tab.switch", json!({"tab": 0})).unwrap();
    f.refresh();
    f
}

#[test]
fn tab_bar_switches_closes_and_adds_tabs() {
    let f = two_tabs();
    assert_eq!(f.model.tabs.len(), 2);
    let mut h = harness(f);
    h.run();
    let c = click(&mut h, "Second");
    assert_eq!((c[0].id.as_str(), &c[0].args), ("tab.switch", &json!({"tab": 1})));
    let c = click(&mut h, "Close Second");
    assert_eq!((c[0].id.as_str(), &c[0].args), ("tab.close", &json!({"tab": 1})));
    let c = click(&mut h, "New tab");
    assert_eq!(c[0].id, "tab.new");
}

#[test]
fn tab_bar_has_no_close_button_for_a_single_tab() {
    let mut h = harness(Fixture::new());
    h.run();
    assert!(h.query_by_label("Close Untitled").is_none());
}

#[test]
fn menu_opens_by_verb_and_its_rows_run_with_the_menu_origin() {
    let mut h = harness(Fixture::new());
    h.run();
    h.state_mut().clear();
    h.get_by_label("Open or close the main menu").click();
    h.run();
    assert_eq!(h.state().actions, vec![UiAction::Verb(Verb::ToggleMenu)]);
    // The app applies the verb (dispatch), then the next frame shows the menu.
    h.state_mut().dispatch();
    assert!(h.state().state.menu_open);
    h.run();
    h.state_mut().clear();
    h.get_by_label("View-only mode").click();
    h.run();
    let calls: Vec<_> = h.state().calls().into_iter().cloned().collect();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].id, "view.set_view_only");
    assert_eq!(calls[0].origin, Origin::Menu);
    // And the menu asks to be closed after a choice.
    assert!(h.state().actions.contains(&UiAction::Verb(Verb::CloseOverlays)));
}

#[test]
fn menu_rows_for_commands_that_are_not_registered_yet_are_greyed_with_a_reason() {
    let f = Fixture::without(&["doc.save"]);
    let reason = f.model.refusal_text("doc.save").unwrap();
    assert!(reason.contains("doc.save"), "{reason}");
    let mut f2 = f;
    f2.state.menu_open = true;
    let mut h = harness(f2);
    h.run();
    h.state_mut().clear();
    h.get_by_label("Save").click();
    h.run();
    assert!(h.state().calls().is_empty());
}

#[test]
fn zen_mode_hides_the_chrome() {
    let mut f = Fixture::new();
    f.state.zen = true;
    let mut h = harness(f);
    h.run();
    assert!(h.query_by_label("Rectangle tool").is_none());
    assert!(h.query_by_label("Undo").is_none());
}

#[test]
fn a_model_is_the_only_input_of_draw() {
    // `draw` takes the model and the UI state, never a session: the signature is the test.
    let m = Model::new(false);
    let mut st = UiState::new();
    let ctx = egui::Context::default();
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
        let actions = drafftink_ui::draw(ui, &m, &mut st);
        assert!(actions.is_empty());
    });
    out.textures_delta.clear();
    let _unused: Option<&Session> = None;
}

#[test]
fn during_a_text_edit_a_style_choice_goes_to_the_editor_and_not_to_style_set() {
    // docs/exceptions.md E16: `style.set` refuses with `text_editing`; the editor takes the choice.
    let mut f = Fixture::new();
    f.model.text_editing = true;
    let mut h = harness(f);
    h.run();
    h.state_mut().clear();
    h.get_by_label("Stroke blue").click();
    h.run();
    assert!(h.state().calls().is_empty(), "no command: {:?}", h.state().actions);
    assert_eq!(
        h.state().actions,
        vec![UiAction::Host(drafftink_ui::HostRequest::TextEditStyle(json!({"stroke_color": "blue-500"})))]
    );
}

// ---- the real engine: a click runs the real command and the session changes ----

type H = egui_kittest::Harness<'static, Fixture>;

/// Clicks the control, lets the app run what the frame returned, and checks that no command was refused.
fn click_and_run(h: &mut H, label: &str) {
    h.state_mut().clear();
    h.get_by_label(label).click();
    h.run();
    let report = h.state_mut().dispatch();
    assert_eq!(report.refusals().count(), 0, "{label}: {:?}", report.ran);
    assert!(!report.ran.is_empty(), "{label} ran no command");
    h.run();
}

#[test]
fn clicking_a_tool_changes_the_tool_of_the_session() {
    let mut h = harness(Fixture::new());
    h.run();
    click_and_run(&mut h, "Rectangle tool");
    assert_eq!(h.state().session.active_tool(), drafftink_engine::state::Tool::Rectangle);
    assert_eq!(h.state().model.tool, drafftink_engine::state::Tool::Rectangle);
    click_and_run(&mut h, "Keep tool active");
    assert!(h.state().model.tool_lock);
}

#[test]
fn clicking_a_stroke_swatch_changes_the_style_of_the_session() {
    let mut h = harness(Fixture::new());
    h.run();
    click_and_run(&mut h, "Stroke blue");
    assert_eq!(h.state().model.style.str_value("stroke_color"), Some("#3b82f6"));
}

#[test]
fn clicking_a_property_changes_the_selected_elements() {
    let mut h = harness(with_two_rects());
    h.run();
    click_and_run(&mut h, "Dashed");
    assert_eq!(h.state().model.style.str_value("stroke_style"), Some("dashed"));
    // The change is in the document: it is one undo step.
    assert_eq!(h.state().model.history.undo_label.as_deref(), Some("Set style"));
}

#[test]
fn the_align_button_moves_the_elements() {
    let mut h = harness(with_two_rects());
    h.run();
    click_and_run(&mut h, "Align left");
    let bounds = h.state_mut().session.execute("selection.get", json!({})).unwrap()["bounds"].clone();
    assert_eq!(bounds["x"], json!(0.0));
    assert_eq!(bounds["w"], json!(100.0), "both are 100 wide and now start at the same x");
}

#[test]
fn the_bottom_bar_changes_the_zoom_and_the_undo_button_undoes() {
    let mut h = harness(Fixture::new());
    h.run();
    click_and_run(&mut h, "Zoom in");
    assert_eq!(h.state().model.zoom_percent, 125.0);
    click_and_run(&mut h, "Reset zoom to 100%");
    assert_eq!(h.state().model.zoom_percent, 100.0);
    click_and_run(&mut h, "Smart guides");
    assert!(
        h.state().model.setting_bool("snap.smart_guides") != Fixture::new().model.setting_bool("snap.smart_guides")
    );

    let mut f = with_two_rects();
    f.session.execute("element.delete", json!({})).unwrap();
    f.refresh();
    assert_eq!(f.model.selection_count, 0);
    let mut h = harness(f);
    h.run();
    click_and_run(&mut h, "Undo");
    assert_eq!(h.state_mut().session.execute("element.query", json!({})).unwrap()["total"], json!(2));
}

#[test]
fn the_tab_bar_adds_switches_and_closes_tabs() {
    let mut h = harness(Fixture::new());
    h.run();
    click_and_run(&mut h, "New tab");
    assert_eq!(h.state().model.tabs.len(), 2);
    assert!(h.state().model.tabs[1].active, "a new tab is the active one");
    // Name the new tab, so the two can be told apart.
    h.state_mut().session.execute("doc.rename", json!({"name": "Second"})).unwrap();
    h.state_mut().refresh();
    h.run();
    click_and_run(&mut h, "Untitled");
    assert!(h.state().model.tabs[0].active);
    // The renamed tab has unsaved changes: the real command refuses, the UI asks, the answer closes it.
    h.state_mut().clear();
    h.get_by_label("Close Second").click();
    h.run();
    let report = h.state_mut().dispatch();
    assert_eq!(report.refusals().map(|r| r.code).collect::<Vec<_>>(), ["unsaved_changes"]);
    assert_eq!(h.state().model.tabs.len(), 2);
    h.run();
    click_and_run(&mut h, "Discard changes");
    assert_eq!(h.state().model.tabs.len(), 1);
    assert!(h.state().state.confirm.is_none());
}

#[test]
fn the_menu_row_switches_the_session_to_view_only() {
    let mut f = Fixture::new();
    f.state.menu_open = true;
    let mut h = harness(f);
    h.run();
    h.state_mut().clear();
    h.get_by_label("View-only mode").click();
    h.run();
    let report = h.state_mut().dispatch();
    assert_eq!(report.refusals().count(), 0, "{:?}", report.ran);
    assert!(h.state().session.view_only);
    assert!(h.state().model.view_only);
}
