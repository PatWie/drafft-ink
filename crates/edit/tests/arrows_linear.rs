#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Editing the points, heads and path type of lines and arrows.

mod arrows_common;

use arrows_common::*;
use drafftink_edit::binding::{End, stale_arrows};
use drafftink_edit::linear::{self, LinearError};
use drafftink_scene::{BindMode, Head, Kind, PathType, SceneRead, SceneStore, SceneTx, validate_element};

fn check_valid(s: &Scene, eid: &drafftink_scene::ElementId) {
    let e = s.get(eid);
    assert_eq!(validate_element(&e), Ok(()), "{e:?}");
}

fn three_points(s: &mut Scene) -> drafftink_scene::ElementId {
    s.arrow("a", &[(0.0, 0.0), (100.0, 50.0), (200.0, 0.0)])
}

#[test]
fn moving_points_keeps_the_box_and_world_positions_right() {
    let mut s = Scene::new();
    let a = three_points(&mut s);
    let mut tx = s.store.begin();
    linear::move_points(&mut tx, &a, &[(1, p(100.0, -80.0)), (0, p(-30.0, 10.0))]).unwrap();
    tx.commit().unwrap();
    assert_eq!(s.world(&a), vec![p(-30.0, 10.0), p(100.0, -80.0), p(200.0, 0.0)]);
    check_valid(&s, &a);
    let e = s.get(&a);
    assert!((e.transform.x + 30.0).abs() < 1e-9 && (e.transform.y + 80.0).abs() < 1e-9);
}

#[test]
fn a_rotated_arrow_keeps_its_other_points_in_place_when_one_moves() {
    let mut s = Scene::new();
    let a = three_points(&mut s);
    s.edit(&a, |e| e.transform.angle = 0.6);
    let before = s.world(&a);
    let mut tx = s.store.begin();
    linear::move_points(&mut tx, &a, &[(1, p(150.0, 90.0))]).unwrap();
    tx.commit().unwrap();
    let after = s.world(&a);
    assert_close(after[0], before[0]);
    assert_close(after[2], before[2]);
    assert_close(after[1], p(150.0, 90.0));
    check_valid(&s, &a);
}

#[test]
fn moving_an_end_of_a_bound_arrow_frees_that_end() {
    let mut s = Scene::new();
    s.rect("t", 0.0, 0.0, 100.0, 100.0);
    let a = s.arrow("a", &[(300.0, 50.0), (106.0, 50.0)]);
    s.bind(&a, End::End, "t", BindMode::Orbit, [0.5, 0.5]);
    s.bind(&a, End::Start, "t", BindMode::Orbit, [0.5, 0.5]);
    let mut tx = s.store.begin();
    linear::move_points(&mut tx, &a, &[(1, p(400.0, 400.0))]).unwrap();
    tx.commit().unwrap();
    assert!(s.binding(&a, End::End).is_none());
    assert!(s.binding(&a, End::Start).is_some());
}

#[test]
fn moving_a_middle_point_re_aims_an_orbit_end() {
    let mut s = Scene::new();
    let t = s.rect("t", 0.0, 0.0, 100.0, 100.0);
    let a = s.arrow("a", &[(400.0, 300.0), (300.0, 50.0), (200.0, 50.0)]);
    s.bind(&a, End::End, "t", BindMode::Orbit, [0.5, 0.5]);
    s.solve(&[&t]);
    let e0 = s.end(&a);
    let mut tx = s.store.begin();
    linear::move_points(&mut tx, &a, &[(1, p(300.0, -200.0))]).unwrap();
    tx.commit().unwrap();
    assert!(orbit_error(&s, &t, s.end(&a), 6.0) < 1e-6);
    assert!((s.end(&a) - e0).hypot() > 1.0, "the end follows the new direction of the last segment");
    assert!(stale_arrows(&s.store).is_empty());
}

#[test]
fn add_and_insert_points() {
    let mut s = Scene::new();
    let a = s.arrow("a", &[(0.0, 0.0), (100.0, 0.0)]);
    let mut tx = s.store.begin();
    let i = linear::insert_midpoint(&mut tx, &a, 1, p(50.0, 40.0)).unwrap();
    assert_eq!(i, 1);
    linear::add_point(&mut tx, &a, 3, p(200.0, 40.0)).unwrap();
    linear::add_point(&mut tx, &a, 0, p(-50.0, -50.0)).unwrap();
    tx.commit().unwrap();
    assert_eq!(s.world(&a), vec![p(-50.0, -50.0), p(0.0, 0.0), p(50.0, 40.0), p(100.0, 0.0), p(200.0, 40.0)]);
    check_valid(&s, &a);
}

#[test]
fn a_closed_line_has_one_more_segment_the_one_that_closes_it() {
    let mut s = Scene::new();
    let line = s.line("l", &[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0)]);
    // An open line has segments 1 and 2 (between points 0-1 and 1-2); segment 3 does not exist.
    let mut tx = s.store.begin();
    assert_eq!(
        linear::insert_midpoint(&mut tx, &line, 3, p(0.0, 50.0)),
        Err(LinearError::NoSuchPoint { index: 3, len: 3 })
    );
    drop(tx);
    s.edit(&line, |e| {
        if let drafftink_scene::ElementKind::Line(d) = &mut e.kind {
            d.closed = true;
        }
    });
    // Closed, segment 3 runs from the last point back to the first. The new point comes last.
    let mut tx = s.store.begin();
    assert_eq!(linear::insert_midpoint(&mut tx, &line, 3, p(0.0, 50.0)).unwrap(), 3);
    tx.commit().unwrap();
    assert_eq!(s.world(&line), vec![p(0.0, 0.0), p(100.0, 0.0), p(100.0, 100.0), p(0.0, 50.0)]);
    check_valid(&s, &line);
    // Four points now: segment 5 is still out of range.
    let mut tx = s.store.begin();
    assert_eq!(
        linear::insert_midpoint(&mut tx, &line, 5, p(0.0, 0.0)),
        Err(LinearError::NoSuchPoint { index: 5, len: 4 })
    );
}

#[test]
fn adding_a_new_end_frees_the_binding_of_the_old_end() {
    let mut s = Scene::new();
    s.rect("t", 0.0, 0.0, 100.0, 100.0);
    let a = s.arrow("a", &[(300.0, 50.0), (106.0, 50.0)]);
    s.bind(&a, End::End, "t", BindMode::Orbit, [0.5, 0.5]);
    let mut tx = s.store.begin();
    linear::add_point(&mut tx, &a, 2, p(106.0, 200.0)).unwrap();
    tx.commit().unwrap();
    assert!(s.binding(&a, End::End).is_none());
    assert_eq!(s.world(&a).len(), 3);
}

#[test]
fn deleting_points_keeps_at_least_two() {
    let mut s = Scene::new();
    let a = three_points(&mut s);
    let mut tx = s.store.begin();
    let d = linear::delete_points(&mut tx, &a, &[1, 1]).unwrap();
    assert_eq!(d.removed, 1);
    let err = linear::delete_points(&mut tx, &a, &[0]).unwrap_err();
    assert_eq!(err, LinearError::TooFewPoints { kind: Kind::Arrow, min: 2, have: 1 });
    assert!(err.to_string().contains("delete the element"));
    tx.commit().unwrap();
    assert_eq!(s.world(&a), vec![p(0.0, 0.0), p(200.0, 0.0)]);
    check_valid(&s, &a);
}

#[test]
fn deleting_an_end_point_frees_the_binding_that_belonged_to_it() {
    let mut s = Scene::new();
    s.rect("t", 0.0, 0.0, 100.0, 100.0);
    let a = s.arrow("a", &[(300.0, 300.0), (300.0, 50.0), (106.0, 50.0)]);
    s.bind(&a, End::End, "t", BindMode::Orbit, [0.5, 0.5]);
    let mut tx = s.store.begin();
    linear::delete_points(&mut tx, &a, &[2]).unwrap();
    tx.commit().unwrap();
    assert!(s.binding(&a, End::End).is_none());
    assert_eq!(s.world(&a), vec![p(300.0, 300.0), p(300.0, 50.0)]);
}

#[test]
fn bad_indices_and_values_are_refused() {
    let mut s = Scene::new();
    let a = three_points(&mut s);
    let t = s.rect("t", 0.0, 0.0, 10.0, 10.0);
    let mut tx = s.store.begin();
    assert_eq!(
        linear::move_points(&mut tx, &a, &[(3, p(0.0, 0.0))]),
        Err(LinearError::NoSuchPoint { index: 3, len: 3 })
    );
    assert_eq!(linear::move_points(&mut tx, &a, &[(0, p(f64::NAN, 0.0))]), Err(LinearError::NotFinite));
    assert_eq!(linear::add_point(&mut tx, &a, 9, p(0.0, 0.0)), Err(LinearError::NoSuchPoint { index: 9, len: 3 }));
    assert_eq!(
        linear::insert_midpoint(&mut tx, &a, 0, p(0.0, 0.0)),
        Err(LinearError::NoSuchPoint { index: 0, len: 3 })
    );
    assert!(matches!(linear::delete_points(&mut tx, &a, &[7]), Err(LinearError::NoSuchPoint { .. })));
    assert_eq!(linear::move_points(&mut tx, &t, &[(0, p(0.0, 0.0))]), Err(LinearError::NotLinear(t.clone())));
    assert!(matches!(linear::move_points(&mut tx, &id("zz"), &[]), Err(LinearError::Missing(_))));
}

#[test]
fn heads_are_set_per_end_and_swapped() {
    let mut s = Scene::new();
    let a = three_points(&mut s);
    let mut tx = s.store.begin();
    linear::set_head(&mut tx, &a, End::End, Head::Triangle).unwrap();
    linear::set_head(&mut tx, &a, End::Start, Head::Circle).unwrap();
    linear::swap_heads(&mut tx, &a).unwrap();
    tx.commit().unwrap();
    let d = s.get(&a).linear_data().unwrap().clone();
    assert_eq!((d.start_head, d.end_head), (Head::Triangle, Head::Circle));
}

#[test]
fn straight_curved_and_elbow_conversion() {
    let mut s = Scene::new();
    let a = s.arrow("a", &[(0.0, 0.0), (100.0, 50.0), (200.0, 80.0)]);
    let mut tx = s.store.begin();
    linear::set_path(&mut tx, &a, PathType::Round).unwrap();
    assert_eq!(linear::world_points(tx.get(&a).unwrap()).unwrap().len(), 3);
    linear::set_path(&mut tx, &a, PathType::Elbow).unwrap();
    tx.commit().unwrap();
    // Elbow keeps only the ends.
    let w = s.world(&a);
    assert_close(w[0], p(0.0, 0.0));
    assert_close(*w.last().unwrap(), p(200.0, 80.0));
    assert_clean_orthogonal(&w);
    assert!(w.len() >= 3, "{w:?}");
    check_valid(&s, &a);
    let mut tx = s.store.begin();
    assert_eq!(linear::move_points(&mut tx, &a, &[(1, p(1.0, 1.0))]), Err(LinearError::ElbowInteriorPoint));
    assert_eq!(linear::add_point(&mut tx, &a, 1, p(1.0, 1.0)), Err(LinearError::ElbowInteriorPoint));
    assert_eq!(linear::delete_points(&mut tx, &a, &[1]), Err(LinearError::ElbowInteriorPoint));
}

#[test]
fn a_closed_line_cannot_become_an_elbow() {
    use drafftink_scene::{Element, ElementKind, Kind};
    let mut s = Scene::new();
    let z = drafftink_scene::ZIndex::between(None, None).unwrap();
    let mut e = Element::linear(id("l"), Kind::Line, z, &[p(0.0, 0.0), p(100.0, 0.0), p(50.0, 80.0)]).unwrap();
    if let ElementKind::Line(d) = &mut e.kind {
        d.closed = true;
    }
    let l = s.add(e);
    let mut tx = s.store.begin();
    assert_eq!(linear::set_path(&mut tx, &l, PathType::Elbow), Err(LinearError::ClosedElbow));
    // A closed line keeps 3 points.
    assert!(matches!(linear::delete_points(&mut tx, &l, &[0]), Err(LinearError::TooFewPoints { min: 3, .. })));
}

#[test]
fn midpoint_handles_hide_on_short_segments_and_sit_at_half_length_on_curves() {
    let mut s = Scene::new();
    let a = s.arrow("a", &[(0.0, 0.0), (100.0, 0.0), (110.0, 0.0), (400.0, 100.0)]);
    let e = s.get(&a);
    let h = linear::midpoint_handles(&e, 1.0);
    assert_eq!(h.iter().map(|m| m.segment).collect::<Vec<_>>(), vec![1, 3], "the 10-unit segment has no handle");
    assert_close(h[0].at, p(50.0, 0.0));
    // Zoomed out, the long ones are short on screen too.
    assert!(linear::midpoint_handles(&e, 0.1).is_empty());

    s.set_path(&a, PathType::Round);
    // The spline overshoots between uneven neighbours, so the short chord has a long arc and keeps its handle.
    let curved = linear::midpoint_handles(&s.get(&a), 1.0);
    assert_eq!(curved.len(), 3);
    // The handle lies on the curve, not on the chord.
    let d = linear::distance_to(&s.get(&a), curved[2].at).unwrap();
    assert!(d < 1e-2, "{d}");
}

#[test]
fn point_under_the_cursor_prefers_the_last_point() {
    let mut s = Scene::new();
    let a = s.arrow("a", &[(0.0, 0.0), (100.0, 0.0), (100.0, 0.0)]);
    let e = s.get(&a);
    assert_eq!(linear::point_at(&e, p(100.0, 0.0), 1.0), Some(2));
    assert_eq!(linear::point_at(&e, p(105.0, 0.0), 1.0), Some(2));
    assert_eq!(linear::point_at(&e, p(105.0, 0.0), 0.2), Some(2), "the hit radius is in pixels");
    assert_eq!(linear::point_at(&e, p(300.0, 0.0), 1.0), None);
    assert_eq!(linear::point_at(&e, p(15.0, 0.0), 1.0), None);
}
