#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! How the UI presents a refusal: the engine's words, unchanged, in the tooltip of a greyed control and in the toast
//! of a refused run; a control is greyed and not hidden; a refused gesture command is a toast; a refused write of
//! an export is a refusal and not a success.

mod common;
mod dialog;
mod support_a;

use common::Fixture;
use drafftink_engine::testing::{add_rect, select};
use drafftink_engine::{Origin, Refusal};
use drafftink_ui::action::CommandSink;
use drafftink_ui::{Call, CollabStatus, CollabView, Scrub, ScrubPhase, UiAction, UiState, Verb, dispatch};
use egui::accesskit::Role;
use egui::pos2;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::{Value, json};
use support_a::menu;

type H = egui_kittest::Harness<'static, Fixture>;

/// The text the engine gives when it refuses `id` with these arguments now.
fn engine_text(f: &mut Fixture, id: &str, args: Value) -> String {
    f.session.execute_from(Origin::Ui, id, args).expect_err("the engine refuses this").message
}

fn menu(mut f: Fixture) -> H {
    f.refresh();
    f.state.open_context_menu(pos2(300.0, 60.0), None);
    let mut h = support_a::themed_lenient(f, false);
    h.run();
    h
}

fn hover_shows(h: &mut H, node: &str, text: &str) {
    menu::row(h, node).hover();
    for _ in 0..10 {
        h.step();
    }
    h.get_by_label_contains(text);
}

// ---- the same words everywhere ------------------------------------------------------------------------------------

#[test]
fn the_tooltip_the_refusal_and_the_toast_of_a_refused_command_are_the_same_words() {
    // View-only mode refuses `clipboard.paste`, the one row of the menu that stays when its command is refused.
    let mut f = dialog::fixture();
    dialog::run(&mut f, "view.set_view_only", json!({ "on": true }));
    let text = engine_text(&mut f, "clipboard.paste", json!({}));
    assert_eq!(f.model.refusal_text("clipboard.paste"), Some(text.clone()), "availability says what execute says");

    let mut h = menu(f);
    assert!(menu::row(&h, "Paste").accesskit_node().is_disabled());
    hover_shows(&mut h, "Paste", &text);

    // The same command run anyway (a shortcut, a script) shows the same words as a toast.
    h.state_mut().actions.push(UiAction::run("clipboard.paste", json!({})));
    let report = h.state_mut().dispatch();
    assert_eq!(report.refusals().next().map(|r| r.message.clone()), Some(text.clone()));
    assert_eq!(h.state().state.toasts.iter().map(|t| t.text.clone()).collect::<Vec<_>>(), vec![text]);
}

// ---- the menu leaves out what does not apply; the panel greys it ----------------------------------------------------

#[test]
fn the_context_menu_leaves_out_rows_the_engine_refuses_and_keeps_the_ones_it_allows() {
    let mut f = dialog::fixture();
    let a = add_rect(&mut f.session, 0.0, 0.0, 50.0, 50.0).unwrap();
    select(&mut f.session, std::slice::from_ref(&a));
    f.refresh();
    let h = menu(f);
    // Each of these rows is missing because the engine refuses its command now, with its own words.
    for (text, id) in [
        ("Group", "group.create"),
        ("Ungroup", "group.ungroup"),
        ("Align", "arrange.align"),
        ("Open link", "element.open_link"),
        ("Paste style", "clipboard.paste_style"),
    ] {
        assert!(h.state().model.refusal_text(id).is_some(), "{id} is refused");
        assert!(!menu::has_row(&h, text), "{text} is shown although {id} is refused");
    }
    // `Link…` is as available as `element.set_link`.
    assert!(h.state().model.refusal_text("element.set_link").is_none());
    assert!(!menu::row(&h, "Link…").accesskit_node().is_disabled());
}

#[test]
fn the_ribbon_leaves_align_out_for_one_element_and_greys_the_link_with_the_engine_text() {
    let mut f = dialog::fixture();
    let a = add_rect(&mut f.session, 0.0, 0.0, 50.0, 50.0).unwrap();
    select(&mut f.session, std::slice::from_ref(&a));
    f.refresh();
    assert!(f.model.refusal_text("arrange.align").is_some(), "one element cannot be aligned");
    let mut h = support_a::themed_lenient(f, false);
    h.run();
    // The group follows the engine's answer: a command that is refused has no buttons.
    assert!(h.query_by_label("Align left").is_none());
    // The link controls are there for the one selected element.
    h.get_by_label("Edit the link");
    h.get_by_label("Open the link");
}

#[test]
fn the_link_controls_stay_for_two_selected_elements_and_follow_their_commands() {
    let mut f = dialog::fixture();
    let a = add_rect(&mut f.session, 0.0, 0.0, 50.0, 50.0).unwrap();
    let b = add_rect(&mut f.session, 100.0, 0.0, 50.0, 50.0).unwrap();
    select(&mut f.session, &[a, b]);
    f.refresh();
    let mut h = support_a::themed_lenient(f, false);
    h.run();
    for (label, id) in [("Edit the link", "element.set_link"), ("Open the link", "element.open_link")] {
        let refused = h.state().model.refusal_text(id);
        assert_eq!(h.get_by_label(label).accesskit_node().is_disabled(), refused.is_some(), "{label}");
    }
}

// ---- no reason of the UI's own ---------------------------------------------------------------------------------------

#[test]
fn a_property_that_does_not_apply_is_greyed_with_the_text_style_set_refuses_with() {
    let mut f = dialog::fixture();
    let made = dialog::run(&mut f, "element.create", json!({ "kind": "arrow", "points": [[0, 0], [80, 30]] }));
    let id = made["id"].as_str().unwrap().to_owned();
    select(&mut f.session, &[drafftink_scene::ElementId::new(&id).unwrap()]);
    f.refresh();
    let reason = f.model.style.reason("fill_color").expect("an arrow has no fill").to_owned();
    // What `style.set` says for exactly this property and this selection.
    let said = engine_text(&mut f, "style.set", json!({ "props": { "fill_color": "red-500" } }));
    assert_eq!(reason, said);
    let mut h = support_a::themed_lenient(f, false);
    h.run();
    let fill = h.get_all_by_label_contains("Fill").find(|n| n.accesskit_node().is_disabled());
    assert!(fill.is_some(), "a fill swatch is greyed, not hidden");
    fill.unwrap().hover();
    for _ in 0..10 {
        h.step();
    }
    h.get_by_label_contains(&reason);
}

fn two_tabs(f: &mut Fixture) {
    dialog::run(f, "doc.rename", json!({ "name": "First" }));
    dialog::run(f, "tab.new", json!({ "name": "Second" }));
    dialog::run(f, "tab.switch", json!({ "tab": "First" }));
}

fn open_tab_menu(h: &mut H) {
    h.get_by_label("First").click_secondary();
    h.run();
}

#[test]
fn the_tab_menu_adds_no_reason_of_its_own() {
    let mut f = dialog::fixture();
    two_tabs(&mut f);
    let mut h = dialog::harness(f);
    h.run();
    open_tab_menu(&mut h);
    for label in ["Move left", "Move right"] {
        assert!(!h.get_by_label(label).accesskit_node().is_disabled(), "{label} is as available as tab.move");
    }
    for invented in ["This is the first tab.", "This is the last tab."] {
        assert!(h.query_by_label_contains(invented).is_none(), "{invented}");
    }
    // Without the command the row is greyed with the engine's words for that.
    let mut g = Fixture::without(&["tab.move"]);
    two_tabs(&mut g);
    let text = g.model.refusal_text("tab.move").unwrap();
    let mut h = dialog::harness(g);
    h.run();
    open_tab_menu(&mut h);
    assert!(h.get_by_label("Move left").accesskit_node().is_disabled());
    h.get_by_label("Move left").hover();
    for _ in 0..10 {
        h.step();
    }
    h.get_by_label_contains(&text);
}

#[test]
fn the_export_dialog_greys_options_with_what_the_engine_says_and_not_with_its_own_words() {
    let mut f = dialog::fixture();
    dialog::run(&mut f, "element.create", json!({ "kind": "rectangle", "bounds": [0, 0, 50, 50] }));
    dialog::run(&mut f, "selection.clear", json!({}));
    f.state.apply_verb(Verb::OpenExport, &mut Vec::new());
    f.refresh();
    let none = f.model.refusal_text("element.delete").expect("nothing to delete");
    let mut h = dialog::harness(f);
    h.run();
    // "Selection only" is greyed with the engine's text for a command that needs a selection.
    assert!(h.get_by_label("Selection only").accesskit_node().is_disabled());
    h.get_by_label("Selection only").hover();
    for _ in 0..10 {
        h.step();
    }
    h.get_by_label_contains(&none);
    // A JSON export has no scale: the 2x choice is greyed with the engine's description of `scale`.
    h.get_by_role_and_label(Role::Button, "JSON").click();
    h.run();
    let doc = drafftink_ui::model::arg_doc("doc.export", "scale").expect("the engine documents `scale`");
    assert!(h.get_by_label("2x").accesskit_node().is_disabled());
    h.get_by_label("2x").hover();
    for _ in 0..10 {
        h.step();
    }
    h.get_by_label_contains(&doc);
    for invented in ["Nothing is selected.", "This format has no picture options."] {
        assert!(h.query_by_label_contains(invented).is_none(), "{invented}");
    }
}

#[test]
fn the_collab_dialog_greys_the_server_and_the_room_with_the_engine_text() {
    let mut f = dialog::connected(Some("team"));
    let connect = engine_text(&mut f, "collab.connect", json!({ "url": "ws://127.0.0.1:3030/ws" }));
    let join = engine_text(&mut f, "collab.join_room", json!({ "room": "other" }));
    assert_eq!(f.model.refusal_text("collab.connect"), Some(connect.clone()));
    assert_eq!(f.model.refusal_text("collab.join_room"), Some(join.clone()));
    f.model.collab =
        CollabView { status: CollabStatus::Connected, room: Some("team".to_owned()), ..CollabView::default() };
    f.state.collab_open = true;
    let mut h = dialog::harness(f);
    h.run();
    for (label, text) in [("Server URL", &connect), ("Room", &join)] {
        let node = h.get_by_role_and_label(Role::TextInput, label);
        assert!(node.accesskit_node().is_disabled(), "{label} is greyed");
        node.hover();
        for _ in 0..10 {
            h.step();
        }
        h.get_by_label_contains(text);
    }
    for invented in ["Disconnect first to change the server.", "Leave the room to join another."] {
        assert!(h.query_by_label_contains(invented).is_none(), "{invented}");
    }
}

// ---- gesture refusals ------------------------------------------------------------------------------------------------

/// A sink that refuses every `gesture.*` command with a code and a text.
struct Refusing {
    code: &'static str,
    text: &'static str,
}

impl CommandSink for Refusing {
    fn run(&mut self, _: Origin, id: &str, _: Value) -> Result<Value, Refusal> {
        if id.starts_with("gesture.") && id != "gesture.begin" {
            Err(Refusal::custom(self.code, id, self.text))
        } else {
            Ok(json!({ "gesture": "g1" }))
        }
    }
}

fn scrub(phase: ScrubPhase) -> UiAction {
    UiAction::Scrub(Scrub { phase, command: "style.set", args: json!({ "props": { "opacity": 0.4 } }) })
}

#[test]
fn a_refused_gesture_command_is_a_toast_with_the_engine_text() {
    for phase in [ScrubPhase::Update, ScrubPhase::End] {
        let mut st = UiState::new();
        let mut sink = Refusing { code: "locked", text: "The element is locked, so it cannot change." };
        dispatch(&mut sink, &mut st, vec![scrub(ScrubPhase::Begin), scrub(phase)], 0.0);
        let texts: Vec<&str> = st.toasts.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(texts, ["The element is locked, so it cannot change."], "{phase:?}");
        assert!(st.scrub_gesture().is_none(), "the control stops scrubbing after a refusal");
    }
}

#[test]
fn a_gesture_that_is_simply_gone_is_not_worth_a_toast() {
    let mut st = UiState::new();
    let mut sink = Refusing { code: "no_gesture", text: "There is no gesture g1." };
    dispatch(&mut sink, &mut st, vec![scrub(ScrubPhase::Begin), scrub(ScrubPhase::End)], 0.0);
    assert!(st.toasts.is_empty(), "{:?}", st.toasts);
}

#[test]
fn the_real_engine_refusal_of_a_scrub_reaches_the_screen() {
    let mut f = dialog::fixture();
    let a = add_rect(&mut f.session, 0.0, 0.0, 50.0, 50.0).unwrap();
    select(&mut f.session, &[a]);
    f.refresh();
    f.actions = vec![scrub(ScrubPhase::Begin)];
    assert_eq!(f.dispatch().refusals().count(), 0);
    // A step the command cannot take: an opacity of 5.
    f.actions = vec![UiAction::Scrub(Scrub {
        phase: ScrubPhase::Update,
        command: "style.set",
        args: json!({ "props": { "opacity": 5.0 } }),
    })];
    let report = f.dispatch();
    let refusal = report.refusals().next().expect("the engine refuses the step").clone();
    assert_ne!(refusal.code, "no_gesture", "{refusal:?}");
    assert_eq!(f.state.toasts.iter().map(|t| t.text.clone()).collect::<Vec<_>>(), vec![refusal.message.clone()]);
    let mut h = dialog::harness(f);
    h.run();
    h.get_by_label_contains(&refusal.message);
}

// ---- doc.export ------------------------------------------------------------------------------------------------------

fn export_dialog(f: Fixture) -> H {
    let mut f = f;
    f.state.apply_verb(Verb::OpenExport, &mut Vec::new());
    let mut h = dialog::harness(f);
    h.run();
    h
}

fn drawing() -> Fixture {
    let mut f = dialog::fixture();
    dialog::run(&mut f, "doc.rename", json!({ "name": "Plan" }));
    add_rect(&mut f.session, 0.0, 0.0, 50.0, 50.0).unwrap();
    f.refresh();
    f
}

#[test]
fn an_export_whose_write_was_refused_keeps_the_dialog_and_shows_the_reason() {
    let f = drawing();
    f.fakes.files.fail_writes(true);
    let mut h = export_dialog(f);
    h.get_by_role_and_label(Role::Button, "JSON").click();
    h.run();
    h.get_by_role_and_label(Role::Button, "Export").click();
    h.run();
    let report = h.state_mut().dispatch();
    let refusal = report.refusals().next().expect("the write was refused, so the export is refused").clone();
    assert_eq!(refusal.code, "io_failed");
    assert!(
        refusal.message.contains("did NOT happen") && refusal.message.contains("Nothing was written"),
        "{}",
        refusal.message
    );
    assert!(h.state().state.export.is_some(), "the dialog stays open");
    assert_eq!(
        h.state().state.toasts.iter().map(|t| t.text.clone()).collect::<Vec<_>>(),
        vec![refusal.message.clone()]
    );

    // When the port works again the same click writes the file and closes the dialog.
    h.state().fakes.files.fail_writes(false);
    h.state_mut().state.toasts.clear();
    h.run();
    h.get_by_role_and_label(Role::Button, "Export").click();
    h.run();
    let report = h.state_mut().dispatch();
    assert_eq!(report.refusals().count(), 0, "{:?}", report.ran);
    assert!(h.state().state.export.is_none());
}

/// A sink that answers every export as the browser's port does when it has not finished writing.
struct Pending;

impl CommandSink for Pending {
    fn run(&mut self, _: Origin, _: &str, _: Value) -> Result<Value, Refusal> {
        Ok(json!({ "changed": false, "pending_write": [7] }))
    }
}

#[test]
fn an_export_whose_write_is_not_confirmed_yet_does_not_close_the_dialog() {
    let mut st = UiState::new();
    st.apply_verb(Verb::OpenExport, &mut Vec::new());
    let call = Call::new(Origin::Ui, "doc.export", json!({ "format": "png" }));
    dispatch(&mut Pending, &mut st, vec![UiAction::Command(call)], 0.0);
    assert!(st.export.is_some(), "not confirmed is not done");
    assert_eq!(st.toasts.len(), 1);
    assert!(st.toasts[0].text.contains("not confirmed"), "{}", st.toasts[0].text);
}
