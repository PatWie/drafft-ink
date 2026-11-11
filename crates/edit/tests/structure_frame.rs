#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Frames: membership, z-order, clipping data, wrap and unwrap, drop rules.

mod structure_support;

use std::collections::BTreeSet;

use drafftink_edit::TargetError;
use drafftink_edit::frame::{self, FrameError, SkipReason, WrapOpts};
use drafftink_edit::transform::{self, ResizeSpec};
use drafftink_geom::kurbo::{Point, Vec2};
use drafftink_geom::select::{BoxHandle, Corner};
use drafftink_scene::*;
use structure_support::*;

fn base() -> MemoryStore {
    scene(vec![
        rect("a", 10.0, 10.0, 20.0, 20.0),
        rect("b", 40.0, 10.0, 20.0, 20.0),
        frame("f", 0.0, 0.0, 100.0, 100.0),
        rect("out", 300.0, 0.0, 20.0, 20.0),
    ])
}

#[test]
fn add_sets_the_frame_and_puts_children_just_below_it() {
    let mut s = scene(vec![
        frame("f", 0.0, 0.0, 100.0, 100.0),
        rect("a", 10.0, 10.0, 20.0, 20.0),
        rect("x", 300.0, 0.0, 5.0, 5.0),
        rect("b", 40.0, 10.0, 20.0, 20.0),
    ]);
    let r = run(&mut s, |tx| frame::add_to_frame(tx, &id("f"), &ids(&["a", "b"]))).unwrap();
    assert_eq!(r.added, ids(&["a", "b"]));
    assert_eq!(get(&s, "a").frame_id, Some(id("f")));
    assert_eq!(order(&s), strs(&["a", "b", "f", "x"]), "children go just below the frame; x stays above it");
    assert_valid(&s);
    let report = validate_scene(&s);
    assert_eq!(report.warnings().count(), 0, "no child above its frame");
}

#[test]
fn new_children_join_above_the_old_ones_in_the_block() {
    let mut s = scene(vec![
        rect("k1", 5.0, 5.0, 10.0, 10.0).in_frame("f"),
        frame("f", 0.0, 0.0, 100.0, 100.0),
        rect("n", 50.0, 50.0, 10.0, 10.0),
    ]);
    run(&mut s, |tx| frame::add_to_frame(tx, &id("f"), &ids(&["n"]))).unwrap();
    assert_eq!(order(&s), strs(&["k1", "n", "f"]));
    assert_eq!(frame::children_of(&s, &id("f")).unwrap(), ids(&["k1", "n"]));
}

#[test]
fn a_group_joins_as_a_whole_and_stays_contiguous() {
    let mut s = scene(vec![
        rect("g1", 10.0, 10.0, 10.0, 10.0).grouped(&["G"]),
        rect("x", 300.0, 0.0, 5.0, 5.0),
        rect("g2", 40.0, 10.0, 10.0, 10.0).grouped(&["G"]),
        frame("f", 0.0, 0.0, 100.0, 100.0),
    ]);
    let r = run(&mut s, |tx| frame::add_to_frame(tx, &id("f"), &ids(&["g1"]))).unwrap();
    assert_eq!(r.added, ids(&["g1", "g2"]));
    assert_eq!(get(&s, "g2").frame_id, Some(id("f")));
    assert_eq!(order(&s), strs(&["x", "g1", "g2", "f"]));
}

#[test]
fn frames_and_groups_with_frames_are_skipped_and_listed() {
    let mut s = scene(vec![
        frame("other", 200.0, 200.0, 50.0, 50.0),
        frame("gf", 300.0, 300.0, 50.0, 50.0).grouped(&["G"]),
        rect("gr", 360.0, 300.0, 10.0, 10.0).grouped(&["G"]),
        rect("a", 10.0, 10.0, 20.0, 20.0),
        frame("f", 0.0, 0.0, 100.0, 100.0),
    ]);
    let r = run(&mut s, |tx| frame::add_to_frame(tx, &id("f"), &ids(&["other", "gr", "a", "f"]))).unwrap();
    assert_eq!(r.added, ids(&["a"]));
    let reasons: Vec<(String, SkipReason)> = r.skipped.iter().map(|k| (k.id.to_string(), k.reason)).collect();
    assert!(reasons.contains(&("other".into(), SkipReason::IsFrame)));
    assert!(reasons.contains(&("gr".into(), SkipReason::GroupContainsFrame)));
    assert!(reasons.contains(&("f".into(), SkipReason::IsFrame)));
    assert!(get(&s, "other").frame_id.is_none() && get(&s, "gr").frame_id.is_none());
}

#[test]
fn bound_text_joins_with_its_container() {
    let mut s = scene(vec![
        rect("a", 10.0, 10.0, 40.0, 20.0),
        label("la", "a", 15.0, 12.0, 20.0, 10.0),
        frame("f", 0.0, 0.0, 100.0, 100.0),
    ]);
    run(&mut s, |tx| frame::add_to_frame(tx, &id("f"), &ids(&["la"]))).unwrap();
    assert_eq!(get(&s, "a").frame_id, Some(id("f")));
    assert_eq!(get(&s, "la").frame_id, Some(id("f")));
    assert_eq!(order(&s), strs(&["a", "la", "f"]));
}

#[test]
fn an_arrow_joins_only_when_attached_to_a_member() {
    let mut bound = arrow("bound", &[(500.0, 500.0), (600.0, 600.0)]);
    bind(&mut bound, false, "a", [0.5, 0.5]);
    let mut s = scene(vec![
        rect("a", 10.0, 10.0, 20.0, 20.0),
        bound,
        arrow("inside", &[(15.0, 15.0), (700.0, 700.0)]),
        arrow("loose", &[(800.0, 800.0), (900.0, 900.0)]),
        frame("f", 0.0, 0.0, 100.0, 100.0),
    ]);
    let r = run(&mut s, |tx| frame::add_to_frame(tx, &id("f"), &ids(&["a", "bound", "inside", "loose"]))).unwrap();
    assert_eq!(r.added, ids(&["a", "bound", "inside"]));
    assert_eq!(r.skipped.len(), 1);
    assert_eq!((r.skipped[0].id.clone(), r.skipped[0].reason), (id("loose"), SkipReason::ArrowNotAttached));
    assert!(get(&s, "loose").frame_id.is_none());
}

#[test]
fn add_refusals() {
    let mut s = scene(vec![
        rect("a", 10.0, 10.0, 20.0, 20.0),
        rect("lk", 10.0, 50.0, 20.0, 20.0).locked(),
        frame("f", 0.0, 0.0, 100.0, 100.0),
        frame("lf", 200.0, 0.0, 100.0, 100.0).locked(),
    ]);
    let rev = s.revision();
    assert!(matches!(
        run(&mut s, |tx| frame::add_to_frame(tx, &id("a"), &ids(&["f"]))),
        Err(FrameError::NotAFrame { .. })
    ));
    assert!(matches!(
        run(&mut s, |tx| frame::add_to_frame(tx, &id("f"), &ids(&["a", "lk"]))),
        Err(FrameError::Target(TargetError::Locked(_)))
    ));
    assert!(matches!(
        run(&mut s, |tx| frame::add_to_frame(tx, &id("lf"), &ids(&["a"]))),
        Err(FrameError::Target(TargetError::Locked(_)))
    ));
    assert!(matches!(
        run(&mut s, |tx| frame::add_to_frame(tx, &id("f"), &ids(&["nope"]))),
        Err(FrameError::Target(TargetError::NotFound(_)))
    ));
    assert_eq!(s.revision(), rev);
}

#[test]
fn moving_to_another_frame_leaves_the_first() {
    let mut s = scene(vec![
        rect("a", 10.0, 10.0, 20.0, 20.0).in_frame("f1"),
        frame("f1", 0.0, 0.0, 100.0, 100.0),
        frame("f2", 200.0, 0.0, 100.0, 100.0),
    ]);
    run(&mut s, |tx| frame::add_to_frame(tx, &id("f2"), &ids(&["a"]))).unwrap();
    assert_eq!(get(&s, "a").frame_id, Some(id("f2")));
    assert!(frame::children_of(&s, &id("f1")).unwrap().is_empty());
    assert_eq!(order(&s), strs(&["f1", "a", "f2"]));
}

#[test]
fn remove_keeps_the_z_order() {
    let mut s = scene(vec![
        rect("a", 10.0, 10.0, 20.0, 20.0).in_frame("f"),
        label("la", "a", 11.0, 11.0, 5.0, 5.0).in_frame("f"),
        rect("b", 40.0, 10.0, 20.0, 20.0).in_frame("f"),
        frame("f", 0.0, 0.0, 100.0, 100.0),
    ]);
    let before = order(&s);
    let gone = run(&mut s, |tx| frame::remove_from_frame(tx, &ids(&["la"]))).unwrap();
    assert_eq!(gone, ids(&["a"]));
    assert!(get(&s, "a").frame_id.is_none() && get(&s, "la").frame_id.is_none());
    assert_eq!(get(&s, "b").frame_id, Some(id("f")));
    assert_eq!(order(&s), before);
    let all = run(&mut s, |tx| frame::remove_all_children(tx, &id("f"))).unwrap();
    assert_eq!(all, ids(&["b"]));
    assert_eq!(run(&mut s, |tx| frame::remove_from_frame(tx, &ids(&["b"]))).unwrap(), Vec::<ElementId>::new());
}

#[test]
fn clip_data_lists_clipping_frames_with_children_in_z_order() {
    let mut s = scene(vec![
        rect("k1", 5.0, 5.0, 10.0, 10.0).in_frame("f"),
        rect("k2", 25.0, 5.0, 10.0, 10.0).in_frame("f"),
        frame("f", 0.0, 0.0, 100.0, 80.0),
        frame("nc", 200.0, 0.0, 50.0, 50.0),
    ]);
    run(&mut s, |tx| {
        tx.update(&id("nc"), |e| {
            if let ElementKind::Frame(d) = &mut e.kind {
                d.clip = false;
            }
        })
    })
    .unwrap();
    let clips = frame::clips(&s);
    assert_eq!(clips.len(), 1);
    assert_eq!(clips[0].frame, id("f"));
    assert_eq!(clips[0].rect, drafftink_geom::kurbo::Rect::new(0.0, 0.0, 100.0, 80.0));
    assert_eq!(clips[0].children, ids(&["k1", "k2"]));
}

#[test]
fn wrap_creates_a_frame_above_the_elements() {
    let mut s = scene(vec![
        rect("a", 10.0, 10.0, 20.0, 20.0),
        rect("x", 500.0, 0.0, 5.0, 5.0),
        rect("b", 50.0, 40.0, 20.0, 20.0),
    ]);
    let mut src = SeqIds::new();
    let r = run(&mut s, |tx| {
        frame::wrap(tx, &ids(&["a", "b"]), &mut src, &WrapOpts { name: Some("Box".into()), padding: 16.0 })
    })
    .unwrap();
    let f = get(&s, r.frame.as_str());
    assert_eq!((f.transform.x, f.transform.y, f.transform.w, f.transform.h), (-6.0, -6.0, 92.0, 82.0));
    assert!(matches!(&f.kind, ElementKind::Frame(d) if d.name.as_deref() == Some("Box") && d.clip));
    assert_eq!(get(&s, "a").frame_id, Some(r.frame.clone()));
    assert_eq!(
        order(&s),
        strs(&["x", "a", "b", r.frame.as_str()]),
        "x keeps its place; the frame sits directly above its children"
    );
    assert_valid(&s);
    assert_eq!(validate_scene(&s).warnings().count(), 0);
}

#[test]
fn wrap_without_padding_for_images_and_nested_frames_refused() {
    let mut s = scene(vec![
        image("i", 10.0, 10.0, 40.0, 20.0),
        rect("r", 60.0, 10.0, 10.0, 10.0),
        frame("f", 300.0, 0.0, 50.0, 50.0),
    ]);
    let mut src = SeqIds::new();
    let r = run(&mut s, |tx| frame::wrap(tx, &ids(&["i", "r"]), &mut src, &WrapOpts::default())).unwrap();
    let f = get(&s, r.frame.as_str());
    assert_eq!((f.transform.x, f.transform.y, f.transform.w, f.transform.h), (10.0, 10.0, 60.0, 20.0));
    let rev = s.revision();
    assert!(matches!(
        run(&mut s, |tx| frame::wrap(tx, &ids(&["f", "i"]), &mut src, &WrapOpts::default())),
        Err(FrameError::NestedFrame(_))
    ));
    assert!(matches!(
        run(&mut s, |tx| frame::wrap(tx, &ids(&["i"]), &mut src, &WrapOpts { padding: -1.0, name: None })),
        Err(FrameError::BadPadding)
    ));
    assert_eq!(s.revision(), rev);
}

#[test]
fn unwrap_releases_the_children_and_deletes_the_frame() {
    let mut s = base();
    run(&mut s, |tx| frame::add_to_frame(tx, &id("f"), &ids(&["a", "b"]))).unwrap();
    let kids = run(&mut s, |tx| frame::unwrap(tx, &id("f"))).unwrap();
    assert_eq!(kids, ids(&["a", "b"]));
    assert!(get(&s, "f").deleted);
    assert!(get(&s, "a").frame_id.is_none() && !get(&s, "a").deleted);
    assert!(matches!(run(&mut s, |tx| frame::unwrap(tx, &id("out"))), Err(FrameError::NotAFrame { .. })));
}

#[test]
fn rename_sets_and_clears_the_label() {
    let mut s = base();
    run(&mut s, |tx| frame::rename(tx, &id("f"), Some("Hero"))).unwrap();
    assert!(matches!(&get(&s, "f").kind, ElementKind::Frame(d) if d.name.as_deref() == Some("Hero")));
    run(&mut s, |tx| frame::rename(tx, &id("f"), None)).unwrap();
    assert!(matches!(&get(&s, "f").kind, ElementKind::Frame(d) if d.name.is_none()));
    assert!(matches!(run(&mut s, |tx| frame::rename(tx, &id("f"), Some(""))), Err(FrameError::BadName)));
    assert!(matches!(run(&mut s, |tx| frame::rename(tx, &id("a"), Some("x"))), Err(FrameError::NotAFrame { .. })));
}

#[test]
fn dropping_over_a_frame_joins_and_dragging_out_leaves() {
    let mut s = base();
    // "out" is moved onto the frame: it joins (centre rule).
    run(&mut s, |tx| transform::move_by(tx, &ids(&["out"]), Vec2::new(-250.0, 20.0))).unwrap();
    let r = run(&mut s, |tx| frame::update_membership_after_move(tx, &ids(&["out"]), None)).unwrap();
    assert_eq!(r.joined, vec![(id("out"), id("f"))]);
    assert_eq!(get(&s, "out").frame_id, Some(id("f")));
    assert_eq!(order(&s), strs(&["a", "b", "out", "f"]));
    // Dragged back out: it leaves.
    run(&mut s, |tx| transform::move_by(tx, &ids(&["out"]), Vec2::new(300.0, 0.0))).unwrap();
    let r = run(&mut s, |tx| frame::update_membership_after_move(tx, &ids(&["out"]), None)).unwrap();
    assert_eq!(r.left, ids(&["out"]));
    assert!(get(&s, "out").frame_id.is_none());
}

#[test]
fn the_pointer_decides_when_given() {
    let mut s = base();
    // The element's centre is outside every frame, but the pointer is over the frame.
    let r = run(&mut s, |tx| frame::update_membership_after_move(tx, &ids(&["out"]), Some(Point::new(50.0, 50.0))))
        .unwrap();
    assert_eq!(r.joined.len(), 1);
}

#[test]
fn a_moved_frame_and_its_children_are_left_alone() {
    let mut s = base();
    run(&mut s, |tx| frame::add_to_frame(tx, &id("f"), &ids(&["a", "b"]))).unwrap();
    run(&mut s, |tx| transform::move_by(tx, &ids(&["f"]), Vec2::new(1000.0, 0.0))).unwrap();
    let r = run(&mut s, |tx| frame::update_membership_after_move(tx, &ids(&["f"]), None)).unwrap();
    assert!(r.joined.is_empty() && r.left.is_empty());
    assert_eq!(get(&s, "a").frame_id, Some(id("f")));
}

#[test]
fn locked_frames_do_not_take_drops() {
    let mut s = scene(vec![frame("f", 0.0, 0.0, 100.0, 100.0).locked(), rect("a", 10.0, 10.0, 10.0, 10.0)]);
    let r = run(&mut s, |tx| frame::update_membership_after_move(tx, &ids(&["a"]), None)).unwrap();
    assert!(r.joined.is_empty());
    assert!(frame::frame_at(&s, Point::new(5.0, 5.0), &BTreeSet::new()).is_none());
}

#[test]
fn capture_takes_what_lies_completely_inside() {
    let mut s = scene(vec![
        rect("in", 10.0, 10.0, 20.0, 20.0),
        rect("half", 90.0, 10.0, 40.0, 20.0),
        rect("g1", 40.0, 40.0, 10.0, 10.0).grouped(&["G"]),
        rect("g2", 400.0, 40.0, 10.0, 10.0).grouped(&["G"]),
        rect("lk", 60.0, 60.0, 10.0, 10.0).locked(),
        frame("inner", 60.0, 80.0, 10.0, 10.0),
        frame("f", 0.0, 0.0, 100.0, 100.0),
    ]);
    let r = run(&mut s, |tx| frame::capture_contained(tx, &id("f"))).unwrap();
    assert_eq!(r.added, ids(&["in"]), "partly outside, locked, frames and half-inside groups stay out");
    assert_eq!(get(&s, "in").frame_id, Some(id("f")));
    assert_eq!(order(&s).last().unwrap(), "f");
}

#[test]
fn resizing_a_frame_recomputes_membership_on_the_committed_run() {
    let mut s = scene(vec![
        rect("a", 10.0, 10.0, 20.0, 20.0).in_frame("f"),
        rect("b", 60.0, 10.0, 20.0, 20.0).in_frame("f"),
        rect("c", 120.0, 10.0, 20.0, 20.0),
        frame("f", 0.0, 0.0, 100.0, 100.0),
    ]);
    // Shrink so that b is outside, and grow elsewhere so that c is inside: first shrink to 50 wide.
    let spec = ResizeSpec {
        handle: BoxHandle::Corner(Corner::BottomRight),
        delta: Vec2::new(-50.0, 0.0),
        keep_aspect: false,
        from_center: false,
    };
    run(&mut s, |tx| transform::resize(tx, &ids(&["f"]), spec)).unwrap();
    assert_eq!(get(&s, "a").transform.x, 10.0, "children are not scaled with their frame");
    let r = run(&mut s, |tx| frame::recompute_after_resize(tx, &id("f"))).unwrap();
    assert_eq!(r.left, ids(&["b"]));
    assert!(get(&s, "b").frame_id.is_none() && get(&s, "a").frame_id.is_some());
    // Grow to 200 wide: c is completely inside and joins; b (60..80) joins too.
    let spec = ResizeSpec {
        handle: BoxHandle::Corner(Corner::BottomRight),
        delta: Vec2::new(150.0, 0.0),
        keep_aspect: false,
        from_center: false,
    };
    run(&mut s, |tx| transform::resize(tx, &ids(&["f"]), spec)).unwrap();
    let r = run(&mut s, |tx| frame::recompute_after_resize(tx, &id("f"))).unwrap();
    assert_eq!(r.joined.len(), 2);
    assert_eq!(get(&s, "c").frame_id, Some(id("f")));
    assert_valid(&s);
}

#[test]
fn leaving_a_frame_keeps_the_block_of_children_contiguous() {
    // Found by the random-operation test: a leaver that stayed in place split the block of its old frame.
    let mut s = scene(vec![
        rect("k1", 5.0, 5.0, 10.0, 10.0).in_frame("f"),
        rect("k2", 25.0, 5.0, 10.0, 10.0).in_frame("f"),
        rect("k3", 45.0, 5.0, 10.0, 10.0).in_frame("f"),
        frame("f", 0.0, 0.0, 100.0, 100.0),
    ]);
    run(&mut s, |tx| frame::remove_from_frame(tx, &ids(&["k2"]))).unwrap();
    assert_eq!(order(&s), strs(&["k2", "k1", "k3", "f"]), "the leaver goes below the lowest remaining child");
    assert_eq!(frame::children_of(&s, &id("f")).unwrap(), ids(&["k1", "k3"]));
    // With the frame in a group the leaver goes above the group instead.
    let mut s = scene(vec![
        rect("k1", 5.0, 5.0, 10.0, 10.0).in_frame("f"),
        rect("k2", 25.0, 5.0, 10.0, 10.0).in_frame("f"),
        frame("f", 0.0, 0.0, 100.0, 100.0).grouped(&["G"]),
        rect("m", 300.0, 0.0, 10.0, 10.0).grouped(&["G"]),
        rect("top", 500.0, 0.0, 10.0, 10.0),
    ]);
    run(&mut s, |tx| frame::remove_from_frame(tx, &ids(&["k1"]))).unwrap();
    assert_eq!(order(&s), strs(&["k2", "f", "m", "k1", "top"]));
    assert!(drafftink_edit::group::non_contiguous_groups(&s).is_empty());
}

#[test]
fn wrapping_a_child_does_not_split_the_block_of_its_old_frame() {
    let mut s = scene(vec![
        rect("k1", 5.0, 5.0, 10.0, 10.0).in_frame("f"),
        rect("k2", 25.0, 5.0, 10.0, 10.0).in_frame("f"),
        rect("k3", 45.0, 5.0, 10.0, 10.0).in_frame("f"),
        frame("f", 0.0, 0.0, 100.0, 100.0),
    ]);
    let mut src = SeqIds::new();
    let r = run(&mut s, |tx| frame::wrap(tx, &ids(&["k2"]), &mut src, &WrapOpts::default())).unwrap();
    let o = order(&s);
    let at = |n: &str| o.iter().position(|x| x == n).unwrap();
    assert!(at("k1") < at("k3") && at("k3") < at("f") && at("k3") == at("k1") + 1, "f keeps k1 and k3 together: {o:?}");
    assert!(at("k2") < at(r.frame.as_str()));
    assert_valid(&s);
    assert_eq!(validate_scene(&s).warnings().count(), 0);
}
