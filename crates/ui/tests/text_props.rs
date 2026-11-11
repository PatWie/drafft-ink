#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The ribbon's text box controls (vertical align, line height, auto-resize), the link section, and the
//! toolbar's sticky note and lasso buttons. Each control runs the real command with the right arguments.

mod common;
mod dialog;

use common::Fixture;
use drafftink_engine::Origin;
use drafftink_engine::testing::{add_rect, add_text_label, select};
use drafftink_scene::ElementId;
use drafftink_ui::{Call, HostRequest, Verb};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::{Value, json};

type H = Harness<'static, Fixture>;

fn calls(h: &H) -> Vec<Call> {
    h.state().calls().into_iter().cloned().collect()
}

fn started(f: Fixture) -> H {
    let mut h = dialog::harness(f);
    h.run();
    h
}

/// Opens the menu or dropdown of the ribbon with this accessible name.
fn open(h: &mut H, label: &str) {
    h.get_by_label(label).click();
    h.run();
    // The font dropdown opens its popover by a verb.
    h.state_mut().dispatch();
    h.run();
}

/// Opens a menu of the ribbon and chooses a row of it, like [`press`].
fn press_in(h: &mut H, menu: &str, label: &str) -> Call {
    open(h, menu);
    press(h, label)
}

/// Clicks the control, returns the one call it made and runs it.
fn press(h: &mut H, label: &str) -> Call {
    h.state_mut().clear();
    h.get_by_label(label).click();
    h.run();
    let mut made = calls(h);
    assert_eq!(made.len(), 1, "{label}: {made:?}");
    let report = h.state_mut().dispatch();
    assert!(report.refusals().next().is_none(), "{label}: {:?}", report.ran);
    h.run();
    made.remove(0)
}

fn style(h: &H, prop: &str) -> Option<Value> {
    h.state().model.style.prop(prop).and_then(|p| p.value.clone())
}

/// A rectangle with a label, selected.
fn labelled() -> Fixture {
    let mut f = dialog::fixture();
    let r = add_rect(&mut f.session, 0.0, 0.0, 200.0, 120.0).unwrap();
    add_text_label(&mut f.session, &r, "Hi").unwrap();
    select(&mut f.session, std::slice::from_ref(&r));
    f.refresh();
    f
}

fn free_text() -> Fixture {
    let mut f = dialog::fixture();
    let out = dialog::run(&mut f, "element.create", json!({ "kind": "text", "text": "hello world", "at": [0, 0] }));
    let id = ElementId::new(out["id"].as_str().unwrap()).unwrap();
    select(&mut f.session, std::slice::from_ref(&id));
    f.refresh();
    f
}

#[test]
fn vertical_align_buttons_set_the_property_of_a_labelled_shape() {
    let mut h = started(labelled());
    for (label, value) in [("Middle", "middle"), ("Bottom", "bottom"), ("Top", "top")] {
        let call = press_in(&mut h, "More options", label);
        assert_eq!(call, Call::new(Origin::Ui, "style.set", json!({ "props": { "vertical_align": value } })));
        assert_eq!(style(&h, "vertical_align"), Some(json!(value)));
    }
}

#[test]
fn vertical_align_is_not_offered_for_a_shape_without_a_label_or_for_free_text() {
    let mut f = dialog::fixture();
    let r = add_rect(&mut f.session, 0.0, 0.0, 50.0, 50.0).unwrap();
    select(&mut f.session, std::slice::from_ref(&r));
    f.refresh();
    let mut h = started(f);
    open(&mut h, "More options");
    h.get_by_label("Architect");
    assert!(h.query_by_label("Middle").is_none());
    let f = free_text();
    let mut h = started(f);
    open(&mut h, "More options");
    h.get_by_label("Fixed width");
    assert!(h.query_by_label("Middle").is_none(), "free text has nothing to align in");
}

#[test]
fn line_height_presets_set_the_property() {
    let f = free_text();
    let mut h = started(f);
    for height in [1.5, 2.0, 1.0] {
        let call = press_in(&mut h, "More options", &format!("Line height {height}"));
        assert_eq!(call, Call::new(Origin::Ui, "style.set", json!({ "props": { "line_height": height } })));
        assert_eq!(style(&h, "line_height").and_then(|v| v.as_f64()), Some(height));
    }
}

#[test]
fn line_height_is_also_offered_before_drawing_text() {
    let mut f = dialog::fixture();
    dialog::run(&mut f, "tool.set", json!({ "tool": "text" }));
    let mut h = started(f);
    let call = press_in(&mut h, "More options", "Line height 1.5");
    assert_eq!(call.args, json!({ "props": { "line_height": 1.5 } }));
    assert_eq!(h.state().session.tool.defaults().to_props().line_height, Some(1.5));
}

#[test]
fn the_font_dropdown_offers_plex_sans_and_sets_the_family() {
    let mut h = started(free_text());
    let call = press_in(&mut h, "Pick the font", "IBM Plex Sans");
    assert_eq!(call, Call::new(Origin::Ui, "style.set", json!({ "props": { "font_family": "plex_sans" } })));
    assert_eq!(style(&h, "font_family"), Some(json!("plex_sans")));
}

#[test]
fn auto_resize_buttons_run_text_set_autoresize() {
    let f = free_text();
    let mut h = started(f);
    let call = press_in(&mut h, "More options", "Fixed width");
    assert_eq!(call, Call::new(Origin::Ui, "text.set_autoresize", json!({ "on": false })));
    let call = press_in(&mut h, "More options", "Fit to text");
    assert_eq!(call.args, json!({ "on": true }));
}

#[test]
fn the_box_choice_is_only_for_text() {
    let mut f = dialog::fixture();
    let r = add_rect(&mut f.session, 0.0, 0.0, 50.0, 50.0).unwrap();
    select(&mut f.session, std::slice::from_ref(&r));
    f.refresh();
    let mut h = started(f);
    open(&mut h, "More options");
    h.get_by_label("Architect");
    assert!(h.query_by_label("Fit to text").is_none());
}

#[test]
fn the_link_buttons_edit_and_open_the_link() {
    let mut f = dialog::fixture();
    let r = add_rect(&mut f.session, 0.0, 0.0, 50.0, 50.0).unwrap();
    select(&mut f.session, std::slice::from_ref(&r));
    dialog::run(&mut f, "element.set_link", json!({ "link": "https://example.com/doc" }));
    let mut h = started(f);
    h.state_mut().clear();
    h.get_by_label("Edit the link").click();
    h.run();
    assert_eq!(dialog::verbs(h.state()), vec![Verb::OpenLinkEditor]);
    h.state_mut().clear();
    h.get_by_label("Open the link").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::Ui, "element.open_link", json!({}))]);
    let report = h.state_mut().dispatch();
    assert_eq!(report.host, vec![HostRequest::OpenUrl("https://example.com/doc".to_owned())]);
}

#[test]
fn without_a_link_the_model_has_none_and_the_open_button_is_still_there() {
    let mut f = dialog::fixture();
    let r = add_rect(&mut f.session, 0.0, 0.0, 50.0, 50.0).unwrap();
    select(&mut f.session, std::slice::from_ref(&r));
    f.refresh();
    assert_eq!(f.model.selection_link, None);
    let h = started(f);
    assert!(!h.get_by_label("Edit the link").accesskit_node().is_disabled());
    // The button is not hidden. The engine decides when it is clicked, and says why in words.
    h.get_by_label("Open the link");
}

#[test]
fn the_model_exposes_what_the_canvas_overlay_needs_about_a_link() {
    let mut f = dialog::fixture();
    let r = add_rect(&mut f.session, 0.0, 0.0, 50.0, 50.0).unwrap();
    let other = add_rect(&mut f.session, 300.0, 0.0, 50.0, 50.0).unwrap();
    select(&mut f.session, std::slice::from_ref(&r));
    let link = format!("#{}", other.as_str());
    dialog::run(&mut f, "element.set_link", json!({ "link": link }));
    let item = &f.model.selection[0];
    assert_eq!((item.id.as_str(), item.kind.as_str()), (r.as_str(), "rectangle"));
    assert_eq!(item.link.as_deref(), Some(link.as_str()));
    assert_eq!(f.model.selection_link.as_deref(), Some(link.as_str()));
}

#[test]
fn the_sticky_note_button_creates_a_note_open_for_typing() {
    let mut h = started(dialog::fixture());
    let before = dialog::element_count(h.state_mut());
    let call = press(&mut h, "Sticky note");
    assert_eq!(call, Call::new(Origin::Ui, "element.create_sticky", json!({ "edit": true })));
    assert_eq!(dialog::element_count(h.state_mut()), before + 1);
    assert!(h.state().session.text_edit.is_some(), "the note is open for typing");
}

#[test]
fn the_lasso_button_is_a_toggle_between_lasso_and_select() {
    let mut h = started(dialog::fixture());
    let call = press(&mut h, "Lasso tool");
    assert_eq!(call.args, json!({ "tool": "lasso" }));
    assert_eq!(h.state().model.tool, drafftink_engine::state::Tool::Lasso);
    let call = press(&mut h, "Lasso tool");
    assert_eq!(call.args, json!({ "tool": "select" }), "the active lasso button goes back to select");
    assert_eq!(h.state().model.tool, drafftink_engine::state::Tool::Select);
}

#[test]
fn the_library_button_toggles_the_panel() {
    let mut h = started(dialog::fixture());
    h.state_mut().clear();
    h.get_by_label("Show or hide the library").click();
    h.run();
    assert_eq!(dialog::verbs(h.state()), vec![Verb::ToggleLibrary]);
}

#[test]
fn the_arrow_tool_ribbon_sets_the_defaults_for_the_next_arrow_with_tool_set_arrow() {
    let mut f = dialog::fixture();
    dialog::run(&mut f, "tool.set", json!({ "tool": "arrow" }));
    let mut h = started(f);
    let call = press(&mut h, "New arrows: Elbow");
    assert_eq!(call, Call::new(Origin::Ui, "tool.set_arrow", json!({ "arrow_type": "elbow" })));
    let call = press_in(&mut h, "End head", "New arrows, End head: Bar");
    assert_eq!(call.args, json!({ "end_head": "bar" }));
    let call = press_in(&mut h, "Start head", "New arrows, Start head: Dot");
    assert_eq!(call.args, json!({ "start_head": "circle" }));
    assert_eq!(h.state().model.setting("arrow.type"), Some(&json!("elbow")));
    assert_eq!(h.state().model.setting("arrow.end_head"), Some(&json!("bar")));
}

// ---- bold and italic ----

fn runs(h: &H) -> Vec<drafftink_scene::TextRun> {
    let id = h.state().session.selection().iter().next().expect("a selection");
    h.state().session.element(id).and_then(|e| e.text_data().map(|t| t.runs.clone())).unwrap_or_default()
}

fn on(h: &H, label: &str) -> bool {
    h.get_by_label(label).accesskit_node().toggled() == Some(egui::accesskit::Toggled::True)
}

#[test]
fn the_emphasis_buttons_run_text_format_and_show_the_state_of_the_selected_text() {
    let mut h = started(free_text());
    assert!(!on(&h, "Bold text") && !on(&h, "Italic text"));
    let call = press(&mut h, "Bold text");
    assert_eq!(call, Call::new(Origin::Ui, "text.format", json!({ "bold": true })));
    assert_eq!(runs(&h), vec![drafftink_scene::TextRun { start: 0, end: 11, bold: true, ..Default::default() }]);
    assert!(on(&h, "Bold text"), "the button shows that all of the text is bold");
    assert!(!on(&h, "Italic text"));
    let call = press(&mut h, "Italic text");
    assert_eq!(call.args, json!({ "italic": true }));
    // A button that is on turns its face off.
    let call = press(&mut h, "Bold text");
    assert_eq!(call.args, json!({ "bold": false }));
    assert!(!on(&h, "Bold text") && on(&h, "Italic text"));
    assert_eq!(runs(&h).iter().filter(|r| r.bold).count(), 0);
}

#[test]
fn the_emphasis_buttons_format_the_label_of_a_selected_shape() {
    let mut h = started(labelled());
    let call = press(&mut h, "Italic text");
    assert_eq!(call.args, json!({ "italic": true }));
    assert!(on(&h, "Italic text"));
}

#[test]
fn a_part_bold_text_has_no_state_and_the_button_makes_all_of_it_bold() {
    let mut f = free_text();
    let id = f.session.selection().iter().next().expect("a selection").as_str().to_owned();
    dialog::run(&mut f, "text.format", json!({ "ids": [id], "range": [0, 5], "bold": true }));
    f.refresh();
    let mut h = started(f);
    assert!(!on(&h, "Bold text"), "mixed is not on");
    let call = press(&mut h, "Bold text");
    assert_eq!(call.args, json!({ "bold": true }));
    assert!(on(&h, "Bold text"));
}

#[test]
fn there_are_no_emphasis_buttons_without_a_text() {
    let mut f = dialog::fixture();
    let r = add_rect(&mut f.session, 0.0, 0.0, 50.0, 50.0).unwrap();
    select(&mut f.session, std::slice::from_ref(&r));
    f.refresh();
    let h = started(f);
    assert!(h.query_by_label("Bold text").is_none());
}

#[test]
fn while_editing_the_buttons_go_to_the_editor_with_its_state() {
    use drafftink_ui::UiAction;
    let mut f = free_text();
    f.model.text_editing = true;
    f.model.editor_faces = Some(drafftink_ui::TextFaces { bold: Some(true), italic: Some(false) });
    let mut h = started(f);
    assert!(on(&h, "Bold text"), "the editor's selection wins over the stored text");
    h.state_mut().clear();
    h.get_by_label("Bold text").click();
    h.run();
    assert!(h.state().calls().is_empty(), "no command: {:?}", h.state().actions);
    assert_eq!(
        h.state().actions,
        vec![UiAction::Host(HostRequest::TextEditFormat(json!({ "bold": false })))],
        "the editor is asked to turn bold off"
    );
    h.state_mut().clear();
    h.get_by_label("Italic text").click();
    h.run();
    assert_eq!(h.state().actions, vec![UiAction::Host(HostRequest::TextEditFormat(json!({ "italic": true })))]);
}
