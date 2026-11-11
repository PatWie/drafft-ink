//! The confirm dialog, the recent documents list, the search box and the stats panel.

mod common;
mod dialog;

use common::Fixture;
use drafftink_engine::Origin;
use drafftink_ui::state::Confirm;
use drafftink_ui::{Call, Verb};
use egui::Key;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::{Value, json};

fn calls(h: &Harness<'static, Fixture>) -> Vec<(String, Value, Origin)> {
    h.state().calls().into_iter().map(|c| (c.id.clone(), c.args.clone(), c.origin)).collect()
}

// ---- confirm ----

const REFUSAL: &str =
    "This tab has unsaved changes. Pass on_unsaved=\"discard\" to drop them or \"save\" to save first.";

fn confirming(origin: Origin) -> Harness<'static, Fixture> {
    let mut f = dialog::fixture();
    let call = Call::new(origin, "doc.open", json!({"source": {"stored": "A"}, "mode": "replace"}));
    f.state.confirm = Some(Confirm { call, message: REFUSAL.to_owned() });
    let mut h = dialog::harness(f);
    h.run();
    h
}

#[test]
fn confirm_shows_the_refusal_text_unchanged() {
    let h = confirming(Origin::Ui);
    h.get_by_label(REFUSAL);
    h.get_by_label("Discard changes");
    h.get_by_label("Save first");
    h.get_by_label("Cancel");
}

#[test]
fn discard_repeats_the_call_with_on_unsaved_discard() {
    let mut h = confirming(Origin::Ui);
    h.get_by_label("Discard changes").click();
    h.run();
    assert_eq!(
        calls(&h),
        [(
            "doc.open".to_owned(),
            json!({"source": {"stored": "A"}, "mode": "replace", "on_unsaved": "discard"}),
            Origin::Ui
        )]
    );
    assert_eq!(dialog::verbs(h.state()), [Verb::CloseOverlays]);
}

#[test]
fn save_first_repeats_the_call_with_on_unsaved_save_and_keeps_the_origin() {
    let mut h = confirming(Origin::Shortcut);
    h.get_by_label("Save first").click();
    h.run();
    let got = calls(&h);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].1["on_unsaved"], json!("save"));
    assert_eq!(got[0].2, Origin::Shortcut, "the repeat comes from the surface of the original call");
    assert_eq!(dialog::verbs(h.state()), [Verb::CloseOverlays]);
}

#[test]
fn cancel_only_closes() {
    let mut h = confirming(Origin::Ui);
    h.get_by_label("Cancel").click();
    h.run();
    assert!(h.state().calls().is_empty());
    assert_eq!(dialog::verbs(h.state()), [Verb::CloseOverlays]);
    h.state_mut().dispatch();
    assert!(h.state().state.confirm.is_none());
}

#[test]
fn confirming_runs_the_command_and_closes_the_box() {
    // A real round trip: unsaved work in the tab, a stored document, the refusal, the box, the choice.
    let mut f = dialog::fixture();
    dialog::store_document(&f, "Other", 2);
    dialog::run(&mut f, "element.create", json!({"kind": "text", "text": "unsaved work", "at": [0, 0]}));
    f.actions.push(drafftink_ui::UiAction::run("doc.open", json!({"source": {"stored": "Other"}, "mode": "replace"})));
    f.dispatch();
    let asked = f.state.confirm.clone().expect("the refusal opens the box");
    assert!(asked.message.contains("unsaved changes"), "{}", asked.message);
    assert_eq!(dialog::element_count(&mut f), 1, "nothing was replaced yet");

    let mut h = dialog::harness(f);
    h.run();
    h.get_by_label("Discard changes").click();
    h.run();
    let report = h.state_mut().dispatch();
    assert_eq!(report.refusals().count(), 0, "{:?}", report.ran);
    assert!(h.state().state.confirm.is_none());
    assert_eq!(dialog::element_count(h.state_mut()), 2, "the stored document replaced the unsaved one");
}

#[test]
fn a_refused_confirmation_command_is_greyed() {
    // A build without doc.open: the buttons that repeat it are greyed with the build text.
    let mut f = Fixture::without(&["doc.open"]);
    let call = Call::new(Origin::Ui, "doc.open", json!({}));
    f.state.confirm = Some(Confirm { call, message: REFUSAL.to_owned() });
    let mut h = dialog::harness(f);
    h.run();
    assert!(h.get_by_label("Discard changes").accesskit_node().is_disabled());
    assert!(!h.get_by_label("Cancel").accesskit_node().is_disabled());
}

// ---- recent documents ----

fn recent(entries: Value) -> Harness<'static, Fixture> {
    let mut f = dialog::fixture();
    f.model.settings.insert("doc.recent".to_owned(), entries);
    f.state.recent_open = true;
    let mut h = dialog::harness(f);
    h.run();
    h
}

#[test]
fn recent_empty_state() {
    let h = recent(json!([]));
    h.get_by_label("No recent documents yet.");
}

#[test]
fn recent_without_the_setting_is_empty_too() {
    let mut f = dialog::fixture();
    f.model.settings.remove("doc.recent");
    f.state.recent_open = true;
    let mut h = dialog::harness(f);
    h.run();
    h.get_by_label("No recent documents yet.");
}

#[test]
fn picking_a_recent_document_opens_it_replacing_the_current_one() {
    let mut h = recent(json!([{"path": "/home/me/plan.drafftink"}, {"stored": "Sketch"}, {"nonsense": true}]));
    assert!(h.query_by_label("No recent documents yet.").is_none());
    h.get_by_label("Sketch");
    h.get_by_label("/home/me/plan.drafftink").click();
    h.run();
    assert_eq!(
        calls(&h),
        [(
            "doc.open".to_owned(),
            json!({"source": {"path": "/home/me/plan.drafftink"}, "mode": "replace"}),
            Origin::Ui
        )]
    );
    assert!(!h.state().state.recent_open, "the list closes");
}

#[test]
fn picking_a_stored_document_passes_the_file_reference_as_is() {
    let mut h = recent(json!([{"stored": "Sketch"}]));
    h.get_by_label("Sketch").click();
    h.run();
    assert_eq!(calls(&h)[0].1, json!({"source": {"stored": "Sketch"}, "mode": "replace"}));
}

#[test]
fn recent_survives_hostile_settings() {
    for v in [json!("x"), json!(7), json!(null), json!([[], 1, "a", {"path": 3}, {"path": null}])] {
        let mut h = recent(v);
        h.run();
        h.get_by_label("No recent documents yet.");
    }
}

#[test]
fn a_click_outside_closes_the_recent_list() {
    let mut h = recent(json!([]));
    dialog::click_at(&mut h, egui::pos2(40.0, 40.0));
    assert_eq!(dialog::verbs(h.state()), [Verb::CloseOverlays]);
}

// ---- search ----

fn search(f: Fixture) -> Harness<'static, Fixture> {
    let mut f = f;
    f.state.search_open = true;
    let mut h = dialog::harness(f);
    h.run();
    h
}

#[test]
fn search_with_an_unregistered_query_command_is_disabled_with_the_text() {
    let f = Fixture::without(&["element.query"]);
    let text = f.model.refusal_text("element.query").expect("not registered in this build");
    assert_eq!(text, "`element.query` is not available in this build.");
    let h = search(f);
    h.get_by_label(&text);
    assert!(dialog::field(&h, "Search text").accesskit_node().is_disabled());
}

/// A fixture with two texts and a rectangle, for the search tests.
fn with_texts() -> Fixture {
    let mut f = dialog::fixture();
    for (text, at) in [("Login box", [10, 20]), ("Logout", [300, 20])] {
        dialog::run(&mut f, "element.create", json!({"kind": "text", "text": text, "at": at}));
    }
    dialog::run(&mut f, "element.create", json!({"kind": "rectangle", "bounds": {"x": 0, "y": 200, "w": 50, "h": 50}}));
    f.state.search_open = true;
    f
}

/// Types into the search box, presses Enter and lets the app run the query.
fn search_for(h: &mut Harness<'static, Fixture>, text: &str) {
    dialog::field(h, "Search text").type_text(text);
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
    h.state_mut().dispatch();
    h.run();
}

#[test]
fn search_runs_element_query_on_enter_and_stays_open() {
    let mut h = search(dialog::fixture());
    assert!(dialog::field(&h, "Search text").is_focused(), "focused when it opens");
    dialog::field(&h, "Search text").type_text("  box ");
    h.run();
    assert!(h.state().calls().is_empty(), "typing runs nothing");
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert_eq!(calls(&h), [("element.query".to_owned(), json!({"filter": {"text": "box"}}), Origin::Ui)]);
    assert!(dialog::verbs(h.state()).is_empty(), "the box stays open for the results");
    assert!(dialog::field(&h, "Search text").is_focused(), "and keeps the focus, so the text can be refined");
}

#[test]
fn search_lists_what_the_real_query_found() {
    let mut h = search(with_texts());
    search_for(&mut h, "log");
    h.get_by_label("2 elements found.");
    h.get_by_label("Login box (text)");
    h.get_by_label("Logout (text)");
    let results = h.state().state.search_results.clone().expect("the query result was kept");
    assert_eq!((results.query.as_str(), results.total, results.hits.len()), ("log", 2, 2));
}

#[test]
fn search_says_when_nothing_matches() {
    let mut h = search(with_texts());
    search_for(&mut h, "zebra");
    h.get_by_label("Nothing contains \"zebra\".");
    assert!(h.query_by_label_contains("(text)").is_none());
}

#[test]
fn choosing_a_result_selects_the_element_centres_the_view_on_it_and_closes() {
    let mut h = search(with_texts());
    search_for(&mut h, "logout");
    let hit = h.state().state.search_results.as_ref().and_then(|r| r.hits.first().cloned()).expect("one hit");
    h.state_mut().clear();
    h.get_by_label("Logout (text)").click();
    h.run();
    let ids: Vec<_> = h.state().calls().iter().map(|c| c.id.clone()).collect();
    assert_eq!(ids, ["selection.select", "view.center"]);
    assert_eq!(dialog::verbs(h.state()), [Verb::CloseOverlays]);
    h.state_mut().dispatch();
    assert_eq!(h.state().report.refusals().count(), 0);
    // The real commands ran: the element is selected and the view is centred on its middle.
    let selected: Vec<_> = h.state().session.selection().iter().map(|i| i.as_str().to_owned()).collect();
    assert_eq!(selected, std::slice::from_ref(&hit.id));
    let scroll = h.state().session.view().scroll;
    let centre = hit.center.expect("a text has bounds");
    assert!((scroll.x - centre[0]).abs() < 0.01 && (scroll.y - centre[1]).abs() < 0.01, "{scroll:?} vs {centre:?}");
    assert!(!h.state().state.search_open && h.state().state.search_results.is_none());
}

#[test]
fn a_query_that_is_not_from_the_search_box_leaves_no_results() {
    let mut f = with_texts();
    f.state.search_open = false;
    f.actions.push(drafftink_ui::UiAction::run("element.query", json!({"filter": {"text": "log"}})));
    let report = f.dispatch();
    assert_eq!(report.refusals().count(), 0);
    assert!(f.state.search_results.is_none());
}

#[test]
fn search_with_nothing_typed_does_nothing() {
    let mut h = search(dialog::fixture());
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert!(h.state().calls().is_empty());
    assert!(dialog::verbs(h.state()).is_empty());
}

#[test]
fn a_bad_pattern_is_refused_with_the_engines_text_and_the_box_stays() {
    let mut f = with_texts();
    let call = drafftink_ui::Call::new(Origin::Ui, "element.query", json!({"filter": {"text": {"regex": "(["}}}));
    f.actions.push(drafftink_ui::UiAction::Command(call));
    let report = f.dispatch();
    assert_eq!(report.refusals().count(), 1);
    assert_eq!(f.state.toasts.len(), 1);
    assert!(f.state.search_results.is_none());
}

#[test]
fn search_works_in_a_view_only_document() {
    // A read: the real command does not refuse in a view-only document, so the field stays enabled.
    let mut f = with_texts();
    f.session.view_only = true;
    f.refresh();
    assert!(f.model.refusal_text("element.query").is_none());
    let h = search(f);
    assert!(!dialog::field(&h, "Search text").accesskit_node().is_disabled());
}

// ---- stats ----

#[test]
fn stats_shows_the_model() {
    let mut f = dialog::fixture();
    f.model.selection_count = 4;
    f.model.zoom_percent = 150.0;
    f.model.view_only = true;
    f.model.history.undo_label = Some("Move".to_owned());
    f.state.stats_open = true;
    let mut h = dialog::harness(f);
    h.run();
    for text in [
        "Selected",
        "4",
        "Tool",
        "select",
        "Zoom",
        "150%",
        "Undo",
        "Move",
        "Redo",
        "nothing",
        "View only",
        "yes",
        "Tabs",
    ] {
        // The bottom bar has buttons named Undo, Redo and 100%: the panel's own text is a label.
        h.get_by_role_and_label(egui::accesskit::Role::Label, text);
    }
}

#[test]
fn stats_sits_at_the_bottom_left() {
    let mut f = dialog::fixture();
    f.state.stats_open = true;
    let mut h = dialog::harness(f);
    h.run();
    let r = h.get_by_label("Zoom").rect();
    assert!(r.left() < 60.0, "{r:?}");
    assert!(r.bottom() > 600.0 && r.bottom() < 800.0 - 40.0, "above the bottom bar: {r:?}");
}

#[test]
fn stats_is_hidden_when_off_and_in_zen_mode() {
    let mut h = dialog::harness(dialog::fixture());
    h.run();
    assert!(h.query_by_label("Zoom").is_none());
    h.state_mut().state.stats_open = true;
    h.state_mut().state.zen = true;
    h.run();
    assert!(h.query_by_label("Zoom").is_none());
}

#[test]
fn stats_reads_a_live_session() {
    let mut f = dialog::fixture();
    f.session.execute("tool.set", json!({"tool": "rectangle"})).unwrap();
    f.refresh();
    f.state.stats_open = true;
    let mut h = dialog::harness(f);
    h.run();
    h.get_by_label("rectangle");
    // The bottom bar's zoom button also shows "100%".
    assert!(h.query_all_by_role_and_label(egui::accesskit::Role::Label, "100%").count() >= 1);
}

/// A fixture with one rectangle at (10, 20), 100 by 60, selected, and the stats panel open.
fn stats_with_a_selection() -> Fixture {
    let mut f = dialog::fixture();
    dialog::run(
        &mut f,
        "element.create",
        json!({"kind": "rectangle", "bounds": {"x": 10, "y": 20, "w": 100, "h": 60}}),
    );
    f.state.stats_open = true;
    f
}

fn type_into_field(h: &mut Harness<'static, Fixture>, label: &str, text: &str) {
    dialog::field(h, label).focus();
    h.run();
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
    h.run();
    dialog::field(h, label).type_text(text);
    h.run();
}

fn bounds_of_selection(f: &mut Fixture) -> Value {
    dialog::run(f, "selection.get", json!({}))["bounds"].clone()
}

#[test]
fn stats_shows_the_bounds_of_the_selection_in_editable_fields() {
    let mut h = dialog::harness(stats_with_a_selection());
    h.run();
    for (label, want) in
        [("Selection X", "10"), ("Selection Y", "20"), ("Selection width", "100"), ("Selection height", "60")]
    {
        assert_eq!(dialog::field(&h, label).value().as_deref(), Some(want), "{label}");
    }
    assert_eq!(h.state().model.selection_bounds.map(|b| (b.x, b.y, b.w, b.h)), Some((10.0, 20.0, 100.0, 60.0)));
}

#[test]
fn stats_has_no_bounds_fields_without_a_selection() {
    let mut f = dialog::fixture();
    f.state.stats_open = true;
    let mut h = dialog::harness(f);
    h.run();
    assert!(h.query_by_label("Selection X").is_none());
}

#[test]
fn editing_a_bounds_field_runs_set_bounds_with_only_that_value() {
    let mut h = dialog::harness(stats_with_a_selection());
    h.run();
    type_into_field(&mut h, "Selection X", "50");
    assert!(h.state().calls().is_empty(), "typing sends nothing");
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert_eq!(calls(&h), [("element.set_bounds".to_owned(), json!({"x": 50.0}), Origin::Ui)]);
    // The real command moved the element; the size stayed.
    let report = h.state_mut().dispatch();
    assert_eq!(report.refusals().count(), 0, "{:?}", report.ran);
    assert_eq!(bounds_of_selection(h.state_mut()), json!({"x": 50.0, "y": 20.0, "w": 100.0, "h": 60.0}));
    // The panel shows the new value on the next frames.
    h.run();
    h.run();
    assert_eq!(dialog::field(&h, "Selection X").value().as_deref(), Some("50"));
}

#[test]
fn editing_the_size_changes_the_element() {
    let mut h = dialog::harness(stats_with_a_selection());
    h.run();
    type_into_field(&mut h, "Selection width", "250.5");
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert_eq!(calls(&h), [("element.set_bounds".to_owned(), json!({"w": 250.5}), Origin::Ui)]);
    h.state_mut().dispatch();
    assert_eq!(bounds_of_selection(h.state_mut())["w"], json!(250.5));
}

#[test]
fn a_bounds_edit_is_sent_when_the_field_loses_focus() {
    let mut h = dialog::harness(stats_with_a_selection());
    h.run();
    type_into_field(&mut h, "Selection Y", "-5");
    dialog::click_canvas(&mut h);
    h.run();
    assert_eq!(calls(&h), [("element.set_bounds".to_owned(), json!({"y": -5.0}), Origin::Ui)]);
}

#[test]
fn text_that_is_not_a_valid_size_sends_nothing_and_the_field_goes_back() {
    for bad in ["abc", "", "0", "-4", "NaN", "inf"] {
        let mut h = dialog::harness(stats_with_a_selection());
        h.run();
        type_into_field(&mut h, "Selection width", bad);
        h.key_press(Key::Enter);
        h.run();
        h.run();
        h.run();
        assert!(h.state().calls().is_empty(), "{bad:?}: {:?}", h.state().calls());
        assert_eq!(dialog::field(&h, "Selection width").value().as_deref(), Some("100"), "{bad:?}");
    }
}

#[test]
fn escape_drops_a_bounds_edit() {
    let mut h = dialog::harness(stats_with_a_selection());
    h.run();
    type_into_field(&mut h, "Selection X", "99");
    h.key_press(Key::Escape);
    h.run();
    h.run();
    assert!(h.state().calls().is_empty());
    assert_eq!(dialog::field(&h, "Selection X").value().as_deref(), Some("10"));
}

#[test]
fn an_unchanged_value_sends_nothing() {
    let mut h = dialog::harness(stats_with_a_selection());
    h.run();
    dialog::field(&h, "Selection X").focus();
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert!(h.state().calls().is_empty());
}

#[test]
fn bounds_fields_are_greyed_with_the_refusal_of_the_real_command() {
    // Locked-down document: the real command refuses in a view-only document.
    let mut f = stats_with_a_selection();
    f.session.view_only = true;
    f.refresh();
    let text = f.model.refusal_text("element.set_bounds").expect("set_bounds refuses in a view-only document");
    let h = dialog::harness(f);
    let mut h = h;
    h.run();
    assert!(dialog::field(&h, "Selection X").accesskit_node().is_disabled());
    dialog::field(&h, "Selection X").hover();
    for _ in 0..10 {
        h.step();
    }
    h.get_by_label_contains(&text);
}

#[test]
fn bounds_fields_are_greyed_when_the_build_has_no_set_bounds() {
    let mut f = Fixture::without(&["element.set_bounds"]);
    dialog::run(
        &mut f,
        "element.create",
        json!({"kind": "rectangle", "bounds": {"x": 10, "y": 20, "w": 100, "h": 60}}),
    );
    f.state.stats_open = true;
    let text = f.model.refusal_text("element.set_bounds").expect("not registered");
    assert_eq!(text, "`element.set_bounds` is not available in this build.");
    let mut h = dialog::harness(f);
    h.run();
    assert!(dialog::field(&h, "Selection width").accesskit_node().is_disabled());
}

#[test]
fn both_themes_draw_every_dialog() {
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        let mut f = dialog::fixture();
        f.model.settings.insert("doc.recent".to_owned(), json!([{"stored": "A"}]));
        f.state.stats_open = true;
        f.state.recent_open = true;
        let mut h = dialog::sized(f, egui::vec2(1280.0, 800.0), theme);
        h.run();
        h.get_by_label("A");
        h.state_mut().state.recent_open = false;
        h.state_mut().state.search_open = true;
        h.run();
        h.state_mut().state.search_open = false;
        h.state_mut().state.confirm =
            Some(Confirm { call: Call::new(Origin::Ui, "doc.open", json!({})), message: "m".into() });
        h.run();
        h.get_by_label("Save first");
    }
}
