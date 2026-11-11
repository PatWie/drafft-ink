//! Tests of the `text` family. Pattern per command: availability, exact refusal text, effect with undo, one negative
//! case that leaves the session untouched. Text is measured by the fake services (every character is half the font
//! size wide); formulas need the real typesetter, so those tests use `testing::session_typeset`.

use drafftink_scene::{Color, ElementId, ElementKind, MathData, SceneRead, SceneTx, TextData, TextRun};
use serde_json::{Value, json};

use crate::refusal::Refusal;
use crate::ring::Origin;
use crate::session::Session;
use crate::testing;

fn run(s: &mut Session, id: &str, args: Value) -> Value {
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => v,
        Err(r) => panic!("{id} refused: {} ({})", r.message, r.code),
    }
}

fn refuse(s: &mut Session, id: &str, args: Value) -> Refusal {
    let before = s.fingerprint();
    let r = match s.execute_from(Origin::Test, id, args) {
        Ok(v) => panic!("{id} should have refused, got {v}"),
        Err(r) => r,
    };
    assert_eq!(s.fingerprint(), before, "{id} refused but changed the session");
    r
}

/// Like `refuse`, for a command that ends the edit: the session ends it before the refusal, and the
/// document is untouched.
fn refuse_keeping_edit(s: &mut Session, id: &str, args: Value) -> Refusal {
    let doc = s.tab().doc.revision();
    let r = match s.execute_from(Origin::Test, id, args) {
        Ok(v) => panic!("{id} should have refused, got {v}"),
        Err(r) => r,
    };
    assert_eq!(s.tab().doc.revision(), doc, "the document must not change");
    r
}

fn text_at(s: &mut Session, content: &str, x: f64, y: f64) -> ElementId {
    let w = content.chars().count() as f64 * 10.0;
    testing::add(s, ElementKind::Text(TextData::new(content)), x, y, w, 25.0).unwrap()
}

fn rect(s: &mut Session) -> ElementId {
    testing::add_rect(s, 0.0, 0.0, 200.0, 100.0).unwrap()
}

fn content(s: &Session, id: &ElementId) -> String {
    s.element(id).unwrap().text_data().unwrap().content.clone()
}

fn label_of(s: &Session, container: &ElementId) -> Option<ElementId> {
    drafftink_edit::text_bind::label_of(s.store(), container)
}

fn font_size(s: &Session, id: &ElementId) -> f64 {
    s.element(id).unwrap().text_data().unwrap().font.size
}

// ---- text.set ----

#[test]
fn set_replaces_the_content_remeasures_and_undoes_in_one_step() {
    let (mut s, _f) = testing::session();
    let t = text_at(&mut s, "hi", 10.0, 20.0);
    let out = run(&mut s, "text.set", json!({"id": t.as_str(), "text": "hello world"}));
    assert_eq!(out["id"], t.as_str());
    assert_eq!(out["removed"], false);
    assert_eq!(content(&s, &t), "hello world");
    // 11 characters at 20 / 2 = 10 each.
    assert_eq!(s.element(&t).unwrap().transform.w, 110.0);
    assert_eq!(s.tab().doc.undo_depth(), 1);
    s.undo_steps(1).unwrap();
    assert_eq!(content(&s, &t), "hi");
}

#[test]
fn set_keeps_colour_runs_of_the_unchanged_ends_unless_asked_not_to() {
    let (mut s, _f) = testing::session();
    let t = text_at(&mut s, "hello", 0.0, 0.0);
    let red = Color::parse("#ff0000").unwrap();
    run(&mut s, "text.style_range", json!({"id": t.as_str(), "start": 0, "end": 2, "color": "#ff0000"}));
    run(&mut s, "text.set", json!({"id": t.as_str(), "text": "heXXXllo"}));
    assert_eq!(s.element(&t).unwrap().text_data().unwrap().runs, vec![TextRun { start: 0, end: 2, color: Some(red) }]);
    run(&mut s, "text.set", json!({"id": t.as_str(), "text": "other", "keep_runs": false}));
    assert!(s.element(&t).unwrap().text_data().unwrap().runs.is_empty());
}

#[test]
fn set_on_a_shape_creates_changes_and_removes_its_label() {
    let (mut s, _f) = testing::session();
    let r = rect(&mut s);
    let out = run(&mut s, "text.set", json!({"id": r.as_str(), "text": "Start"}));
    let label = label_of(&s, &r).unwrap();
    assert_eq!(out["id"], label.as_str());
    assert_eq!(content(&s, &label), "Start");
    run(&mut s, "text.set", json!({"id": r.as_str(), "text": "Begin"}));
    assert_eq!(content(&s, &label), "Begin");
    let out = run(&mut s, "text.set", json!({"id": r.as_str(), "text": ""}));
    assert_eq!(out["removed"], true);
    assert!(label_of(&s, &r).is_none());
    // A shape without label and an empty text: nothing happens.
    let depth = s.tab().doc.undo_depth();
    let out = run(&mut s, "text.set", json!({"id": r.as_str(), "text": " "}));
    assert_eq!(out["changed"], false);
    assert_eq!(s.tab().doc.undo_depth(), depth);
}

#[test]
fn set_refusals_have_the_catalog_texts() {
    let (mut s, _f) = testing::session();
    let t = text_at(&mut s, "hi", 0.0, 0.0);
    let m = testing::add(&mut s, ElementKind::Math(MathData::new("x")), 0.0, 100.0, 20.0, 20.0).unwrap();
    let line = testing::add(&mut s, ElementKind::Line(drafftink_scene::LinearData::new(vec![
        drafftink_geom::kurbo::Point::new(0.0, 0.0),
        drafftink_geom::kurbo::Point::new(10.0, 10.0),
    ])), 0.0, 200.0, 10.0, 10.0)
    .unwrap();
    let r = refuse(&mut s, "text.set", json!({"id": m.as_str(), "text": "a"}));
    assert_eq!((r.code, r.message.as_str()), ("wrong_kind", "`text.set` edits text elements and labels. For math use `text.set_latex`."));
    assert_eq!(refuse(&mut s, "text.set", json!({"id": line.as_str(), "text": "a"})).code, "wrong_kind");
    let r = refuse(&mut s, "text.set", json!({"id": t.as_str(), "text": "  "}));
    assert_eq!(r.code, "empty_text");
    assert_eq!(r.message, "A text element cannot be empty. Delete it with `element.delete`.");
    let r = refuse(&mut s, "text.set", json!({"id": "nope", "text": "a"}));
    assert_eq!(r.code, "not_found");
    assert!(r.message.contains("`doc.outline`"));
}

// ---- text.set_latex ----

#[test]
fn set_latex_lays_the_formula_out_once_and_is_undoable() {
    let (mut s, _f) = testing::session_typeset();
    let m = testing::add(&mut s, ElementKind::Math(MathData::new("x")), 5.0, 6.0, 10.0, 10.0).unwrap();
    let out = run(&mut s, "text.set_latex", json!({"id": m.as_str(), "latex": "\\frac{a}{b} + \\sqrt{x}"}));
    assert!(out["bounds"]["w"].as_f64().unwrap() > 10.0);
    let e = s.element(&m).unwrap();
    assert!(matches!(&e.kind, ElementKind::Math(d) if d.latex == "\\frac{a}{b} + \\sqrt{x}"));
    assert_eq!((e.transform.x, e.transform.y), (5.0, 6.0));
    assert_eq!(s.tab().doc.undo_depth(), 1);
    s.undo_steps(1).unwrap();
    assert!(matches!(&s.element(&m).unwrap().kind, ElementKind::Math(d) if d.latex == "x"));
}

#[test]
fn set_latex_with_a_parse_error_names_the_problem_and_changes_nothing() {
    let (mut s, _f) = testing::session_typeset();
    let m = testing::add(&mut s, ElementKind::Math(MathData::new("x")), 0.0, 0.0, 10.0, 10.0).unwrap();
    let r = refuse(&mut s, "text.set_latex", json!({"id": m.as_str(), "latex": "\\nosuchcommand{x}"}));
    assert_eq!(r.code, "invalid_latex");
    assert!(r.message.starts_with("The LaTeX could not be parsed: "), "{}", r.message);
    assert!(r.message.contains("Fix the expression. Examples: `x^2`"), "{}", r.message);
    let r = refuse(&mut s, "text.set_latex", json!({"id": m.as_str(), "latex": "  "}));
    assert_eq!(r.code, "invalid_latex");
}

#[test]
fn set_latex_on_text_refuses_with_the_catalog_text() {
    let (mut s, _f) = testing::session_typeset();
    let t = text_at(&mut s, "hi", 0.0, 0.0);
    let r = refuse(&mut s, "text.set_latex", json!({"id": t.as_str(), "latex": "x"}));
    assert_eq!(r.message, "`text.set_latex` edits math elements. For text use `text.set`.");
}

// ---- text.style_range ----

#[test]
fn style_range_sets_and_clears_character_colours() {
    let (mut s, _f) = testing::session();
    let t = text_at(&mut s, "hello", 0.0, 0.0);
    let out = run(&mut s, "text.style_range", json!({"id": t.as_str(), "start": 1, "end": 3, "color": "#00ff00"}));
    assert_eq!(out["runs"], json!([[1, 3, "#00ff00"]]));
    assert_eq!(s.tab().doc.undo_depth(), 1);
    let out = run(&mut s, "text.style_range", json!({"id": t.as_str(), "start": 1, "end": 3}));
    assert_eq!(out["runs"], json!([]));
    // Setting the colour that is already there changes nothing and pushes no step.
    run(&mut s, "text.style_range", json!({"id": t.as_str(), "start": 0, "end": 2, "color": "#00ff00"}));
    let depth = s.tab().doc.undo_depth();
    let out = run(&mut s, "text.style_range", json!({"id": t.as_str(), "start": 0, "end": 2, "color": "#00ff00"}));
    assert_eq!(out["changed"], false);
    assert_eq!(s.tab().doc.undo_depth(), depth);
}

#[test]
fn style_range_checks_range_and_kind() {
    let (mut s, _f) = testing::session();
    let t = text_at(&mut s, "hello", 0.0, 0.0);
    let r = refuse(&mut s, "text.style_range", json!({"id": t.as_str(), "start": 2, "end": 9, "color": "#ff0000"}));
    assert_eq!(r.code, "bad_range");
    assert_eq!(r.message, "The range 2..9 is outside the text, which has 5 characters. Use a range within 0..5.");
    assert_eq!(refuse(&mut s, "text.style_range", json!({"id": t.as_str(), "start": 3, "end": 3})).code, "bad_range");
    let r = rect(&mut s);
    assert_eq!(refuse(&mut s, "text.style_range", json!({"id": r.as_str(), "start": 0, "end": 1})).code, "wrong_kind");
}

// ---- text.step_size ----

#[test]
fn step_size_walks_the_presets_and_undoes() {
    let (mut s, _f) = testing::session();
    let t = text_at(&mut s, "abcd", 10.0, 20.0);
    testing::select(&mut s, std::slice::from_ref(&t));
    let sizes: Vec<f64> = (0..5)
        .map(|_| {
            run(&mut s, "text.step_size", json!({"direction": "increase"}));
            font_size(&s, &t)
        })
        .collect();
    assert_eq!(sizes, [28.0, 36.0, 44.0, 52.0, 60.0]);
    run(&mut s, "text.step_size", json!({"direction": "decrease"}));
    assert_eq!(font_size(&s, &t), 52.0);
    assert_eq!(s.element(&t).unwrap().transform.w, 4.0 * 26.0);
    // Repeats within a second are merged into one undo step (`coalesce`), so one undo takes them all back.
    s.undo_steps(1).unwrap();
    assert_eq!(font_size(&s, &t), 20.0);
}

#[test]
fn step_size_at_the_end_of_the_range_is_a_no_op() {
    let (mut s, _f) = testing::session();
    let t = text_at(&mut s, "a", 0.0, 0.0);
    for _ in 0..3 {
        run(&mut s, "text.step_size", json!({"direction": "decrease", "ids": [t.as_str()]}));
    }
    assert_eq!(font_size(&s, &t), 8.0);
    let depth = s.tab().doc.undo_depth();
    let out = run(&mut s, "text.step_size", json!({"direction": "decrease", "ids": [t.as_str()]}));
    assert_eq!(out["changed"], false);
    assert_eq!(out["at_limit"], json!([t.as_str()]));
    assert_eq!(s.tab().doc.undo_depth(), depth);
}

#[test]
fn step_size_on_a_shape_steps_its_label_and_math_changes_its_size() {
    let (mut s, _f) = testing::session_typeset();
    let r = rect(&mut s);
    run(&mut s, "text.set", json!({"id": r.as_str(), "text": "Go"}));
    let l = label_of(&s, &r).unwrap();
    let m = testing::add(&mut s, ElementKind::Math(MathData::new("x^2")), 300.0, 0.0, 10.0, 10.0).unwrap();
    let before_h = s.element(&m).unwrap().transform.h;
    let out = run(&mut s, "text.step_size", json!({"direction": "increase", "ids": [r.as_str(), m.as_str()]}));
    assert_eq!(font_size(&s, &l), 28.0);
    assert!(matches!(&s.element(&m).unwrap().kind, ElementKind::Math(d) if d.font_size == 28.0));
    assert!(s.element(&m).unwrap().transform.h > before_h);
    assert_eq!(out["resized"].as_array().unwrap().len(), 2);
}

#[test]
fn step_size_refusals() {
    let (mut s, _f) = testing::session();
    let r = refuse(&mut s, "text.step_size", json!({"direction": "increase"}));
    assert_eq!(r.code, "no_target");
    assert_eq!(s.availability("text.step_size").unwrap_err().code, "no_target");
    let rc = rect(&mut s);
    let r = refuse(&mut s, "text.step_size", json!({"direction": "increase", "ids": [rc.as_str()]}));
    assert_eq!(r.code, "not_applicable");
    assert!(r.message.contains("text and math elements"), "{}", r.message);
}

// ---- text.bind, unbind, wrap_in_container, set_autoresize ----

#[test]
fn bind_makes_a_label_unbind_frees_it() {
    let (mut s, _f) = testing::session();
    let r = rect(&mut s);
    let t = text_at(&mut s, "label", 500.0, 500.0);
    let out = run(&mut s, "text.bind", json!({"text": t.as_str(), "container": r.as_str()}));
    assert_eq!(out["container"], r.as_str());
    assert_eq!(label_of(&s, &r), Some(t.clone()));
    assert_eq!(s.tab().doc.undo_depth(), 1);
    let c = s.element(&r).unwrap().rect().center();
    let tc = s.element(&t).unwrap().rect().center();
    assert!((c - tc).hypot() < 1.0, "the label is centred");
    run(&mut s, "text.unbind", json!({"text": t.as_str()}));
    assert!(label_of(&s, &r).is_none());
    assert!(s.element(&t).is_some());
    s.undo_steps(1).unwrap();
    assert_eq!(label_of(&s, &r), Some(t));
}

#[test]
fn bind_and_unbind_refusals() {
    let (mut s, _f) = testing::session();
    let r = rect(&mut s);
    let t1 = text_at(&mut s, "one", 0.0, 0.0);
    let t2 = text_at(&mut s, "two", 0.0, 50.0);
    run(&mut s, "text.bind", json!({"text": t1.as_str(), "container": r.as_str()}));
    let e = refuse(&mut s, "text.bind", json!({"text": t2.as_str(), "container": r.as_str()}));
    assert_eq!(e.code, "already_bound");
    assert_eq!(e.message, format!("`{r}` already has a label. Remove it first with `text.unbind`."));
    let e = refuse(&mut s, "text.unbind", json!({"text": t2.as_str()}));
    assert_eq!((e.code, e.message.as_str()), ("nothing_to_do", format!("`{t2}` is not a label of any element.").as_str()));
    let e = refuse(&mut s, "text.bind", json!({"text": r.as_str(), "container": r.as_str()}));
    assert_eq!(e.code, "wrong_kind");
    let line = testing::add(&mut s, ElementKind::Ellipse, 900.0, 900.0, 50.0, 50.0).unwrap();
    let e = refuse(&mut s, "text.bind", json!({"text": t1.as_str(), "container": line.as_str()}));
    assert_eq!(e.code, "already_bound");
    assert!(e.message.contains("already the label of"), "{}", e.message);
}

#[test]
fn wrap_in_container_boxes_each_free_text_and_selects_the_shapes() {
    let (mut s, _f) = testing::session();
    let t1 = text_at(&mut s, "alpha", 0.0, 0.0);
    let t2 = text_at(&mut s, "beta", 0.0, 200.0);
    testing::select(&mut s, &[t1.clone(), t2.clone()]);
    let out = run(&mut s, "text.wrap_in_container", json!({"shape": "ellipse"}));
    let cs = out["containers"].as_array().unwrap();
    assert_eq!(cs.len(), 2);
    for (c, t) in cs.iter().zip([&t1, &t2]) {
        let cid = ElementId::new(c.as_str().unwrap()).unwrap();
        assert_eq!(s.element(&cid).unwrap().tag(), drafftink_scene::Kind::Ellipse);
        assert_eq!(label_of(&s, &cid), Some(t.clone()));
    }
    assert_eq!(s.selection().len(), 2);
    assert!(s.selection().iter().all(|i| cs.iter().any(|c| c == i.as_str())));
    assert_eq!(s.tab().doc.undo_depth(), 1);
    // The texts are labels now: a second wrap has nothing to do.
    let e = refuse(&mut s, "text.wrap_in_container", json!({"ids": [t1.as_str()]}));
    assert_eq!(e.code, "not_applicable");
    let e = refuse(&mut s, "text.wrap_in_container", json!({"ids": [t2.as_str()], "padding": -1}));
    assert_eq!(e.code, "bad_args");
}

#[test]
fn set_autoresize_toggles_between_following_the_content_and_wrapping() {
    let (mut s, _f) = testing::session();
    let t = text_at(&mut s, "aaaa bbbb cccc", 0.0, 0.0);
    run(&mut s, "text.set", json!({"id": t.as_str(), "text": "aaaa bbbb cccc"}));
    let wide = s.element(&t).unwrap().transform.w;
    run(&mut s, "text.set_autoresize", json!({"ids": [t.as_str()], "on": false}));
    run(&mut s, "text.set", json!({"id": t.as_str(), "text": "aaaa bbbb cccc dddd"}));
    assert_eq!(s.element(&t).unwrap().transform.w, wide, "the width is kept");
    assert!(s.element(&t).unwrap().transform.h > 25.0, "the text wraps");
    run(&mut s, "text.set_autoresize", json!({"ids": [t.as_str()], "on": true}));
    assert!(s.element(&t).unwrap().transform.w > wide);
    let r = rect(&mut s);
    let e = refuse(&mut s, "text.set_autoresize", json!({"ids": [r.as_str()], "on": true}));
    assert_eq!(e.code, "not_applicable");
}

#[test]
fn text_commands_refuse_locked_elements() {
    let (mut s, _f) = testing::session();
    let t = text_at(&mut s, "hi", 0.0, 0.0);
    s.tab_mut().doc.edit::<(), drafftink_scene::StoreError>(|tx| tx.update(&t, |e| e.locked = true)).unwrap();
    s.tab_mut().doc.commit_untracked().unwrap();
    for (id, args) in [
        ("text.set", json!({"id": t.as_str(), "text": "x"})),
        ("text.style_range", json!({"id": t.as_str(), "start": 0, "end": 1, "color": "#ff0000"})),
        ("text.step_size", json!({"direction": "increase", "ids": [t.as_str()]})),
        ("text.set_autoresize", json!({"ids": [t.as_str()], "on": false})),
        ("text.wrap_in_container", json!({"ids": [t.as_str()]})),
    ] {
        let e = refuse(&mut s, id, args);
        assert_eq!(e.code, "locked", "{id}: {}", e.message);
        assert!(e.message.contains("`element.unlock_all`"));
    }
}

// ---- the text edit session ----

#[test]
fn a_draft_is_created_by_edit_end_in_one_undo_step() {
    let (mut s, _f) = testing::session();
    let out = run(&mut s, "text.edit_begin", json!({"at": [100, 50]}));
    assert_eq!((out["draft"].clone(), out["kind"].clone()), (json!(true), json!("text")));
    assert_eq!(s.tab().doc.undo_depth(), 0);
    assert!(s.store().iter().next().is_none(), "nothing exists in the document yet");
    assert!(s.text_edit.is_some());
    let out = run(&mut s, "text.edit_end", json!({"text": "Hello"}));
    let id = ElementId::new(out["id"].as_str().unwrap()).unwrap();
    assert_eq!(content(&s, &id), "Hello");
    let e = s.element(&id).unwrap();
    assert_eq!((e.transform.x, e.transform.y, e.transform.w), (100.0, 50.0, 50.0));
    assert!(s.text_edit.is_none());
    assert_eq!(s.selection().iter().collect::<Vec<_>>(), vec![&id]);
    assert_eq!(s.tab().doc.undo_depth(), 1, "creation and content are one step");
    s.undo_steps(1).unwrap();
    assert!(s.element(&id).is_none());
}

#[test]
fn a_draft_with_blank_text_or_commit_false_leaves_no_trace() {
    let (mut s, _f) = testing::session();
    run(&mut s, "text.edit_begin", json!({"at": [0, 0]}));
    let out = run(&mut s, "text.edit_end", json!({"text": "   "}));
    assert_eq!(out["discarded"], true);
    run(&mut s, "text.edit_begin", json!({"at": [0, 0]}));
    let out = run(&mut s, "text.edit_end", json!({"commit": false, "text": "kept?"}));
    assert_eq!(out["discarded"], true);
    run(&mut s, "text.edit_begin", json!({"at": [0, 0]}));
    run(&mut s, "text.edit_end", json!({}));
    assert!(s.store().iter().next().is_none());
    assert_eq!(s.tab().doc.undo_depth(), 0);
    assert!(s.text_edit.is_none());
}

#[test]
fn editing_an_existing_text_replaces_it_and_a_blank_result_deletes_it() {
    let (mut s, _f) = testing::session();
    let t = text_at(&mut s, "old", 10.0, 10.0);
    let out = run(&mut s, "text.edit_begin", json!({"id": t.as_str(), "caret": "all"}));
    assert_eq!((out["text"].clone(), out["caret"].clone()), (json!("old"), json!([0, 3])));
    assert_eq!(s.selection().len(), 1);
    run(&mut s, "text.edit_end", json!({"text": "brand new", "runs": [{"start": 0, "end": 5, "color": "#ff0000"}]}));
    assert_eq!(content(&s, &t), "brand new");
    assert_eq!(s.element(&t).unwrap().text_data().unwrap().runs.len(), 1);
    assert_eq!(s.tab().doc.undo_depth(), 1);
    run(&mut s, "text.edit_begin", json!({"id": t.as_str()}));
    let out = run(&mut s, "text.edit_end", json!({"text": ""}));
    assert_eq!(out["deleted"], true);
    assert!(s.element(&t).is_none());
    assert!(s.selection().is_empty());
    s.undo_steps(1).unwrap();
    assert_eq!(content(&s, &t), "brand new");
}

#[test]
fn edit_end_without_text_keeps_the_content() {
    let (mut s, _f) = testing::session();
    let t = text_at(&mut s, "keep", 0.0, 0.0);
    run(&mut s, "text.edit_begin", json!({"id": t.as_str()}));
    let out = run(&mut s, "text.edit_end", json!({}));
    assert_eq!(out["changed"], false);
    assert_eq!(content(&s, &t), "keep");
    assert_eq!(s.tab().doc.undo_depth(), 0);
}

#[test]
fn edit_end_applies_style_props_in_the_same_step() {
    let (mut s, _f) = testing::session();
    let t = text_at(&mut s, "abc", 0.0, 0.0);
    run(&mut s, "text.edit_begin", json!({"id": t.as_str()}));
    run(&mut s, "text.edit_end", json!({"text": "abcd", "style": {"font_size": 40, "font_family": "gelpen_serif", "text_align": "center", "stroke_color": "#112233"}}));
    let e = s.element(&t).unwrap();
    let td = e.text_data().unwrap();
    assert_eq!((td.font.size, td.font.family), (40.0, drafftink_scene::FontFamily::GelPenSerif));
    assert_eq!(e.style.stroke_color, Color::parse("#112233").unwrap());
    assert_eq!(e.transform.w, 4.0 * 20.0);
    assert_eq!(s.tab().doc.undo_depth(), 1);
    run(&mut s, "text.edit_begin", json!({"at": [0, 300]}));
    let out = run(&mut s, "text.edit_end", json!({"text": "new", "style": {"font_size": 36}}));
    let id = ElementId::new(out["id"].as_str().unwrap()).unwrap();
    assert_eq!(font_size(&s, &id), 36.0);
}

/// Makes 36 the font size of new text, as the style panel does.
fn default_font_size_36(s: &mut Session) {
    run(s, "style.set", json!({"props": {"font_size": 36}, "scope": "defaults"}));
}

#[test]
fn new_text_from_edit_end_uses_the_text_defaults_of_the_tool() {
    let (mut s, _f) = testing::session();
    default_font_size_36(&mut s);
    run(&mut s, "text.edit_begin", json!({"at": [0, 0]}));
    let out = run(&mut s, "text.edit_end", json!({"text": "abc"}));
    let id = ElementId::new(out["id"].as_str().unwrap()).unwrap();
    assert_eq!(font_size(&s, &id), 36.0);
    // A style given at the end wins over the defaults.
    run(&mut s, "text.edit_begin", json!({"at": [0, 200]}));
    let out = run(&mut s, "text.edit_end", json!({"text": "abc", "style": {"font_size": 20}}));
    let id = ElementId::new(out["id"].as_str().unwrap()).unwrap();
    assert_eq!(font_size(&s, &id), 20.0);
}

#[test]
fn a_new_label_uses_the_text_defaults_of_the_tool() {
    let (mut s, _f) = testing::session();
    default_font_size_36(&mut s);
    let r = rect(&mut s);
    run(&mut s, "text.edit_begin", json!({"id": r.as_str()}));
    run(&mut s, "text.edit_end", json!({"text": "Decision"}));
    assert_eq!(font_size(&s, &label_of(&s, &r).unwrap()), 36.0);
    // `text.set` on a shape creates its label the same way.
    let r2 = testing::add_rect(&mut s, 400.0, 0.0, 200.0, 100.0).unwrap();
    run(&mut s, "text.set", json!({"id": r2.as_str(), "text": "Other"}));
    assert_eq!(font_size(&s, &label_of(&s, &r2).unwrap()), 36.0);
}

#[test]
fn edit_end_style_takes_the_properties_of_style_set_and_refuses_what_text_cannot_use() {
    let (mut s, _f) = testing::session();
    let t = text_at(&mut s, "abc", 0.0, 0.0);
    run(&mut s, "text.edit_begin", json!({"id": t.as_str()}));
    // A palette name works, as in `style.set`.
    run(&mut s, "text.edit_end", json!({"style": {"stroke_color": "red-500"}}));
    assert_ne!(s.element(&t).unwrap().style.stroke_color, Color::parse("#000000").unwrap());
    for (style, code) in [
        (json!({"fill_color": "#ff0000"}), "not_applicable"),
        (json!({"line_height": 5}), "bad_args"),
        (json!({"opacity": 2}), "bad_args"),
    ] {
        run(&mut s, "text.edit_begin", json!({"id": t.as_str()}));
        let e = refuse_keeping_edit(&mut s, "text.edit_end", json!({"style": style.clone()}));
        assert_eq!(e.code, code, "{style}: {}", e.message);
        s.text_edit = None;
    }
}

#[test]
fn editing_a_shape_edits_its_label_and_creates_it_on_commit() {
    let (mut s, _f) = testing::session();
    let r = rect(&mut s);
    let out = run(&mut s, "text.edit_begin", json!({"id": r.as_str()}));
    assert_eq!(out["text"], "");
    assert!(label_of(&s, &r).is_none(), "the label is created on commit");
    let out = run(&mut s, "text.edit_end", json!({"text": "Decision"}));
    let l = label_of(&s, &r).unwrap();
    assert_eq!(out["id"], l.as_str());
    assert_eq!(out["selected"], r.as_str());
    assert_eq!(s.tab().doc.undo_depth(), 1);
    // Editing again starts from the label text, and blank removes the label.
    let out = run(&mut s, "text.edit_begin", json!({"id": r.as_str(), "caret": 3}));
    assert_eq!((out["text"].clone(), out["caret"].clone()), (json!("Decision"), json!([3, 3])));
    run(&mut s, "text.edit_end", json!({"text": ""}));
    assert!(label_of(&s, &r).is_none());
}

#[test]
fn a_draft_with_a_container_becomes_its_label() {
    let (mut s, _f) = testing::session();
    let r = rect(&mut s);
    run(&mut s, "text.edit_begin", json!({"at": [10, 10], "container": r.as_str()}));
    run(&mut s, "text.edit_end", json!({"text": "inside"}));
    assert_eq!(content(&s, &label_of(&s, &r).unwrap()), "inside");
    let e = refuse(&mut s, "text.edit_begin", json!({"at": [10, 10], "container": r.as_str()}));
    assert_eq!(e.code, "bad_args");
    assert!(e.message.contains("already has a label"), "{}", e.message);
}

#[test]
fn a_math_edit_ends_with_latex() {
    let (mut s, _f) = testing::session_typeset();
    let m = testing::add(&mut s, ElementKind::Math(MathData::new("x")), 0.0, 0.0, 10.0, 10.0).unwrap();
    let out = run(&mut s, "text.edit_begin", json!({"id": m.as_str()}));
    assert_eq!((out["kind"].clone(), out["latex"].clone()), (json!("math"), json!("x")));
    // A parse error keeps the session open so the editor can fix it.
    let e = refuse(&mut s, "text.edit_end", json!({"latex": "\\nosuch"}));
    assert_eq!(e.code, "invalid_latex");
    assert!(s.text_edit.is_some());
    let e = refuse(&mut s, "text.edit_end", json!({"text": "plain"}));
    assert_eq!(e.code, "bad_args");
    run(&mut s, "text.edit_end", json!({"latex": "a^2 + b^2"}));
    assert!(matches!(&s.element(&m).unwrap().kind, ElementKind::Math(d) if d.latex == "a^2 + b^2"));
    assert_eq!(s.tab().doc.undo_depth(), 1);
}

#[test]
fn begin_without_arguments_uses_the_single_selected_element() {
    let (mut s, _f) = testing::session();
    let e = refuse(&mut s, "text.edit_begin", json!({}));
    assert_eq!(e.code, "needs_single");
    assert!(e.message.contains("text, math, shape or arrow"), "{}", e.message);
    let t = text_at(&mut s, "abc", 0.0, 0.0);
    let t2 = text_at(&mut s, "def", 0.0, 100.0);
    testing::select(&mut s, &[t.clone(), t2]);
    assert_eq!(refuse(&mut s, "text.edit_begin", json!({})).code, "needs_single");
    testing::select(&mut s, std::slice::from_ref(&t));
    let out = run(&mut s, "text.edit_begin", json!({}));
    assert_eq!(out["id"], t.as_str());
}

#[test]
fn begin_refusals() {
    let (mut s, _f) = testing::session();
    let t = text_at(&mut s, "abc", 0.0, 0.0);
    let e = refuse(&mut s, "text.edit_begin", json!({"id": t.as_str(), "at": [0, 0]}));
    assert_eq!(e.code, "bad_args");
    assert_eq!(e.message, "Give `id`, or `at`, or neither (then exactly one editable element must be selected).");
    let e = refuse(&mut s, "text.edit_begin", json!({"id": t.as_str(), "caret": 9}));
    assert_eq!(e.code, "bad_args");
    assert!(e.message.contains("0 to 3"), "{}", e.message);
    let f = testing::add(&mut s, ElementKind::Frame(drafftink_scene::FrameData { name: None, clip: true }), 0.0, 400.0, 50.0, 50.0).unwrap();
    let e = refuse(&mut s, "text.edit_begin", json!({"id": f.as_str()}));
    assert_eq!(e.code, "wrong_kind");
    assert!(e.message.contains("text, math, rectangle, diamond, ellipse and arrow"), "{}", e.message);
    assert_eq!(refuse(&mut s, "text.edit_begin", json!({"container": t.as_str()})).code, "bad_args");
    s.view_only = true;
    assert_eq!(refuse(&mut s, "text.edit_begin", json!({"id": t.as_str()})).code, "view_only");
}

#[test]
fn edit_end_is_unavailable_without_a_session() {
    let (mut s, _f) = testing::session();
    let e = s.availability("text.edit_end").unwrap_err();
    assert_eq!(e.code, "not_editing");
    assert_eq!(e.message, "No text is being edited. Start with `text.edit_begin`.");
    assert_eq!(refuse(&mut s, "text.edit_end", json!({})).message, e.message);
}

#[test]
fn commands_that_change_text_refuse_during_an_edit_and_others_commit_it() {
    let (mut s, _f) = testing::session();
    let t = text_at(&mut s, "abc", 0.0, 0.0);
    run(&mut s, "text.edit_begin", json!({"id": t.as_str()}));
    let e = refuse(&mut s, "text.set", json!({"id": t.as_str(), "text": "x"}));
    assert_eq!(e.code, "text_editing");
    // Starting another edit commits the first (without text: it keeps the content) and opens the second.
    let t2 = text_at(&mut s, "def", 0.0, 100.0);
    run(&mut s, "text.edit_begin", json!({"id": t2.as_str()}));
    assert_eq!(s.text_edit.as_ref().and_then(|e| e.element.clone()), Some(t2));
    // A draft is discarded by the gate when another edit starts.
    run(&mut s, "text.edit_begin", json!({"at": [5, 5]}));
    run(&mut s, "text.edit_begin", json!({"id": t.as_str()}));
    assert!(s.store().iter().count() == 2);
}

#[test]
fn a_draft_inside_a_frame_joins_it() {
    let (mut s, _f) = testing::session();
    let f = testing::add(&mut s, ElementKind::Frame(drafftink_scene::FrameData { name: None, clip: true }), 0.0, 0.0, 400.0, 400.0).unwrap();
    run(&mut s, "text.edit_begin", json!({"at": [50, 50]}));
    let out = run(&mut s, "text.edit_end", json!({"text": "in frame"}));
    let id = ElementId::new(out["id"].as_str().unwrap()).unwrap();
    assert_eq!(s.element(&id).unwrap().frame_id, Some(f));
}
