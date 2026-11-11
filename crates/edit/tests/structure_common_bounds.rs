#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Common bounds of several elements, the eraser path and rounding of resize noise.

mod structure_support;

use drafftink_edit::query::hit_by_path;
use drafftink_edit::transform::{self, ResizeSpec};
use drafftink_edit::transform::{CommonBounds, set_common_bounds, tidy};
use drafftink_geom::select::{BoxHandle, Corner};
use drafftink_scene::SceneStore;
use kurbo::{Point, Vec2};
use structure_support::*;

#[test]
fn several_elements_get_their_common_box_at_the_requested_place() {
    let mut s = scene(vec![rect("a", 0.0, 0.0, 100.0, 50.0), rect("b", 200.0, 0.0, 50.0, 50.0)]);
    let spec = CommonBounds { x: Some(10.0), y: Some(20.0), w: Some(500.0), h: Some(100.0), ..Default::default() };
    run(&mut s, |tx| set_common_bounds(tx, &ids(&["a", "b"]), spec)).unwrap();
    let b = transform::common_bounds(&s, &ids(&["a", "b"])).unwrap();
    assert!((b.x0 - 10.0).abs() < 1e-9 && (b.y0 - 20.0).abs() < 1e-9, "{b:?}");
    assert!((b.width() - 500.0).abs() < 1e-9 && (b.height() - 100.0).abs() < 1e-9, "{b:?}");
}

#[test]
fn a_centre_anchor_keeps_the_size_and_moves_the_centre() {
    let mut s = scene(vec![rect("a", 0.0, 0.0, 100.0, 50.0), rect("b", 200.0, 0.0, 50.0, 50.0)]);
    let spec = CommonBounds { x: Some(0.0), y: Some(0.0), center: true, ..Default::default() };
    run(&mut s, |tx| set_common_bounds(tx, &ids(&["a", "b"]), spec)).unwrap();
    let b = transform::common_bounds(&s, &ids(&["a", "b"])).unwrap();
    assert!((b.center().x).abs() < 1e-9 && (b.center().y).abs() < 1e-9, "{b:?}");
    assert!((b.width() - 250.0).abs() < 1e-9);
}

#[test]
fn a_zero_or_missing_size_is_refused_and_nothing_changes() {
    let mut s = scene(vec![rect("a", 0.0, 0.0, 100.0, 50.0), rect("b", 200.0, 0.0, 50.0, 50.0)]);
    let rev = s.revision();
    let spec = CommonBounds { w: Some(0.0), ..Default::default() };
    assert!(run(&mut s, |tx| set_common_bounds(tx, &ids(&["a", "b"]), spec)).is_err());
    let spec = CommonBounds { x: Some(f64::NAN), ..Default::default() };
    assert!(run(&mut s, |tx| set_common_bounds(tx, &ids(&["a", "b"]), spec)).is_err());
    assert_eq!(s.revision(), rev);
}

#[test]
fn the_eraser_path_hits_along_segments_and_skips_what_is_locked() {
    let mut locked = rect("c", 0.0, 200.0, 50.0, 50.0);
    locked.locked = true;
    let s = scene(vec![rect("a", 0.0, 0.0, 100.0, 50.0), rect("b", 300.0, 0.0, 50.0, 50.0), locked]);
    // The stroke starts and ends in empty space and crosses `a` between samples.
    let hit = hit_by_path(&s, &[Point::new(-50.0, 25.0), Point::new(150.0, 25.0)], 4.0);
    assert_eq!(hit, ids(&["a"]));
    let hit = hit_by_path(&s, &[Point::new(25.0, 225.0)], 4.0);
    assert!(hit.is_empty(), "a locked element is not erased");
    assert!(hit_by_path(&s, &[Point::new(f64::NAN, 0.0)], 4.0).is_empty());
    assert!(hit_by_path(&s, &[], 4.0).is_empty());
}

#[test]
fn a_hit_with_a_locked_label_is_left_out_whole() {
    let mut lab = label("la", "a", 10.0, 10.0, 30.0, 10.0);
    lab.locked = true;
    let s = scene(vec![rect("a", 0.0, 0.0, 100.0, 50.0), lab]);
    assert!(hit_by_path(&s, &[Point::new(2.0, 2.0)], 3.0).is_empty());
}

#[test]
fn tidy_removes_the_noise_of_a_resize_and_changes_nothing_else() {
    let mut s = scene(vec![rect("a", 0.0, 0.0, 100.0, 60.0)]);
    let spec = ResizeSpec {
        handle: BoxHandle::Corner(Corner::BottomRight),
        delta: Vec2::new(40.0, 20.0),
        keep_aspect: false,
        from_center: false,
    };
    let r = run(&mut s, |tx| transform::resize(tx, &ids(&["a"]), spec)).unwrap();
    run(&mut s, |tx| tidy(tx, &r.changed)).unwrap();
    let t = get(&s, "a").transform;
    assert_eq!((t.x, t.y, t.w, t.h), (0.0, 0.0, 140.0, 80.0));
    let rev = s.revision();
    run(&mut s, |tx| tidy(tx, &ids(&["a"]))).unwrap();
    assert_eq!(s.revision(), rev, "already tidy: no write");
}
