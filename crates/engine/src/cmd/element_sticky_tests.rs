//! Tests of `element.create_sticky`: availability, exact refusal texts, the note and its label, selection, the
//! text edit, undo and redo, and negative cases that leave the fingerprint unchanged. No UI, no DOM: a headless
//! session over the fake ports (every character is half the font size wide).

use drafftink_edit::sticky::{FONT_CEILING, FONT_MIN, INK, NoteColor};
use drafftink_scene::{Element, ElementId, ElementKind};
use serde_json::{Value, json};

use crate::refusal::Refusal;
use crate::ring::Origin;
use crate::session::Session;
use crate::testing;

const ID: &str = "element.create_sticky";

fn run(s: &mut Session, args: Value) -> Value {
    match s.execute_from(Origin::Test, ID, args) {
        Ok(v) => v,
        Err(r) => panic!("{ID} refused: {} ({})", r.message, r.code),
    }
}

fn refuse(s: &mut Session, args: Value) -> Refusal {
    let before = s.fingerprint();
    let r = match s.execute_from(Origin::Test, ID, args) {
        Ok(v) => panic!("{ID} should have refused, got {v}"),
        Err(r) => r,
    };
    assert_eq!(s.fingerprint(), before, "the refusal left a trace: {}", r.message);
    r
}

fn eid(s: &str) -> ElementId {
    ElementId::new(s).expect("a valid id")
}

fn el(s: &Session, id: &str) -> Element {
    s.element(&eid(id)).unwrap_or_else(|| panic!("no live element {id}")).clone()
}

fn font_size(e: &Element) -> f64 {
    match &e.kind {
        ElementKind::Text(t) => t.font.size,
        _ => panic!("not text"),
    }
}

// ---- availability -------------------------------------------------------------------------------------------

#[test]
fn it_is_available_and_refuses_in_view_only_mode_with_the_shared_text() {
    let mut s = testing::headless_session();
    assert!(s.availability(ID).is_ok());
    s.view_only = true;
    let r = s.availability(ID).unwrap_err();
    assert_eq!(r.code, "view_only");
    let e = refuse(&mut s, json!({}));
    assert_eq!((e.code, e.message), (r.code, r.message));
}

// ---- effect -------------------------------------------------------------------------------------------------

#[test]
fn a_note_without_arguments_is_a_yellow_250_square_in_the_middle_of_the_view() {
    let mut s = testing::headless_session();
    let centre = s.view().scroll;
    let out = run(&mut s, json!({}));
    let e = el(&s, out["id"].as_str().unwrap());
    assert!(matches!(e.kind, ElementKind::Rectangle));
    assert_eq!((e.transform.w, e.transform.h), (250.0, 250.0));
    assert_eq!((e.transform.x + 125.0, e.transform.y + 125.0), (centre.x, centre.y));
    assert_eq!(e.style.fill_color, Some(NoteColor::Yellow.fill()));
    assert_eq!(e.style.stroke_color, INK);
    assert_eq!(e.style.corner_radius, 32.0);
    assert_eq!(out["label"], Value::Null, "no text, no label");
    assert_eq!(out["undo_label"], "Add sticky note");
    assert_eq!(s.selection().iter().map(ElementId::as_str).collect::<Vec<_>>(), vec![e.id.as_str()]);
}

#[test]
fn at_size_and_color_place_and_paint_the_note() {
    let mut s = testing::headless_session();
    let out = run(&mut s, json!({"at": [200, 100], "size": 120, "color": "pink"}));
    let e = el(&s, out["id"].as_str().unwrap());
    assert_eq!((e.transform.x, e.transform.y, e.transform.w, e.transform.h), (140.0, 40.0, 120.0, 120.0));
    assert_eq!(e.style.fill_color, Some(NoteColor::Pink.fill()));
    assert_eq!(e.style.corner_radius, 30.0, "a quarter of the short side");
}

#[test]
fn bounds_give_the_exact_box_and_a_negative_size_is_normalised() {
    let mut s = testing::headless_session();
    let out = run(&mut s, json!({"bounds": {"x": 300, "y": 200, "w": -160, "h": 90}}));
    assert_eq!(out["bounds"], json!({"x": 140.0, "y": 200.0, "w": 160.0, "h": 90.0}));
}

#[test]
fn text_becomes_a_centred_label_with_the_largest_fitting_font() {
    let mut s = testing::headless_session();
    let out = run(&mut s, json!({"text": "Buy milk", "at": [0, 0], "name": "todo"}));
    let note = el(&s, out["id"].as_str().unwrap());
    let label = el(&s, out["label"].as_str().unwrap());
    assert_eq!(note.name.as_deref(), Some("todo"));
    let ElementKind::Text(t) = &label.kind else { panic!("the label is text") };
    assert_eq!(t.content, "Buy milk");
    assert_eq!(t.container_id.as_ref(), Some(&note.id));
    assert_eq!(font_size(&label), FONT_CEILING);
    assert_eq!(out["font_size"], FONT_CEILING);
    assert_eq!(label.style.stroke_color, INK, "the label has the ink of the note");
    let (cx, cy) = (note.transform.x + 125.0, note.transform.y + 125.0);
    assert!((label.transform.x + label.transform.w / 2.0 - cx).abs() < 0.01, "centred across");
    assert!((label.transform.y + label.transform.h / 2.0 - cy).abs() < 0.01, "centred down");
}

#[test]
fn long_text_gets_a_smaller_font_and_font_size_lowers_the_ceiling() {
    let mut s = testing::headless_session();
    let long = "word ".repeat(40);
    let out = run(&mut s, json!({"text": long}));
    let size = out["font_size"].as_f64().unwrap();
    assert!((FONT_MIN..FONT_CEILING).contains(&size), "{size}");
    assert_eq!(el(&s, out["id"].as_str().unwrap()).transform.h, 250.0, "the note keeps its size");
    let out = run(&mut s, json!({"text": "Hi", "font_size": 20}));
    assert_eq!(out["font_size"], 20.0);
}

#[test]
fn text_that_cannot_fit_even_at_the_smallest_font_grows_the_note() {
    let mut s = testing::headless_session();
    let out = run(&mut s, json!({"text": "word ".repeat(400), "size": 100}));
    assert_eq!(out["font_size"], FONT_MIN);
    assert!(out["bounds"]["h"].as_f64().unwrap() > 100.0);
}

#[test]
fn a_note_over_a_frame_joins_it_unless_frame_is_null() {
    let mut s = testing::headless_session();
    let f = run_other(&mut s, json!({"kind": "frame", "bounds": {"x": -500, "y": -500, "w": 2000, "h": 2000}}));
    let centre = s.view().scroll;
    let out = run(&mut s, json!({"at": [centre.x, centre.y], "text": "in"}));
    let note = el(&s, out["id"].as_str().unwrap());
    assert_eq!(note.frame_id.as_ref().map(ElementId::as_str), Some(f.as_str()));
    assert_eq!(el(&s, out["label"].as_str().unwrap()).frame_id, note.frame_id, "the label is in the frame too");
    let out = run(&mut s, json!({"at": [centre.x, centre.y], "frame": null}));
    assert!(el(&s, out["id"].as_str().unwrap()).frame_id.is_none());
}

fn run_other(s: &mut Session, args: Value) -> String {
    let out = s.execute_from(Origin::Test, "element.create", args).expect("element.create");
    out["id"].as_str().unwrap().to_owned()
}

#[test]
fn select_false_leaves_the_selection_and_edit_true_starts_the_label_edit() {
    let (mut s, ids) = testing::session_with_shapes().unwrap();
    run(&mut s, json!({"select": false}));
    assert_eq!(s.selection().iter().collect::<Vec<_>>(), vec![&ids[0]]);
    assert!(s.text_edit.is_none());
    let out = run(&mut s, json!({"edit": true}));
    assert!(s.text_edit.is_some(), "the edit of the new note's label has begun");
    assert_eq!(s.selection().iter().map(ElementId::as_str).collect::<Vec<_>>(), vec![out["id"].as_str().unwrap()]);
}

// ---- undo ---------------------------------------------------------------------------------------------------

#[test]
fn one_undo_step_takes_note_and_label_away_and_redo_brings_both_back() {
    let (mut s, ids) = testing::session_with_shapes().unwrap();
    let out = run(&mut s, json!({"text": "Hello"}));
    let (note, label) = (out["id"].as_str().unwrap().to_owned(), out["label"].as_str().unwrap().to_owned());
    assert_eq!(s.tab().doc.undo_depth(), 1);
    s.undo_steps(1).unwrap();
    assert!(s.element(&eid(&note)).is_none() && s.element(&eid(&label)).is_none());
    assert_eq!(s.selection().iter().collect::<Vec<_>>(), vec![&ids[0]]);
    s.redo_steps(1).unwrap();
    assert!(s.element(&eid(&note)).is_some() && s.element(&eid(&label)).is_some());
}

// ---- refusals -----------------------------------------------------------------------------------------------

#[test]
fn a_note_under_75_units_is_refused_in_plain_words() {
    let mut s = testing::headless_session();
    let r = refuse(&mut s, json!({"size": 60}));
    assert_eq!(r.code, "too_small");
    assert_eq!(
        r.message,
        "The sticky note is smaller than 75 units in width and height. Give a size, or bounds with w and h, of at least 75."
    );
    let r = refuse(&mut s, json!({"bounds": {"x": 0, "y": 0, "w": 200, "h": 30}}));
    assert_eq!(
        r.message,
        "The sticky note is smaller than 75 units in height. Give a size, or bounds with w and h, of at least 75."
    );
}

#[test]
fn bounds_together_with_at_or_size_is_refused() {
    let mut s = testing::headless_session();
    let b = json!({"x": 0, "y": 0, "w": 100, "h": 100});
    for extra in [("at", json!([0, 0])), ("size", json!(100))] {
        let r = refuse(&mut s, json!({"bounds": b, extra.0: extra.1}));
        assert_eq!(r.code, "bad_args");
        assert!(r.message.contains(&format!("`{}`", extra.0)) && r.message.contains("`bounds`"), "{}", r.message);
    }
}

#[test]
fn a_font_size_outside_16_to_512_a_bad_color_and_a_bad_size_are_refused() {
    let mut s = testing::headless_session();
    for v in [json!(15), json!(513), json!(0)] {
        let r = refuse(&mut s, json!({"text": "x", "font_size": v}));
        assert_eq!(r.code, "bad_args");
        assert!(r.message.contains("`font_size`") && r.message.contains("16 to 512"), "{}", r.message);
    }
    assert_eq!(refuse(&mut s, json!({"color": "purple"})).code, "bad_args");
    assert_eq!(refuse(&mut s, json!({"size": -5})).code, "bad_args");
    assert_eq!(refuse(&mut s, json!({"size": "big"})).code, "bad_args");
    assert_eq!(refuse(&mut s, json!({"at": [f64::NAN, 0]})).code, "bad_args");
}

#[test]
fn id_and_name_must_be_free() {
    let mut s = testing::headless_session();
    run(&mut s, json!({"id": "note1", "name": "first"}));
    let r = refuse(&mut s, json!({"id": "note1"}));
    assert_eq!(r.code, "id_in_use");
    assert_eq!(r.message, "The id `note1` is already used by another element. Omit `id` to get a fresh one.");
    let r = refuse(&mut s, json!({"name": "first"}));
    assert_eq!(r.code, "name_taken");
    assert_eq!(refuse(&mut s, json!({"name": "@bad"})).code, "bad_name");
}

#[test]
fn a_missing_or_wrong_frame_target_is_refused_and_leaves_nothing() {
    let mut s = testing::headless_session();
    let rect = run_other(&mut s, json!({"kind": "rectangle", "bounds": {"x": 0, "y": 0, "w": 50, "h": 50}}));
    assert_eq!(refuse(&mut s, json!({"frame": "nope"})).code, "not_found");
    let r = refuse(&mut s, json!({"frame": rect}));
    assert_eq!(r.code, "wrong_kind");
    assert!(r.message.contains("not a frame"), "{}", r.message);
}

#[test]
fn a_locked_frame_target_is_refused() {
    let mut s = testing::headless_session();
    let f = run_other(&mut s, json!({"kind": "frame", "bounds": {"x": 0, "y": 0, "w": 400, "h": 400}}));
    run_other_cmd(&mut s, "element.lock", json!({"ids": [f.clone()]}));
    let r = refuse(&mut s, json!({"frame": f, "at": [200, 200]}));
    assert_eq!(r.code, "locked", "{}", r.message);
}

fn run_other_cmd(s: &mut Session, id: &str, args: Value) {
    s.execute_from(Origin::Test, id, args).unwrap_or_else(|r| panic!("{id} refused: {}", r.message));
}
