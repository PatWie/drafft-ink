//! The LaTeX editor dialog.

mod common;
mod synth;

use common::Fixture;
use drafftink_engine::Origin;
use drafftink_ui::{HostRequest, Verb};
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::{Value, json};

fn open(mut f: Fixture, latex: &str) -> Harness<'static, Fixture> {
    f.state.open_equation(latex);
    let mut h = synth::harness(f);
    h.run();
    h
}

fn calls(h: &Harness<'static, Fixture>) -> Vec<(String, Value, Origin)> {
    h.state().calls().into_iter().map(|c| (c.id.clone(), c.args.clone(), c.origin)).collect()
}

fn edit_end(args: Value) -> (String, Value, Origin) {
    ("text.edit_end".to_owned(), args, Origin::Ui)
}

fn replace_text(h: &mut Harness<'static, Fixture>, text: &str) {
    synth::field(h, "LaTeX").focus();
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run();
    synth::field(h, "LaTeX").type_text(text);
    h.run();
}

#[test]
fn apply_sends_the_edited_text() {
    let mut h = open(synth::fixture(), "x^2");
    replace_text(&mut h, r"\frac{a}{b}");
    h.get_by_label("Apply").click();
    h.run();
    assert_eq!(calls(&h), [edit_end(json!({"commit": true, "latex": r"\frac{a}{b}"}))]);
}

#[test]
fn apply_without_edits_keeps_the_latex_it_was_opened_with() {
    let mut h = open(synth::fixture(), "x^2");
    h.get_by_label("Apply").click();
    h.run();
    assert_eq!(calls(&h), [edit_end(json!({"commit": true, "latex": "x^2"}))]);
}

#[test]
fn cancel_and_the_x_end_the_edit_without_committing() {
    let mut h = open(synth::fixture(), "x^2");
    h.get_by_label("Cancel").click();
    h.run();
    assert_eq!(calls(&h), [edit_end(json!({"commit": false}))]);
    h.state_mut().clear();
    h.get_by_label("Close").click();
    h.run();
    assert_eq!(calls(&h), [edit_end(json!({"commit": false}))]);
}

#[test]
fn a_click_on_the_backdrop_cancels() {
    let mut h = open(synth::fixture(), "x^2");
    synth::click_at(&mut h, egui::pos2(40.0, 40.0));
    assert_eq!(calls(&h), [edit_end(json!({"commit": false}))]);
}

#[test]
fn mod_enter_applies_and_does_not_add_a_line() {
    let mut h = open(synth::fixture(), "a");
    replace_text(&mut h, "x+y");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    h.run();
    assert_eq!(calls(&h), [edit_end(json!({"commit": true, "latex": "x+y"}))]);
}

#[test]
fn plain_enter_adds_a_line() {
    let mut h = open(synth::fixture(), "a");
    replace_text(&mut h, "x");
    h.key_press(Key::Enter);
    h.run();
    assert!(h.state().calls().is_empty(), "plain Enter is a new line");
    assert_eq!(h.state().state.equation.as_ref().map(|e| e.latex.as_str()), Some("x\n"));
}

#[test]
fn copy_and_paste_are_host_requests() {
    let mut h = open(synth::fixture(), "x^2");
    h.get_by_label("Copy").click();
    h.run();
    assert_eq!(synth::hosts(h.state()), [HostRequest::CopyText("x^2".to_owned())]);
    h.state_mut().clear();
    h.get_by_label("Paste").click();
    h.run();
    assert_eq!(synth::hosts(h.state()), [HostRequest::PasteText]);
    assert!(h.state().calls().is_empty());
}

#[test]
fn pasted_text_lands_at_the_end_of_the_field() {
    let mut h = open(synth::fixture(), "x^2");
    h.get_by_label("Paste").click();
    h.run();
    h.state_mut().state.set_pasted_text("+y".to_owned());
    h.run();
    h.run();
    assert_eq!(h.state().state.equation.as_ref().map(|e| e.latex.as_str()), Some("x^2+y"));
    h.state_mut().clear();
    h.get_by_label("Apply").click();
    h.run();
    assert_eq!(calls(&h), [edit_end(json!({"commit": true, "latex": "x^2+y"}))]);
    // Taken once: it is not pasted again.
    assert!(h.state_mut().state.take_pasted_text().is_none());
}

#[test]
fn the_field_is_focused_on_the_first_frame_only() {
    let mut h = open(synth::fixture(), "x^2");
    assert!(synth::field(&h, "LaTeX").is_focused());
    assert!(h.state().state.equation.as_ref().is_some_and(|e| e.focused));
    // The user moves away; the dialog does not pull the focus back.
    h.get_by_label_contains("Examples").click();
    h.run();
    h.run();
    h.run();
    assert!(!synth::field(&h, "LaTeX").is_focused());
}

#[test]
fn the_hint_lists_the_examples() {
    let h = open(synth::fixture(), "");
    h.get_by_label(r"Examples: x^2, \frac{a}{b}, \sqrt{x}, \sum_{i=1}^n");
}

#[test]
fn the_dialog_closes_when_the_command_ran() {
    let mut h = open(synth::fixture(), "x");
    h.get_by_label("Apply").click();
    h.run();
    h.state_mut().dispatch();
    assert!(h.state().state.equation.is_none(), "after_command closes the dialog");
    assert_eq!(h.state().report.refusals().count(), 0);
}

#[test]
fn unregistered_text_edit_end_greys_apply_and_cancel() {
    // The real engine registers no text.edit_end yet.
    let f = Fixture::new();
    assert!(f.model.refusal_text("text.edit_end").is_some_and(|t| t.contains("not available in this build")));
    let mut h = open(f, "x^2");
    for label in ["Apply", "Cancel"] {
        assert!(h.get_by_label(label).accesskit_node().is_disabled(), "{label}");
    }
    h.get_by_label("Apply").click();
    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    h.run();
    assert!(h.state().calls().is_empty());
}

#[test]
fn it_does_not_use_close_overlays() {
    let mut h = open(synth::fixture(), "x");
    h.get_by_label("Cancel").click();
    h.run();
    assert!(!synth::verbs(h.state()).contains(&Verb::CloseOverlays));
}

#[test]
fn both_themes_draw_the_dialog() {
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        let mut f = synth::fixture();
        f.state.open_equation("x^2");
        let mut h = synth::sized(f, egui::vec2(1280.0, 800.0), theme);
        h.run();
        h.get_by_label("Apply");
    }
}
