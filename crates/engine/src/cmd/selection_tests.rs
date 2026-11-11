//! Tests of the `selection` family. Pattern per command: availability, exact refusal text, effect on the
//! session, a negative case that leaves the fingerprint alone. No UI: a headless session and the fake ports.

use std::collections::BTreeSet;
use std::sync::Arc;

use drafftink_geom::kurbo::Point;
use drafftink_scene::{
    BindMode, Binding as ArrowBinding, Color, Element, ElementId, ElementKind, FrameData, GroupId, LinearData,
    SceneRead, SceneTx, StoreError, TextData,
};
use serde_json::{Value, json};

use crate::batch::{BatchOpts, BatchStep};
use crate::ports::fake::Fakes;
use crate::refusal::Refusal;
use crate::registry::Registry;
use crate::ring::Origin;
use crate::session::Session;
use crate::spec::{CommandSpec, Facts, Gate, Mutates, NoArgs, Redraw, Scope, TextGate, Typed, Undo};
use crate::state::{LinearEditor, TextEdit};
use crate::testing;

fn run(s: &mut Session, id: &str, args: Value) -> Value {
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => v,
        Err(r) => panic!("{id} refused: {} ({})", r.message, r.code),
    }
}

fn refuse(s: &mut Session, id: &str, args: Value) -> Refusal {
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => panic!("{id} should have refused, got {v}"),
        Err(r) => r,
    }
}

/// The command refuses and leaves every state class as it was.
fn refuse_clean(s: &mut Session, id: &str, args: Value) -> Refusal {
    let before = s.fingerprint();
    let r = refuse(s, id, args);
    assert_eq!(s.fingerprint(), before, "{id} refused with `{}` but changed state", r.code);
    r
}

fn gid(s: &str) -> GroupId {
    GroupId::new(s).unwrap()
}

fn update(s: &mut Session, id: &ElementId, f: impl FnOnce(&mut Element)) {
    s.tab_mut().doc.edit::<(), StoreError>(|tx| tx.update(id, f)).unwrap();
    s.tab_mut().doc.forget_journal();
}

fn fill(s: &mut Session, id: &ElementId) {
    update(s, id, |e| e.style.fill_color = Some(Color::rgb(200, 200, 255)));
}

fn filled(s: &mut Session, kind: ElementKind, x: f64, y: f64, w: f64, h: f64) -> ElementId {
    let id = testing::add(s, kind, x, y, w, h).unwrap();
    fill(s, &id);
    id
}

fn selected(s: &Session) -> BTreeSet<String> {
    s.selection().iter().map(|i| i.as_str().to_owned()).collect()
}


fn out_ids(v: &Value) -> Vec<String> {
    v["selected"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_owned()).collect()
}

/// The elements of the fixture, by role. Every shape has a fill, so a click inside hits it.
struct World {
    s: Session,
    a: ElementId,
    b: ElementId,
    c: ElementId,
    g1: ElementId,
    g2: ElementId,
    e: ElementId,
    l: ElementId,
    f: ElementId,
    kid: ElementId,
}

/// a (0,0,100,60) named `alpha`, b (200,0,80,80), c (400,0,40,40), group G of g1 (0,200) and g2 (100,200),
/// ellipse e (300,200), locked l (500,200), frame f (0,400,300,300) with child kid (20,420). Nothing selected.
fn world() -> World {
    let mut s = testing::headless_session();
    let a = filled(&mut s, ElementKind::Rectangle, 0.0, 0.0, 100.0, 60.0);
    let b = filled(&mut s, ElementKind::Rectangle, 200.0, 0.0, 80.0, 80.0);
    let c = filled(&mut s, ElementKind::Rectangle, 400.0, 0.0, 40.0, 40.0);
    let g1 = filled(&mut s, ElementKind::Rectangle, 0.0, 200.0, 50.0, 50.0);
    let g2 = filled(&mut s, ElementKind::Rectangle, 100.0, 200.0, 50.0, 50.0);
    let e = filled(&mut s, ElementKind::Ellipse, 300.0, 200.0, 60.0, 40.0);
    let l = filled(&mut s, ElementKind::Rectangle, 500.0, 200.0, 50.0, 50.0);
    let f = testing::add(&mut s, ElementKind::Frame(FrameData { name: None, clip: true }), 0.0, 400.0, 300.0, 300.0)
        .unwrap();
    let kid = filled(&mut s, ElementKind::Rectangle, 20.0, 420.0, 50.0, 50.0);
    update(&mut s, &a, |x| x.name = Some("alpha".into()));
    for g in [&g1, &g2] {
        update(&mut s, g, |x| x.group_ids.push(gid("G")));
    }
    update(&mut s, &l, |x| x.locked = true);
    let fid = f.clone();
    update(&mut s, &kid, |x| x.frame_id = Some(fid));
    s.take_dirty();
    World { s, a, b, c, g1, g2, e, l, f, kid }
}

fn strs(ids: &[&ElementId]) -> Vec<String> {
    ids.iter().map(|i| i.as_str().to_owned()).collect()
}

fn set_of(ids: &[&ElementId]) -> BTreeSet<String> {
    strs(ids).into_iter().collect()
}

// ---- facts shared by the family ----

#[test]
fn selection_commands_change_only_observed_state_and_never_make_an_undo_step() {
    let mut w = world();
    for (id, args) in [
        ("selection.select", json!({"ids": [w.a.as_str()]})),
        ("selection.select_at", json!({"point": [50, 30]})),
        ("selection.select_region", json!({"rect": {"x": -10, "y": -10, "w": 500, "h": 100}})),
        ("selection.all", json!({})),
        ("selection.clear", json!({})),
        ("selection.get", json!({})),
    ] {
        run(&mut w.s, id, args);
        assert_eq!(w.s.tab().doc.undo_depth(), 0, "{id} made an undo step");
    }
    let spec = w.s.spec("selection.select").unwrap();
    assert_eq!(spec.undo, Undo::NoUndo);
    assert_eq!(spec.mutates, Mutates::OBSERVED);
}

#[test]
fn a_changed_selection_raises_the_overlay_redraw_and_an_unchanged_one_does_not() {
    let mut w = world();
    run(&mut w.s, "selection.select", json!({"ids": [w.a.as_str()]}));
    assert_eq!(w.s.take_dirty(), Some(Redraw::Overlay));
    let out = run(&mut w.s, "selection.select", json!({"ids": [w.a.as_str()]}));
    assert_eq!(out["changed"], false);
    assert_eq!(w.s.take_dirty(), None, "nothing changed, nothing to draw");
}

#[test]
fn selection_commands_run_in_view_only_mode_and_refuse_during_a_gesture_by_their_facts() {
    let mut w = world();
    w.s.view_only = true;
    for id in ["selection.select", "selection.select_at", "selection.select_region", "selection.clear", "selection.get"] {
        assert!(w.s.availability(id).is_ok(), "{id}");
    }
    run(&mut w.s, "selection.select", json!({"ids": [w.a.as_str()]}));
    assert_eq!(selected(&w.s), set_of(&[&w.a]));
    for id in ["selection.select", "selection.select_at", "selection.select_region", "selection.all", "selection.clear", "selection.navigate"] {
        assert_eq!(w.s.spec(id).unwrap().while_gesture, Gate::Refused, "{id}");
        assert_eq!(w.s.spec(id).unwrap().while_text_edit, TextGate::Commit, "{id}");
    }
    assert_eq!(w.s.spec("selection.get").unwrap().while_gesture, Gate::Allowed);
}

#[test]
fn a_running_text_edit_is_committed_before_a_selection_command_runs() {
    let mut w = world();
    w.s.text_edit = Some(TextEdit { element: None, draft: true });
    run(&mut w.s, "selection.select", json!({"ids": [w.b.as_str()]}));
    assert!(w.s.text_edit.is_none());
    assert_eq!(selected(&w.s), set_of(&[&w.b]));
}

// ---- selection.select ----

#[test]
fn select_replaces_adds_removes_and_toggles() {
    let mut w = world();
    let out = run(&mut w.s, "selection.select", json!({"ids": [w.a.as_str(), w.b.as_str()]}));
    assert_eq!(selected(&w.s), set_of(&[&w.a, &w.b]));
    assert_eq!(out["changed"], true);
    assert_eq!(out_ids(&out), strs(&[&w.a, &w.b]), "back to front");
    assert_eq!(out["editing_group"], Value::Null);
    run(&mut w.s, "selection.select", json!({"ids": [w.c.as_str()], "mode": "add"}));
    assert_eq!(selected(&w.s), set_of(&[&w.a, &w.b, &w.c]));
    run(&mut w.s, "selection.select", json!({"ids": [w.b.as_str()], "mode": "remove"}));
    assert_eq!(selected(&w.s), set_of(&[&w.a, &w.c]));
    run(&mut w.s, "selection.select", json!({"ids": [w.a.as_str()], "mode": "toggle"}));
    assert_eq!(selected(&w.s), set_of(&[&w.c]));
    run(&mut w.s, "selection.select", json!({"ids": [w.a.as_str()], "mode": "toggle"}));
    assert_eq!(selected(&w.s), set_of(&[&w.a, &w.c]));
    run(&mut w.s, "selection.select", json!({"ids": [w.e.as_str()]}));
    assert_eq!(selected(&w.s), set_of(&[&w.e]), "replace is the default");
}

#[test]
fn select_takes_at_names() {
    let mut w = world();
    run(&mut w.s, "selection.select", json!({"ids": ["@alpha"]}));
    assert_eq!(selected(&w.s), set_of(&[&w.a]));
}

#[test]
fn selecting_a_member_selects_its_outermost_group_unless_deep() {
    let mut w = world();
    let out = run(&mut w.s, "selection.select", json!({"ids": [w.g1.as_str()]}));
    assert_eq!(selected(&w.s), set_of(&[&w.g1, &w.g2]));
    assert_eq!(out["groups"], json!(["G"]));
    let out = run(&mut w.s, "selection.select", json!({"ids": [w.g1.as_str()], "deep": true}));
    assert_eq!(selected(&w.s), set_of(&[&w.g1]));
    assert_eq!(out["groups"], json!([]));
    assert_eq!(out["editing_group"], "G", "deep enters the innermost group");
    assert_eq!(w.s.tab().observed.editing_group, Some(gid("G")));
}

#[test]
fn select_by_group_selects_all_members() {
    let mut w = world();
    let out = run(&mut w.s, "selection.select", json!({"group": "G"}));
    assert_eq!(selected(&w.s), set_of(&[&w.g1, &w.g2]));
    assert_eq!(out["groups"], json!(["G"]));
    run(&mut w.s, "selection.select", json!({"group": "G", "mode": "toggle"}));
    assert!(selected(&w.s).is_empty());
}

#[test]
fn select_by_filter_uses_the_query_of_element_query() {
    let mut w = world();
    run(&mut w.s, "selection.select", json!({"filter": {"kind": "ellipse"}}));
    assert_eq!(selected(&w.s), set_of(&[&w.e]));
    run(&mut w.s, "selection.select", json!({"filter": {"region": {"rect": {"x": -5, "y": -5, "w": 120, "h": 80}}}}));
    assert_eq!(selected(&w.s), set_of(&[&w.a]));
    // `@name` works inside the filter, and `selected` sees the selection from before the command.
    run(&mut w.s, "selection.select", json!({"filter": {"ids": ["@alpha", w.b.as_str()]}, "mode": "add"}));
    assert_eq!(selected(&w.s), set_of(&[&w.a, &w.b]));
    run(&mut w.s, "selection.select", json!({"filter": {"selected": true, "kind": "rectangle"}, "mode": "remove"}));
    assert!(selected(&w.s).is_empty());
}

#[test]
fn locked_elements_are_skipped_with_a_warning_unless_include_locked() {
    let mut w = world();
    let out = run(&mut w.s, "selection.select", json!({"ids": [w.a.as_str(), w.l.as_str()]}));
    assert_eq!(selected(&w.s), set_of(&[&w.a]));
    assert_eq!(out["skipped_locked"], json!([w.l.as_str()]));
    let warn = out["warnings"][0].as_str().unwrap();
    assert!(warn.contains(w.l.as_str()) && warn.contains("include_locked=true"), "{warn}");
    let out = run(&mut w.s, "selection.select", json!({"ids": [w.a.as_str(), w.l.as_str()], "include_locked": true}));
    assert_eq!(selected(&w.s), set_of(&[&w.a, &w.l]));
    assert!(out.get("skipped_locked").is_none());
}

#[test]
fn selecting_only_locked_elements_keeps_the_selection() {
    let mut w = world();
    run(&mut w.s, "selection.select", json!({"ids": [w.a.as_str()]}));
    let out = run(&mut w.s, "selection.select", json!({"ids": [w.l.as_str()]}));
    assert_eq!(out["changed"], false);
    assert_eq!(selected(&w.s), set_of(&[&w.a]));
}

#[test]
fn a_selected_frame_excludes_its_children_and_a_label_selects_its_container() {
    let mut w = world();
    run(&mut w.s, "selection.select", json!({"ids": [w.f.as_str(), w.kid.as_str()]}));
    assert_eq!(selected(&w.s), set_of(&[&w.f]));
    let label = testing::add(&mut w.s, ElementKind::Text(TextData::new("hi")), 5.0, 5.0, 30.0, 10.0).unwrap();
    let a = w.a.clone();
    update(&mut w.s, &label, |e| {
        if let ElementKind::Text(t) = &mut e.kind {
            t.container_id = Some(a);
        }
    });
    run(&mut w.s, "selection.select", json!({"ids": [label.as_str()]}));
    assert_eq!(selected(&w.s), set_of(&[&w.a]));
}

#[test]
fn select_needs_exactly_one_of_ids_group_or_filter() {
    let mut w = world();
    let want = "Invalid arguments for `selection.select`: Give exactly one of `ids`, `group` or `filter`. See `engine.describe` for the schema.";
    for args in [
        json!({}),
        json!({"mode": "add"}),
        json!({"ids": [w.a.as_str()], "group": "G"}),
        json!({"ids": [w.a.as_str()], "filter": {}}),
        json!({"group": "G", "filter": {}}),
        json!({"ids": [w.a.as_str()], "group": "G", "filter": {}}),
    ] {
        let r = refuse_clean(&mut w.s, "selection.select", args.clone());
        assert_eq!(r.code, "bad_args", "{args}");
        assert_eq!(r.message, want, "{args}");
    }
}

#[test]
fn select_refuses_unknown_targets_with_not_found() {
    let mut w = world();
    let r = refuse_clean(&mut w.s, "selection.select", json!({"ids": [w.a.as_str(), "nope"]}));
    assert_eq!(r.code, "not_found");
    assert_eq!(r.message, "No element matches `nope`. Look up ids with `element.query` or `doc.outline`.");
    let r = refuse_clean(&mut w.s, "selection.select", json!({"ids": ["@ghost"]}));
    assert_eq!(r.message, "No element matches `@ghost`. Look up ids with `element.query` or `doc.outline`.");
    let r = refuse_clean(&mut w.s, "selection.select", json!({"group": "nogroup"}));
    assert_eq!(r.code, "not_found");
    assert!(r.message.starts_with("No element matches `nogroup`."), "{}", r.message);
    assert_eq!(r.details.as_ref().unwrap()["group"], "nogroup");
    // A group with fewer than two live members is no group.
    let r = refuse_clean(&mut w.s, "selection.select", json!({"filter": {"kind": "freehand"}}));
    assert_eq!(r.code, "not_found");
    assert_eq!(
        r.message,
        "No element matches the filter (kind: freehand). Loosen the filter, or look up ids with `element.query` or `doc.outline`."
    );
    assert_eq!(r.details.as_ref().unwrap()["filter"], "kind: freehand");
    let r = refuse_clean(&mut w.s, "selection.select", json!({"filter": {"text": {"contains": "nothing"}, "locked": true}}));
    assert_eq!(r.code, "not_found");
    assert!(r.message.starts_with("No element matches the filter (locked: true, text: {contains: nothing})."), "{}", r.message);
    let r = refuse_clean(&mut w.s, "selection.select", json!({"filter": {"frame": "@ghost"}}));
    assert_eq!(r.code, "not_found");
}

#[test]
fn select_refuses_bad_filters_empty_lists_and_too_many_ids() {
    let mut w = world();
    let r = refuse_clean(&mut w.s, "selection.select", json!({"ids": []}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("`ids`: must name at least one element"), "{}", r.message);
    let r = refuse_clean(&mut w.s, "selection.select", json!({"filter": {"text": {"regex": "(("}}}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("`filter`") && r.message.contains("regex"), "{}", r.message);
    let r = refuse_clean(&mut w.s, "selection.select", json!({"filter": {"colour": "red"}}));
    assert_eq!(r.code, "bad_args");
    let r = refuse_clean(&mut w.s, "selection.select", json!({"mode": "xor", "ids": [w.a.as_str()]}));
    assert_eq!(r.code, "bad_args");
    let many: Vec<String> = (0..10_001).map(|i| format!("e{i}")).collect();
    let r = refuse_clean(&mut w.s, "selection.select", json!({"ids": many}));
    assert_eq!(r.code, "limit_exceeded");
    assert_eq!(
        r.message,
        "`selection.select` was given 10001 ids, and the limit is 10000. Split the work into several calls."
    );
}

#[test]
fn select_leaves_the_point_editor_only_while_its_element_stays_selected() {
    let mut w = world();
    run(&mut w.s, "selection.select", json!({"ids": [w.a.as_str()]}));
    w.s.tab_mut().observed.linear_editor = Some(LinearEditor { element: w.a.clone(), selected_points: BTreeSet::new() });
    run(&mut w.s, "selection.select", json!({"ids": [w.a.as_str(), w.b.as_str()]}));
    assert!(w.s.tab().observed.linear_editor.is_some(), "still selected");
    run(&mut w.s, "selection.select", json!({"ids": [w.b.as_str()]}));
    assert!(w.s.tab().observed.linear_editor.is_none());
}

#[test]
fn a_selection_inside_a_failed_atomic_batch_is_rolled_back() {
    let mut w = world();
    run(&mut w.s, "selection.select", json!({"ids": [w.a.as_str()]}));
    let before = w.s.fingerprint();
    let steps = [
        BatchStep::new("selection.select", json!({"ids": [w.b.as_str()]})),
        BatchStep::new("selection.select", json!({"ids": ["nope"]})),
    ];
    let out = w.s.execute_batch(Origin::Test, &steps, &BatchOpts::default());
    assert!(out.failed.is_some() && out.rolled_back);
    assert_eq!(w.s.fingerprint(), before);
    assert_eq!(selected(&w.s), set_of(&[&w.a]));
}

// ---- selection.select_at ----

#[test]
fn a_click_on_a_shape_selects_it_and_a_click_on_empty_canvas_clears() {
    let mut w = world();
    let out = run(&mut w.s, "selection.select_at", json!({"point": [50, 30]}));
    assert_eq!(selected(&w.s), set_of(&[&w.a]));
    assert_eq!(out["changed"], true);
    run(&mut w.s, "selection.select_at", json!({"point": [240, 40]}));
    assert_eq!(selected(&w.s), set_of(&[&w.b]), "replace");
    let out = run(&mut w.s, "selection.select_at", json!({"point": [1000, 1000]}));
    assert!(selected(&w.s).is_empty());
    assert_eq!(out["changed"], true);
    let out = run(&mut w.s, "selection.select_at", json!({"point": [1000, 1000]}));
    assert_eq!(out["changed"], false, "clearing an empty selection changes nothing");
}

#[test]
fn shift_click_toggles_and_empty_canvas_with_toggle_changes_nothing() {
    let mut w = world();
    run(&mut w.s, "selection.select_at", json!({"point": [50, 30]}));
    run(&mut w.s, "selection.select_at", json!({"point": [240, 40], "mode": "toggle"}));
    assert_eq!(selected(&w.s), set_of(&[&w.a, &w.b]));
    run(&mut w.s, "selection.select_at", json!({"point": [50, 30], "mode": "toggle"}));
    assert_eq!(selected(&w.s), set_of(&[&w.b]));
    let out = run(&mut w.s, "selection.select_at", json!({"point": [1000, 1000], "mode": "toggle"}));
    assert_eq!(out["changed"], false);
    assert_eq!(selected(&w.s), set_of(&[&w.b]));
    run(&mut w.s, "selection.select_at", json!({"point": [240, 40], "mode": "remove"}));
    assert!(selected(&w.s).is_empty());
    run(&mut w.s, "selection.select_at", json!({"point": [240, 40], "mode": "add"}));
    assert_eq!(selected(&w.s), set_of(&[&w.b]));
}

#[test]
fn a_click_on_a_group_member_selects_the_group_and_a_deep_click_the_member() {
    let mut w = world();
    let out = run(&mut w.s, "selection.select_at", json!({"point": [25, 225]}));
    assert_eq!(selected(&w.s), set_of(&[&w.g1, &w.g2]));
    assert_eq!(out["groups"], json!(["G"]));
    let out = run(&mut w.s, "selection.select_at", json!({"point": [25, 225], "deep": true}));
    assert_eq!(selected(&w.s), set_of(&[&w.g1]));
    assert_eq!(out["editing_group"], "G");
    // Inside the editing group a click on another member selects just that member, and a click in the gap
    // between members (inside the group's bounds) leaves the selection empty but keeps the editing group.
    run(&mut w.s, "selection.select_at", json!({"point": [125, 225]}));
    assert_eq!(selected(&w.s), set_of(&[&w.g2]));
    assert_eq!(w.s.tab().observed.editing_group, Some(gid("G")));
    run(&mut w.s, "selection.select_at", json!({"point": [75, 225], "tolerance": 1}));
    assert!(selected(&w.s).is_empty());
    assert_eq!(w.s.tab().observed.editing_group, Some(gid("G")), "the gap lies inside the editing group");
    // Outside it the editing group is left.
    run(&mut w.s, "selection.select_at", json!({"point": [1000, 1000]}));
    assert_eq!(w.s.tab().observed.editing_group, None);
}

#[test]
fn clicking_inside_a_multi_selection_keeps_it() {
    let mut w = world();
    run(&mut w.s, "selection.select", json!({"ids": [w.a.as_str(), w.b.as_str()]}));
    // The gap between a and b lies inside their common bounds.
    let out = run(&mut w.s, "selection.select_at", json!({"point": [150, 30], "tolerance": 1}));
    assert_eq!(out["changed"], false);
    assert_eq!(selected(&w.s), set_of(&[&w.a, &w.b]));
    // A click on a selected member keeps both too (the drag that follows moves them together).
    run(&mut w.s, "selection.select_at", json!({"point": [50, 30]}));
    assert_eq!(selected(&w.s), set_of(&[&w.a, &w.b]));
    // A click on something else replaces it.
    run(&mut w.s, "selection.select_at", json!({"point": [420, 20]}));
    assert_eq!(selected(&w.s), set_of(&[&w.c]));
}

#[test]
fn alt_click_cycles_through_the_units_under_the_point() {
    let mut w = world();
    let top = filled(&mut w.s, ElementKind::Rectangle, 0.0, 0.0, 100.0, 60.0);
    let mut seen = Vec::new();
    for _ in 0..3 {
        run(&mut w.s, "selection.select_at", json!({"point": [50, 30], "cycle": true}));
        seen.push(selected(&w.s));
    }
    assert_eq!(seen, vec![set_of(&[&top]), set_of(&[&w.a]), set_of(&[&top])], "top, below, wraps");
}

#[test]
fn locked_elements_and_frame_interiors_are_not_hit() {
    let mut w = world();
    let out = run(&mut w.s, "selection.select_at", json!({"point": [525, 225]}));
    assert!(out_ids(&out).is_empty(), "locked");
    run(&mut w.s, "selection.select_at", json!({"point": [525, 225], "include_locked": true}));
    assert_eq!(selected(&w.s), set_of(&[&w.l]));
    run(&mut w.s, "selection.clear", json!({}));
    // The frame interior hits nothing; the kid inside it is hit as itself.
    run(&mut w.s, "selection.select_at", json!({"point": [200, 600]}));
    assert!(selected(&w.s).is_empty());
    run(&mut w.s, "selection.select_at", json!({"point": [45, 445]}));
    assert_eq!(selected(&w.s), set_of(&[&w.kid]));
    // The frame outline selects the frame.
    run(&mut w.s, "selection.select_at", json!({"point": [150, 400]}));
    assert_eq!(selected(&w.s), set_of(&[&w.f]));
}

#[test]
fn the_tolerance_is_in_screen_pixels() {
    let mut w = world();
    // 10 world units outside b's right edge at the base zoom of 1.68: 16.8 px.
    let p = json!([290, 40]);
    run(&mut w.s, "selection.select_at", json!({"point": p}));
    assert!(selected(&w.s).is_empty());
    run(&mut w.s, "selection.select_at", json!({"point": p, "tolerance": 20}));
    assert_eq!(selected(&w.s), set_of(&[&w.b]));
    run(&mut w.s, "selection.clear", json!({}));
    // Zoomed out to 0.5, the default 6 px reach 12 world units.
    w.s.tab_mut().view.zoom = 0.5;
    run(&mut w.s, "selection.select_at", json!({"point": p}));
    assert_eq!(selected(&w.s), set_of(&[&w.b]));
}

#[test]
fn select_at_refuses_bad_points_and_tolerances_without_changing_state() {
    let mut w = world();
    run(&mut w.s, "selection.select", json!({"ids": [w.a.as_str()]}));
    for (args, key) in [
        (json!({"point": [1e308, 0]}), "point[0]"),
        (json!({"point": [0, -1e308]}), "point[1]"),
        (json!({"point": [0, 0], "tolerance": -1}), "tolerance"),
        (json!({"point": [0, 0], "tolerance": 1e9}), "tolerance"),
    ] {
        let r = refuse_clean(&mut w.s, "selection.select_at", args.clone());
        assert_eq!(r.code, "bad_args", "{args}");
        assert!(r.message.contains(&format!("`{key}`")), "{}", r.message);
    }
    let r = refuse_clean(&mut w.s, "selection.select_at", json!({}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("point"), "{}", r.message);
    assert_eq!(selected(&w.s), set_of(&[&w.a]));
}

// ---- selection.select_region ----

#[test]
fn a_marquee_selects_what_it_touches_or_what_it_contains() {
    let mut w = world();
    // Covers the right half of a and the left half of b.
    let rect = json!({"x": 50, "y": 10, "w": 200, "h": 30});
    run(&mut w.s, "selection.select_region", json!({"rect": rect, "contain": "overlap"}));
    assert_eq!(selected(&w.s), set_of(&[&w.a, &w.b]));
    run(&mut w.s, "selection.select_region", json!({"rect": rect, "contain": "contain"}));
    assert!(selected(&w.s).is_empty(), "nothing lies completely inside");
    run(&mut w.s, "selection.select_region", json!({"rect": {"x": -10, "y": -10, "w": 130, "h": 90}, "contain": "contain"}));
    assert_eq!(selected(&w.s), set_of(&[&w.a]));
}

#[test]
fn the_default_of_contain_is_the_setting_input_box_select() {
    let mut w = world();
    let rect = json!({"x": 50, "y": 10, "w": 200, "h": 30});
    run(&mut w.s, "selection.select_region", json!({"rect": rect}));
    assert_eq!(selected(&w.s), set_of(&[&w.a, &w.b]), "overlap by default");
    w.s.settings.set_raw("input.box_select", json!("contain"));
    run(&mut w.s, "selection.select_region", json!({"rect": rect}));
    assert!(selected(&w.s).is_empty());
    run(&mut w.s, "selection.select_region", json!({"rect": rect, "contain": "overlap"}));
    assert_eq!(selected(&w.s), set_of(&[&w.a, &w.b]), "an explicit argument beats the setting");
}

#[test]
fn a_marquee_replaces_or_adds_and_a_negative_size_is_normalised() {
    let mut w = world();
    run(&mut w.s, "selection.select", json!({"ids": [w.c.as_str()]}));
    run(&mut w.s, "selection.select_region", json!({"rect": {"x": 90, "y": 70, "w": -100, "h": -80}}));
    assert_eq!(selected(&w.s), set_of(&[&w.a]), "replace drops c");
    run(&mut w.s, "selection.select_region", json!({"rect": {"x": 390, "y": -10, "w": 100, "h": 100}, "mode": "add"}));
    assert_eq!(selected(&w.s), set_of(&[&w.a, &w.c]));
    let out = run(&mut w.s, "selection.select_region", json!({"rect": {"x": 3000, "y": 3000, "w": 10, "h": 10}}));
    assert!(selected(&w.s).is_empty(), "a marquee over nothing clears, as in the old app");
    assert_eq!(out["changed"], true);
}

#[test]
fn a_marquee_selects_whole_groups_and_ignores_locked_and_frame_children() {
    let mut w = world();
    run(&mut w.s, "selection.select_region", json!({"rect": {"x": 10, "y": 210, "w": 5, "h": 5}}));
    assert_eq!(selected(&w.s), set_of(&[&w.g1, &w.g2]), "one member selects the group");
    run(&mut w.s, "selection.select_region", json!({"rect": {"x": 490, "y": 190, "w": 100, "h": 100}}));
    assert!(selected(&w.s).is_empty(), "locked");
    run(&mut w.s, "selection.select_region", json!({"rect": {"x": 490, "y": 190, "w": 100, "h": 100}, "include_locked": true}));
    assert_eq!(selected(&w.s), set_of(&[&w.l]));
    run(&mut w.s, "selection.select_region", json!({"rect": {"x": -10, "y": 390, "w": 400, "h": 400}, "contain": "contain"}));
    assert_eq!(selected(&w.s), set_of(&[&w.f]), "the children of a selected frame are not selected with it");
}

#[test]
fn select_region_refuses_bad_rectangles() {
    let mut w = world();
    run(&mut w.s, "selection.select", json!({"ids": [w.a.as_str()]}));
    for (rect, key) in [
        (json!({"x": 1e308, "y": 0, "w": 1, "h": 1}), "rect.x"),
        (json!({"x": 0, "y": 0, "w": -1e308, "h": 1}), "rect.w"),
        (json!({"x": 0, "y": 0, "w": 1, "h": 1e10}), "rect.h"),
    ] {
        let r = refuse_clean(&mut w.s, "selection.select_region", json!({"rect": rect}));
        assert_eq!(r.code, "bad_args");
        assert!(r.message.contains(&format!("`{key}`")), "{}", r.message);
    }
    let r = refuse_clean(&mut w.s, "selection.select_region", json!({"rect": {"x": 0, "y": 0, "w": 1}}));
    assert_eq!(r.code, "bad_args");
    let r = refuse_clean(&mut w.s, "selection.select_region", json!({"rect": {"x": 0, "y": 0, "w": 1, "h": 1}, "mode": "toggle"}));
    assert_eq!(r.code, "bad_args");
    assert_eq!(selected(&w.s), set_of(&[&w.a]));
}

// ---- selection.all ----

#[test]
fn select_all_is_greyed_out_on_an_empty_document_with_the_catalog_text() {
    let mut s = testing::headless_session();
    let r = s.availability("selection.all").unwrap_err();
    assert_eq!(r.code, "nothing_to_do");
    assert_eq!(r.message, "There are no elements to select.");
    let e = refuse_clean(&mut s, "selection.all", json!({}));
    assert_eq!((e.code, e.message.as_str()), (r.code, r.message.as_str()));
}

#[test]
fn select_all_selects_unlocked_elements_as_groups() {
    let mut w = world();
    let out = run(&mut w.s, "selection.all", json!({}));
    let all = set_of(&[&w.a, &w.b, &w.c, &w.g1, &w.g2, &w.e, &w.f]);
    assert_eq!(selected(&w.s), all, "the locked l is left out, the kid is left out with its frame");
    assert_eq!(out["groups"], json!(["G"]));
    let again = run(&mut w.s, "selection.all", json!({}));
    assert_eq!(again["changed"], false);
}

#[test]
fn select_all_inside_an_editing_group_selects_its_members() {
    let mut w = world();
    run(&mut w.s, "selection.select", json!({"ids": [w.g1.as_str()], "deep": true}));
    let out = run(&mut w.s, "selection.all", json!({}));
    assert_eq!(selected(&w.s), set_of(&[&w.g1, &w.g2]));
    assert_eq!(out["editing_group"], "G");
}

#[test]
fn select_all_within_a_frame_a_named_frame_or_a_group() {
    let mut w = world();
    run(&mut w.s, "selection.all", json!({"within": w.f.as_str()}));
    assert_eq!(selected(&w.s), set_of(&[&w.kid]));
    update(&mut w.s, &w.f.clone(), |e| e.name = Some("panel".into()));
    run(&mut w.s, "selection.clear", json!({}));
    run(&mut w.s, "selection.all", json!({"within": "@panel"}));
    assert_eq!(selected(&w.s), set_of(&[&w.kid]));
    run(&mut w.s, "selection.all", json!({"within": "G"}));
    assert_eq!(selected(&w.s), set_of(&[&w.g1, &w.g2]));
}

#[test]
fn select_all_within_refuses_what_is_neither_a_frame_nor_a_group() {
    let mut w = world();
    let r = refuse_clean(&mut w.s, "selection.all", json!({"within": w.a.as_str()}));
    assert_eq!(r.code, "wrong_kind");
    assert_eq!(r.message, "`selection.all` does not work on rectangle elements. It works on frames and groups.");
    let r = refuse_clean(&mut w.s, "selection.all", json!({"within": "nope"}));
    assert_eq!(r.code, "not_found");
    let r = refuse_clean(&mut w.s, "selection.all", json!({"within": "@nope"}));
    assert_eq!(r.code, "not_found");
    // A frame without children has nothing to select.
    let empty = testing::add(&mut w.s, ElementKind::Frame(FrameData { name: None, clip: true }), 900.0, 0.0, 50.0, 50.0).unwrap();
    let r = refuse_clean(&mut w.s, "selection.all", json!({"within": empty.as_str()}));
    assert_eq!((r.code, r.message.as_str()), ("nothing_to_do", "There are no elements to select."));
}

#[test]
fn select_all_when_everything_is_locked_refuses_at_execute() {
    let mut s = testing::headless_session();
    let a = testing::add_rect(&mut s, 0.0, 0.0, 10.0, 10.0).unwrap();
    update(&mut s, &a, |e| e.locked = true);
    assert!(s.availability("selection.all").is_ok());
    let r = refuse_clean(&mut s, "selection.all", json!({}));
    assert_eq!((r.code, r.message.as_str()), ("nothing_to_do", "There are no elements to select."));
}

#[test]
fn select_all_is_bound_to_mod_a_on_the_canvas_and_is_in_the_palette() {
    let s = testing::headless_session();
    let spec = s.spec("selection.all").unwrap();
    assert!(spec.palette);
    assert_eq!(spec.shortcuts.len(), 1);
    assert_eq!((spec.shortcuts[0].keys, spec.shortcuts[0].when), (&["Mod+A"][..], Scope::Canvas));
}

// ---- selection.clear ----

#[test]
fn clear_empties_the_selection_the_editing_group_and_the_point_editor() {
    let mut w = world();
    run(&mut w.s, "selection.select", json!({"ids": [w.g1.as_str()], "deep": true}));
    w.s.tab_mut().observed.linear_editor = Some(LinearEditor { element: w.g1.clone(), selected_points: BTreeSet::new() });
    let out = run(&mut w.s, "selection.clear", json!({}));
    assert_eq!(out["changed"], true);
    assert_eq!(out["selected"], json!([]));
    assert_eq!(out["editing_group"], Value::Null);
    let o = &w.s.tab().observed;
    assert!(o.selection.is_empty() && o.selected_groups.is_empty() && o.editing_group.is_none() && o.linear_editor.is_none());
}

#[test]
fn clear_is_idempotent_and_always_available() {
    let mut w = world();
    assert!(w.s.availability("selection.clear").is_ok());
    let before = w.s.fingerprint();
    let out = run(&mut w.s, "selection.clear", json!({}));
    assert_eq!(out["changed"], false);
    assert_eq!(w.s.fingerprint(), before);
    assert_eq!(w.s.take_dirty(), None);
    let r = refuse_clean(&mut w.s, "selection.clear", json!({"all": true}));
    assert_eq!(r.code, "bad_args");
    assert!(w.s.spec("selection.clear").unwrap().palette);
}

// ---- selection.get ----

#[test]
fn get_summarises_the_selection() {
    let mut w = world();
    run(&mut w.s, "selection.select", json!({"ids": [w.a.as_str(), w.e.as_str(), w.g1.as_str()]}));
    let v = run(&mut w.s, "selection.get", json!({}));
    assert_eq!(v["count"], 4);
    assert_eq!(v["ids"], json!(strs(&[&w.a, &w.g1, &w.g2, &w.e])));
    assert_eq!(v["groups"], json!(["G"]));
    assert_eq!(v["editing_group"], Value::Null);
    assert_eq!(v["kinds"], json!({"ellipse": 1, "rectangle": 3}));
    assert_eq!(v["bounds"], json!({"x": 0.0, "y": 0.0, "w": 360.0, "h": 250.0}));
    assert!(v.get("linear_editor").is_none() && v.get("elements").is_none());
    assert_eq!(v["changed"], false);
}

#[test]
fn get_ids_is_short_and_full_describes_every_element() {
    let mut w = world();
    run(&mut w.s, "selection.select", json!({"ids": [w.a.as_str()]}));
    let v = run(&mut w.s, "selection.get", json!({"detail": "ids"}));
    assert_eq!(v["ids"], json!([w.a.as_str()]));
    assert_eq!(v["count"], 1);
    assert!(v.get("groups").is_none() && v.get("bounds").is_none());
    let v = run(&mut w.s, "selection.get", json!({"detail": "full"}));
    let el = &v["elements"][0];
    assert_eq!((el["id"].as_str(), el["kind"].as_str(), el["name"].as_str()), (Some(w.a.as_str()), Some("rectangle"), Some("alpha")));
    for k in ["text", "bounds", "style", "angle", "group_ids", "frame", "bindings", "bound_text", "locked", "z"] {
        assert!(el.get(k).is_some(), "{k}");
    }
}

#[test]
fn get_on_an_empty_selection_has_null_bounds_and_reports_the_point_editor() {
    let mut w = world();
    let v = run(&mut w.s, "selection.get", json!({}));
    assert_eq!((v["count"].as_u64(), v["ids"].clone(), v["bounds"].clone(), v["kinds"].clone()), (Some(0), json!([]), Value::Null, json!({})));
    run(&mut w.s, "selection.select", json!({"ids": [w.a.as_str()]}));
    w.s.tab_mut().observed.linear_editor = Some(LinearEditor { element: w.a.clone(), selected_points: BTreeSet::from([1, 0]) });
    let v = run(&mut w.s, "selection.get", json!({}));
    assert_eq!(v["linear_editor"], json!({"element": w.a.as_str(), "selected_points": [0, 1]}));
}

#[test]
fn get_is_read_only_and_works_in_every_state() {
    let mut w = world();
    w.s.view_only = true;
    w.s.text_edit = Some(TextEdit { element: None, draft: true });
    assert!(w.s.availability("selection.get").is_ok());
    let before = w.s.fingerprint();
    run(&mut w.s, "selection.get", json!({"detail": "full"}));
    assert_eq!(w.s.fingerprint(), before);
    let r = refuse_clean(&mut w.s, "selection.get", json!({"detail": "everything"}));
    assert_eq!(r.code, "bad_args");
}

// ---- selection.navigate ----

/// a (0,0,100,60) -> b (200,0,80,80) by a bound arrow; c is not connected.
fn flow() -> World {
    let mut w = world();
    let arrow = testing::add(
        &mut w.s,
        ElementKind::Arrow(LinearData::new(vec![Point::new(0.0, 0.0), Point::new(100.0, 10.0)])),
        100.0,
        30.0,
        100.0,
        10.0,
    )
    .unwrap();
    let (a, b) = (w.a.clone(), w.b.clone());
    update(&mut w.s, &arrow, |e| {
        if let ElementKind::Arrow(d) = &mut e.kind {
            d.start_binding = Some(ArrowBinding { target: a, mode: BindMode::Orbit, fixed_point: [1.0, 0.5] });
            d.end_binding = Some(ArrowBinding { target: b, mode: BindMode::Orbit, fixed_point: [0.0, 0.5] });
        }
    });
    w
}

#[test]
fn navigate_selects_the_connected_element_in_the_direction() {
    let mut w = flow();
    run(&mut w.s, "selection.select", json!({"ids": [w.a.as_str()]}));
    let out = run(&mut w.s, "selection.navigate", json!({"direction": "right"}));
    assert_eq!(selected(&w.s), set_of(&[&w.b]));
    assert_eq!(out["changed"], true);
    assert_eq!(out_ids(&out), strs(&[&w.b]));
    run(&mut w.s, "selection.navigate", json!({"direction": "left"}));
    assert_eq!(selected(&w.s), set_of(&[&w.a]));
    assert_eq!(w.s.tab().doc.undo_depth(), 0);
}

#[test]
fn navigate_needs_exactly_one_selected_element_in_availability_and_at_execute() {
    let mut w = flow();
    let want0 = "`selection.navigate` needs exactly one element selected, and 0 are selected. Select one element first, or pass it by id.";
    let r = w.s.availability("selection.navigate").unwrap_err();
    assert_eq!((r.code, r.message.as_str()), ("needs_single", want0));
    let e = refuse_clean(&mut w.s, "selection.navigate", json!({"direction": "right"}));
    assert_eq!((e.code, e.message.as_str()), ("needs_single", want0));
    run(&mut w.s, "selection.select", json!({"ids": [w.a.as_str(), w.b.as_str()]}));
    let r = w.s.availability("selection.navigate").unwrap_err();
    assert_eq!(
        r.message,
        "`selection.navigate` needs exactly one element selected, and 2 are selected. Select one element first, or pass it by id."
    );
    assert_eq!(refuse_clean(&mut w.s, "selection.navigate", json!({"direction": "up"})).code, "needs_single");
    run(&mut w.s, "selection.select", json!({"ids": [w.a.as_str()]}));
    assert!(w.s.availability("selection.navigate").is_ok());
}

#[test]
fn navigate_without_a_connection_in_that_direction_says_so() {
    let mut w = flow();
    run(&mut w.s, "selection.select", json!({"ids": [w.a.as_str()]}));
    let r = refuse_clean(&mut w.s, "selection.navigate", json!({"direction": "up"}));
    assert_eq!(r.code, "nothing_to_do");
    assert_eq!(r.message, format!("`{}` has no connected element to the up.", w.a.as_str()));
    run(&mut w.s, "selection.select", json!({"ids": [w.c.as_str()]}));
    let r = refuse_clean(&mut w.s, "selection.navigate", json!({"direction": "right"}));
    assert_eq!(r.message, format!("`{}` has no connected element to the right.", w.c.as_str()));
}

#[test]
fn navigate_does_not_select_a_locked_target() {
    let mut w = flow();
    let b = w.b.clone();
    update(&mut w.s, &b, |e| e.locked = true);
    run(&mut w.s, "selection.select", json!({"ids": [w.a.as_str()]}));
    let r = refuse_clean(&mut w.s, "selection.navigate", json!({"direction": "right"}));
    assert_eq!(r.code, "locked");
    assert_eq!(
        r.message,
        format!("Element `{}` is locked. Unlock it first with `element.lock` (locked=false) or `element.unlock_all`.", w.b.as_str())
    );
}

#[test]
fn navigate_rejects_an_unknown_direction_and_is_bound_to_alt_arrows() {
    let mut w = flow();
    run(&mut w.s, "selection.select", json!({"ids": [w.a.as_str()]}));
    let r = refuse_clean(&mut w.s, "selection.navigate", json!({"direction": "sideways"}));
    assert_eq!(r.code, "bad_args");
    let r = refuse_clean(&mut w.s, "selection.navigate", json!({}));
    assert_eq!(r.code, "bad_args");
    let spec = w.s.spec("selection.navigate").unwrap();
    let keys: Vec<(&str, &str)> = spec.shortcuts.iter().map(|b| (b.keys[0], b.args)).collect();
    assert_eq!(
        keys,
        [
            ("Alt+Up", r#"{"direction":"up"}"#),
            ("Alt+Down", r#"{"direction":"down"}"#),
            ("Alt+Left", r#"{"direction":"left"}"#),
            ("Alt+Right", r#"{"direction":"right"}"#),
        ]
    );
    assert!(spec.shortcuts.iter().all(|b| b.when == Scope::Canvas));
}

// ---- undo: the selection round-trips through a document step ----

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, default)]
struct AddArgs {
    x: f64,
    group: Option<String>,
}

/// A document command for the test: adds a rectangle (selection untouched).
fn add(s: &mut Session, a: AddArgs) -> Result<Value, Refusal> {
    let id = s.ports.ids.element_id();
    let cmd = s.cmd();
    s.tab_mut()
        .doc
        .edit::<(), StoreError>(|tx| {
            let z = tx.next_top_index().map_err(|e| StoreError::Backend(e.to_string()))?;
            let mut e = Element::new(id, ElementKind::Rectangle, z);
            e.transform = drafftink_scene::Transform::new(a.x, 0.0, 50.0, 50.0);
            if let Some(g) = &a.group {
                e.group_ids.push(GroupId::new(g).map_err(|e| StoreError::Backend(e.to_string()))?);
            }
            tx.insert(e)
        })
        .map_err(|e| Refusal::internal(cmd, e.to_string()))?;
    Ok(json!({}))
}

/// A document command for the test: nudges the first element and selects the second.
fn touch(s: &mut Session, _: NoArgs) -> Result<Value, Refusal> {
    let ids: Vec<ElementId> = s.store().iter().map(|e| e.id.clone()).collect();
    let cmd = s.cmd();
    let (Some(first), Some(second)) = (ids.first().cloned(), ids.get(1).cloned()) else {
        return Err(Refusal::no_target(cmd));
    };
    s.tab_mut()
        .doc
        .edit::<(), StoreError>(|tx| tx.update(&first, |e| e.transform.x += 5.0))
        .map_err(|e| Refusal::internal(cmd, e.to_string()))?;
    let o = &mut s.tab_mut().observed;
    o.selection = std::iter::once(second).collect();
    o.selected_groups.clear();
    Ok(json!({}))
}

const DOC: Facts = Facts {
    undo: Undo::Undoable,
    reads_only: false,
    mutates: Mutates::DOC.with(Mutates::OBSERVED),
    view_only: Gate::Refused,
    while_gesture: Gate::Refused,
    redraw: Redraw::Scene,
    ..Facts::READ_ONLY
};

static TEST_COMMANDS: [CommandSpec; 2] = [
    CommandSpec::new("t.touch", "t", "Touch", "Moves the first element and selects the second one.", &Typed::new(touch), DOC),
    CommandSpec::new("t.add", "t", "Add", "Adds a rectangle without touching the selection.", &Typed::new(add), DOC),
];

/// A session with the selection commands and two test document commands. Three rectangles exist, made by
/// `t.add`, so they are in the replicated store and undo leaves them alone. The returned depth is the undo
/// depth after the setup.
fn session_with_touch(group: Option<&str>) -> (Session, Vec<ElementId>, usize) {
    let fakes = Fakes::new();
    let reg = Arc::new(Registry::from_families(&[super::COMMANDS, &TEST_COMMANDS]));
    let mut s = Session::with_registry(fakes.ports(), reg);
    for x in [0.0, 100.0, 200.0] {
        let g = group.filter(|_| x < 150.0);
        run(&mut s, "t.add", json!({"x": x, "group": g}));
    }
    let ids: Vec<ElementId> = s.store().iter().map(|e| e.id.clone()).collect();
    let depth = s.tab().doc.undo_depth();
    (s, ids, depth)
}

#[test]
fn a_document_step_records_the_selection_before_and_after_and_undo_restores_it() {
    let (mut s, ids, base) = session_with_touch(None);
    let (a, b, c) = (&ids[0], &ids[1], &ids[2]);
    run(&mut s, "selection.select", json!({"ids": [a.as_str(), c.as_str()]}));
    run(&mut s, "selection.select", json!({"ids": [c.as_str()]}));
    assert_eq!(s.tab().doc.undo_depth(), base, "selection changes alone make no step");
    run(&mut s, "t.touch", json!({}));
    assert_eq!(s.tab().doc.undo_depth(), base + 1);
    assert_eq!(selected(&s), set_of(&[b]));
    // More selection changes after the step do not add steps.
    run(&mut s, "selection.select", json!({"ids": [a.as_str()]}));
    assert_eq!(s.tab().doc.undo_depth(), base + 1);
    // Undo: the document and the selection from before the step (c, not a).
    s.undo_steps(1).unwrap();
    assert_eq!(selected(&s), set_of(&[c]));
    assert_eq!(s.element(a).unwrap().transform.x, 0.0);
    // Selection commands do not clear the redo stack.
    run(&mut s, "selection.select", json!({"ids": [a.as_str()]}));
    assert_eq!(s.tab().doc.redo_depth(), 1);
    // Redo: the selection from after the step.
    s.redo_steps(1).unwrap();
    assert_eq!(selected(&s), set_of(&[b]));
    assert_eq!(s.element(a).unwrap().transform.x, 5.0);
}

#[test]
fn the_editing_group_round_trips_through_undo_too() {
    let (mut s, ids, _) = session_with_touch(Some("G"));
    let a = ids[0].clone();
    run(&mut s, "selection.select", json!({"ids": [a.as_str()], "deep": true}));
    assert_eq!(s.tab().observed.editing_group, Some(gid("G")));
    run(&mut s, "t.touch", json!({}));
    run(&mut s, "selection.clear", json!({}));
    assert_eq!(s.tab().observed.editing_group, None);
    s.undo_steps(1).unwrap();
    assert_eq!(s.tab().observed.editing_group, Some(gid("G")));
    assert_eq!(selected(&s), set_of(&[&a]));
}

#[test]
fn a_failed_batch_after_a_document_step_restores_the_selection_it_started_with() {
    let (mut s, ids, _) = session_with_touch(None);
    run(&mut s, "selection.select", json!({"ids": [ids[2].as_str()]}));
    let steps = [BatchStep::new("t.touch", json!({})), BatchStep::new("selection.select", json!({"ids": ["nope"]}))];
    let before = s.fingerprint();
    let out = s.execute_batch(Origin::Test, &steps, &BatchOpts::default());
    assert!(out.rolled_back);
    assert_eq!(s.fingerprint(), before);
    assert_eq!(selected(&s), set_of(&[&ids[2]]));
}

// ---- hostile input ----

#[test]
fn hostile_references_are_refused_with_short_messages() {
    let mut w = world();
    let long = "x".repeat(5000);
    for (id, args) in [
        ("selection.select", json!({"ids": [long.clone()]})),
        ("selection.select", json!({"ids": ["\u{0}", "../../etc/passwd", "@", "@@"]})),
        ("selection.select", json!({"group": long.clone()})),
        ("selection.select", json!({"filter": {"name": long.clone(), "ids": [long.clone()]}})),
        ("selection.all", json!({"within": long.clone()})),
        ("selection.all", json!({"within": "@"})),
        ("selection.navigate", json!({"direction": long.clone()})),
    ] {
        let r = refuse_clean(&mut w.s, id, args);
        assert!(matches!(r.code, "bad_args" | "not_found"), "{id}: {}", r.code);
        assert!(r.message.chars().count() < 400, "{id}: {} characters", r.message.chars().count());
    }
}

#[test]
fn deeply_nested_and_oversized_arguments_are_refused_not_followed() {
    let mut w = world();
    let deep = (0..200).fold(json!(1), |acc, _| json!([acc]));
    for (id, args) in [
        ("selection.select", json!({"ids": deep.clone()})),
        ("selection.select_at", json!({"point": deep.clone()})),
        ("selection.select_region", json!({"rect": deep})),
        ("selection.get", json!({"detail": ["full"]})),
    ] {
        assert_eq!(refuse_clean(&mut w.s, id, args).code, "bad_args", "{id}");
    }
}
