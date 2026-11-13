#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Elbow arrows: routing around bound shapes, stored geometry, pinned segments.

mod arrows_common;

use arrows_common::*;
use drafftink_edit::binding::{End, stale_arrows};
use drafftink_edit::elbow::{self, RouteOpts};
use drafftink_edit::linear;
use drafftink_scene::{PathType, SceneRead, SceneStore, SceneTx};

fn opts() -> RouteOpts<'static> {
    RouteOpts::default()
}

#[test]
fn a_free_elbow_arrow_is_a_z_through_the_middle() {
    let mut s = Scene::new();
    let a = s.arrow("a", &[(-45.0, -100.1), (45.0, 99.9)]);
    let mut tx = s.store.begin();
    linear::set_path(&mut tx, &a, PathType::Elbow).unwrap();
    tx.commit().unwrap();
    assert_pts(&s.rel(&a), &[(0.0, 0.0), (0.0, 100.0), (90.0, 100.0), (90.0, 200.0)]);
    let e = s.get(&a);
    assert_eq!(e.transform.angle, 0.0);
    assert!((e.transform.w - 90.0).abs() < 1e-9 && (e.transform.h - 200.0).abs() < 1e-9);
}

#[test]
fn a_routed_copy_shows_the_route_of_an_element_that_is_not_in_the_document() {
    let mut s = Scene::new();
    s.rect("r1", -150.0, -150.0, 100.0, 100.0);
    s.rect("r2", 50.0, 50.0, 100.0, 100.0);
    let stored = s.elbow_between("a", p(-45.0, -100.0), p(45.0, 100.0));
    // The preview of an arrow being drawn: two straight points, the elbow path and the two bindings.
    let d = s.get(&stored).linear_data().unwrap().clone();
    let mut draft = drafftink_scene::Element::linear(
        id("draft"),
        drafftink_scene::LinearKind::Arrow,
        drafftink_scene::ZIndex::first(),
        &[p(-45.0, -100.0), p(45.0, 100.0)],
    )
    .unwrap();
    if let drafftink_scene::ElementKind::Arrow(l) = &mut draft.kind {
        l.path = PathType::Elbow;
        l.start_binding = d.start_binding.clone();
        l.end_binding = d.end_binding.clone();
    }
    assert_eq!(linear::world_points(&draft).unwrap().len(), 2);
    let routed = elbow::routed_copy(&s.store, &draft, opts()).unwrap();
    let got: Vec<_> = linear::world_points(&routed).unwrap();
    let want = s.world(&stored);
    assert_eq!(got.len(), want.len(), "{got:?} vs {want:?}");
    for (g, w) in got.iter().zip(&want) {
        assert!((*g - *w).hypot() < 0.02, "{got:?} vs {want:?}");
    }
    assert!(s.store.get(&id("draft")).is_none(), "nothing was written");
    // An element that is not an elbow comes back as it is.
    let plain = s.arrow("plain", &[(0.0, 0.0), (10.0, 10.0)]);
    let e = s.get(&plain);
    assert_eq!(elbow::routed_copy(&s.store, &e, opts()).unwrap(), e);
}

#[test]
fn two_facing_shapes_are_joined_with_two_corners_halfway() {
    let mut s = Scene::new();
    s.rect("r1", -150.0, -150.0, 100.0, 100.0);
    s.rect("r2", 50.0, 50.0, 100.0, 100.0);
    let a = s.elbow_between("a", p(-45.0, -100.0), p(45.0, 100.0));
    // gap = 6: the ends sit at x = -44 and x = 44, the turn halfway.
    assert_pts(&s.rel(&a), &[(0.0, 0.0), (44.0, 0.0), (44.0, 200.0), (88.0, 200.0)]);
    assert_near(s.start(&a), p(-44.0, -100.0));
    assert_near(s.end(&a), p(44.0, 100.0));
    assert!(stale_arrows(&s.store).is_empty());
}

#[test]
fn a_perpendicular_pair_gets_one_corner() {
    let mut s = Scene::new();
    s.rect("r1", 0.0, 0.0, 100.0, 100.0);
    s.rect("r2", 300.0, 200.0, 100.0, 100.0);
    let a = s.elbow_between("a", p(105.0, 50.0), p(350.0, 195.0));
    let w = s.world(&a);
    assert_eq!(w.len(), 3, "{w:?}");
    assert_near(w[0], p(106.0, 50.0));
    assert_near(w[1], p(350.0, 50.0));
    assert_near(w[2], p(350.0, 194.0));
}

#[test]
fn moving_a_bound_shape_re_routes_the_arrow() {
    let mut s = Scene::new();
    s.rect("r1", -150.0, -150.0, 100.0, 100.0);
    let r2 = s.rect("r2", 50.0, 50.0, 100.0, 100.0);
    let a = s.elbow_between("a", p(-45.0, -100.0), p(45.0, 100.0));
    let before = s.rel(&a);
    s.move_by(&r2, 100.0, 100.0);
    assert_near(s.end(&a), p(144.0, 200.0));
    assert_clean_orthogonal(&s.world(&a));
    assert!(stale_arrows(&s.store).is_empty());
    assert_ne!(s.rel(&a), before);
    // Moving it back restores the original route.
    s.move_by(&r2, -100.0, -100.0);
    assert_pts(&s.rel(&a), &before);
}

#[test]
fn an_arrow_that_wraps_around_a_shape_does_not_cross_it() {
    let mut s = Scene::new();
    let r1 = s.rect("r1", 0.0, 0.0, 100.0, 100.0);
    let r2 = s.rect("r2", 0.0, 220.0, 100.0, 100.0);
    // Leave the right side of the upper shape and enter the right side of the lower one.
    let a = s.elbow_between("a", p(105.0, 50.0), p(105.0, 270.0));
    let w = s.world(&a);
    assert_clean_orthogonal(&w);
    for (shape, name) in [(&r1, "r1"), (&r2, "r2")] {
        let t = s.target_of(shape);
        for seg in w.windows(2) {
            let mid = seg[0].midpoint(seg[1]);
            assert!(!t.contains(mid) || signed(&s, shape, mid) > -1e-9, "route crosses {name}: {w:?}");
        }
    }
    // Both ends leave to the right, so the route has to bulge out beyond the shapes.
    assert!(w.iter().any(|q| q.x > 140.0), "{w:?}");
}

#[test]
fn reroute_is_idempotent() {
    let mut s = Scene::new();
    s.rect("r1", -150.0, -150.0, 100.0, 100.0);
    s.rect("r2", 50.0, 50.0, 100.0, 100.0);
    let a = s.elbow_between("a", p(-45.0, -100.0), p(45.0, 100.0));
    let before = s.get(&a);
    let mut tx = s.store.begin();
    assert!(!elbow::reroute(&mut tx, &a, opts()).unwrap());
    tx.commit().unwrap();
    assert_eq!(s.get(&a).kind, before.kind);
}

#[test]
fn arrowheads_make_room() {
    let mut s = Scene::new();
    s.rect("r1", 0.0, 0.0, 100.0, 100.0);
    s.rect("r2", 280.0, 0.0, 100.0, 100.0);
    let a = s.elbow_between("a", p(105.0, 50.0), p(275.0, 50.0));
    // Aligned: one straight segment.
    assert_eq!(s.world(&a).len(), 2);
    s.move_by(&drafftink_scene::ElementId::new("r2").unwrap(), 0.0, 60.0);
    let w = s.world(&a);
    assert_eq!(w.len(), 4, "{w:?}");
    let mut tx = s.store.begin();
    linear::set_head(&mut tx, &a, End::End, drafftink_scene::Head::Arrow).unwrap();
    tx.commit().unwrap();
    let w2 = s.world(&a);
    // The last leg is at least as long as the arrowhead.
    let last = (w2[w2.len() - 1] - w2[w2.len() - 2]).hypot();
    assert!(last >= 25.0, "{w2:?}");
}

// ---- pinned segments ----

fn free_elbow(s: &mut Scene) -> drafftink_scene::ElementId {
    let a = s.arrow("a", &[(0.0, 0.0), (250.0, 200.0)]);
    let mut tx = s.store.begin();
    linear::set_path(&mut tx, &a, PathType::Elbow).unwrap();
    tx.commit().unwrap();
    a
}

#[test]
fn dragging_the_middle_segment_pins_it_and_releasing_restores_the_middle() {
    let mut s = Scene::new();
    let a = free_elbow(&mut s);
    assert_pts(&s.rel(&a), &[(0.0, 0.0), (125.0, 0.0), (125.0, 200.0), (250.0, 200.0)]);
    let mut tx = s.store.begin();
    elbow::move_segment_of(&mut tx, &a, 2, p(130.0, 100.0), opts()).unwrap();
    tx.commit().unwrap();
    assert_pts(&s.rel(&a), &[(0.0, 0.0), (130.0, 0.0), (130.0, 200.0), (250.0, 200.0)]);
    assert_eq!(s.pins(&a), vec![2]);
    // The pin is stored with the segment's two points.
    let pin = s.get(&a).linear_data().unwrap().fixed_segments[0].clone();
    assert!((pin.start.x - 130.0).abs() < 1e-9 && (pin.end.x - 130.0).abs() < 1e-9);

    let mut tx = s.store.begin();
    elbow::release_segment_of(&mut tx, &a, 2, opts()).unwrap();
    tx.commit().unwrap();
    assert_pts(&s.rel(&a), &[(0.0, 0.0), (125.0, 0.0), (125.0, 200.0), (250.0, 200.0)]);
    assert!(s.pins(&a).is_empty());
}

#[test]
fn a_pinned_segment_survives_moving_an_end() {
    let mut s = Scene::new();
    let a = free_elbow(&mut s);
    let mut tx = s.store.begin();
    elbow::move_segment_of(&mut tx, &a, 2, p(130.0, 100.0), opts()).unwrap();
    linear::move_points(&mut tx, &a, &[(1 + 2, p(300.0, 260.0))]).unwrap();
    tx.commit().unwrap();
    let w = s.world(&a);
    assert_clean_orthogonal(&w);
    assert_eq!(w.len(), 4, "{w:?}");
    assert!((w[1].x - 130.0).abs() < 1e-9 && (w[2].x - 130.0).abs() < 1e-9, "the pin stays at x = 130: {w:?}");
    assert_near(w[3], p(300.0, 260.0));
    assert_eq!(s.pins(&a), vec![2]);
}

#[test]
fn a_pinned_segment_survives_moving_a_bound_shape() {
    let mut s = Scene::new();
    s.rect("r1", -150.0, -150.0, 100.0, 100.0);
    let r2 = s.rect("r2", 250.0, 50.0, 100.0, 100.0);
    let a = s.elbow_between("a", p(-45.0, -100.0), p(245.0, 100.0));
    let mut tx = s.store.begin();
    elbow::move_segment_of(&mut tx, &a, 2, p(60.0, 0.0), opts()).unwrap();
    tx.commit().unwrap();
    assert_eq!(s.pins(&a), vec![2]);
    let pinned_x = s.world(&a)[1].x;
    assert!((pinned_x - 60.0).abs() < 1e-9, "{:?}", s.world(&a));

    s.move_by(&r2, 0.0, 120.0);
    let w = s.world(&a);
    assert_clean_orthogonal(&w);
    assert_near(*w.last().unwrap(), p(244.0, 220.0));
    assert!(w.iter().filter(|q| (q.x - 60.0).abs() < 1e-9).count() >= 2, "the pin kept x = 60: {w:?}");
    assert!(stale_arrows(&s.store).is_empty());
    assert_eq!(s.pins(&a).len(), 1);
}

#[test]
fn moving_the_first_segment_of_a_bound_arrow_keeps_a_stub_leaving_the_shape() {
    let mut s = Scene::new();
    s.rect("r1", -150.0, -150.0, 100.0, 100.0);
    s.rect("r2", 250.0, 50.0, 100.0, 100.0);
    let a = s.elbow_between("a", p(-45.0, -100.0), p(245.0, 100.0));
    let mut tx = s.store.begin();
    elbow::move_segment_of(&mut tx, &a, 1, p(0.0, -60.0), opts()).unwrap();
    tx.commit().unwrap();
    let w = s.world(&a);
    assert_clean_orthogonal(&w);
    assert_near(w[0], p(-44.0, -100.0));
    // It still leaves the shape to the right first.
    assert!(w[1].x > w[0].x && (w[1].y - w[0].y).abs() < 1e-9, "{w:?}");
    assert!(s.pins(&a).iter().all(|i| *i >= 2));
    assert!(
        s.get(&a)
            .linear_data()
            .unwrap()
            .fixed_segments
            .iter()
            .all(|f| (f.start.x - f.end.x).abs() < 1e-9 || (f.start.y - f.end.y).abs() < 1e-9)
    );
}

#[test]
fn a_pin_that_lines_up_with_its_neighbours_disappears() {
    let mut s = Scene::new();
    let a = free_elbow(&mut s);
    let mut tx = s.store.begin();
    // Pin the middle segment, then move the end so that all legs line up on one row: nothing is left to pin.
    elbow::move_segment_of(&mut tx, &a, 2, p(130.0, 100.0), opts()).unwrap();
    linear::move_points(&mut tx, &a, &[(3, p(250.0, 0.0))]).unwrap();
    tx.commit().unwrap();
    let w = s.world(&a);
    assert_clean_orthogonal(&w);
    assert!(s.pins(&a).is_empty(), "{:?} {:?}", w, s.pins(&a));
    assert_eq!(w.len(), 2, "{w:?}");
}

#[test]
fn segment_edits_report_bad_input_as_errors() {
    let mut s = Scene::new();
    let a = free_elbow(&mut s);
    let mut tx = s.store.begin();
    assert!(elbow::move_segment_of(&mut tx, &a, 0, p(1.0, 1.0), opts()).is_err());
    assert!(elbow::move_segment_of(&mut tx, &a, 9, p(1.0, 1.0), opts()).is_err());
    assert!(elbow::move_segment_of(&mut tx, &a, 2, p(f64::NAN, 1.0), opts()).is_err());
    // Releasing a segment that is not pinned changes nothing.
    elbow::release_segment_of(&mut tx, &a, 2, opts()).unwrap();
}

#[test]
fn rounded_corner_data_for_the_stored_route() {
    let mut s = Scene::new();
    let a = free_elbow(&mut s);
    let w = s.world(&a);
    let path = elbow::rounded_path(&w, elbow::CORNER_RADIUS);
    // M, then per corner a line and a quad, then the last line.
    assert_eq!(path.elements().len(), 1 + 2 * 2 + 1);
}

#[test]
fn converting_between_elbow_and_straight_keeps_the_arrow_attached() {
    let mut s = Scene::new();
    let r1 = s.rect("r1", -150.0, -150.0, 100.0, 100.0);
    let r2 = s.rect("r2", 50.0, 50.0, 100.0, 100.0);
    let a = s.elbow_between("a", p(-45.0, -100.0), p(45.0, 100.0));
    let mut tx = s.store.begin();
    linear::set_path(&mut tx, &a, PathType::Sharp).unwrap();
    tx.commit().unwrap();
    assert!(s.binding(&a, End::Start).is_some() && s.binding(&a, End::End).is_some());
    assert!(s.get(&a).linear_data().unwrap().fixed_segments.is_empty());
    s.move_by(&r2, 0.0, 300.0);
    assert!(stale_arrows(&s.store).is_empty());
    assert!((signed(&s, &r2, s.end(&a)) - 6.0).abs() < 0.5, "{:?}", s.end(&a));
    assert!(signed(&s, &r1, s.start(&a)) < 20.0);

    // And back to elbow.
    let mut tx = s.store.begin();
    linear::set_path(&mut tx, &a, PathType::Elbow).unwrap();
    tx.commit().unwrap();
    assert_clean_orthogonal(&s.world(&a));
    assert!(s.binding(&a, End::End).is_some());
}

#[test]
fn degenerate_and_extreme_shapes_never_panic() {
    use drafftink_edit::binding::BindOpts;
    for (w, h, x, y, angle) in [
        (0.0, 0.0, 0.0, 0.0, 0.0),
        (0.5, 300.0, 500.0, 0.0, 0.3),
        (1.0e6, 1.0e6, -5.0e5, -5.0e5, 0.0),
        (80.0, 80.0, 9.0e8, 9.0e8, 1.0),
        (100.0, 100.0, 40.0, 40.0, 0.0),
    ] {
        let mut s = Scene::new();
        s.rect("a", 0.0, 0.0, 100.0, 100.0);
        let b = s.rect("b", x, y, w, h);
        s.rotate(&b, angle);
        let a = s.arrow("arrow", &[(300.0, 300.0), (200.0, 200.0)]);
        s.set_path(&a, PathType::Elbow);
        for at in [p(x, y), p(x + w / 2.0, y + h / 2.0), p(x + w + 3.0, y + h / 2.0), p(50.0, 50.0), p(-3.0, 50.0)] {
            s.drag_end_with(&a, End::End, at, &BindOpts::default());
            s.drag_end_with(&a, End::Start, p(106.0, 50.0), &BindOpts::default());
            assert!(s.world(&a).iter().all(|q| q.x.is_finite() && q.y.is_finite()));
        }
        s.move_by(&b, 10.0, -10.0);
        s.resize(&b, x, y, w + 1.0, h + 1.0);
    }
}

#[test]
fn the_stored_route_is_what_hit_testing_measures() {
    let mut s = Scene::new();
    let a = free_elbow(&mut s);
    let e = s.get(&a);
    // On the stored middle leg (x = 125), not on the straight chord between the ends.
    assert!(linear::distance_to(&e, p(125.0, 100.0)).unwrap() < 1e-9);
    assert!(linear::distance_to(&e, p(125.0, 100.0)).unwrap() < linear::distance_to(&e, p(100.0, 80.0)).unwrap());
    // Its segment handles are at the legs' midpoints.
    let handles = linear::midpoint_handles(&e, 1.0);
    assert_eq!(handles.iter().map(|h| h.segment).collect::<Vec<_>>(), vec![1, 2, 3]);
    assert_near(handles[1].at, p(125.0, 100.0));
}

#[test]
fn stacked_shapes_turn_halfway_between_them() {
    let mut s = Scene::new();
    s.rect("r1", 0.0, 0.0, 100.0, 100.0);
    s.rect("r2", 300.0, 160.0, 100.0, 100.0);
    let a = s.elbow_between("a", p(50.0, 105.0), p(350.0, 155.0));
    let w = s.world(&a);
    assert_eq!(w.len(), 4, "{w:?}");
    assert_near(w[0], p(50.0, 106.0));
    assert_near(w[1], p(50.0, 130.0));
    assert_near(w[2], p(350.0, 130.0));
    assert_near(w[3], p(350.0, 154.0));
}

#[test]
fn an_arrowhead_that_does_not_fit_in_a_short_leg_sends_the_route_around_the_grid() {
    let mut s = Scene::new();
    s.rect("r1", 0.0, 0.0, 100.0, 100.0);
    s.rect("r2", 300.0, 160.0, 100.0, 100.0);
    let a = s.elbow_between("a", p(50.0, 105.0), p(350.0, 155.0));
    let mut tx = s.store.begin();
    linear::set_head(&mut tx, &a, End::End, drafftink_scene::Head::Arrow).unwrap();
    tx.commit().unwrap();
    let w = s.world(&a);
    assert_clean_orthogonal(&w);
    assert_near(w[0], p(50.0, 106.0));
    assert_near(*w.last().unwrap(), p(350.0, 154.0));
    let last_leg = (w[w.len() - 1] - w[w.len() - 2]).hypot();
    assert!(last_leg >= 25.0, "the head fits in the last leg: {w:?}");
}

#[test]
fn legs_too_short_for_their_corners_fall_back_to_the_grid() {
    let mut s = Scene::new();
    s.rect("r1", 0.0, 0.0, 100.0, 100.0);
    s.rect("r2", 300.0, 120.0, 100.0, 100.0);
    // The first leg would be 7 long: shorter than half a corner radius.
    let a = s.elbow_between("a", p(50.0, 105.0), p(350.0, 113.0));
    let w = s.world(&a);
    assert_clean_orthogonal(&w);
    assert!(w.len() > 4, "the grid route steps out by the padding: {w:?}");
    assert!(stale_arrows(&s.store).is_empty());
}
