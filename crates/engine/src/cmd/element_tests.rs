//! Tests of `element.delete`, `duplicate`, `move`, `nudge`, `resize`, `set_bounds`, `rotate`, `flip`, `lock`,
//! `unlock_all` and `erase`. Registry-wide checks (hostile arguments, refusal leaves state alone, facts,
//! catalog) run over these commands in `registry_tests.rs` and `catalog_tests.rs`.

use drafftink_geom::kurbo::Point;
use drafftink_scene::{
    Binding, BindMode, Element, ElementId, ElementKind, FrameData, GroupId, LinearData, PathType, SceneRead,
    SceneTx, StoreError, TextData,
};
use serde_json::{Value, json};

use super::{MoveArgs, ResizeArgs};
use crate::ring::Origin;
use crate::session::Session;
use crate::testing;

// ---- helpers ----

fn run(s: &mut Session, id: &str, args: Value) -> Value {
    match s.execute(id, args) {
        Ok(v) => v,
        Err(r) => panic!("{id} refused: {} ({})", r.message, r.code),
    }
}

fn refuse(s: &mut Session, id: &str, args: Value) -> crate::Refusal {
    let before = s.fingerprint();
    match s.execute(id, args) {
        Ok(v) => panic!("{id} should refuse, got {v}"),
        Err(r) => {
            assert_eq!(s.fingerprint(), before, "{id}: a refusal must leave the state unchanged");
            r
        }
    }
}

fn eid(s: &str) -> ElementId {
    ElementId::new(s).unwrap()
}

/// Three rectangles `e1` (0,0,100,60), `e2` (200,50,80,80), `e3` (-150,300,40,40); `e1` selected.
fn shapes() -> Session {
    testing::session_with_shapes().unwrap().0
}

fn pos(s: &Session, id: &str) -> (f64, f64) {
    let e = s.element(&eid(id)).unwrap();
    (e.transform.x, e.transform.y)
}

fn size(s: &Session, id: &str) -> (f64, f64) {
    let e = s.element(&eid(id)).unwrap();
    (e.transform.w, e.transform.h)
}

fn sel(s: &Session) -> Vec<String> {
    s.selection().iter().map(|i| i.as_str().to_owned()).collect()
}

/// Edits the document for setup; the change is not an undo step.
fn setup(s: &mut Session, f: impl FnOnce(&mut crate::store::DocTx<'_, '_>) -> Result<(), StoreError>) {
    s.tab_mut().doc.edit::<(), StoreError>(f).unwrap();
    s.tab_mut().doc.commit_untracked().unwrap();
}

fn lock_it(s: &mut Session, id: &str) {
    let id = eid(id);
    setup(s, |tx| tx.update(&id, |e| e.locked = true));
}

/// An arrow from the right edge of `from` to the left edge of `to`, bound orbit at both ends.
fn bound_arrow(s: &mut Session, from: &str, to: &str) -> ElementId {
    let (a, b) = (eid(from), eid(to));
    let id = s.ports.ids.element_id();
    let ret = id.clone();
    setup(s, |tx| {
        let z = tx.next_top_index().map_err(StoreError::backend)?;
        let mut d = LinearData::new(vec![Point::new(0.0, 0.0), Point::new(100.0, 0.0)]);
        d.start_binding = Some(Binding { target: a, mode: BindMode::Orbit, fixed_point: [1.0, 0.5] });
        d.end_binding = Some(Binding { target: b, mode: BindMode::Orbit, fixed_point: [0.0, 0.5] });
        d.end_head = drafftink_scene::Head::Arrow;
        let mut e = Element::new(id, ElementKind::Arrow(d), z);
        e.transform = drafftink_scene::Transform::new(100.0, 30.0, 100.0, 0.0);
        tx.insert(e)?;
        drafftink_edit::binding::update_bound_arrows(tx, &[eid("e1"), eid("e2")])
            .map_err(StoreError::backend)?;
        Ok(())
    });
    ret
}

fn group_of(s: &mut Session, ids: &[&str], g: &str) {
    let ids: Vec<ElementId> = ids.iter().map(|i| eid(i)).collect();
    let g = GroupId::new(g).unwrap();
    setup(s, |tx| {
        drafftink_edit::group::group(tx, &ids, None, g).map_err(StoreError::backend)?;
        Ok(())
    });
}

// ---- registration ----

#[test]
fn the_part_registers_its_eleven_commands_with_the_catalog_keys() {
    let s = shapes();
    for id in [
        "element.delete",
        "element.duplicate",
        "element.move",
        "element.nudge",
        "element.resize",
        "element.set_bounds",
        "element.rotate",
        "element.flip",
        "element.lock",
        "element.unlock_all",
        "element.erase",
    ] {
        assert!(s.spec(id).is_some(), "{id} is registered");
    }
    for id in ["element.move", "element.resize", "element.rotate"] {
        assert!(s.spec(id).unwrap().is_gesture(), "{id} runs as a gesture");
    }
    let nudge = s.spec("element.nudge").unwrap();
    assert_eq!((nudge.coalesce.map(|c| c.ms), nudge.shortcuts.len()), (Some(1000), 8));
    assert_eq!(s.spec("element.delete").unwrap().shortcuts[0].keys, &["Delete", "Backspace"]);
    assert_eq!(s.spec("element.duplicate").unwrap().shortcuts[0].keys, &["Mod+D"]);
    assert_eq!(s.spec("element.lock").unwrap().shortcuts[0].keys, &["Mod+Shift+L"]);
    assert_eq!(s.spec("element.flip").unwrap().shortcuts.len(), 2);
}

#[test]
fn commands_that_need_a_selection_are_greyed_out_without_one() {
    let mut s = shapes();
    s.tab_mut().observed.selection.clear();
    for id in [
        "element.delete",
        "element.duplicate",
        "element.move",
        "element.nudge",
        "element.resize",
        "element.set_bounds",
        "element.rotate",
        "element.flip",
        "element.lock",
    ] {
        let r = s.availability(id).unwrap_err();
        assert_eq!(r.code, "no_target", "{id}");
        assert_eq!(r.message, "Nothing to act on: no elements are selected. Select elements first, or pass `ids`.");
    }
    assert!(s.availability("element.erase").is_ok());
    // Explicit ids work with an empty selection.
    run(&mut s, "element.move", json!({"ids": ["e2"], "dx": 5}));
    assert_eq!(pos(&s, "e2"), (205.0, 50.0));
    let r = refuse(&mut s, "element.delete", json!({}));
    assert_eq!(r.code, "no_target");
}

#[test]
fn every_command_refuses_in_view_only_mode() {
    let mut s = shapes();
    s.view_only = true;
    for id in ["element.delete", "element.move", "element.nudge", "element.lock", "element.erase"] {
        let r = s.availability(id).unwrap_err();
        assert_eq!(r.code, "view_only", "{id}");
    }
}

// ---- element.delete ----

#[test]
fn delete_removes_the_selection_and_undo_brings_back_both_elements_and_selection() {
    let mut s = shapes();
    let out = run(&mut s, "element.delete", json!({}));
    assert_eq!(out["deleted"], json!(["e1"]));
    assert_eq!(out["undo_label"], "Delete");
    assert!(s.element(&eid("e1")).is_none());
    assert!(sel(&s).is_empty());
    assert_eq!(s.tab().doc.undo_depth(), 1);
    s.undo_steps(1).unwrap();
    assert!(s.element(&eid("e1")).is_some());
    assert_eq!(sel(&s), ["e1"]);
    s.redo_steps(1).unwrap();
    assert!(s.element(&eid("e1")).is_none());
    assert!(sel(&s).is_empty());
}

#[test]
fn delete_by_explicit_ids_and_unknown_or_locked_ids() {
    let mut s = shapes();
    run(&mut s, "element.delete", json!({"ids": ["e2", "e3"]}));
    assert!(s.element(&eid("e2")).is_none() && s.element(&eid("e3")).is_none());
    assert_eq!(sel(&s), ["e1"], "the selection keeps what was not deleted");

    let r = refuse(&mut s, "element.delete", json!({"ids": ["nope"]}));
    assert_eq!(r.code, "not_found");
    assert_eq!(r.message, "No element matches `nope`. Look up ids with `element.query` or `doc.outline`.");

    lock_it(&mut s, "e1");
    let r = refuse(&mut s, "element.delete", json!({"ids": ["e1"]}));
    assert_eq!(r.code, "locked");
    assert_eq!(
        r.message,
        "Element `e1` is locked. Unlock it first with `element.lock` (locked=false) or `element.unlock_all`."
    );
    assert_eq!(r.details, Some(json!({"ids": ["e1"]})));
}

#[test]
fn delete_by_name_reference() {
    let mut s = shapes();
    setup(&mut s, |tx| tx.update(&eid("e2"), |e| e.name = Some("box".to_owned())));
    run(&mut s, "element.delete", json!({"ids": ["@box"]}));
    assert!(s.element(&eid("e2")).is_none());
}

#[test]
fn delete_takes_the_bound_text_and_unbinds_arrows_that_stay() {
    let mut s = shapes();
    let label = testing::add(&mut s, ElementKind::Text(TextData::new("hi")), 10.0, 10.0, 20.0, 20.0).unwrap();
    setup(&mut s, |tx| {
        tx.update(&label, |e| {
            if let ElementKind::Text(t) = &mut e.kind {
                t.container_id = Some(eid("e1"));
            }
        })
    });
    let arrow = bound_arrow(&mut s, "e1", "e2");
    let out = run(&mut s, "element.delete", json!({"ids": ["e1"]}));
    let mut deleted: Vec<String> = serde_json::from_value(out["deleted"].clone()).unwrap();
    deleted.sort();
    assert_eq!(deleted, ["e1", label.as_str()]);
    let a = s.element(&arrow).unwrap();
    let d = a.linear_data().unwrap();
    assert!(d.start_binding.is_none(), "the end bound to the deleted element is released");
    assert!(d.end_binding.is_some(), "the other end stays bound");
}

#[test]
fn delete_a_frame_deletes_or_releases_its_children() {
    let mut s = shapes();
    let f = testing::add(&mut s, ElementKind::Frame(FrameData { name: None, clip: true }), -20.0, -20.0, 400.0, 300.0)
        .unwrap();
    setup(&mut s, |tx| tx.update(&eid("e1"), |e| e.frame_id = Some(f.clone())));
    let mut s2 = shapes();
    let f2 = testing::add(&mut s2, ElementKind::Frame(FrameData { name: None, clip: true }), -20.0, -20.0, 400.0, 300.0)
        .unwrap();
    setup(&mut s2, |tx| tx.update(&eid("e1"), |e| e.frame_id = Some(f2.clone())));

    run(&mut s, "element.delete", json!({"ids": [f.as_str()]}));
    assert!(s.element(&eid("e1")).is_none(), "children are deleted with the frame");

    let out = run(&mut s2, "element.delete", json!({"ids": [f2.as_str()], "frame_children": "release"}));
    assert_eq!(out["released"], json!(["e1"]));
    assert!(s2.element(&eid("e1")).unwrap().frame_id.is_none());
    s2.undo_steps(1).unwrap();
    assert_eq!(s2.element(&eid("e1")).unwrap().frame_id.as_ref(), Some(&f2));
}

#[test]
fn delete_inside_an_editing_group_keeps_the_group_while_it_has_two_members() {
    let mut s = shapes();
    group_of(&mut s, &["e1", "e2", "e3"], "g1");
    {
        let o = &mut s.tab_mut().observed;
        o.editing_group = Some(GroupId::new("g1").unwrap());
        o.selection = [eid("e1")].into();
    }
    run(&mut s, "element.delete", json!({}));
    assert_eq!(s.tab().observed.editing_group.as_ref().map(GroupId::as_str), Some("g1"));
    // Two members left: delete one more, the group is gone and so is the editing group.
    run(&mut s, "element.delete", json!({"ids": ["e2"]}));
    assert!(s.tab().observed.editing_group.is_none());
}

// ---- element.duplicate ----

#[test]
fn duplicate_copies_offsets_and_selects_the_copies() {
    let mut s = shapes();
    let out = run(&mut s, "element.duplicate", json!({}));
    let copy = out["ids"][0].as_str().unwrap().to_owned();
    assert_ne!(copy, "e1");
    assert_eq!(out["created"], json!([copy]));
    assert_eq!(pos(&s, &copy), (20.0, 20.0));
    assert_eq!(size(&s, &copy), (100.0, 60.0));
    assert_eq!(sel(&s), [copy.as_str()]);
    assert_ne!(
        s.element(&eid(&copy)).unwrap().style.seed,
        s.element(&eid("e1")).unwrap().style.seed,
        "a copy gets its own seed"
    );
    assert_eq!(s.tab().doc.undo_depth(), 1);
    s.undo_steps(1).unwrap();
    assert!(s.element(&eid(&copy)).is_none());
    assert_eq!(sel(&s), ["e1"]);
}

#[test]
fn duplicate_with_offset_without_selecting_and_with_explicit_ids() {
    let mut s = shapes();
    run(&mut s, "element.duplicate", json!({"ids": ["e2"], "dx": -5, "dy": 0.5, "select": false}));
    assert_eq!(sel(&s), ["e1"]);
    let copy = s.store().iter().find(|e| e.id.as_str() != "e2" && e.transform.x == 195.0).unwrap();
    assert_eq!(copy.transform.y, 50.5);
}

#[test]
fn duplicate_regenerates_group_ids_consistently_and_remaps_bindings() {
    let mut s = shapes();
    group_of(&mut s, &["e1", "e2"], "g1");
    let arrow = bound_arrow(&mut s, "e1", "e2");
    s.tab_mut().observed.selection = [eid("e1"), eid("e2"), arrow.clone()].into();
    let out = run(&mut s, "element.duplicate", json!({}));
    let copies: Vec<String> = serde_json::from_value(out["ids"].clone()).unwrap();
    assert_eq!(copies.len(), 3);
    let groups: std::collections::BTreeSet<_> = copies
        .iter()
        .filter_map(|c| s.element(&eid(c)))
        .flat_map(|e| e.group_ids.iter().cloned())
        .collect();
    assert_eq!(groups.len(), 1, "all copies of the group's members share one new group id");
    assert!(!groups.contains(&GroupId::new("g1").unwrap()));
    let new_arrow = copies
        .iter()
        .filter_map(|c| s.element(&eid(c)))
        .find(|e| matches!(e.kind, ElementKind::Arrow(_)))
        .unwrap();
    let d = new_arrow.linear_data().unwrap();
    for b in [d.start_binding.as_ref(), d.end_binding.as_ref()].into_iter().flatten() {
        assert!(copies.contains(&b.target.as_str().to_owned()), "bindings point at the copies");
    }
}

#[test]
fn duplicate_an_arrow_alone_drops_its_bindings_and_refuses_locked() {
    let mut s = shapes();
    let arrow = bound_arrow(&mut s, "e1", "e2");
    let out = run(&mut s, "element.duplicate", json!({"ids": [arrow.as_str()]}));
    let copy = s.element(&eid(out["ids"][0].as_str().unwrap())).unwrap();
    let d = copy.linear_data().unwrap();
    assert!(d.start_binding.is_none() && d.end_binding.is_none());

    lock_it(&mut s, "e2");
    let r = refuse(&mut s, "element.duplicate", json!({"ids": ["e2"]}));
    assert_eq!(r.code, "locked");
    let r = refuse(&mut s, "element.duplicate", json!({"dx": 1e300}));
    assert_eq!(r.code, "bad_args");
}

// ---- element.move ----

#[test]
fn move_translates_and_reports_the_applied_offset() {
    let mut s = shapes();
    let out = run(&mut s, "element.move", json!({"dx": 30, "dy": -10}));
    assert_eq!(pos(&s, "e1"), (30.0, -10.0));
    assert_eq!((out["dx"].as_f64(), out["dy"].as_f64()), (Some(30.0), Some(-10.0)));
    assert_eq!(out["guides"], json!([]));
    assert_eq!(out["undo_label"], "Move");
    assert_eq!(s.tab().doc.undo_depth(), 1);
    s.undo_steps(1).unwrap();
    assert_eq!(pos(&s, "e1"), (0.0, 0.0));
}

#[test]
fn move_follows_the_transform_rule_for_arrows() {
    let mut s = shapes();
    let arrow = bound_arrow(&mut s, "e1", "e2");
    // Both ends in the set: the arrow keeps its bindings and moves with them.
    run(&mut s, "element.move", json!({"ids": ["e1", "e2", arrow.as_str()], "dx": 10, "dy": 10}));
    let d = s.element(&arrow).unwrap().linear_data().unwrap().clone();
    assert!(d.start_binding.is_some() && d.end_binding.is_some());
    // The arrow alone: both ends unbind.
    run(&mut s, "element.move", json!({"ids": [arrow.as_str()], "dx": 5}));
    let d = s.element(&arrow).unwrap().linear_data().unwrap().clone();
    assert!(d.start_binding.is_none() && d.end_binding.is_none());
}

#[test]
fn a_shape_dragged_alone_drags_its_bound_arrow_along() {
    let mut s = shapes();
    let arrow = bound_arrow(&mut s, "e1", "e2");
    let before = s.element(&arrow).unwrap().transform;
    run(&mut s, "element.move", json!({"ids": ["e2"], "dx": 0, "dy": 100}));
    let d = s.element(&arrow).unwrap().linear_data().unwrap().clone();
    assert!(d.start_binding.is_some() && d.end_binding.is_some(), "both bindings stay");
    assert_ne!(s.element(&arrow).unwrap().transform, before, "the arrow was re-solved");
}

#[test]
fn move_refuses_locked_elements_whole() {
    let mut s = shapes();
    lock_it(&mut s, "e2");
    let r = refuse(&mut s, "element.move", json!({"ids": ["e1", "e2"], "dx": 10}));
    assert_eq!(r.code, "locked");
    assert_eq!(r.details, Some(json!({"ids": ["e2"]})));
    assert_eq!(pos(&s, "e1"), (0.0, 0.0), "nothing moved partially");
}

#[test]
fn move_snaps_to_the_grid_and_to_smart_guides_but_not_a_click() {
    let mut s = shapes();
    // Grid: e1 at 0,0 dragged by 33,27 lands on 40,20 (top-left on the grid).
    run(&mut s, "element.move", json!({"dx": 33, "dy": 27, "snap": {"grid": true}}));
    assert_eq!(pos(&s, "e1"), (40.0, 20.0));
    // A sub-threshold drag does not snap (the old "movement threshold").
    run(&mut s, "element.move", json!({"dx": 1, "dy": 1, "snap": {"grid": true}}));
    assert_eq!(pos(&s, "e1"), (41.0, 21.0));
    // "none" ignores the settings.
    s.settings.set_raw("snap.grid", json!(true));
    run(&mut s, "element.move", json!({"dx": 13, "dy": 0, "snap": "none"}));
    assert_eq!(pos(&s, "e1"), (54.0, 21.0));
    // "auto" follows them.
    run(&mut s, "element.move", json!({"dx": 13, "dy": 0}));
    assert_eq!(pos(&s, "e1").0 % 20.0, 0.0);
}

#[test]
fn move_smart_guides_align_to_a_neighbour_and_return_the_guides() {
    let mut s = shapes();
    // e1 is 100x60 at (0,0). e2 starts at x=200. Dragging e1 right by 197 puts its right edge near 297 and its
    // left edge at 197: within 8 of e2's left edge (200).
    let out = run(&mut s, "element.move", json!({"dx": 197, "dy": 0, "snap": {"smart": true}}));
    assert_eq!(pos(&s, "e1").0, 200.0);
    assert_eq!(out["dx"].as_f64(), Some(200.0));
    assert!(out["guides"].as_array().is_some_and(|g| !g.is_empty()), "{out}");
    assert_eq!(out["guides"][0]["kind"], "vertical");
}

#[test]
fn move_duplicate_leaves_the_originals_and_selects_the_copies() {
    let mut s = shapes();
    let out = run(&mut s, "element.move", json!({"dx": 50, "dy": 0, "duplicate": true}));
    assert_eq!(pos(&s, "e1"), (0.0, 0.0));
    let copy = out["ids"][0].as_str().unwrap().to_owned();
    assert_eq!(pos(&s, &copy), (50.0, 0.0));
    assert_eq!(sel(&s), [copy]);
    assert_eq!(s.tab().doc.undo_depth(), 1, "copy and move are one step");
}

#[test]
fn move_joins_and_leaves_frames_on_the_committed_run_only() {
    let mut s = shapes();
    let f = testing::add(&mut s, ElementKind::Frame(FrameData { name: None, clip: true }), 400.0, 0.0, 300.0, 300.0)
        .unwrap();
    // Dropped over the frame: joins.
    let out = run(&mut s, "element.move", json!({"dx": 480, "dy": 100, "pointer": [500, 150]}));
    assert_eq!(s.element(&eid("e1")).unwrap().frame_id.as_ref(), Some(&f));
    assert_eq!(out["frame_highlight"], json!(f.as_str()));
    // Dragged out: leaves.
    run(&mut s, "element.move", json!({"dx": -480, "dy": 0, "pointer": [0, 100]}));
    assert!(s.element(&eid("e1")).unwrap().frame_id.is_none());
    // frame=keep never changes membership.
    run(&mut s, "element.move", json!({"dx": 480, "dy": 100, "pointer": [500, 150], "frame": "keep"}));
    assert!(s.element(&eid("e1")).unwrap().frame_id.is_none());
}

#[test]
fn move_as_a_gesture_equals_the_plain_command() {
    let mut a = shapes();
    let mut b = shapes();
    let args = |dx, dy| MoveArgs { dx, dy, ..MoveArgs::default() };
    let (g, _) = a.gesture_begin_typed(Origin::Ui, "element.move", &args(0.0, 0.0)).unwrap();
    for i in 1..=5 {
        a.gesture_update(g, &args(f64::from(i) * 10.0, f64::from(i) * 5.0)).unwrap();
        assert_eq!(a.tab().doc.undo_depth(), 0, "no undo step during the drag");
    }
    a.gesture_end_typed(g, Some(&args(50.0, 25.0))).unwrap();
    run(&mut b, "element.move", json!({"dx": 50, "dy": 25}));
    assert_eq!(pos(&a, "e1"), pos(&b, "e1"));
    assert_eq!(a.tab().doc.undo_depth(), 1);
    // Cancel restores the base.
    let (g, _) = a.gesture_begin_typed(Origin::Ui, "element.move", &args(1.0, 1.0)).unwrap();
    a.gesture_update(g, &args(99.0, 99.0)).unwrap();
    a.gesture_cancel(Some(g)).unwrap();
    assert_eq!(pos(&a, "e1"), (50.0, 25.0));
}

#[test]
fn move_rejects_hostile_numbers() {
    let mut s = shapes();
    let r = refuse(&mut s, "element.move", json!({"dx": 1e300}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("`dx`"), "{}", r.message);
    let r = refuse(&mut s, "element.move", json!({"dx": 1, "pointer": [0, 1e300]}));
    assert!(r.message.contains("pointer[1]"), "{}", r.message);
    let r = refuse(&mut s, "element.move", json!({"dxx": 1}));
    assert!(r.message.contains("did you mean `dx`"), "{}", r.message);
}

// ---- element.nudge ----

#[test]
fn nudge_moves_by_the_setting_one_unit_when_fine_or_a_given_step() {
    let mut s = shapes();
    let out = run(&mut s, "element.nudge", json!({"dir": "right"}));
    assert_eq!((out["dx"].as_f64(), out["dy"].as_f64()), (Some(20.0), Some(0.0)));
    assert_eq!(pos(&s, "e1"), (20.0, 0.0));
    run(&mut s, "element.nudge", json!({"dir": "down", "fine": true}));
    assert_eq!(pos(&s, "e1"), (20.0, 1.0));
    run(&mut s, "element.nudge", json!({"dir": "left", "step": 5}));
    run(&mut s, "element.nudge", json!({"dir": "up", "step": 0.5, "ids": ["e2"]}));
    assert_eq!(pos(&s, "e1"), (15.0, 1.0));
    assert_eq!(pos(&s, "e2"), (200.0, 49.5));
    s.settings.set_raw("input.nudge_step", json!(10));
    run(&mut s, "element.nudge", json!({"dir": "right"}));
    assert_eq!(pos(&s, "e1"), (25.0, 1.0));
    assert_eq!(s.tab().doc.undo_depth(), 3, "repeats on the same element merge; the nudge of e2 and the later one are own steps");
}

/// docs/exceptions.md E15: holding an arrow key repeats `element.nudge`; the burst is one undo step and
/// undoing it restores the start.
#[test]
#[allow(non_snake_case)]
fn exception_E15_nudge_burst_is_one_undo_step() {
    let mut s = shapes();
    let start = pos(&s, "e1");
    let depth = s.tab().doc.undo_depth();
    for _ in 0..30 {
        run(&mut s, "element.nudge", json!({"dir": "right"}));
    }
    assert_eq!(pos(&s, "e1"), (start.0 + 600.0, start.1));
    assert_eq!(s.tab().doc.undo_depth(), depth + 1, "30 repeats are one step");
    s.undo_steps(1).unwrap();
    assert_eq!(pos(&s, "e1"), start);
}

#[test]
fn nudge_refuses_a_bad_step_and_locked_elements() {
    let mut s = shapes();
    for bad in [json!(0), json!(-3), json!(1e300)] {
        let r = refuse(&mut s, "element.nudge", json!({"dir": "left", "step": bad}));
        assert_eq!(r.code, "bad_args");
        assert!(r.message.contains("`step`"), "{}", r.message);
    }
    let r = refuse(&mut s, "element.nudge", json!({"dir": "sideways"}));
    assert_eq!(r.code, "bad_args");
    let r = refuse(&mut s, "element.nudge", json!({}));
    assert_eq!(r.code, "bad_args");
    lock_it(&mut s, "e1");
    let r = refuse(&mut s, "element.nudge", json!({"dir": "left", "ids": ["e1"]}));
    assert_eq!(r.code, "locked");
}

#[test]
fn nudging_an_arrow_alone_unbinds_it_and_nudging_both_ends_keeps_the_binding() {
    let mut s = shapes();
    let arrow = bound_arrow(&mut s, "e1", "e2");
    run(&mut s, "element.nudge", json!({"dir": "down", "ids": ["e1", "e2", arrow.as_str()]}));
    assert!(s.element(&arrow).unwrap().linear_data().unwrap().start_binding.is_some());
    run(&mut s, "element.nudge", json!({"dir": "down", "ids": [arrow.as_str()]}));
    assert!(s.element(&arrow).unwrap().linear_data().unwrap().start_binding.is_none());
}

// ---- element.resize ----

#[test]
fn resize_drags_a_corner_about_the_opposite_corner() {
    let mut s = shapes();
    let out = run(&mut s, "element.resize", json!({"handle": "se", "dx": 50, "dy": 20}));
    assert_eq!((pos(&s, "e1"), size(&s, "e1")), ((0.0, 0.0), (150.0, 80.0)));
    assert_eq!(out["bounds"], json!({"x": 0.0, "y": 0.0, "w": 150.0, "h": 80.0}));
    assert_eq!(out["size_badge"], json!({"w": 150.0, "h": 80.0}));
    run(&mut s, "element.resize", json!({"handle": "nw", "dx": 10, "dy": 10}));
    assert_eq!((pos(&s, "e1"), size(&s, "e1")), ((10.0, 10.0), (140.0, 70.0)));
    assert_eq!(s.tab().doc.undo_depth(), 2);
}

#[test]
fn resize_edge_handles_stretch_one_axis_and_default_handle_is_se() {
    let mut s = shapes();
    run(&mut s, "element.resize", json!({"handle": "e", "dx": 40, "dy": 999}));
    assert_eq!(size(&s, "e1"), (140.0, 60.0));
    run(&mut s, "element.resize", json!({"handle": "s", "dx": 999, "dy": -10}));
    assert_eq!(size(&s, "e1"), (140.0, 50.0));
    run(&mut s, "element.resize", json!({"handle": "w", "dx": -20, "dy": 0}));
    assert_eq!((pos(&s, "e1"), size(&s, "e1")), ((-20.0, 0.0), (160.0, 50.0)));
    run(&mut s, "element.resize", json!({"dx": 10, "dy": 10}));
    assert_eq!(size(&s, "e1"), (170.0, 60.0));
}

#[test]
fn resize_keep_aspect_from_center_and_minimum_size() {
    let mut s = shapes();
    run(&mut s, "element.resize", json!({"handle": "se", "dx": 100, "dy": 0, "keep_aspect": true}));
    let (w, h) = size(&s, "e1");
    assert!((w / h - 100.0 / 60.0).abs() < 1e-9, "ratio kept: {w}x{h}");
    let mut s = shapes();
    run(&mut s, "element.resize", json!({"handle": "se", "dx": 20, "dy": 20, "from_center": true}));
    assert_eq!((pos(&s, "e1"), size(&s, "e1")), ((-20.0, -20.0), (140.0, 100.0)));
    let mut s = shapes();
    run(&mut s, "element.resize", json!({"handle": "se", "dx": -99.9999, "dy": -59.9999}));
    let (w, h) = size(&s, "e1");
    assert!(w >= 1.0 && h >= 1.0, "minimum size is 1 unit, never a refusal: {w}x{h}");
}

#[test]
fn resize_a_group_scales_the_members_as_a_unit() {
    let mut s = shapes();
    s.tab_mut().observed.selection = [eid("e1"), eid("e2")].into();
    let before = (pos(&s, "e1"), pos(&s, "e2"));
    run(&mut s, "element.resize", json!({"handle": "se", "dx": 140, "dy": 0, "keep_aspect": true}));
    // The common box was (0,0)-(280,130); it doubles in width and (kept ratio) height.
    assert_ne!((pos(&s, "e1"), pos(&s, "e2")), before);
    let e2 = s.element(&eid("e2")).unwrap().transform;
    assert!((e2.w / e2.h - 1.0).abs() < 1e-6, "members keep their own ratio");
}

#[test]
fn resize_a_frame_recomputes_membership_on_the_committed_run() {
    let mut s = shapes();
    let f = testing::add(&mut s, ElementKind::Frame(FrameData { name: None, clip: true }), -10.0, -10.0, 130.0, 100.0)
        .unwrap();
    setup(&mut s, |tx| tx.update(&eid("e1"), |e| e.frame_id = Some(f.clone())));
    s.tab_mut().observed.selection = [f.clone()].into();
    // Grow the frame so that e2 lies completely inside: it joins.
    run(&mut s, "element.resize", json!({"handle": "se", "dx": 200, "dy": 100}));
    assert_eq!(s.element(&eid("e2")).unwrap().frame_id.as_ref(), Some(&f));
    // Shrink it back: e2 leaves.
    run(&mut s, "element.resize", json!({"handle": "se", "dx": -200, "dy": -100}));
    assert!(s.element(&eid("e2")).unwrap().frame_id.is_none());
}

#[test]
fn resize_snaps_the_dragged_handle_to_the_grid() {
    let mut s = shapes();
    run(&mut s, "element.resize", json!({"handle": "se", "dx": 47, "dy": 33, "snap": {"grid": true}}));
    let (w, h) = size(&s, "e1");
    assert_eq!((w % 20.0, h % 20.0), (0.0, 0.0), "{w}x{h}");
}

#[test]
fn resize_as_a_gesture_equals_the_plain_command_and_refuses_locked() {
    let mut a = shapes();
    let mut b = shapes();
    let args = |dx, dy| ResizeArgs {
        ids: None,
        handle: super::Handle::Se,
        dx,
        dy,
        keep_aspect: false,
        from_center: false,
        snap: Default::default(),
    };
    let (g, _) = a.gesture_begin_typed(Origin::Ui, "element.resize", &args(0.0, 0.0)).unwrap();
    for i in 1..=4 {
        a.gesture_update(g, &args(f64::from(i) * 10.0, f64::from(i) * 4.0)).unwrap();
    }
    a.gesture_end_typed(g, Some(&args(40.0, 16.0))).unwrap();
    run(&mut b, "element.resize", json!({"dx": 40, "dy": 16}));
    assert_eq!(size(&a, "e1"), size(&b, "e1"));
    assert_eq!(a.tab().doc.undo_depth(), 1);
    lock_it(&mut a, "e1");
    assert_eq!(refuse(&mut a, "element.resize", json!({"dx": 1, "dy": 1})).code, "locked");
    let r = refuse(&mut a, "element.resize", json!({"handle": "se"}));
    assert_eq!(r.code, "bad_args");
    let r = refuse(&mut a, "element.resize", json!({"handle": "up", "dx": 1, "dy": 1}));
    assert_eq!(r.code, "bad_args");
}

// ---- element.set_bounds ----

#[test]
fn set_bounds_gives_one_element_exactly_that_box() {
    let mut s = shapes();
    run(&mut s, "element.set_bounds", json!({"x": 100, "y": 50, "w": 200, "h": 80}));
    assert_eq!((pos(&s, "e1"), size(&s, "e1")), ((100.0, 50.0), (200.0, 80.0)));
    run(&mut s, "element.set_bounds", json!({"w": 50}));
    assert_eq!((pos(&s, "e1"), size(&s, "e1")), ((100.0, 50.0), (50.0, 80.0)));
    run(&mut s, "element.set_bounds", json!({"w": 100, "keep_aspect": true}));
    assert_eq!(size(&s, "e1"), (100.0, 160.0));
    run(&mut s, "element.set_bounds", json!({"x": 0, "y": 0, "anchor": "center"}));
    assert_eq!(pos(&s, "e1"), (-50.0, -80.0));
    assert_eq!(size(&s, "e1"), (100.0, 160.0));
}

#[test]
fn set_bounds_of_several_elements_sets_their_common_box() {
    let mut s = shapes();
    s.tab_mut().observed.selection = [eid("e1"), eid("e2")].into();
    run(&mut s, "element.set_bounds", json!({"x": 1000, "y": 500}));
    let b = drafftink_edit::transform::common_bounds(s.store(), &[eid("e1"), eid("e2")]).unwrap();
    assert_eq!((b.x0, b.y0), (1000.0, 500.0));
    assert_eq!((b.width(), b.height()), (280.0, 130.0), "size unchanged when only the position is given");
    run(&mut s, "element.set_bounds", json!({"w": 560, "h": 260, "keep_aspect": true}));
    let b = drafftink_edit::transform::common_bounds(s.store(), &[eid("e1"), eid("e2")]).unwrap();
    assert!((b.width() - 560.0).abs() < 1e-6 && (b.height() - 260.0).abs() < 1e-6, "{b:?}");
    assert_eq!((b.x0, b.y0), (1000.0, 500.0));
}

#[test]
fn set_bounds_refuses_a_zero_size_and_locked() {
    let mut s = shapes();
    for key in ["w", "h"] {
        let r = refuse(&mut s, "element.set_bounds", json!({ key: 0 }));
        assert_eq!(r.code, "bad_args");
        assert!(r.message.contains(&format!("`{key}`")), "{}", r.message);
    }
    lock_it(&mut s, "e1");
    assert_eq!(refuse(&mut s, "element.set_bounds", json!({"x": 1})).code, "locked");
}

// ---- element.rotate ----

#[test]
fn rotate_by_and_to_with_snapping() {
    let mut s = shapes();
    let out = run(&mut s, "element.rotate", json!({"angle": 30}));
    assert!((s.element(&eid("e1")).unwrap().transform.angle.to_degrees() - 30.0).abs() < 1e-9);
    assert!((out["angle"].as_f64().unwrap() - 30.0).abs() < 1e-9);
    assert!(out["angle_badge"]["center"].is_array());
    run(&mut s, "element.rotate", json!({"angle": 100, "mode": "to", "snap_degrees": 15}));
    assert!((s.element(&eid("e1")).unwrap().transform.angle.to_degrees() - 105.0).abs() < 1e-9);
    run(&mut s, "element.rotate", json!({"angle": 0, "mode": "to"}));
    assert!(s.element(&eid("e1")).unwrap().transform.angle.abs() < 1e-12, "double-click on the handle resets");
    assert_eq!(s.tab().doc.undo_depth(), 3);
}

#[test]
fn rotate_several_elements_changes_each_own_angle() {
    let mut s = shapes();
    s.tab_mut().observed.selection = [eid("e1"), eid("e2")].into();
    run(&mut s, "element.rotate", json!({"angle": 90}));
    for id in ["e1", "e2"] {
        assert!((s.element(&eid(id)).unwrap().transform.angle.to_degrees() - 90.0).abs() < 1e-9, "{id}");
    }
    let r = refuse(&mut s, "element.rotate", json!({"angle": 10, "mode": "to"}));
    assert_eq!(r.code, "bad_args");
    assert_eq!(
        r.message,
        "mode \"to\" sets an absolute angle and needs exactly one element; use mode \"by\" to rotate several."
    );
}

#[test]
fn rotate_refuses_frames_and_elbow_arrows_with_the_catalog_text() {
    let mut s = shapes();
    let f = testing::add(&mut s, ElementKind::Frame(FrameData { name: None, clip: true }), 500.0, 0.0, 100.0, 100.0)
        .unwrap();
    let r = refuse(&mut s, "element.rotate", json!({"ids": ["e1", f.as_str()], "angle": 10}));
    assert_eq!(r.code, "not_rotatable");
    assert_eq!(r.message, "Frames cannot be rotated.");
    assert_eq!(r.details, Some(json!({"ids": [f.as_str()]})));

    let arrow = bound_arrow(&mut s, "e1", "e2");
    setup(&mut s, |tx| {
        tx.update(&arrow, |e| {
            if let ElementKind::Arrow(d) = &mut e.kind {
                d.path = PathType::Elbow;
            }
        })
    });
    let r = refuse(&mut s, "element.rotate", json!({"ids": [arrow.as_str()], "angle": 10}));
    assert_eq!(r.code, "not_rotatable");
    assert_eq!(
        r.message,
        "Elbow arrows cannot be rotated. Change the arrow type first with `arrow.set_type` (sharp or round)."
    );
    assert_eq!(refuse(&mut s, "element.rotate", json!({"angle": 1e12})).code, "bad_args");
    assert_eq!(refuse(&mut s, "element.rotate", json!({"angle": 5, "snap_degrees": 0})).code, "bad_args");
}

#[test]
fn rotating_a_single_arrow_unbinds_both_ends() {
    let mut s = shapes();
    let arrow = bound_arrow(&mut s, "e1", "e2");
    run(&mut s, "element.rotate", json!({"ids": [arrow.as_str()], "angle": 20}));
    let d = s.element(&arrow).unwrap().linear_data().unwrap().clone();
    assert!(d.start_binding.is_none() && d.end_binding.is_none());
}

#[test]
fn rotate_as_a_gesture_equals_the_plain_command() {
    use super::{RotateArgs, RotateModeArg};
    let mut a = shapes();
    let mut b = shapes();
    let args = |angle| RotateArgs { ids: None, angle, mode: RotateModeArg::By, snap_degrees: None };
    let (g, _) = a.gesture_begin_typed(Origin::Ui, "element.rotate", &args(0.0)).unwrap();
    for i in 1..=3 {
        a.gesture_update(g, &args(f64::from(i) * 10.0)).unwrap();
    }
    a.gesture_end_typed(g, Some(&args(30.0))).unwrap();
    run(&mut b, "element.rotate", json!({"angle": 30}));
    assert_eq!(a.element(&eid("e1")).unwrap().transform, b.element(&eid("e1")).unwrap().transform);
    assert_eq!(a.tab().doc.undo_depth(), 1);
}

// ---- element.flip ----

#[test]
fn flip_mirrors_about_the_common_centre() {
    let mut s = shapes();
    s.tab_mut().observed.selection = [eid("e1"), eid("e2")].into();
    run(&mut s, "element.flip", json!({"axis": "horizontal"}));
    // The common box is x 0..280: e1 (0..100) mirrors to 180..280, e2 (200..280) to 0..80.
    assert_eq!(pos(&s, "e1").0, 180.0);
    assert_eq!(pos(&s, "e2").0, 0.0);
    assert_eq!(pos(&s, "e1").1, 0.0, "a horizontal flip keeps y");
    run(&mut s, "element.flip", json!({"axis": "vertical"}));
    assert_eq!(s.tab().doc.undo_depth(), 2);
    s.undo_steps(2).unwrap();
    assert_eq!(pos(&s, "e1"), (0.0, 0.0));
    assert_eq!(refuse(&mut s, "element.flip", json!({})).code, "bad_args");
    assert_eq!(refuse(&mut s, "element.flip", json!({"axis": "diagonal"})).code, "bad_args");
    lock_it(&mut s, "e1");
    assert_eq!(refuse(&mut s, "element.flip", json!({"axis": "vertical"})).code, "locked");
}

// ---- element.lock / unlock_all ----

#[test]
fn lock_toggles_locks_the_whole_group_and_drops_the_selection() {
    let mut s = shapes();
    group_of(&mut s, &["e1", "e2"], "g1");
    s.tab_mut().observed.selection = [eid("e1"), eid("e2")].into();
    let out = run(&mut s, "element.lock", json!({}));
    assert_eq!(out["locked"], true);
    assert_eq!(out["ids"].as_array().unwrap().len(), 2);
    assert!(s.element(&eid("e1")).unwrap().locked && s.element(&eid("e2")).unwrap().locked);
    assert!(sel(&s).is_empty(), "locked elements leave the selection");
    s.undo_steps(1).unwrap();
    assert!(!s.element(&eid("e1")).unwrap().locked);
    assert_eq!(sel(&s), ["e1", "e2"], "undo restores the selection too");
}

#[test]
fn lock_one_member_of_a_group_locks_the_group_and_unlock_by_id() {
    let mut s = shapes();
    group_of(&mut s, &["e1", "e2"], "g1");
    run(&mut s, "element.lock", json!({"ids": ["e2"], "locked": true}));
    assert!(s.element(&eid("e1")).unwrap().locked, "a group stays one unit");
    let out = run(&mut s, "element.lock", json!({"ids": ["e2"], "locked": true}));
    assert_eq!(out["changed"], false, "locking a locked element is Ok with changed=false");
    assert_eq!(s.tab().doc.undo_depth(), 1, "and pushes no undo step");
    run(&mut s, "element.lock", json!({"ids": ["e1"]}));
    assert!(!s.element(&eid("e2")).unwrap().locked, "omitting `locked` toggles: all locked, so unlock");
    assert_eq!(refuse(&mut s, "element.lock", json!({"ids": ["zzz"]})).code, "not_found");
}

#[test]
fn unlock_all_is_greyed_out_when_nothing_is_locked_and_selects_what_it_unlocks() {
    let mut s = shapes();
    let r = s.availability("element.unlock_all").unwrap_err();
    assert_eq!(r.code, "nothing_to_do");
    assert_eq!(r.message, "Nothing to unlock: no element is locked.");
    assert_eq!(s.execute("element.unlock_all", json!({})).unwrap_err().message, r.message);
    lock_it(&mut s, "e2");
    lock_it(&mut s, "e3");
    assert!(s.availability("element.unlock_all").is_ok());
    let out = run(&mut s, "element.unlock_all", json!({}));
    assert_eq!(out["ids"].as_array().unwrap().len(), 2);
    assert!(!s.element(&eid("e2")).unwrap().locked && !s.element(&eid("e3")).unwrap().locked);
    assert_eq!(sel(&s), ["e2", "e3"]);
    s.undo_steps(1).unwrap();
    assert!(s.element(&eid("e2")).unwrap().locked);
    assert_eq!(sel(&s), ["e1"]);
}

#[test]
fn unlock_all_with_ids_and_without_selecting() {
    let mut s = shapes();
    lock_it(&mut s, "e2");
    lock_it(&mut s, "e3");
    run(&mut s, "element.unlock_all", json!({"ids": ["e3"], "select": false}));
    assert!(s.element(&eid("e2")).unwrap().locked && !s.element(&eid("e3")).unwrap().locked);
    assert_eq!(sel(&s), ["e1"]);
    assert_eq!(refuse(&mut s, "element.unlock_all", json!({"ids": ["nope"]})).code, "not_found");
}

#[test]
fn unlock_all_with_ids_that_are_not_locked_has_nothing_to_do() {
    let mut s = shapes();
    lock_it(&mut s, "e2");
    let r = refuse(&mut s, "element.unlock_all", json!({"ids": ["e1"]}));
    assert_eq!((r.code, r.message.as_str()), ("nothing_to_do", "Nothing to unlock: none of the given elements is locked."));
    assert!(s.element(&eid("e2")).unwrap().locked, "the refusal changed nothing");
    let out = run(&mut s, "element.unlock_all", json!({"ids": ["e2"]}));
    assert_eq!(out["ids"], json!(["e2"]));
    assert_eq!(sel(&s), ["e2"], "the unlocked element is selected, as Excalidraw's menu leaves it");
    s.undo_steps(1).unwrap();
    assert!(s.element(&eid("e2")).unwrap().locked);
}

// ---- element.erase ----

#[test]
fn erase_deletes_what_the_path_touches_in_one_undo_step() {
    let mut s = shapes();
    // e1 and e2 are crossed by a stroke along y=55 from x=-10 to x=250; e3 is far away.
    let out = run(&mut s, "element.erase", json!({"path": [[-10, 55], [250, 55]], "radius": 5, "units": "world"}));
    let mut hit: Vec<String> = serde_json::from_value(out["hit"].clone()).unwrap();
    hit.sort();
    assert_eq!(hit, ["e1", "e2"]);
    assert!(s.element(&eid("e3")).is_some());
    assert_eq!(out["deleted"].as_array().unwrap().len(), 2);
    assert_eq!(s.tab().doc.undo_depth(), 1);
    assert!(sel(&s).is_empty());
    s.undo_steps(1).unwrap();
    assert!(s.element(&eid("e1")).is_some() && s.element(&eid("e2")).is_some());
}

#[test]
fn erase_nothing_is_ok_unchanged_and_screen_units_follow_the_zoom() {
    let mut s = shapes();
    let out = run(&mut s, "element.erase", json!({"path": [[1000, 1000]]}));
    assert_eq!(out["changed"], false);
    assert_eq!(out["hit"], json!([]));
    assert_eq!(s.tab().doc.undo_depth(), 0);
    // Zoom 2: 10 screen px are 5 world units. A point 8 world units outside the edge misses.
    run(&mut s, "view.set", json!({"zoom": 2}));
    let out = run(&mut s, "element.erase", json!({"path": [[-8, 30]], "radius": 10}));
    assert_eq!(out["hit"], json!([]));
    let out = run(&mut s, "element.erase", json!({"path": [[-8, 30]], "radius": 10, "units": "world"}));
    assert_eq!(out["hit"], json!(["e1"]));
}

#[test]
fn erase_skips_locked_elements_and_validates_the_path() {
    let mut s = shapes();
    lock_it(&mut s, "e1");
    let out = run(&mut s, "element.erase", json!({"path": [[0, 0], [100, 60]], "units": "world"}));
    assert_eq!(out["hit"], json!([]));
    assert!(s.element(&eid("e1")).is_some());
    assert_eq!(refuse(&mut s, "element.erase", json!({"path": []})).code, "bad_args");
    assert_eq!(refuse(&mut s, "element.erase", json!({"path": [[0, 0]], "radius": 0})).code, "bad_args");
    assert_eq!(refuse(&mut s, "element.erase", json!({"path": [[0, 1e300]]})).code, "bad_args");
    let long: Vec<[f64; 2]> = (0..10_001).map(|i| [f64::from(i), 0.0]).collect();
    let r = refuse(&mut s, "element.erase", json!({"path": long}));
    assert_eq!(r.code, "limit_exceeded");
}

// ---- batches and the selection round trip ----

#[test]
fn a_batch_of_edits_is_one_undo_step_and_restores_the_selection() {
    use crate::batch::{BatchOpts, BatchStep};
    let mut s = shapes();
    let out = s.execute_batch(
        Origin::Agent,
        &[
            BatchStep::new("element.duplicate", json!({"dx": 300, "dy": 0})),
            BatchStep::new("element.nudge", json!({"dir": "down"})),
            BatchStep::new("element.delete", json!({"ids": ["e2"]})),
        ],
        &BatchOpts::default(),
    );
    assert!(out.failed.is_none(), "{:?}", out.failed);
    assert_eq!(s.tab().doc.undo_depth(), 1);
    assert!(s.element(&eid("e2")).is_none());
    s.undo_steps(1).unwrap();
    assert!(s.element(&eid("e2")).is_some());
    assert_eq!(s.store().len(), 3);
    assert_eq!(sel(&s), ["e1"]);
}

#[test]
fn a_failing_step_rolls_back_the_batch_including_the_selection() {
    use crate::batch::{BatchOpts, BatchStep};
    let mut s = shapes();
    lock_it(&mut s, "e3");
    let before = s.fingerprint();
    let out = s.execute_batch(
        Origin::Agent,
        &[
            BatchStep::new("element.duplicate", json!({})),
            BatchStep::new("element.delete", json!({"ids": ["e3"]})),
        ],
        &BatchOpts::default(),
    );
    let f = out.failed.expect("the locked delete fails");
    assert_eq!((f.index, f.refusal.code), (1, "locked"));
    assert!(out.rolled_back);
    assert_eq!(s.fingerprint(), before);
}

#[test]
fn the_selection_before_and_after_a_step_is_what_undo_and_redo_restore() {
    let mut s = shapes();
    // The selection changes alone make no undo step (a selection command only edits the observed state).
    s.tab_mut().observed.selection = [eid("e2")].into();
    assert_eq!(s.tab().doc.undo_depth(), 0);
    run(&mut s, "element.duplicate", json!({}));
    let copy = sel(&s);
    assert_ne!(copy, ["e2"]);
    s.tab_mut().observed.selection = [eid("e3")].into();
    s.undo_steps(1).unwrap();
    assert_eq!(sel(&s), ["e2"], "undo restores the selection from before the step");
    s.redo_steps(1).unwrap();
    assert_eq!(sel(&s), copy, "redo restores the selection from after the step");
}
