#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Align, distribute and flip.

mod structure_support;

use std::f64::consts::PI;

use drafftink_edit::arrange::{self, AlignMode, ArrangeError, Axis, DistributeOpts, FlipAxis};
use drafftink_edit::transform::geometry::{world_bounds, world_points};
use drafftink_scene::*;
use structure_support::*;

fn x(s: &MemoryStore, n: &str) -> f64 {
    get(s, n).transform.x
}

fn three() -> MemoryStore {
    scene(vec![rect("a", 10.0, 0.0, 20.0, 10.0), rect("b", 50.0, 30.0, 40.0, 10.0), rect("c", 120.0, 60.0, 10.0, 10.0)])
}

#[test]
fn align_edges_and_centres_follow_the_axis() {
    let mut s = three();
    run(&mut s, |tx| arrange::align(tx, &ids(&["a", "b", "c"]), AlignMode::Left, None, None)).unwrap();
    assert!(near(x(&s, "a"), 10.0) && near(x(&s, "b"), 10.0) && near(x(&s, "c"), 10.0));
    let mut s = three();
    run(&mut s, |tx| arrange::align(tx, &ids(&["a", "b", "c"]), AlignMode::Right, None, None)).unwrap();
    assert!(near(x(&s, "a") + 20.0, 130.0) && near(x(&s, "b") + 40.0, 130.0));
    let mut s = three();
    run(&mut s, |tx| arrange::align(tx, &ids(&["a", "b", "c"]), AlignMode::CenterX, None, None)).unwrap();
    let cx: Vec<f64> = ["a", "b", "c"].iter().map(|n| get(&s, n).transform.center().x).collect();
    assert!(cx.iter().all(|v| near(*v, 70.0)), "{cx:?}");
    let mut s = three();
    run(&mut s, |tx| arrange::align(tx, &ids(&["a", "b", "c"]), AlignMode::CenterY, None, None)).unwrap();
    let cy: Vec<f64> = ["a", "b", "c"].iter().map(|n| get(&s, n).transform.center().y).collect();
    assert!(cy.iter().all(|v| near(*v, 35.0)), "{cy:?}");
    let mut s = three();
    run(&mut s, |tx| arrange::align(tx, &ids(&["a", "b", "c"]), AlignMode::Top, None, None)).unwrap();
    assert!(near(get(&s, "c").transform.y, 0.0));
    run(&mut s, |tx| arrange::align(tx, &ids(&["a", "b", "c"]), AlignMode::Bottom, None, None)).unwrap();
    assert!(near(get(&s, "a").transform.y, 0.0), "the tallest bottom is 10");
}

#[test]
fn a_group_aligns_as_one_unit() {
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 10.0, 10.0).grouped(&["g"]),
        rect("b", 100.0, 0.0, 10.0, 10.0).grouped(&["g"]),
        rect("c", 300.0, 50.0, 10.0, 10.0),
    ]);
    run(&mut s, |tx| arrange::align(tx, &ids(&["a", "b", "c"]), AlignMode::Right, None, None)).unwrap();
    // Only c moves: the group's right edge (110) is the leftmost-right; c moves to x1 = 310 ... the union decides.
    assert!(
        near(x(&s, "a"), 200.0) && near(x(&s, "b"), 300.0),
        "the group moved as one block: {} {}",
        x(&s, "a"),
        x(&s, "b")
    );
    assert!(near(x(&s, "c"), 300.0));
}

#[test]
fn one_group_and_nothing_else_is_too_few() {
    let mut s =
        scene(vec![rect("a", 0.0, 0.0, 10.0, 10.0).grouped(&["g"]), rect("b", 100.0, 0.0, 10.0, 10.0).grouped(&["g"])]);
    let r = run(&mut s, |tx| arrange::align(tx, &ids(&["a", "b"]), AlignMode::Left, None, None));
    assert!(matches!(r, Err(ArrangeError::TooFew { need: 2, have: 1, .. })));
    // Inside the editing group the members are separate units.
    run(&mut s, |tx| arrange::align(tx, &ids(&["a", "b"]), AlignMode::Left, None, Some(&gid("g")))).unwrap();
    assert!(near(x(&s, "b"), 0.0));
}

#[test]
fn align_to_a_reference_element_keeps_it_still() {
    let mut s = three();
    run(&mut s, |tx| arrange::align(tx, &ids(&["a", "c"]), AlignMode::Left, Some(&id("b")), None)).unwrap();
    assert!(near(x(&s, "a"), 50.0) && near(x(&s, "c"), 50.0) && near(x(&s, "b"), 50.0));
    // The reference alone is not enough.
    let r = run(&mut s, |tx| arrange::align(tx, &ids(&["b"]), AlignMode::Left, Some(&id("b")), None));
    assert!(matches!(r, Err(ArrangeError::TooFew { need: 1, .. })));
}

#[test]
fn align_uses_the_rotated_bounds() {
    let mut s = scene(vec![rect("a", 0.0, 0.0, 100.0, 10.0).angle(PI / 2.0), rect("b", 200.0, 0.0, 10.0, 10.0)]);
    run(&mut s, |tx| arrange::align(tx, &ids(&["a", "b"]), AlignMode::Left, None, None)).unwrap();
    let ba = world_bounds(get(&s, "a"));
    let bb = world_bounds(get(&s, "b"));
    assert!(near(ba.x0, bb.x0));
}

#[test]
fn align_with_locked_or_missing_elements_changes_nothing() {
    let mut s = scene(vec![rect("a", 0.0, 0.0, 10.0, 10.0), rect("b", 50.0, 0.0, 10.0, 10.0).locked()]);
    let rev = s.revision();
    assert!(matches!(
        run(&mut s, |tx| arrange::align(tx, &ids(&["a", "b"]), AlignMode::Right, None, None)),
        Err(ArrangeError::Locked(_))
    ));
    assert!(matches!(
        run(&mut s, |tx| arrange::align(tx, &ids(&["a", "zz"]), AlignMode::Right, None, None)),
        Err(ArrangeError::NotFound(_))
    ));
    assert_eq!(s.revision(), rev);
}

#[test]
fn bound_text_travels_with_an_aligned_container() {
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 100.0, 50.0),
        label("la", "a", 25.0, 15.0, 50.0, 20.0),
        rect("b", 200.0, 100.0, 100.0, 50.0),
    ]);
    run(&mut s, |tx| arrange::align(tx, &ids(&["a", "b"]), AlignMode::Left, None, None)).unwrap();
    assert!(near(x(&s, "b"), 0.0));
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 100.0, 50.0),
        label("la", "a", 25.0, 15.0, 50.0, 20.0),
        rect("b", 200.0, 100.0, 100.0, 50.0),
    ]);
    run(&mut s, |tx| arrange::align(tx, &ids(&["a", "b"]), AlignMode::Right, None, None)).unwrap();
    assert!(near(x(&s, "a"), 200.0) && near(x(&s, "la"), 225.0));
}

#[test]
fn arrows_keep_bindings_to_aligned_targets() {
    let mut ar = arrow("arr", &[(100.0, 25.0), (150.0, 100.0)]);
    bind(&mut ar, true, "t", [1.0, 0.5]);
    let mut s = scene(vec![rect("t", 0.0, 0.0, 100.0, 50.0), rect("o", 300.0, 200.0, 50.0, 50.0), ar]);
    run(&mut s, |tx| arrange::align(tx, &ids(&["t", "o", "arr"]), AlignMode::Top, None, None)).unwrap();
    assert!(get(&s, "arr").linear_data().unwrap().start_binding.is_some());
    // Without the target in the set the arrow would detach.
    let mut ar = arrow("arr", &[(100.0, 25.0), (150.0, 100.0)]);
    bind(&mut ar, true, "t", [1.0, 0.5]);
    let mut s = scene(vec![rect("t", 0.0, 0.0, 100.0, 50.0), rect("o", 300.0, 200.0, 50.0, 50.0), ar]);
    run(&mut s, |tx| arrange::align(tx, &ids(&["o", "arr"]), AlignMode::Top, None, None)).unwrap();
    assert!(get(&s, "arr").linear_data().unwrap().start_binding.is_none());
}

// ---- distribute ------------------------------------------------------------------------------------------------

#[test]
fn distribute_equalises_the_gaps_and_keeps_the_outer_two() {
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 10.0, 10.0),
        rect("b", 15.0, 0.0, 30.0, 10.0),
        rect("c", 100.0, 0.0, 10.0, 10.0),
    ]);
    run(&mut s, |tx| arrange::distribute(tx, &ids(&["a", "b", "c"]), Axis::X, DistributeOpts::default(), None))
        .unwrap();
    assert!(near(x(&s, "a"), 0.0) && near(x(&s, "c"), 100.0));
    // Free space 110 - (10 + 30 + 10) = 60, two gaps of 30.
    assert!(near(x(&s, "b"), 40.0), "{}", x(&s, "b"));
}

#[test]
fn distribute_with_an_exact_gap() {
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 10.0, 10.0),
        rect("b", 15.0, 0.0, 30.0, 10.0),
        rect("c", 100.0, 0.0, 10.0, 10.0),
    ]);
    run(&mut s, |tx| arrange::distribute(tx, &ids(&["a", "b", "c"]), Axis::X, DistributeOpts { gap: Some(5.0) }, None))
        .unwrap();
    assert!(
        near(x(&s, "a"), 0.0) && near(x(&s, "b"), 15.0) && near(x(&s, "c"), 50.0),
        "{} {} {}",
        x(&s, "a"),
        x(&s, "b"),
        x(&s, "c")
    );
    assert!(matches!(
        run(&mut s, |tx| arrange::distribute(
            tx,
            &ids(&["a", "b", "c"]),
            Axis::X,
            DistributeOpts { gap: Some(f64::NAN) },
            None
        )),
        Err(ArrangeError::BadGap)
    ));
}

#[test]
fn distribute_vertically_and_needs_three_units() {
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 10.0, 10.0),
        rect("b", 0.0, 12.0, 10.0, 10.0),
        rect("c", 0.0, 100.0, 10.0, 10.0),
    ]);
    run(&mut s, |tx| arrange::distribute(tx, &ids(&["a", "b", "c"]), Axis::Y, DistributeOpts::default(), None))
        .unwrap();
    assert!(near(get(&s, "b").transform.y, 50.0));
    let r = run(&mut s, |tx| arrange::distribute(tx, &ids(&["a", "b"]), Axis::Y, DistributeOpts::default(), None));
    assert!(matches!(r, Err(ArrangeError::TooFew { need: 3, have: 2, .. })));
}

#[test]
fn distribute_moves_groups_as_units() {
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 10.0, 10.0),
        rect("g1", 20.0, 0.0, 10.0, 10.0).grouped(&["g"]),
        rect("g2", 40.0, 0.0, 10.0, 10.0).grouped(&["g"]),
        rect("c", 200.0, 0.0, 10.0, 10.0),
    ]);
    run(&mut s, |tx| arrange::distribute(tx, &ids(&["a", "g1", "g2", "c"]), Axis::X, DistributeOpts::default(), None))
        .unwrap();
    // Group spans 20..50 (width 30); free = 210 - 10 - 30 - 10 = 160; gap 80 -> group at 90.
    assert!(near(x(&s, "g1"), 90.0) && near(x(&s, "g2"), 110.0), "{} {}", x(&s, "g1"), x(&s, "g2"));
    assert!(near(x(&s, "g2") - x(&s, "g1"), 20.0), "inner spacing is untouched");
}

// ---- flip ------------------------------------------------------------------------------------------------------

#[test]
fn old_flip_lands_on_the_mirrored_side() {
    let mut s = scene(vec![rect("a", 0.0, 0.0, 20.0, 10.0), rect("b", 80.0, 0.0, 20.0, 10.0)]);
    run(&mut s, |tx| arrange::flip(tx, &ids(&["a", "b"]), FlipAxis::Horizontal)).unwrap();
    assert!(near(x(&s, "a"), 80.0) && near(x(&s, "b"), 0.0));
    assert!(near(get(&s, "a").transform.w, 20.0), "sizes stay positive");
    assert_valid(&s);
}

#[test]
fn flipping_twice_restores_everything() {
    let mut s = scene(vec![
        rect("a", 3.0, 4.0, 20.0, 10.0).angle(0.5),
        line("l", &[(0.0, 0.0), (30.0, 10.0), (40.0, 50.0)]),
        freehand("f", &[(100.0, 100.0), (120.0, 90.0), (110.0, 130.0)]),
        image("i", 200.0, 20.0, 40.0, 20.0),
        text("t", 60.0, 80.0, 50.0, 20.0, "hi").angle(0.3),
    ]);
    let before: Vec<Element> = s.iter().cloned().collect();
    for axis in [FlipAxis::Horizontal, FlipAxis::Vertical] {
        run(&mut s, |tx| arrange::flip(tx, &ids(&["a", "l", "f", "i", "t"]), axis)).unwrap();
        run(&mut s, |tx| arrange::flip(tx, &ids(&["a", "l", "f", "i", "t"]), axis)).unwrap();
        for b in &before {
            let a = get(&s, b.id.as_str());
            assert!(
                near(a.transform.x, b.transform.x) && near(a.transform.y, b.transform.y),
                "{}: {:?} vs {:?}",
                b.id,
                a.transform,
                b.transform
            );
            assert!(near(a.transform.w, b.transform.w) && near(a.transform.h, b.transform.h), "{}", b.id);
            assert!(
                near(a.transform.angle, b.transform.angle),
                "{}: {} vs {}",
                b.id,
                a.transform.angle,
                b.transform.angle
            );
            assert_eq!((a.transform.flip_x, a.transform.flip_y), (b.transform.flip_x, b.transform.flip_y), "{}", b.id);
            if let (Some(pa), Some(pb)) = (world_points(a), world_points(b)) {
                for (p, q) in pa.iter().zip(pb) {
                    assert!((*p - q).hypot() < 1e-9, "{}", b.id);
                }
            }
        }
    }
    assert_valid(&s);
}

#[test]
fn flip_negates_the_angle_and_mirrors_points() {
    let mut s = scene(vec![rect("a", 0.0, 0.0, 20.0, 10.0).angle(0.5), line("l", &[(100.0, 0.0), (140.0, 30.0)])]);
    let before = drafftink_edit::transform::common_bounds(&s, &ids(&["a", "l"])).unwrap();
    run(&mut s, |tx| arrange::flip(tx, &ids(&["a", "l"]), FlipAxis::Horizontal)).unwrap();
    assert!(near(get(&s, "a").transform.angle, 2.0 * PI - 0.5), "{}", get(&s, "a").transform.angle);
    let p = world_points(get(&s, "l")).unwrap();
    let sum = before.x0 + before.x1;
    assert!(near(p[0].x, sum - 100.0) && near(p[1].x, sum - 140.0), "{p:?}");
    assert!(near(p[0].y, 0.0) && near(p[1].y, 30.0));
    let after = drafftink_edit::transform::common_bounds(&s, &ids(&["a", "l"])).unwrap();
    assert!(near(after.x0, before.x0) && near(after.x1, before.x1));
    assert_valid(&s);
}

#[test]
fn flip_keeps_text_upright_and_toggles_image_flags() {
    let mut s = scene(vec![text("t", 0.0, 0.0, 50.0, 20.0, "hi"), image("i", 100.0, 0.0, 40.0, 20.0)]);
    run(&mut s, |tx| arrange::flip(tx, &ids(&["t", "i"]), FlipAxis::Horizontal)).unwrap();
    let t = get(&s, "t").transform;
    assert!(near(t.x, 90.0) && near(t.w, 50.0) && near(t.angle, 0.0) && !t.flip_x, "text only moves: {t:?}");
    let i = get(&s, "i").transform;
    assert!(i.flip_x && !i.flip_y && near(i.angle, 0.0), "{i:?}");
    assert!(near(i.x, 0.0));
    run(&mut s, |tx| arrange::flip(tx, &ids(&["i"]), FlipAxis::Vertical)).unwrap();
    let i = get(&s, "i").transform;
    assert!(i.flip_x && i.flip_y && near(i.angle, 0.0), "{i:?}");
    // A rotated text keeps its glyphs upright: its angle is negated.
    let mut s = scene(vec![text("t", 0.0, 0.0, 50.0, 20.0, "hi").angle(0.4), rect("r", 100.0, 0.0, 10.0, 10.0)]);
    run(&mut s, |tx| arrange::flip(tx, &ids(&["t", "r"]), FlipAxis::Horizontal)).unwrap();
    assert!(near(get(&s, "t").transform.angle, 2.0 * PI - 0.4));
}

#[test]
fn old_flipping_a_group_does_not_displace_children() {
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 10.0, 10.0).grouped(&["g"]).angle(0.3),
        ellipse("b", 50.0, 20.0, 30.0, 10.0).grouped(&["g"]),
        line("c", &[(10.0, 40.0), (90.0, 60.0)]).grouped(&["g"]),
    ]);
    let bounds_before = drafftink_edit::transform::common_bounds(&s, &ids(&["a", "b", "c"])).unwrap();
    run(&mut s, |tx| arrange::flip(tx, &ids(&["a", "b", "c"]), FlipAxis::Horizontal)).unwrap();
    let bounds_after = drafftink_edit::transform::common_bounds(&s, &ids(&["a", "b", "c"])).unwrap();
    assert!(
        near(bounds_before.x0, bounds_after.x0) && near(bounds_before.x1, bounds_after.x1),
        "{bounds_before:?} {bounds_after:?}"
    );
    assert!(near(bounds_before.y0, bounds_after.y0) && near(bounds_before.y1, bounds_after.y1));
    // b was on the right, so it is now on the left, and still inside the common box.
    let mid = (bounds_before.x0 + bounds_before.x1) / 2.0;
    assert!(near(get(&s, "b").transform.center().x, 2.0 * mid - 65.0), "b mirrors about the common centre line");
    assert!(group_stacks_intact(&s));
}

fn group_stacks_intact(s: &MemoryStore) -> bool {
    ["a", "b", "c"].iter().all(|n| stack(s, n) == strs(&["g"]))
}

#[test]
fn flip_mirrors_binding_fixed_points_when_the_target_is_flipped_too() {
    let mut ar = arrow("arr", &[(100.0, 25.0), (250.0, 25.0)]);
    bind(&mut ar, true, "t", [1.0, 0.5]);
    let mut s = scene(vec![rect("t", 0.0, 0.0, 100.0, 50.0), ar, rect("far", 250.0, 100.0, 50.0, 50.0)]);
    run(&mut s, |tx| arrange::flip(tx, &ids(&["t", "arr", "far"]), FlipAxis::Horizontal)).unwrap();
    let b = get(&s, "arr").linear_data().unwrap().start_binding.clone().unwrap();
    assert!(near(b.fixed_point[0], 0.0) && near(b.fixed_point[1], 0.5), "{:?}", b.fixed_point);
    assert!(near(get(&s, "t").transform.x, 200.0));
    // The arrow start (100,25) is now at (200, 25): on the mirrored target's left edge.
    let p = world_points(get(&s, "arr")).unwrap();
    assert!(near(p[0].x, 200.0) && near(p[1].x, 50.0), "{p:?}");
}

#[test]
fn flipping_an_arrow_alone_unbinds_it() {
    let mut ar = arrow("arr", &[(100.0, 25.0), (250.0, 25.0)]);
    bind(&mut ar, true, "t", [1.0, 0.5]);
    let mut s = scene(vec![rect("t", 0.0, 0.0, 100.0, 50.0), ar]);
    let r = run(&mut s, |tx| arrange::flip(tx, &ids(&["arr"]), FlipAxis::Horizontal)).unwrap();
    assert_eq!(r.unbound.len(), 1);
    assert!(get(&s, "arr").linear_data().unwrap().start_binding.is_none());
}

#[test]
fn flip_vertical_mirrors_about_the_horizontal_centre_line() {
    let mut s = scene(vec![rect("a", 0.0, 0.0, 10.0, 20.0), rect("b", 0.0, 80.0, 10.0, 20.0)]);
    run(&mut s, |tx| arrange::flip(tx, &ids(&["a", "b"]), FlipAxis::Vertical)).unwrap();
    assert!(near(get(&s, "a").transform.y, 80.0) && near(get(&s, "b").transform.y, 0.0));
}
