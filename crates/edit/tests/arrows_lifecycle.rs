#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The rules for transforms, copies, loads and flips: who stays bound, who is freed.

mod arrows_common;

use std::collections::HashMap;

use arrows_common::*;
use drafftink_edit::binding::{
    End, can_move_with, heal_bindings, mirror_fixed_points, reanchor_to_outline, remap_bindings, stale_arrows,
    unbind_detached, unbind_end, update_bound_arrows, update_bound_arrows_indexed,
};
use drafftink_scene::{BindMode, SceneRead, SceneStore, SceneTx, validate_scene};

fn two_boxes_and_an_arrow(
    s: &mut Scene,
) -> (drafftink_scene::ElementId, drafftink_scene::ElementId, drafftink_scene::ElementId) {
    let a = s.rect("a", 0.0, 0.0, 100.0, 100.0);
    let b = s.rect("b", 400.0, 0.0, 100.0, 100.0);
    let arrow = s.arrow("arrow", &[(106.0, 50.0), (394.0, 50.0)]);
    s.bind(&arrow, End::Start, "a", BindMode::Orbit, [0.5, 0.5]);
    s.bind(&arrow, End::End, "b", BindMode::Orbit, [0.5, 0.5]);
    s.solve(&[&a]);
    (a, b, arrow)
}

#[test]
fn moving_an_arrow_with_both_shapes_keeps_both_bindings_and_its_shape() {
    let mut s = Scene::new();
    let (a, b, arrow) = two_boxes_and_an_arrow(&mut s);
    let before = s.world(&arrow);
    let mut tx = s.store.begin();
    for e in [&a, &b, &arrow] {
        tx.update(e, |el| {
            el.transform.x += 30.0;
            el.transform.y += 40.0;
        })
        .unwrap();
    }
    let set = [a.clone(), b.clone(), arrow.clone()];
    assert!(unbind_detached(&mut tx, &set).unwrap().is_empty());
    update_bound_arrows(&mut tx, &set).unwrap();
    tx.commit().unwrap();
    assert!(s.binding(&arrow, End::Start).is_some() && s.binding(&arrow, End::End).is_some());
    for (b4, after) in before.iter().zip(s.world(&arrow)) {
        assert_close(after, *b4 + v(30.0, 40.0));
    }
}

#[test]
fn moving_an_arrow_alone_frees_both_ends_where_they_are() {
    let mut s = Scene::new();
    let (_a, _b, arrow) = two_boxes_and_an_arrow(&mut s);
    let mut tx = s.store.begin();
    tx.update(&arrow, |el| el.transform.y += 80.0).unwrap();
    let freed = unbind_detached(&mut tx, std::slice::from_ref(&arrow)).unwrap();
    update_bound_arrows(&mut tx, std::slice::from_ref(&arrow)).unwrap();
    tx.commit().unwrap();
    assert_eq!(freed, vec![arrow.clone()]);
    assert!(s.binding(&arrow, End::Start).is_none() && s.binding(&arrow, End::End).is_none());
    assert_near(s.start(&arrow), p(106.0, 130.0));
}

#[test]
fn rotating_an_arrow_alone_frees_both_ends_and_keeps_its_points() {
    let mut s = Scene::new();
    let (_a, _b, arrow) = two_boxes_and_an_arrow(&mut s);
    let before = s.world(&arrow);
    let mut tx = s.store.begin();
    tx.update(&arrow, |el| el.transform.angle = 0.5).unwrap();
    // The rotation moved the arrow's points in the world; the rule frees the ends, and nothing re-aims them.
    let moved = {
        let e = tx.get(&arrow).unwrap();
        drafftink_edit::linear::world_points(e).unwrap()
    };
    let freed = unbind_detached(&mut tx, std::slice::from_ref(&arrow)).unwrap();
    let solved = update_bound_arrows(&mut tx, std::slice::from_ref(&arrow)).unwrap();
    tx.commit().unwrap();
    assert_eq!(freed, vec![arrow.clone()]);
    assert!(solved.is_empty());
    assert_eq!(s.world(&arrow), moved);
    assert_ne!(s.world(&arrow), before);
}

#[test]
fn with_one_of_two_shapes_selected_only_the_other_end_is_freed() {
    let mut s = Scene::new();
    let (a, _b, arrow) = two_boxes_and_an_arrow(&mut s);
    let mut tx = s.store.begin();
    let set = [a.clone(), arrow.clone()];
    let freed = unbind_detached(&mut tx, &set).unwrap();
    tx.commit().unwrap();
    assert_eq!(freed, vec![arrow.clone()]);
    assert!(s.binding(&arrow, End::Start).is_some());
    assert!(s.binding(&arrow, End::End).is_none());
}

#[test]
fn a_bound_elbow_arrow_cannot_be_dragged_on_its_own() {
    let mut s = Scene::new();
    s.rect("r1", 0.0, 0.0, 100.0, 100.0);
    s.rect("r2", 300.0, 200.0, 100.0, 100.0);
    let a = s.elbow_between("a", p(105.0, 50.0), p(350.0, 195.0));
    assert!(!can_move_with(&s.store, &a, std::slice::from_ref(&a)));
    assert!(can_move_with(&s.store, &a, &[a.clone(), id("r1"), id("r2")]));
    let free = s.arrow("free", &[(0.0, 0.0), (5.0, 5.0)]);
    assert!(can_move_with(&s.store, &free, std::slice::from_ref(&free)));
}

#[test]
fn unbind_end_is_total() {
    let mut s = Scene::new();
    let (a, _b, arrow) = two_boxes_and_an_arrow(&mut s);
    let mut tx = s.store.begin();
    assert!(unbind_end(&mut tx, &arrow, End::Start).unwrap());
    assert!(!unbind_end(&mut tx, &arrow, End::Start).unwrap(), "already free");
    assert!(!unbind_end(&mut tx, &a, End::Start).unwrap(), "not an arrow: nothing to free");
    assert!(unbind_end(&mut tx, &id("missing"), End::End).is_err());
}

#[test]
fn copies_are_bound_to_copies_only() {
    let mut s = Scene::new();
    let (_a, _b, arrow) = two_boxes_and_an_arrow(&mut s);
    // Pretend the arrow and one shape were duplicated.
    let copy = s.arrow("arrow2", &[(106.0, 150.0), (394.0, 150.0)]);
    s.bind(&copy, End::Start, "a", BindMode::Orbit, [0.3, 0.6]);
    s.bind(&copy, End::End, "b", BindMode::Orbit, [0.5, 0.5]);
    let a2 = s.rect("a2", 0.0, 100.0, 100.0, 100.0);
    let map: HashMap<_, _> = [(id("a"), a2.clone()), (arrow.clone(), copy.clone())].into_iter().collect();
    let mut tx = s.store.begin();
    let changed = remap_bindings(&mut tx, std::slice::from_ref(&copy), &map).unwrap();
    tx.commit().unwrap();
    assert_eq!(changed, vec![copy.clone()]);
    let st = s.binding(&copy, End::Start).unwrap();
    assert_eq!(st.target, a2);
    assert_eq!(st.fixed_point, [0.3, 0.6], "mode and ratio are kept");
    assert!(s.binding(&copy, End::End).is_none(), "b was not copied");
}

#[test]
fn heal_removes_dangling_self_and_non_bindable_bindings_and_is_idempotent() {
    let mut s = Scene::new();
    let t = s.rect("t", 0.0, 0.0, 100.0, 100.0);
    let other_arrow = s.arrow("oa", &[(0.0, 0.0), (10.0, 10.0)]);
    let a = s.arrow("a", &[(200.0, 0.0), (300.0, 0.0)]);
    s.bind(&a, End::Start, "ghost", BindMode::Orbit, [0.5, 0.5]);
    s.bind(&a, End::End, "oa", BindMode::Orbit, [0.5, 0.5]);
    let b = s.arrow("b", &[(200.0, 50.0), (300.0, 50.0)]);
    s.bind(&b, End::End, "t", BindMode::Inside, [3.0, 4.0]);
    let _ = (t, other_arrow);
    let mut tx = s.store.begin();
    let changed = heal_bindings(&mut tx, None).unwrap();
    assert_eq!(changed, vec![a.clone()]);
    assert!(heal_bindings(&mut tx, None).unwrap().is_empty());
    tx.commit().unwrap();
    assert!(s.binding(&a, End::Start).is_none() && s.binding(&a, End::End).is_none());
    assert_eq!(s.binding(&b, End::End).unwrap().fixed_point, [3.0, 4.0], "valid ratios outside the box are kept");
    let report = validate_scene(&s.store);
    assert!(!report.has_errors(), "{report:?}");
}

#[test]
fn heal_keeps_a_binding_to_a_deleted_shape() {
    let mut s = Scene::new();
    let t = s.rect("t", 0.0, 0.0, 100.0, 100.0);
    let a = s.arrow("a", &[(200.0, 0.0), (300.0, 0.0)]);
    s.bind(&a, End::End, "t", BindMode::Orbit, [0.5, 0.5]);
    s.tombstone(&t, true);
    let mut tx = s.store.begin();
    assert!(heal_bindings(&mut tx, None).unwrap().is_empty());
}

#[test]
fn flipping_a_pair_mirrors_the_fixed_points_so_a_later_move_keeps_the_side() {
    let mut s = Scene::new();
    let t = s.rect("t", 0.0, 0.0, 100.0, 100.0);
    let a = s.arrow("a", &[(300.0, 300.0), (25.0, 50.0)]);
    s.bind(&a, End::End, "t", BindMode::Inside, [0.25, 0.5]);
    let mut tx = s.store.begin();
    let changed = mirror_fixed_points(&mut tx, &[t.clone(), a.clone()], true, false).unwrap();
    update_bound_arrows(&mut tx, &[t.clone(), a.clone()]).unwrap();
    tx.commit().unwrap();
    assert_eq!(changed, vec![a.clone()]);
    let fp = s.binding(&a, End::End).unwrap().fixed_point;
    assert!((fp[0] - 0.75).abs() < 1e-9);
    assert_near(s.end(&a), p(75.0, 50.0));
    // An arrow whose target is not in the set is not touched.
    let mut tx = s.store.begin();
    assert!(mirror_fixed_points(&mut tx, std::slice::from_ref(&a), true, true).unwrap().is_empty());
}

#[test]
fn an_orbit_focus_that_falls_off_a_new_outline_is_pulled_back_onto_it() {
    let mut s = Scene::new();
    let t = s.rect("t", 0.0, 0.0, 100.0, 100.0);
    let a = s.arrow("a", &[(300.0, 300.0), (150.0, 150.0)]);
    // A focus in the corner, which a rectangle contains and an ellipse does not.
    s.bind(&a, End::End, "t", BindMode::Orbit, [0.97, 0.97]);
    s.edit(&t, |e| e.kind = drafftink_scene::ElementKind::Ellipse);
    let mut tx = s.store.begin();
    let changed = reanchor_to_outline(&mut tx, std::slice::from_ref(&t)).unwrap();
    update_bound_arrows(&mut tx, std::slice::from_ref(&t)).unwrap();
    tx.commit().unwrap();
    assert_eq!(changed, vec![a.clone()]);
    assert!(orbit_error(&s, &t, s.end(&a), 6.0) < 1e-6);
}

#[test]
fn the_indexed_solver_matches_the_scanning_one() {
    let mut s = Scene::new();
    let (a, _b, arrow) = two_boxes_and_an_arrow(&mut s);
    s.edit(&a, |e| e.transform.y += 70.0);
    let derived = s.store.derived().clone();
    let mut tx = s.store.begin();
    let changed = update_bound_arrows_indexed(&mut tx, std::slice::from_ref(&a), &derived).unwrap();
    tx.commit().unwrap();
    assert_eq!(changed, vec![arrow.clone()]);
    assert!(stale_arrows(&s.store).is_empty());
}

#[test]
fn solving_twice_changes_nothing_the_second_time() {
    let mut s = Scene::new();
    let (a, _b, _arrow) = two_boxes_and_an_arrow(&mut s);
    s.edit(&a, |e| e.transform.y += 70.0);
    assert_eq!(s.solve(&[&a]).len(), 1);
    assert!(s.solve(&[&a]).is_empty());
}

#[test]
fn nothing_changed_means_no_work() {
    let mut s = Scene::new();
    let (_a, _b, _arrow) = two_boxes_and_an_arrow(&mut s);
    let mut tx = s.store.begin();
    assert!(update_bound_arrows(&mut tx, &[]).unwrap().is_empty());
    assert!(update_bound_arrows(&mut tx, &[id("nope")]).unwrap().is_empty());
}
