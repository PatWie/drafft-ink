#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Binding maintenance: what happens to bound arrows when shapes move, resize, rotate, are deleted or restored.

mod arrows_common;

use arrows_common::*;
use drafftink_edit::binding::{End, bound_end_position, stale_arrows};
use drafftink_scene::{BindMode, Head};

const GAP: f64 = 6.0; // 5 + stroke 2 / 2

#[test]
fn orbit_end_sits_on_the_outline_plus_gap_and_follows_a_move() {
    let mut s = Scene::new();
    let t = s.rect("t", 0.0, 0.0, 100.0, 100.0);
    let a = s.arrow("a", &[(300.0, 50.0), (150.0, 50.0)]);
    s.bind(&a, End::End, "t", BindMode::Orbit, [0.5, 0.5]);
    assert_eq!(s.solve(&[&t]), vec![a.clone()]);
    let e = s.end(&a);
    assert!((signed(&s, &t, e) - GAP).abs() < 1e-6, "{e:?}");
    assert!((e.y - 50.0).abs() < 0.05);
    // The other end did not move.
    assert_close(s.start(&a), p(300.0, 50.0));

    s.move_by(&t, 20.0, 100.0);
    let e2 = s.end(&a);
    assert!((signed(&s, &t, e2) - GAP).abs() < 1e-6, "{e2:?}");
    assert!(stale_arrows(&s.store).is_empty());
}

#[test]
fn inside_end_keeps_its_ratio_through_move_resize_and_rotate() {
    let mut s = Scene::new();
    let t = s.rect("t", 0.0, 0.0, 100.0, 100.0);
    let a = s.arrow("a", &[(300.0, 300.0), (25.0, 75.0)]);
    s.bind(&a, End::End, "t", BindMode::Inside, [0.25, 0.75]);
    s.solve(&[&t]);
    assert_close(s.end(&a), p(25.0, 75.0));

    s.move_by(&t, 10.0, -20.0);
    assert_close(s.end(&a), p(35.0, 55.0));

    s.resize(&t, 10.0, -20.0, 200.0, 40.0);
    assert_close(s.end(&a), p(10.0 + 50.0, -20.0 + 30.0));

    // Rotate a quarter turn about the centre (110, 0): the unrotated point (60, 10) turns to (100, -50)...
    s.rotate(&t, std::f64::consts::FRAC_PI_2);
    let c = p(110.0, 0.0);
    let local = p(60.0, 10.0) - c;
    let want = c + v(-local.y, local.x);
    assert_close(s.end(&a), want.to_vec2().to_point());
    assert!(stale_arrows(&s.store).is_empty());
}

#[test]
fn orbit_end_stays_on_the_outline_when_the_target_is_resized_or_rotated() {
    let mut s = Scene::new();
    let t = s.ellipse("t", 0.0, 0.0, 120.0, 60.0);
    let a = s.arrow("a", &[(400.0, 30.0), (130.0, 30.0)]);
    s.bind(&a, End::End, "t", BindMode::Orbit, [0.6, 0.5]);
    s.solve(&[&t]);
    assert!(orbit_error(&s, &t, s.end(&a), GAP) < 1e-6);
    s.resize(&t, 0.0, 0.0, 240.0, 120.0);
    assert!(orbit_error(&s, &t, s.end(&a), GAP) < 1e-6, "resized: {:?}", s.end(&a));
    s.rotate(&t, 0.8);
    assert!(orbit_error(&s, &t, s.end(&a), GAP) < 1e-6, "rotated: {:?}", s.end(&a));
}

#[test]
fn both_ends_bound_follow_their_own_targets() {
    let mut s = Scene::new();
    let a_t = s.rect("ta", 0.0, 0.0, 100.0, 100.0);
    let b_t = s.rect("tb", 400.0, 0.0, 100.0, 100.0);
    let a = s.arrow("a", &[(106.0, 50.0), (394.0, 50.0)]);
    s.bind(&a, End::Start, "ta", BindMode::Orbit, [0.5, 0.5]);
    s.bind(&a, End::End, "tb", BindMode::Orbit, [0.5, 0.5]);
    s.solve(&[&a_t]);
    s.move_by(&b_t, 0.0, 200.0);
    assert!((signed(&s, &b_t, s.end(&a)) - GAP).abs() < 1e-6);
    assert!((signed(&s, &a_t, s.start(&a)) - GAP).abs() < 1e-6, "the start re-aims too: {:?}", s.start(&a));
    assert!(s.end(&a).y > s.start(&a).y + 50.0);
    assert!(stale_arrows(&s.store).is_empty());
}

#[test]
fn deleting_the_target_frees_the_end_in_place_and_restoring_rebinds() {
    let mut s = Scene::new();
    let t = s.rect("t", 0.0, 0.0, 100.0, 100.0);
    let a = s.arrow("a", &[(300.0, 50.0), (106.0, 50.0)]);
    s.bind(&a, End::End, "t", BindMode::Orbit, [0.5, 0.5]);
    s.solve(&[&t]);
    let before = s.end(&a);

    // Deleted but the binding stays stored: the solver leaves the end alone.
    s.tombstone(&t, true);
    assert!(s.solve(&[&t]).is_empty());
    assert_close(s.end(&a), before);
    assert!(s.binding(&a, End::End).is_some());

    // The shape comes back somewhere else (undo then a move): the stored binding re-binds.
    s.edit(&t, |e| {
        e.deleted = false;
        e.transform.y += 200.0;
    });
    assert_eq!(s.solve(&[&t]), vec![a.clone()]);
    assert!((signed(&s, &t, s.end(&a)) - GAP).abs() < 1e-6);
    assert!(s.end(&a).y > 200.0);
}

#[test]
fn free_ends_bound_to_deleted_shapes_leaves_geometry_alone() {
    use drafftink_edit::binding::free_ends_bound_to;
    use drafftink_scene::{SceneStore, SceneTx};
    let mut s = Scene::new();
    let t = s.rect("t", 0.0, 0.0, 100.0, 100.0);
    let other = s.rect("o", 500.0, 0.0, 100.0, 100.0);
    let a = s.arrow("a", &[(494.0, 50.0), (106.0, 50.0)]);
    s.bind(&a, End::End, "t", BindMode::Orbit, [0.5, 0.5]);
    s.bind(&a, End::Start, "o", BindMode::Orbit, [0.5, 0.5]);
    s.solve(&[&t]);
    let pts = s.world(&a);
    let mut tx = s.store.begin();
    tx.update(&t, |e| e.deleted = true).unwrap();
    let changed = free_ends_bound_to(&mut tx, std::slice::from_ref(&t)).unwrap();
    tx.commit().unwrap();
    assert_eq!(changed, vec![a.clone()]);
    assert!(s.binding(&a, End::End).is_none());
    assert!(s.binding(&a, End::Start).is_some(), "the other end keeps its shape");
    assert_eq!(s.world(&a), pts);
    let _ = other;
}

#[test]
fn a_self_loop_moves_with_its_shape() {
    let mut s = Scene::new();
    let t = s.rect("t", 0.0, 0.0, 100.0, 100.0);
    let a = s.arrow("a", &[(70.0, 20.0), (70.0, -60.0), (30.0, -60.0), (30.0, 20.0)]);
    s.bind(&a, End::Start, "t", BindMode::Inside, [0.7, 0.2]);
    s.bind(&a, End::End, "t", BindMode::Inside, [0.3, 0.2]);
    s.solve(&[&t]);
    let before = s.world(&a);
    s.move_by(&t, 50.0, 30.0);
    let after = s.world(&a);
    for (b, a) in before.iter().zip(&after) {
        assert_close(*a, *b + v(50.0, 30.0));
    }
}

#[test]
fn a_short_orbit_arrow_keeps_the_focus_instead_of_inverting() {
    let mut s = Scene::new();
    let t = s.rect("t", 0.0, 0.0, 100.0, 100.0);
    // The other end is only 5 units from where the outline would be.
    let a = s.arrow("a", &[(110.0, 50.0), (106.0, 50.0)]);
    s.bind(&a, End::End, "t", BindMode::Orbit, [0.5, 0.5]);
    s.solve(&[&t]);
    assert_close(s.end(&a), s.target_of(&t).global_fixed_point(v(0.5, 0.5)));
}

#[test]
fn nested_shapes_do_not_invert_the_arrow() {
    let mut s = Scene::new();
    let outer = s.rect("outer", 0.0, 0.0, 400.0, 400.0);
    let inner = s.rect("inner", 100.0, 100.0, 100.0, 100.0);
    let a = s.arrow("a", &[(150.0, 150.0), (350.0, 350.0)]);
    s.bind(&a, End::Start, "inner", BindMode::Orbit, [0.5, 0.5]);
    s.bind(&a, End::End, "outer", BindMode::Orbit, [0.8, 0.8]);
    s.solve(&[&inner, &outer]);
    // Whatever the solver chose, the arrow still points from the inner shape towards the lower right.
    let (st, en) = (s.start(&a), s.end(&a));
    assert!(en.x > st.x && en.y > st.y, "{st:?} -> {en:?}");
}

#[test]
fn a_head_on_the_bound_end_keeps_the_focus_when_the_other_shape_covers_the_outline() {
    let mut s = Scene::new();
    let big = s.rect("big", 0.0, 0.0, 300.0, 300.0);
    let small = s.rect("small", 100.0, 100.0, 50.0, 50.0);
    let a = s.arrow("a", &[(125.0, 125.0), (140.0, 140.0)]);
    s.heads(&a, Head::None, Head::Arrow);
    s.bind(&a, End::Start, "small", BindMode::Orbit, [0.5, 0.5]);
    s.bind(&a, End::End, "big", BindMode::Orbit, [0.5, 0.5]);
    s.solve(&[&small, &big]);
    assert!(s.world(&a).iter().all(|p| p.x.is_finite() && p.y.is_finite()));
}

#[test]
fn bound_end_position_reports_without_changing_anything() {
    let mut s = Scene::new();
    let t = s.rect("t", 0.0, 0.0, 100.0, 100.0);
    let a = s.arrow("a", &[(300.0, 50.0), (200.0, 50.0)]);
    s.bind(&a, End::End, "t", BindMode::Orbit, [0.5, 0.5]);
    let at = bound_end_position(&s.store, &s.get(&a), End::End).unwrap();
    assert!((signed(&s, &t, at) - GAP).abs() < 1e-6);
    assert_close(s.end(&a), p(200.0, 50.0));
    assert!(bound_end_position(&s.store, &s.get(&a), End::Start).is_none());
}

#[test]
fn rotated_arrow_is_re_solved_in_the_world_frame() {
    let mut s = Scene::new();
    let t = s.rect("t", 0.0, 0.0, 100.0, 100.0);
    let a = s.arrow("a", &[(300.0, 50.0), (200.0, 120.0), (150.0, 50.0)]);
    s.edit(&a, |e| e.transform.angle = 0.4);
    let before = s.world(&a);
    s.bind(&a, End::End, "t", BindMode::Orbit, [0.5, 0.5]);
    s.solve(&[&t]);
    let after = s.world(&a);
    assert_close(after[0], before[0]);
    assert_close(after[1], before[1]);
    assert!((signed(&s, &t, after[2]) - GAP).abs() < 1e-6, "{:?}", after[2]);
}
