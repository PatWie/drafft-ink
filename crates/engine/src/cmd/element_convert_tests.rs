//! Tests of `element.convert`: availability, exact refusal texts, the effect on shapes, labels, bindings, lines and
//! arrows, the Tab cycle, undo and redo, and negative cases that leave the fingerprint unchanged. No UI, no DOM: a
//! headless session over the fake ports.

use drafftink_edit::binding::stale_arrows;
use drafftink_scene::{Element, ElementId, ElementKind, Head, Kind, PathType, SceneRead};
use serde_json::{Value, json};

use crate::refusal::Refusal;
use crate::ring::Origin;
use crate::session::Session;
use crate::testing;

const ID: &str = "element.convert";

fn exec(s: &mut Session, id: &str, args: Value) -> Value {
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => v,
        Err(r) => panic!("{id} refused: {} ({})", r.message, r.code),
    }
}

fn run(s: &mut Session, args: Value) -> Value {
    exec(s, ID, args)
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

fn tag(s: &Session, id: &str) -> Kind {
    el(s, id).tag()
}

fn create(s: &mut Session, args: Value) -> String {
    exec(s, "element.create", args)["id"].as_str().expect("an id").to_owned()
}

fn rect(s: &mut Session, x: f64, y: f64, w: f64, h: f64) -> String {
    create(s, json!({"kind": "rectangle", "bounds": {"x": x, "y": y, "w": w, "h": h}, "select": false}))
}

fn arrow(s: &mut Session, points: Value, arrow_type: &str) -> String {
    create(s, json!({"kind": "arrow", "points": points, "arrow_type": arrow_type, "bind": "off", "select": false}))
}

fn line(s: &mut Session, points: Value) -> String {
    create(s, json!({"kind": "line", "points": points, "bind": "off", "select": false}))
}

fn linear(s: &Session, id: &str) -> drafftink_scene::LinearData {
    el(s, id).linear_data().expect("a line or arrow").clone()
}

fn select(s: &mut Session, ids: &[&str]) {
    exec(s, "selection.select", json!({"ids": ids}));
}

// ---- availability ---------------------------------------------------------------------------------------------

#[test]
fn it_refuses_in_view_only_mode_and_without_a_target() {
    let mut s = testing::headless_session();
    let r = refuse(&mut s, json!({}));
    assert_eq!(r.code, "no_target");
    s.view_only = true;
    let a = s.availability(ID).unwrap_err();
    assert_eq!(a.code, "view_only");
    let e = refuse(&mut s, json!({}));
    assert_eq!((e.code, e.message), (a.code, a.message));
}

// ---- shapes ---------------------------------------------------------------------------------------------------

#[test]
fn a_rectangle_becomes_a_diamond_in_place() {
    let mut s = testing::headless_session();
    let a = rect(&mut s, 10.0, 20.0, 100.0, 60.0);
    let b = rect(&mut s, 300.0, 0.0, 50.0, 50.0);
    exec(&mut s, "style.set", json!({"ids": [a.clone()], "props": {"stroke_width": 4, "fill_color": "#ff0000"}}));
    let before = el(&s, &a);
    select(&mut s, &[&a]);
    let out = run(&mut s, json!({"to": "diamond"}));
    assert_eq!(out["to"], "diamond");
    assert_eq!(out["converted"], json!([a]));
    let after = el(&s, &a);
    assert_eq!(after.tag(), Kind::Diamond);
    assert_eq!((after.transform, after.style.clone(), after.index.clone()), (before.transform, before.style, before.index));
    assert_eq!(tag(&s, &b), Kind::Rectangle, "other elements are untouched");
    assert_eq!(s.selection().iter().map(ElementId::as_str).collect::<Vec<_>>(), vec![a.as_str()], "the selection is kept");
    assert_eq!(out["undo_label"], "Convert element type");
}

#[test]
fn without_to_the_next_shape_is_taken_and_reverse_goes_back() {
    let mut s = testing::headless_session();
    let a = rect(&mut s, 0.0, 0.0, 100.0, 60.0);
    select(&mut s, &[&a]);
    let seen: Vec<Kind> = (0..4)
        .map(|_| {
            run(&mut s, json!({}));
            tag(&s, &a)
        })
        .collect();
    assert_eq!(seen, [Kind::Diamond, Kind::Ellipse, Kind::Rectangle, Kind::Diamond]);
    run(&mut s, json!({"reverse": true}));
    assert_eq!(tag(&s, &a), Kind::Rectangle);
}

#[test]
fn several_shapes_convert_together_and_those_already_there_are_listed() {
    let mut s = testing::headless_session();
    let a = rect(&mut s, 0.0, 0.0, 100.0, 60.0);
    let b = rect(&mut s, 200.0, 0.0, 100.0, 60.0);
    exec(&mut s, "element.convert", json!({"ids": [b.clone()], "to": "ellipse"}));
    select(&mut s, &[&a, &b]);
    let out = run(&mut s, json!({"to": "ellipse"}));
    assert_eq!((out["converted"].clone(), out["unchanged"].clone()), (json!([a.clone()]), json!([b.clone()])));
    assert_eq!((tag(&s, &a), tag(&s, &b)), (Kind::Ellipse, Kind::Ellipse));
}

#[test]
fn the_label_stays_bound_and_the_shape_grows_when_the_new_inner_box_is_smaller() {
    let mut s = testing::headless_session();
    let a = rect(&mut s, 0.0, 0.0, 200.0, 60.0);
    let label = testing::add_text_label(&mut s, &eid(&a), "a label of some length").unwrap();
    let w0 = el(&s, &a).transform;
    let out = run(&mut s, json!({"ids": [a.clone()], "to": "diamond"}));
    assert_eq!(out["converted"], json!([a.clone()]));
    let d = el(&s, &a);
    let t = el(&s, label.as_str());
    let ElementKind::Text(td) = &t.kind else { panic!("the label is text") };
    assert_eq!(td.container_id.as_ref().map(ElementId::as_str), Some(a.as_str()), "still bound to the same id");
    assert_eq!(td.content, "a label of some length");
    assert!(d.transform.w > w0.w || d.transform.h > w0.h, "a diamond holds less text, so it grew: {:?} -> {:?}", w0, d.transform);
    let (cx, cy) = (d.transform.x + d.transform.w / 2.0, d.transform.y + d.transform.h / 2.0);
    assert!((t.transform.x + t.transform.w / 2.0 - cx).abs() < 0.5 && (t.transform.y + t.transform.h / 2.0 - cy).abs() < 0.5);
    assert!(t.transform.w <= d.transform.w / 2.0 && t.transform.h <= d.transform.h / 2.0, "the text lies inside the diamond's inner box");
    assert!(s.store().iter().all(|e| !e.deleted || e.id != label), "the label was not deleted");
}

#[test]
fn arrows_bound_to_the_shape_are_put_back_on_the_new_outline() {
    let mut s = testing::headless_session();
    let a = rect(&mut s, 0.0, 0.0, 100.0, 100.0);
    let b = rect(&mut s, 400.0, 0.0, 100.0, 100.0);
    let arrow = create(
        &mut s,
        json!({"kind": "arrow", "points": [[110, 50], [250, 50]], "select": false,
               "start": {"target": a.clone()}, "end": {"target": b.clone(), "mode": "orbit", "fixed_point": [0.02, 0.02]}}),
    );
    let before = drafftink_edit::linear::world_points(&el(&s, &arrow)).unwrap();
    let out = run(&mut s, json!({"ids": [b.clone()], "to": "ellipse"}));
    assert_eq!(out["followers"], json!([arrow.clone()]));
    let after = drafftink_edit::linear::world_points(&el(&s, &arrow)).unwrap();
    assert_ne!(before.last(), after.last(), "the end moved off the old corner");
    let end = after.last().copied().unwrap();
    let (cx, cy) = (450.0, 50.0);
    let d = ((end.x - cx) / 50.0).powi(2) + ((end.y - cy) / 50.0).powi(2);
    assert!((0.9..1.6).contains(&d), "the end sits on (or just outside, by the gap) the ellipse outline: {d}");
    assert!(el(&s, &arrow).linear_data().unwrap().end_binding.is_some(), "the binding is kept");
    assert!(stale_arrows(s.store()).is_empty(), "no arrow is left unsolved");
}

#[test]
fn groups_frame_and_z_order_are_kept() {
    let mut s = testing::headless_session();
    let f = create(&mut s, json!({"kind": "frame", "bounds": {"x": -50, "y": -50, "w": 500, "h": 500}, "select": false}));
    let a = rect(&mut s, 0.0, 0.0, 100.0, 60.0);
    let b = rect(&mut s, 200.0, 0.0, 100.0, 60.0);
    exec(&mut s, "group.create", json!({"ids": [a.clone(), b.clone()]}));
    let before = (el(&s, &a).group_ids, el(&s, &a).frame_id, el(&s, &a).index);
    run(&mut s, json!({"ids": [a.clone()]}));
    let after = el(&s, &a);
    assert_eq!((after.group_ids.clone(), after.frame_id.clone(), after.index.clone()), before);
    assert_eq!(after.frame_id.as_ref().map(ElementId::as_str), Some(f.as_str()));
}

// ---- lines and arrows -----------------------------------------------------------------------------------------

#[test]
fn a_line_becomes_an_arrow_with_an_end_head_and_back_without_heads() {
    let mut s = testing::headless_session();
    let l = line(&mut s, json!([[0, 0], [100, 50]]));
    assert_eq!(linear(&s, &l).end_head, Head::None);
    let out = run(&mut s, json!({"ids": [l.clone()], "to": "arrow"}));
    assert_eq!(out["to"], "sharp arrow");
    let a = el(&s, &l);
    assert_eq!(a.tag(), Kind::Arrow);
    let d = linear(&s, &l);
    assert_eq!((d.start_head, d.end_head, d.path), (Head::None, Head::Arrow, PathType::Sharp));
    assert_eq!(d.points.len(), 2);
    exec(&mut s, "arrow.set_head", json!({"ids": [l.clone()], "end": "both", "head": "triangle"}));
    run(&mut s, json!({"ids": [l.clone()], "to": "line"}));
    let d = linear(&s, &l);
    assert_eq!((el(&s, &l).tag(), d.start_head, d.end_head), (Kind::Line, Head::None, Head::None));
}

#[test]
fn arrow_types_are_chosen_with_arrow_type() {
    let mut s = testing::headless_session();
    let l = line(&mut s, json!([[0, 0], [100, 50], [200, 0]]));
    for (ty, path) in [("round", PathType::Round), ("sharp", PathType::Sharp), ("elbow", PathType::Elbow)] {
        let out = run(&mut s, json!({"ids": [l.clone()], "to": "arrow", "arrow_type": ty}));
        assert_eq!(out["to"], format!("{ty} arrow"));
        assert_eq!(linear(&s, &l).path, path);
    }
    assert_eq!(linear(&s, &l).points.len(), 2, "an elbow keeps only its two ends");
    let out = run(&mut s, json!({"ids": [l.clone()], "to": "line"}));
    assert_eq!(out["to"], "line");
    assert_eq!(linear(&s, &l).path, PathType::Sharp, "a line is never an elbow");
}

#[test]
fn tab_walks_line_sharp_round_elbow_and_wraps() {
    let mut s = testing::headless_session();
    let l = line(&mut s, json!([[0, 0], [120, 80]]));
    select(&mut s, &[&l]);
    let seen: Vec<(Kind, PathType)> = (0..4)
        .map(|_| {
            run(&mut s, json!({}));
            (tag(&s, &l), linear(&s, &l).path)
        })
        .collect();
    assert_eq!(
        seen,
        [
            (Kind::Arrow, PathType::Sharp),
            (Kind::Arrow, PathType::Round),
            (Kind::Arrow, PathType::Elbow),
            (Kind::Line, PathType::Sharp)
        ]
    );
    run(&mut s, json!({"reverse": true}));
    assert_eq!((tag(&s, &l), linear(&s, &l).path), (Kind::Arrow, PathType::Elbow));
}

#[test]
fn a_bound_arrow_changes_its_path_type_and_keeps_its_bindings() {
    let mut s = testing::headless_session();
    let a = rect(&mut s, 0.0, 0.0, 100.0, 100.0);
    let b = rect(&mut s, 400.0, 200.0, 100.0, 100.0);
    let arrow = exec(&mut s, "element.connect", json!({"from": a, "to": b}))["id"].as_str().unwrap().to_owned();
    run(&mut s, json!({"ids": [arrow.clone()], "to": "arrow", "arrow_type": "elbow"}));
    let d = linear(&s, &arrow);
    assert_eq!(d.path, PathType::Elbow);
    assert!(d.start_binding.is_some() && d.end_binding.is_some());
    assert!(stale_arrows(s.store()).is_empty());
}

// ---- undo -----------------------------------------------------------------------------------------------------

#[test]
fn one_undo_step_restores_the_old_type_and_redo_converts_again() {
    let mut s = testing::headless_session();
    let a = rect(&mut s, 0.0, 0.0, 200.0, 60.0);
    testing::add_text_label(&mut s, &eid(&a), "some text that needs room").unwrap();
    s.tab_mut().doc.commit_untracked().unwrap();
    let before = el(&s, &a);
    let depth = s.tab().doc.undo_depth();
    run(&mut s, json!({"ids": [a.clone()], "to": "diamond"}));
    assert_eq!(s.tab().doc.undo_depth(), depth + 1);
    s.undo_steps(1).unwrap();
    let back = el(&s, &a);
    assert_eq!((back.tag(), back.transform), (Kind::Rectangle, before.transform), "type and the grown size are back");
    s.redo_steps(1).unwrap();
    assert_eq!(tag(&s, &a), Kind::Diamond);
}

// ---- refusals -------------------------------------------------------------------------------------------------

#[test]
fn kinds_without_other_types_are_refused_by_name() {
    let mut s = testing::headless_session();
    let t = create(&mut s, json!({"kind": "text", "text": "hi", "at": [0, 0], "select": false}));
    let f = create(&mut s, json!({"kind": "frame", "bounds": {"x": 500, "y": 0, "w": 100, "h": 100}, "select": false}));
    let p = create(&mut s, json!({"kind": "freehand", "points": [[0, 300], [50, 320], [90, 300]], "select": false}));
    for (id, kind) in [(t, "text"), (f, "frame"), (p, "freehand")] {
        let r = refuse(&mut s, json!({"ids": [id.clone()], "to": "diamond"}));
        assert_eq!(r.code, "wrong_kind");
        assert_eq!(
            r.message,
            format!("`element.convert` does not work on {kind} elements. It works on rectangles, diamonds, ellipses, lines and arrows.")
        );
        assert_eq!(r.details, Some(json!({"ids": [id]})));
    }
}

#[test]
fn shapes_and_lines_together_are_refused() {
    let mut s = testing::headless_session();
    let a = rect(&mut s, 0.0, 0.0, 100.0, 60.0);
    let l = line(&mut s, json!([[0, 200], [100, 250]]));
    let r = refuse(&mut s, json!({"ids": [a, l]}));
    assert_eq!(r.code, "mixed_kinds");
    assert_eq!(
        r.message,
        "Shapes (rectangle, diamond, ellipse) and lines or arrows cannot be converted together. Select only shapes, or only lines and arrows."
    );
}

#[test]
fn a_target_from_the_other_family_is_refused() {
    let mut s = testing::headless_session();
    let a = rect(&mut s, 0.0, 0.0, 100.0, 60.0);
    let l = line(&mut s, json!([[0, 200], [100, 250]]));
    let r = refuse(&mut s, json!({"ids": [a.clone()], "to": "arrow"}));
    assert_eq!(r.code, "not_convertible");
    assert_eq!(
        r.message,
        format!("`{a}` is a rectangle and cannot become a sharp arrow. Shapes become a rectangle, diamond or ellipse; lines and arrows become a line or an arrow.")
    );
    let r = refuse(&mut s, json!({"ids": [l.clone()], "to": "ellipse"}));
    assert_eq!(r.code, "not_convertible");
    assert!(r.message.starts_with(&format!("`{l}` is a line and cannot become an ellipse. ")), "{}", r.message);
}

#[test]
fn an_arrow_with_a_binding_or_a_label_cannot_become_a_line() {
    let mut s = testing::headless_session();
    let a = rect(&mut s, 0.0, 0.0, 100.0, 100.0);
    let b = rect(&mut s, 400.0, 0.0, 100.0, 100.0);
    let bound = exec(&mut s, "element.connect", json!({"from": a, "to": b}))["id"].as_str().unwrap().to_owned();
    let r = refuse(&mut s, json!({"ids": [bound.clone()], "to": "line"}));
    assert_eq!(r.code, "not_convertible");
    assert_eq!(
        r.message,
        format!("The arrow `{bound}` is attached to a shape, and a line cannot be attached. Detach it first with `arrow.unbind`, or convert it to another arrow type.")
    );
    assert_eq!(r.suggest.first().map(|x| x.command.as_str()), Some("arrow.unbind"));
    let labelled = arrow(&mut s, json!([[0, 300], [200, 300]]), "sharp");
    exec(&mut s, "element.relabel", json!({"id": labelled.clone(), "text": "yes"}));
    let r = refuse(&mut s, json!({"ids": [labelled.clone()], "to": "line"}));
    assert_eq!(
        r.message,
        format!("The arrow `{labelled}` has a label, and a line cannot hold one. Remove the label first with `element.relabel` (empty text), or convert it to another arrow type.")
    );
    // The same arrow may still change its path type, label and all.
    run(&mut s, json!({"ids": [labelled.clone()], "to": "arrow", "arrow_type": "round"}));
    assert_eq!(linear(&s, &labelled).path, PathType::Round);
}

#[test]
fn a_closed_line_cannot_become_an_arrow() {
    let mut s = testing::headless_session();
    let l = create(
        &mut s,
        json!({"kind": "line", "points": [[0, 0], [100, 0], [50, 80]], "closed": true, "select": false}),
    );
    let r = refuse(&mut s, json!({"ids": [l.clone()], "to": "arrow"}));
    assert_eq!(r.code, "not_convertible");
    assert_eq!(r.message, format!("The line `{l}` is closed, so it is an outline and not a path. Only open lines become arrows."));
}

#[test]
fn converting_to_the_type_it_has_is_nothing_to_do() {
    let mut s = testing::headless_session();
    let a = rect(&mut s, 0.0, 0.0, 100.0, 60.0);
    let r = refuse(&mut s, json!({"ids": [a], "to": "rectangle"}));
    assert_eq!(r.code, "nothing_to_do");
    assert_eq!(r.message, "Nothing to convert: every element is already a rectangle.");
    let ar = arrow(&mut s, json!([[0, 0], [100, 100]]), "elbow");
    let r = refuse(&mut s, json!({"ids": [ar], "to": "arrow", "arrow_type": "elbow"}));
    assert_eq!(r.message, "Nothing to convert: every element is already an elbow arrow.");
}

#[test]
fn locked_and_missing_elements_are_refused_and_nothing_changes() {
    let mut s = testing::headless_session();
    let a = rect(&mut s, 0.0, 0.0, 100.0, 60.0);
    let b = rect(&mut s, 200.0, 0.0, 100.0, 60.0);
    exec(&mut s, "element.lock", json!({"ids": [b.clone()], "locked": true}));
    let r = refuse(&mut s, json!({"ids": [a.clone(), b.clone()], "to": "ellipse"}));
    assert_eq!(r.code, "locked");
    assert_eq!(r.details, Some(json!({"ids": [b]})));
    assert_eq!(tag(&s, &a), Kind::Rectangle, "the unlocked one was not converted either");
    assert_eq!(refuse(&mut s, json!({"ids": ["nope"], "to": "ellipse"})).code, "not_found");
}

#[test]
fn argument_combinations_that_make_no_sense_are_refused() {
    let mut s = testing::headless_session();
    let a = rect(&mut s, 0.0, 0.0, 100.0, 60.0);
    let r = refuse(&mut s, json!({"ids": [a.clone()], "to": "diamond", "arrow_type": "round"}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("`arrow_type`") && r.message.contains("`arrow`"), "{}", r.message);
    let r = refuse(&mut s, json!({"ids": [a.clone()], "to": "diamond", "reverse": true}));
    assert!(r.message.contains("`reverse`"), "{}", r.message);
    assert_eq!(refuse(&mut s, json!({"ids": [a], "to": "triangle"})).code, "bad_args");
}

#[test]
fn a_failed_conversion_of_a_later_element_rolls_back_the_earlier_ones() {
    let mut s = testing::headless_session();
    let a = rect(&mut s, 0.0, 0.0, 100.0, 60.0);
    let t = create(&mut s, json!({"kind": "text", "text": "hi", "at": [0, 300], "select": false}));
    refuse(&mut s, json!({"ids": [a.clone(), t], "to": "ellipse"}));
    assert_eq!(tag(&s, &a), Kind::Rectangle);
}
