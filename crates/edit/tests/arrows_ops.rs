#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! `edit::arrow`: bind an end to a named target, reroute, and snapping of a dragged point.

mod arrows_common;

use arrows_common::*;
use drafftink_edit::arrow::{
    ArrowError, BindRequest, SnapFlags, bind_end, reroute, snap_candidates, snap_dragged_point,
};
use drafftink_edit::binding::{End, stale_arrows};
use drafftink_geom::kurbo::{Point, Rect};
use drafftink_scene::{BindMode, Kind, PathType, SceneStore, SceneTx};

const GAP: f64 = 6.0; // 5 + stroke 2 / 2

fn request(target: &drafftink_scene::ElementId, mode: BindMode) -> BindRequest<'_> {
    BindRequest { target, mode, fixed_point: None, zoom: 1.0, midpoint_snap: false }
}

fn run_bind(
    s: &mut Scene,
    arrow: &drafftink_scene::ElementId,
    end: End,
    req: &BindRequest<'_>,
) -> Result<drafftink_scene::Binding, ArrowError> {
    let mut tx = s.store.begin();
    let out = bind_end(&mut tx, arrow, end, req);
    if out.is_ok() {
        tx.commit().unwrap();
    }
    out
}

#[test]
fn bind_end_faces_the_arrow_and_sits_on_the_outline_plus_gap() {
    let mut s = Scene::new();
    let t = s.rect("t", 0.0, 0.0, 100.0, 100.0);
    let a = s.arrow("a", &[(300.0, 50.0), (150.0, 50.0)]);
    let b = run_bind(&mut s, &a, End::End, &request(&t, BindMode::Orbit)).unwrap();
    assert_eq!(b.target, t);
    // The fixed point is on the right edge, in the middle.
    assert!((b.fixed_point[0] - 1.0).abs() < 1e-6 && (b.fixed_point[1] - 0.5).abs() < 1e-3, "{:?}", b.fixed_point);
    assert!((signed(&s, &t, s.end(&a)) - GAP).abs() < 1e-6);
    assert!(stale_arrows(&s.store).is_empty());
}

#[test]
fn bind_end_inside_with_a_given_fixed_point() {
    let mut s = Scene::new();
    let t = s.rect("t", 0.0, 0.0, 100.0, 100.0);
    let a = s.arrow("a", &[(300.0, 50.0), (150.0, 50.0)]);
    let req = BindRequest { fixed_point: Some([0.25, 0.75]), ..request(&t, BindMode::Inside) };
    run_bind(&mut s, &a, End::End, &req).unwrap();
    assert_close(s.end(&a), p(25.0, 75.0));
}

#[test]
fn bind_end_refuses_what_an_arrow_cannot_attach_to() {
    let mut s = Scene::new();
    let a = s.arrow("a", &[(0.0, 0.0), (100.0, 0.0)]);
    let other = s.arrow("b", &[(0.0, 50.0), (100.0, 60.0)]);
    let r = run_bind(&mut s, &a, End::End, &request(&other, BindMode::Orbit));
    assert!(matches!(r, Err(ArrowError::NotBindable { kind: Kind::Arrow, label: false, .. })), "{r:?}");
    let r = run_bind(&mut s, &a, End::End, &request(&a, BindMode::Orbit));
    assert!(matches!(r, Err(ArrowError::NotBindable { .. })), "{r:?}");
    let ghost = arrows_common::id("ghost");
    assert_eq!(run_bind(&mut s, &a, End::End, &request(&ghost, BindMode::Orbit)), Err(ArrowError::Missing(ghost)));
    let t = s.rect("t", 0.0, 100.0, 50.0, 50.0);
    let req = BindRequest { fixed_point: Some([f64::NAN, 0.5]), ..request(&t, BindMode::Orbit) };
    let r = run_bind(&mut s, &t.clone(), End::End, &request(&t, BindMode::Orbit));
    assert!(matches!(r, Err(ArrowError::NotLinear(_))), "{r:?}");
    assert_eq!(run_bind(&mut s, &a, End::End, &req), Err(ArrowError::NotFinite));
    assert!(s.binding(&a, End::End).is_none(), "a refused bind changes nothing");
}

#[test]
fn bind_end_on_an_elbow_arrow_binds_orbit_and_routes() {
    let mut s = Scene::new();
    let t = s.rect("t", 300.0, 200.0, 100.0, 60.0);
    let a = s.arrow("a", &[(0.0, 0.0), (200.0, 100.0)]);
    s.set_path(&a, PathType::Elbow);
    let b = run_bind(&mut s, &a, End::End, &request(&t, BindMode::Inside)).unwrap();
    assert_eq!(b.mode, BindMode::Orbit);
    let w = s.world(&a);
    assert!(w.windows(2).all(|q| (q[0].x - q[1].x).abs() < 1e-6 || (q[0].y - q[1].y).abs() < 1e-6), "{w:?}");
    assert!(stale_arrows(&s.store).is_empty());
}

#[test]
fn reroute_solves_stale_ends_and_clears_pins() {
    let mut s = Scene::new();
    let t = s.rect("t", 300.0, 0.0, 100.0, 100.0);
    let a = s.arrow("a", &[(0.0, 50.0), (200.0, 50.0)]);
    s.bind(&a, End::End, "t", BindMode::Orbit, [0.0, 0.5]);
    s.solve(&[&t]);
    // Move the target without solving.
    s.edit(&t, |e| e.transform.y += 80.0);
    assert_eq!(stale_arrows(&s.store), vec![a.clone()]);
    let mut tx = s.store.begin();
    let changed = reroute(&mut tx, &[a.clone(), t.clone()], 1.0).unwrap();
    tx.commit().unwrap();
    assert_eq!(changed, vec![a.clone()]);
    assert!(stale_arrows(&s.store).is_empty());
}

#[test]
fn reroute_of_an_elbow_arrow_drops_pins() {
    let mut s = Scene::new();
    let t1 = s.rect("t1", 0.0, 0.0, 100.0, 60.0);
    let t2 = s.rect("t2", 400.0, 200.0, 100.0, 60.0);
    let a = s.arrow("a", &[(100.0, 30.0), (400.0, 230.0)]);
    let req = |t| request(t, BindMode::Orbit);
    run_bind(&mut s, &a, End::Start, &req(&t1)).unwrap();
    run_bind(&mut s, &a, End::End, &req(&t2)).unwrap();
    s.set_path(&a, PathType::Elbow);
    let mut tx = s.store.begin();
    reroute(&mut tx, std::slice::from_ref(&a), 1.0).unwrap();
    drafftink_edit::elbow::move_segment_of(&mut tx, &a, 2, p(250.0, 400.0), Default::default()).unwrap();
    tx.commit().unwrap();
    assert!(!s.get(&a).linear_data().unwrap().fixed_segments.is_empty());
    let mut tx = s.store.begin();
    reroute(&mut tx, std::slice::from_ref(&a), 1.0).unwrap();
    tx.commit().unwrap();
    assert!(s.get(&a).linear_data().unwrap().fixed_segments.is_empty());
    assert!(stale_arrows(&s.store).is_empty());
}

#[test]
fn reroute_skips_ids_that_are_not_lines_and_arrows() {
    let mut s = Scene::new();
    let t = s.rect("t", 0.0, 0.0, 10.0, 10.0);
    let mut tx = s.store.begin();
    assert_eq!(reroute(&mut tx, &[t, arrows_common::id("ghost")], 1.0).unwrap(), Vec::new());
}

// ---- snapping ----

fn flags(grid: bool, smart: bool, angle: bool) -> SnapFlags {
    SnapFlags { grid, smart, angle }
}

#[test]
fn snap_to_the_grid_and_to_guides() {
    let none = snap_dragged_point(p(113.0, 107.0), None, &[], flags(false, false, false), 15.0);
    assert_eq!(none.point, p(113.0, 107.0));
    let grid = snap_dragged_point(p(113.0, 107.0), None, &[], flags(true, false, false), 15.0);
    assert_eq!(grid.point, p(120.0, 100.0));
    let others = [Rect::new(300.0, 0.0, 400.0, 60.0)];
    let smart = snap_dragged_point(p(303.0, 107.0), None, &others, flags(false, true, false), 15.0);
    assert_eq!(smart.point.x, 300.0);
    assert_eq!(smart.guides.len(), 1);
    // Grid snaps the axis the guides left alone.
    let both = snap_dragged_point(p(303.0, 107.0), None, &others, flags(true, true, false), 15.0);
    assert_eq!((both.point.x, both.point.y), (300.0, 100.0));
}

#[test]
fn snap_the_angle_keeps_the_length() {
    let origin = p(0.0, 0.0);
    let r = snap_dragged_point(p(100.0, 40.0), Some(origin), &[], flags(false, false, true), 15.0);
    let len = 100.0f64.hypot(40.0);
    assert!((r.point.to_vec2().hypot() - len).abs() < 1e-9);
    let deg = r.point.y.atan2(r.point.x).to_degrees();
    assert!((deg - 15.0).abs() < 1e-9 || (deg - 30.0).abs() < 1e-9, "{deg}");
    assert_eq!(r.angle, Some(15.0), "100 by 40 is about 22 degrees, which rounds to 15");
    // A different step.
    let r = snap_dragged_point(p(100.0, 10.0), Some(origin), &[], flags(false, false, true), 45.0);
    assert!(r.point.y.abs() < 1e-9);
    // Without a reference the angle snap does nothing.
    let r = snap_dragged_point(p(100.0, 40.0), None, &[], flags(false, false, true), 15.0);
    assert_eq!(r.point, p(100.0, 40.0));
    assert_eq!(r.angle, None);
}

#[test]
fn snap_survives_hostile_input() {
    for bad in [f64::NAN, f64::INFINITY, -f64::INFINITY, 1e300] {
        let r = snap_dragged_point(
            p(bad, 0.0),
            Some(p(0.0, bad)),
            &[Rect::new(bad, 0.0, 1.0, 1.0)],
            flags(true, true, true),
            bad,
        );
        let _ = r;
    }
    let r = snap_dragged_point(p(f64::NAN, 0.0), None, &[], flags(true, true, true), 15.0);
    assert!(r.point.x.is_nan(), "a non-finite point is returned unchanged");
    let r = snap_dragged_point(p(5.0, 5.0), Some(p(5.0, 5.0)), &[], flags(false, false, true), 15.0);
    assert_eq!(r.point, p(5.0, 5.0), "a zero-length drag has no angle");
    // A bad step snaps nothing and does not panic.
    let r = snap_dragged_point(p(100.0, 40.0), Some(p(0.0, 0.0)), &[], flags(false, false, true), 0.0);
    assert!(r.point.x.is_finite());
}

#[test]
fn snap_candidates_are_the_bounds_of_nearby_elements_without_the_skipped_ones() {
    let mut s = Scene::new();
    let near = s.rect("near", 100.0, 100.0, 50.0, 50.0);
    let _far = s.rect("far", 90_000.0, 0.0, 50.0, 50.0);
    let skip = s.rect("skip", 0.0, 0.0, 10.0, 10.0);
    let out = snap_candidates(&s.store, std::slice::from_ref(&skip), Point::new(0.0, 0.0));
    assert_eq!(out, vec![Rect::new(100.0, 100.0, 150.0, 150.0)]);
    let _ = near;
    assert!(snap_candidates(&s.store, &[], Point::new(f64::NAN, 0.0)).is_empty());
}
