//! Tests of `selection.select_lasso`: availability, exact refusal text, the effect on the selection (modes,
//! contain and overlap, groups, locked elements, frames, bound text), no undo step, and refusals that change nothing.

use std::collections::BTreeSet;

use drafftink_scene::{Color, Element, ElementId, ElementKind, FrameData, GroupId, SceneTx, StoreError};
use serde_json::{Value, json};

use crate::refusal::Refusal;
use crate::ring::Origin;
use crate::session::Session;
use crate::spec::{Gate, Mutates, Redraw, TextGate, Undo};
use crate::testing;

fn run(s: &mut Session, id: &str, args: Value) -> Value {
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => v,
        Err(r) => panic!("{id} refused: {} ({})", r.message, r.code),
    }
}

fn refuse_clean(s: &mut Session, id: &str, args: Value) -> Refusal {
    let before = s.fingerprint();
    let r = match s.execute_from(Origin::Test, id, args) {
        Ok(v) => panic!("{id} should have refused, got {v}"),
        Err(r) => r,
    };
    assert_eq!(s.fingerprint(), before, "{id} refused with `{}` but changed state", r.code);
    r
}

fn update(s: &mut Session, id: &ElementId, f: impl FnOnce(&mut Element)) {
    s.tab_mut().doc.edit::<(), StoreError>(|tx| tx.update(id, f)).unwrap();
    s.tab_mut().doc.forget_journal();
}

fn filled(s: &mut Session, kind: ElementKind, x: f64, y: f64, w: f64, h: f64) -> ElementId {
    let id = testing::add(s, kind, x, y, w, h).unwrap();
    update(s, &id, |e| e.style.fill_color = Some(Color::rgb(200, 200, 255)));
    id
}

fn selected(s: &Session) -> BTreeSet<String> {
    s.selection().iter().map(|i| i.as_str().to_owned()).collect()
}

fn set_of(ids: &[&ElementId]) -> BTreeSet<String> {
    ids.iter().map(|i| i.as_str().to_owned()).collect()
}

struct World {
    s: Session,
    a: ElementId,
    b: ElementId,
    c: ElementId,
    g1: ElementId,
    g2: ElementId,
    l: ElementId,
    f: ElementId,
    kid: ElementId,
}

/// a (0,0,100,60), b (200,0,80,80), c (400,0,40,40), group G of g1 (0,200) and g2 (100,200) with 50 by 50 boxes,
/// a locked l (500,200), frame f (0,400,300,300) with child kid (20,420). Every shape has a fill. Nothing selected.
fn world() -> World {
    let mut s = testing::headless_session();
    let a = filled(&mut s, ElementKind::Rectangle, 0.0, 0.0, 100.0, 60.0);
    let b = filled(&mut s, ElementKind::Rectangle, 200.0, 0.0, 80.0, 80.0);
    let c = filled(&mut s, ElementKind::Rectangle, 400.0, 0.0, 40.0, 40.0);
    let g1 = filled(&mut s, ElementKind::Rectangle, 0.0, 200.0, 50.0, 50.0);
    let g2 = filled(&mut s, ElementKind::Rectangle, 100.0, 200.0, 50.0, 50.0);
    let l = filled(&mut s, ElementKind::Rectangle, 500.0, 200.0, 50.0, 50.0);
    let f = testing::add(&mut s, ElementKind::Frame(FrameData { name: None, clip: true }), 0.0, 400.0, 300.0, 300.0).unwrap();
    let kid = filled(&mut s, ElementKind::Rectangle, 20.0, 420.0, 50.0, 50.0);
    for g in [&g1, &g2] {
        update(&mut s, g, |x| x.group_ids.push(GroupId::new("G").unwrap()));
    }
    update(&mut s, &l, |x| x.locked = true);
    let fid = f.clone();
    update(&mut s, &kid, |x| x.frame_id = Some(fid));
    s.take_dirty();
    World { s, a, b, c, g1, g2, l, f, kid }
}

/// A box path around the rectangle `(x, y, w, h)`, 10 units out.
fn around(x: f64, y: f64, w: f64, h: f64) -> Value {
    json!([[x - 10.0, y - 10.0], [x + w + 10.0, y - 10.0], [x + w + 10.0, y + h + 10.0], [x - 10.0, y + h + 10.0]])
}

#[test]
fn it_is_available_in_view_only_mode_and_refused_during_a_gesture_by_its_facts() {
    let mut w = world();
    w.s.view_only = true;
    assert!(w.s.availability("selection.select_lasso").is_ok());
    let spec = w.s.spec("selection.select_lasso").unwrap();
    assert_eq!((spec.undo, spec.mutates), (Undo::NoUndo, Mutates::OBSERVED));
    assert_eq!((spec.while_gesture, spec.while_text_edit), (Gate::Refused, TextGate::Commit));
    assert!(!spec.palette && spec.agent, "it needs a path, so it is not a palette entry; agents can use it");
}

#[test]
fn a_lasso_selects_what_it_surrounds_and_makes_no_undo_step() {
    let mut w = world();
    let out = run(&mut w.s, "selection.select_lasso", json!({ "points": around(0.0, 0.0, 290.0, 80.0), "contain": "contain" }));
    assert_eq!(selected(&w.s), set_of(&[&w.a, &w.b]));
    assert_eq!(out["changed"], true);
    assert_eq!(out["selected"], json!([w.a.as_str(), w.b.as_str()]));
    assert_eq!(w.s.tab().doc.undo_depth(), 0);
    assert_eq!(w.s.take_dirty(), Some(Redraw::Overlay));
}

#[test]
fn a_slanted_side_leaves_out_what_a_rectangle_would_take() {
    let mut w = world();
    // The triangle holds a, and b is on the far side of its slanted edge.
    let triangle = json!([[-10, -10], [300, -10], [-10, 150]]);
    run(&mut w.s, "selection.select_lasso", json!({ "points": triangle, "contain": "contain" }));
    assert_eq!(selected(&w.s), set_of(&[&w.a]));
    run(&mut w.s, "selection.select_lasso", json!({ "points": triangle, "contain": "overlap" }));
    assert_eq!(selected(&w.s), set_of(&[&w.a, &w.b]), "overlap also takes b, whose corner the slanted edge cuts");
}

#[test]
fn overlap_takes_a_crossed_shape_and_contain_does_not() {
    let mut w = world();
    let across = json!([[50, -10], [250, -10], [250, 100], [50, 100]]);
    run(&mut w.s, "selection.select_lasso", json!({ "points": across, "contain": "overlap" }));
    assert_eq!(selected(&w.s), set_of(&[&w.a, &w.b]));
    run(&mut w.s, "selection.select_lasso", json!({ "points": across, "contain": "contain" }));
    assert!(selected(&w.s).is_empty(), "nothing lies completely inside, and a lasso over nothing clears");
}

#[test]
fn the_default_of_contain_is_the_setting_input_box_select() {
    let mut w = world();
    let across = json!([[50, -10], [250, -10], [250, 100], [50, 100]]);
    run(&mut w.s, "selection.select_lasso", json!({ "points": across }));
    assert_eq!(selected(&w.s), set_of(&[&w.a, &w.b]), "overlap by default");
    w.s.settings.set_raw("input.box_select", json!("contain"));
    run(&mut w.s, "selection.select_lasso", json!({ "points": across }));
    assert!(selected(&w.s).is_empty());
    run(&mut w.s, "selection.select_lasso", json!({ "points": across, "contain": "overlap" }));
    assert_eq!(selected(&w.s), set_of(&[&w.a, &w.b]), "an explicit argument beats the setting");
}

#[test]
fn replace_drops_the_old_selection_and_add_keeps_it() {
    let mut w = world();
    run(&mut w.s, "selection.select", json!({ "ids": [w.c.as_str()] }));
    run(&mut w.s, "selection.select_lasso", json!({ "points": around(0.0, 0.0, 100.0, 60.0), "contain": "contain" }));
    assert_eq!(selected(&w.s), set_of(&[&w.a]), "replace drops c");
    run(&mut w.s, "selection.select_lasso", json!({ "points": around(400.0, 0.0, 40.0, 40.0), "contain": "contain", "mode": "add" }));
    assert_eq!(selected(&w.s), set_of(&[&w.a, &w.c]));
    let out = run(&mut w.s, "selection.select_lasso", json!({ "points": [[3000, 3000], [3010, 3000], [3010, 3010]] }));
    assert!(selected(&w.s).is_empty(), "a lasso over nothing clears, as the marquee does");
    assert_eq!(out["changed"], true);
}

#[test]
fn a_lasso_selects_whole_groups_and_ignores_locked_elements_and_frame_children() {
    let mut w = world();
    run(&mut w.s, "selection.select_lasso", json!({ "points": around(0.0, 200.0, 50.0, 50.0), "contain": "contain" }));
    assert_eq!(selected(&w.s), set_of(&[&w.g1, &w.g2]), "one member selects the group");
    let locked = around(500.0, 200.0, 50.0, 50.0);
    run(&mut w.s, "selection.select_lasso", json!({ "points": locked, "contain": "contain" }));
    assert!(selected(&w.s).is_empty(), "locked");
    run(&mut w.s, "selection.select_lasso", json!({ "points": locked, "contain": "contain", "include_locked": true }));
    assert_eq!(selected(&w.s), set_of(&[&w.l]));
    run(&mut w.s, "selection.select_lasso", json!({ "points": around(0.0, 400.0, 300.0, 300.0), "contain": "contain" }));
    assert_eq!(selected(&w.s), set_of(&[&w.f]), "the children of a selected frame are not selected with it");
    assert!(!selected(&w.s).contains(w.kid.as_str()));
}

#[test]
fn bound_text_follows_its_container_and_is_not_picked_on_its_own() {
    let mut w = world();
    let hollow = testing::add(&mut w.s, ElementKind::Rectangle, 600.0, 0.0, 100.0, 60.0).unwrap();
    let label = testing::add_text_label(&mut w.s, &hollow, "hi").unwrap();
    // Inside the hollow container, over its label only: the label is ignored and the container is not hit.
    let inside_label = json!([[640, 25], [644, 25], [642, 29]]);
    run(&mut w.s, "selection.select_lasso", json!({ "points": inside_label, "contain": "overlap" }));
    assert!(selected(&w.s).is_empty(), "{:?}", selected(&w.s));
    run(&mut w.s, "selection.select_lasso", json!({ "points": around(600.0, 0.0, 100.0, 60.0), "contain": "contain" }));
    assert_eq!(selected(&w.s), set_of(&[&hollow]));
    assert!(!selected(&w.s).contains(label.as_str()));
}

#[test]
fn the_path_is_simplified_by_the_simplify_argument() {
    let mut w = world();
    // A wobbly path around a: with a tolerance the wobble is dropped, and the result is the same.
    let mut pts = vec![json!([-10, -10])];
    pts.extend((0..=11).map(|i| json!([-10.0 + 10.0 * f64::from(i), -10.0 + 0.2 * f64::from(i % 2)])));
    pts.extend([json!([110, 70]), json!([-10, 70])]);
    for simplify in [0.0, 1.0, 5.0] {
        run(&mut w.s, "selection.clear", json!({}));
        run(&mut w.s, "selection.select_lasso", json!({ "points": pts, "contain": "contain", "simplify": simplify }));
        assert_eq!(selected(&w.s), set_of(&[&w.a]), "simplify {simplify}");
    }
}

#[test]
fn it_refuses_a_path_that_cannot_be_a_lasso_and_changes_nothing() {
    let mut w = world();
    run(&mut w.s, "selection.select", json!({ "ids": [w.a.as_str()] }));
    let r = refuse_clean(&mut w.s, "selection.select_lasso", json!({ "points": [[0, 0], [50, 50]] }));
    assert_eq!(r.code, "bad_args");
    assert_eq!(
        r.message,
        "Invalid arguments for `selection.select_lasso`: `points`: the path has only 2 distinct point(s) after simplifying, and a lasso needs 3. Draw a loop around what you want to select, or give 3 or more points that make a closed shape. See `engine.describe` for the schema."
    );
    let r = refuse_clean(&mut w.s, "selection.select_lasso", json!({ "points": [[0, 0], [50, 50], [100, 100]] }));
    assert!(r.message.contains("the points lie on one line, so the path encloses no area"), "{}", r.message);
    let r = refuse_clean(&mut w.s, "selection.select_lasso", json!({ "points": [] }));
    assert_eq!(r.code, "bad_args");
    assert_eq!(selected(&w.s), set_of(&[&w.a]));
}

#[test]
fn it_refuses_hostile_arguments() {
    let mut w = world();
    for (args, key) in [
        (json!({ "points": [[0, 0], [1e308, 0], [0, 10]] }), "points[1][0]"),
        (json!({ "points": [[0, 0], [10, 0], [0, 1e10]] }), "points[2][1]"),
        (json!({ "points": [[0, 0], [10, 0], [0, 10]], "simplify": -1 }), "simplify"),
    ] {
        let r = refuse_clean(&mut w.s, "selection.select_lasso", args);
        assert_eq!(r.code, "bad_args");
        assert!(r.message.contains(&format!("`{key}`")), "{}", r.message);
    }
    for args in [
        json!({}),
        json!({ "points": [[0, 0], [10, 0], [0, 10]], "mode": "toggle" }),
        json!({ "points": [[0, 0], [10, 0], [0, 10]], "contain": "inside" }),
        json!({ "points": [[0, 0, 0], [10, 0], [0, 10]] }),
        json!({ "points": "no" }),
        json!({ "points": [[0, 0], [10, 0], [0, 10]], "extra": 1 }),
    ] {
        let r = refuse_clean(&mut w.s, "selection.select_lasso", args);
        assert_eq!(r.code, "bad_args");
    }
}

#[test]
fn a_path_with_too_many_points_is_refused_with_the_limit() {
    let mut w = world();
    let pts: Vec<Value> = (0..200_001).map(|i| json!([f64::from(i % 100), f64::from(i / 100 % 100)])).collect();
    let r = refuse_clean(&mut w.s, "selection.select_lasso", json!({ "points": pts }));
    assert_eq!(r.code, "limit_exceeded");
    assert!(r.message.contains("200001 points") && r.message.contains("200000"), "{}", r.message);
}
