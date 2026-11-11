//! The confirm dialog, the recent documents list, the search box and the stats panel.

mod common;
mod synth;

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
    let mut f = synth::fixture();
    let call = Call::new(origin, "doc.open", json!({"source": {"stored": "A"}, "mode": "replace"}));
    f.state.confirm = Some(Confirm { call, message: REFUSAL.to_owned() });
    let mut h = synth::harness(f);
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
    assert_eq!(synth::verbs(h.state()), [Verb::CloseOverlays]);
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
    assert_eq!(synth::verbs(h.state()), [Verb::CloseOverlays]);
}

#[test]
fn cancel_only_closes() {
    let mut h = confirming(Origin::Ui);
    h.get_by_label("Cancel").click();
    h.run();
    assert!(h.state().calls().is_empty());
    assert_eq!(synth::verbs(h.state()), [Verb::CloseOverlays]);
    h.state_mut().dispatch();
    assert!(h.state().state.confirm.is_none());
}

#[test]
fn confirming_runs_the_command_and_closes_the_box() {
    let mut h = confirming(Origin::Ui);
    h.get_by_label("Discard changes").click();
    h.run();
    h.state_mut().dispatch();
    assert!(h.state().state.confirm.is_none());
    assert_eq!(h.state().report.refusals().count(), 0);
}

#[test]
fn a_refused_confirmation_command_is_greyed() {
    // The real engine has no doc.open yet, so the buttons that repeat it are greyed with the build text.
    let mut f = Fixture::new();
    let call = Call::new(Origin::Ui, "doc.open", json!({}));
    f.state.confirm = Some(Confirm { call, message: REFUSAL.to_owned() });
    let mut h = synth::harness(f);
    h.run();
    assert!(h.get_by_label("Discard changes").accesskit_node().is_disabled());
    assert!(!h.get_by_label("Cancel").accesskit_node().is_disabled());
}

// ---- recent documents ----

fn recent(entries: Value) -> Harness<'static, Fixture> {
    let mut f = synth::fixture();
    f.model.settings.insert("doc.recent".to_owned(), entries);
    f.state.recent_open = true;
    let mut h = synth::harness(f);
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
    let mut f = synth::fixture();
    f.model.settings.remove("doc.recent");
    f.state.recent_open = true;
    let mut h = synth::harness(f);
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
    synth::click_at(&mut h, egui::pos2(40.0, 40.0));
    assert_eq!(synth::verbs(h.state()), [Verb::CloseOverlays]);
}

// ---- search ----

fn search(f: Fixture) -> Harness<'static, Fixture> {
    let mut f = f;
    f.state.search_open = true;
    let mut h = synth::harness(f);
    h.run();
    h
}

#[test]
fn search_with_an_unregistered_query_command_is_disabled_with_the_text() {
    // The real engine has no element.query yet.
    let f = Fixture::new();
    let text = f.model.refusal_text("element.query").expect("not registered in this build");
    assert_eq!(text, "`element.query` is not available in this build.");
    let h = search(f);
    h.get_by_label(&text);
    assert!(synth::field(&h, "Search text").accesskit_node().is_disabled());
}

#[test]
fn search_runs_element_query_on_enter_and_closes() {
    let mut h = search(synth::fixture());
    assert!(synth::field(&h, "Search text").is_focused(), "focused when it opens");
    synth::field(&h, "Search text").type_text("  box ");
    h.run();
    assert!(h.state().calls().is_empty(), "typing runs nothing");
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert_eq!(calls(&h), [("element.query".to_owned(), json!({"filter": {"text": "box"}}), Origin::Ui)]);
    assert_eq!(synth::verbs(h.state()), [Verb::CloseOverlays]);
}

#[test]
fn search_with_nothing_typed_does_nothing() {
    let mut h = search(synth::fixture());
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert!(h.state().calls().is_empty());
    assert!(synth::verbs(h.state()).is_empty());
}

#[test]
fn search_is_greyed_by_a_refusal_with_its_text() {
    let mut f = synth::fixture();
    f.session.view_only = true;
    f.refresh();
    let text = f.model.refusal_text("element.query").expect("the stand-in refuses in a view-only document");
    let h = search(f);
    h.get_by_label(&text);
    assert!(synth::field(&h, "Search text").accesskit_node().is_disabled());
}

// ---- stats ----

#[test]
fn stats_shows_the_model() {
    let mut f = synth::fixture();
    f.model.selection_count = 4;
    f.model.zoom_percent = 150.0;
    f.model.view_only = true;
    f.model.history.undo_label = Some("Move".to_owned());
    f.state.stats_open = true;
    let mut h = synth::harness(f);
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
    let mut f = synth::fixture();
    f.state.stats_open = true;
    let mut h = synth::harness(f);
    h.run();
    let r = h.get_by_label("Zoom").rect();
    assert!(r.left() < 60.0, "{r:?}");
    assert!(r.bottom() > 600.0 && r.bottom() < 800.0 - 40.0, "above the bottom bar: {r:?}");
}

#[test]
fn stats_is_hidden_when_off_and_in_zen_mode() {
    let mut h = synth::harness(synth::fixture());
    h.run();
    assert!(h.query_by_label("Zoom").is_none());
    h.state_mut().state.stats_open = true;
    h.state_mut().state.zen = true;
    h.run();
    assert!(h.query_by_label("Zoom").is_none());
}

#[test]
fn stats_reads_a_live_session() {
    let mut f = synth::fixture();
    f.session.execute("tool.set", json!({"tool": "rectangle"})).unwrap();
    f.refresh();
    f.state.stats_open = true;
    let mut h = synth::harness(f);
    h.run();
    h.get_by_label("rectangle");
    // The bottom bar's zoom button also shows "100%".
    assert!(h.query_all_by_role_and_label(egui::accesskit::Role::Label, "100%").count() >= 1);
}

#[test]
fn both_themes_draw_every_dialog() {
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        let mut f = synth::fixture();
        f.model.settings.insert("doc.recent".to_owned(), json!([{"stored": "A"}]));
        f.state.stats_open = true;
        f.state.recent_open = true;
        let mut h = synth::sized(f, egui::vec2(1280.0, 800.0), theme);
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
