#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Move, nudge, resize, rotate, set-bounds. Old tests that survive are marked `old_`.

mod structure_support;

use std::f64::consts::{FRAC_PI_2, PI};

use drafftink_edit::TargetError;
use drafftink_edit::group;
use drafftink_edit::transform::{
    self, BoundsSpec, End, ResizeSpec, RotateMode, TransformError, geometry::world_bounds, geometry::world_points,
};
use drafftink_geom::select::{BoxHandle, Corner, Edge};
use drafftink_scene::*;
use kurbo::{Point, Vec2};
use structure_support::*;

fn br(dx: f64, dy: f64) -> ResizeSpec {
    ResizeSpec {
        handle: BoxHandle::Corner(Corner::BottomRight),
        delta: Vec2::new(dx, dy),
        keep_aspect: false,
        from_center: false,
    }
}

fn corner(e: &Element, which: usize) -> Point {
    drafftink_edit::transform::geometry::obox(e).corners()[which]
}

#[test]
fn move_translates_and_reports() {
    let mut s = scene(vec![rect("a", 0.0, 0.0, 10.0, 10.0), rect("b", 50.0, 0.0, 10.0, 10.0)]);
    let r = run(&mut s, |tx| transform::move_by(tx, &ids(&["a"]), Vec2::new(5.0, -3.0))).unwrap();
    assert_eq!(r.changed, ids(&["a"]));
    assert_eq!((get(&s, "a").transform.x, get(&s, "a").transform.y), (5.0, -3.0));
    assert_eq!(get(&s, "b").transform.x, 50.0);
    assert_valid(&s);
}

#[test]
fn zero_move_changes_nothing() {
    let mut s = scene(vec![rect("a", 0.0, 0.0, 10.0, 10.0)]);
    let rev = s.revision();
    let r = run(&mut s, |tx| transform::move_by(tx, &ids(&["a"]), Vec2::ZERO)).unwrap();
    assert!(r.changed.is_empty());
    assert_eq!(s.revision(), rev);
}

#[test]
fn moving_a_group_moves_every_member() {
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 10.0, 10.0).grouped(&["g"]),
        rect("b", 50.0, 0.0, 10.0, 10.0).grouped(&["g"]),
        rect("c", 90.0, 0.0, 10.0, 10.0),
    ]);
    run(&mut s, |tx| transform::move_by(tx, &ids(&["a", "b"]), Vec2::new(0.0, 7.0))).unwrap();
    assert_eq!(get(&s, "a").transform.y, 7.0);
    assert_eq!(get(&s, "b").transform.y, 7.0);
    assert_eq!(get(&s, "c").transform.y, 0.0);
}

#[test]
fn a_frame_brings_its_children_and_bound_text() {
    let mut s = scene(vec![
        rect("kid", 5.0, 5.0, 10.0, 10.0).in_frame("f"),
        label("kl", "kid", 6.0, 6.0, 5.0, 5.0).in_frame("f"),
        frame("f", 0.0, 0.0, 100.0, 100.0),
        rect("other", 200.0, 0.0, 10.0, 10.0),
    ]);
    let r = run(&mut s, |tx| transform::move_by(tx, &ids(&["f"]), Vec2::new(10.0, 20.0))).unwrap();
    assert_eq!(r.changed.len(), 3);
    assert_eq!((get(&s, "kid").transform.x, get(&s, "kid").transform.y), (15.0, 25.0));
    assert_eq!((get(&s, "kl").transform.x, get(&s, "kl").transform.y), (16.0, 26.0));
    assert_eq!(get(&s, "other").transform.x, 200.0);
}

#[test]
fn bound_text_follows_its_container_when_the_text_id_is_given() {
    let mut s = scene(vec![rect("a", 0.0, 0.0, 100.0, 50.0), label("la", "a", 10.0, 10.0, 50.0, 20.0)]);
    run(&mut s, |tx| transform::move_by(tx, &ids(&["la"]), Vec2::new(4.0, 0.0))).unwrap();
    assert_eq!(get(&s, "a").transform.x, 4.0);
    assert_eq!(get(&s, "la").transform.x, 14.0);
}

#[test]
fn locked_elements_refuse_everything_and_change_nothing() {
    let mut s = scene(vec![rect("a", 0.0, 0.0, 10.0, 10.0), rect("b", 50.0, 0.0, 10.0, 10.0).locked()]);
    let rev = s.revision();
    let r = run(&mut s, |tx| transform::move_by(tx, &ids(&["a", "b"]), Vec2::new(1.0, 1.0)));
    assert!(matches!(r, Err(TransformError::Target(TargetError::Locked(ref v))) if v == &ids(&["b"])));
    assert_eq!(s.revision(), rev);
    let r = run(&mut s, |tx| transform::rotate(tx, &ids(&["a", "b"]), 1.0, RotateMode::By, None));
    assert!(matches!(r, Err(TransformError::Target(TargetError::Locked(_)))));
    let r = run(&mut s, |tx| transform::resize(tx, &ids(&["a", "b"]), br(5.0, 5.0)));
    assert!(matches!(r, Err(TransformError::Target(TargetError::Locked(_)))));
}

#[test]
fn hostile_numbers_are_refused_without_changes() {
    let mut s = scene(vec![rect("a", 0.0, 0.0, 10.0, 10.0)]);
    let rev = s.revision();
    assert!(matches!(
        run(&mut s, |tx| transform::move_by(tx, &ids(&["a"]), Vec2::new(f64::NAN, 0.0))),
        Err(TransformError::NotFinite)
    ));
    assert!(matches!(
        run(&mut s, |tx| transform::move_by(tx, &ids(&["a"]), Vec2::new(1e300, 0.0))),
        Err(TransformError::Invalid { .. })
    ));
    assert!(matches!(
        run(&mut s, |tx| transform::rotate(tx, &ids(&["a"]), f64::INFINITY, RotateMode::By, None)),
        Err(TransformError::NotFinite)
    ));
    assert!(matches!(
        run(&mut s, |tx| transform::move_by(tx, &[], Vec2::new(1.0, 0.0))),
        Err(TransformError::NoElements)
    ));
    assert!(matches!(
        run(&mut s, |tx| transform::move_by(tx, &ids(&["zz"]), Vec2::new(1.0, 0.0))),
        Err(TransformError::Target(TargetError::NotFound(_)))
    ));
    assert_eq!(s.revision(), rev);
}

#[test]
fn an_error_in_the_middle_leaves_every_element_untouched() {
    // b would leave the allowed range; a must not be moved either.
    let mut s = scene(vec![rect("a", 0.0, 0.0, 10.0, 10.0), rect("b", 999_999_990.0, 0.0, 5.0, 5.0)]);
    let rev = s.revision();
    let r = run(&mut s, |tx| transform::move_by(tx, &ids(&["a", "b"]), Vec2::new(100.0, 0.0)));
    assert!(matches!(r, Err(TransformError::Invalid { .. })));
    assert_eq!(s.revision(), rev);
    assert_eq!(get(&s, "a").transform.x, 0.0);
}

// ---- arrows and bindings ---------------------------------------------------------------------------------------

fn bound_pair() -> MemoryStore {
    let mut a = arrow("arr", &[(100.0, 50.0), (200.0, 50.0)]);
    bind(&mut a, true, "t", [1.0, 0.5]);
    scene(vec![rect("t", 0.0, 0.0, 100.0, 100.0), a])
}

fn end_bound(s: &MemoryStore, which: End) -> bool {
    let d = get(s, "arr").linear_data().unwrap().clone();
    match which {
        End::Start => d.start_binding.is_some(),
        End::End => d.end_binding.is_some(),
    }
}

#[test]
fn an_arrow_moved_alone_unbinds_the_end_whose_target_stays() {
    let mut s = bound_pair();
    let r = run(&mut s, |tx| transform::move_by(tx, &ids(&["arr"]), Vec2::new(0.0, 30.0))).unwrap();
    assert_eq!(r.unbound, vec![(id("arr"), End::Start)]);
    assert!(!end_bound(&s, End::Start));
    assert_eq!(get(&s, "arr").transform.y, 80.0);
}

#[test]
fn an_arrow_moved_with_its_target_keeps_the_binding() {
    let mut s = bound_pair();
    let r = run(&mut s, |tx| transform::move_by(tx, &ids(&["t", "arr"]), Vec2::new(10.0, 10.0))).unwrap();
    assert!(r.unbound.is_empty());
    assert!(end_bound(&s, End::Start));
    let b = get(&s, "arr").linear_data().unwrap().start_binding.clone().unwrap();
    assert_eq!(b.fixed_point, [1.0, 0.5]);
    assert_valid(&s);
}

#[test]
fn nudge_follows_the_same_rule() {
    let mut s = bound_pair();
    run(&mut s, |tx| transform::nudge(tx, &ids(&["arr"]), 1.0, 0.0)).unwrap();
    assert!(!end_bound(&s, End::Start));
    let mut s = bound_pair();
    run(&mut s, |tx| transform::nudge(tx, &ids(&["t", "arr"]), 1.0, 0.0)).unwrap();
    assert!(end_bound(&s, End::Start));
}

#[test]
fn rotating_or_resizing_a_single_arrow_unbinds_it() {
    let mut s = bound_pair();
    run(&mut s, |tx| transform::rotate(tx, &ids(&["arr"]), 0.3, RotateMode::By, None)).unwrap();
    assert!(!end_bound(&s, End::Start));
    let mut s = bound_pair();
    run(&mut s, |tx| transform::resize(tx, &ids(&["arr"]), br(20.0, 0.0))).unwrap();
    assert!(!end_bound(&s, End::Start));
}

#[test]
fn resizing_target_and_arrow_together_keeps_binding_and_ratio() {
    let mut s = bound_pair();
    run(&mut s, |tx| transform::resize(tx, &ids(&["t", "arr"]), br(100.0, 100.0))).unwrap();
    assert!(end_bound(&s, End::Start));
    // The common box is 0..200 wide and 0..100 high (the arrow sticks out to the right), so the scale is 1.5 by 2.
    let t = get(&s, "t");
    assert!(near(t.transform.w, 150.0) && near(t.transform.h, 200.0), "{:?}", t.transform);
    let b = get(&s, "arr").linear_data().unwrap().start_binding.clone().unwrap();
    assert!(near(b.fixed_point[0], 1.0) && near(b.fixed_point[1], 0.5));
}

// ---- resize ----------------------------------------------------------------------------------------------------

#[test]
fn old_rectangle_corner_resize() {
    let mut s = scene(vec![rect("r", 0.0, 0.0, 100.0, 100.0)]);
    let r = run(&mut s, |tx| transform::resize(tx, &ids(&["r"]), br(50.0, 50.0))).unwrap();
    let t = get(&s, "r").transform;
    assert!(near(t.w, 150.0) && near(t.h, 150.0));
    assert!(near(t.x, 0.0) && near(t.y, 0.0), "the opposite corner stays");
    assert!(near(r.bounds.unwrap().x1, 150.0));
}

#[test]
fn old_aspect_ratio_is_kept() {
    let mut s = scene(vec![rect("r", 0.0, 0.0, 100.0, 50.0)]);
    let spec = ResizeSpec { keep_aspect: true, ..br(100.0, 100.0) };
    run(&mut s, |tx| transform::resize(tx, &ids(&["r"]), spec)).unwrap();
    let t = get(&s, "r").transform;
    assert!(near(t.w / t.h, 2.0), "{t:?}");
    assert!(near(t.x, 0.0) && near(t.y, 0.0), "every corner keeps its anchor");
}

#[test]
fn old_image_resize_keeps_the_aspect_ratio() {
    let mut s = scene(vec![image("i", 0.0, 0.0, 100.0, 100.0)]);
    run(&mut s, |tx| transform::resize(tx, &ids(&["i"]), br(50.0, 50.0))).unwrap();
    let t = get(&s, "i").transform;
    assert!(near(t.w, 150.0) && near(t.h, 150.0));
    // Non-square image: a lopsided drag still keeps the ratio.
    let mut s = scene(vec![image("i", 0.0, 0.0, 100.0, 50.0)]);
    run(&mut s, |tx| transform::resize(tx, &ids(&["i"]), br(100.0, 0.0))).unwrap();
    let t = get(&s, "i").transform;
    assert!(near(t.w / t.h, 2.0));
}

#[test]
fn old_freehand_resize_scales_the_points() {
    let mut s = scene(vec![freehand("f", &[(0.0, 0.0), (50.0, 0.0), (50.0, 50.0), (0.0, 50.0)])]);
    run(&mut s, |tx| transform::resize(tx, &ids(&["f"]), br(50.0, 50.0))).unwrap();
    let b = world_bounds(get(&s, "f"));
    assert!(near(b.width(), 100.0) && near(b.height(), 100.0), "{b:?}");
    assert!(near(b.x0, 0.0) && near(b.y0, 0.0));
    assert_valid(&s);
}

#[test]
fn line_and_arrow_points_scale() {
    let mut s = scene(vec![line("l", &[(0.0, 0.0), (10.0, 20.0), (20.0, 0.0)])]);
    run(&mut s, |tx| transform::resize(tx, &ids(&["l"]), br(20.0, 20.0))).unwrap();
    let p = world_points(get(&s, "l")).unwrap();
    assert!(near(p[1].x, 20.0) && near(p[1].y, 40.0), "{p:?}");
    assert!(near(p[2].x, 40.0) && near(p[2].y, 0.0));
    assert_valid(&s);
}

#[test]
fn a_rotated_rectangle_resizes_in_its_own_frame_and_keeps_the_opposite_corner() {
    let mut s = scene(vec![rect("r", 0.0, 0.0, 100.0, 50.0).angle(0.6)]);
    let before = corner(get(&s, "r"), 0); // top-left
    // Drag the bottom-right handle along the rotated x axis by 40.
    let d = Vec2::new(0.6_f64.cos(), 0.6_f64.sin()) * 40.0;
    run(&mut s, |tx| transform::resize(tx, &ids(&["r"]), br(d.x, d.y))).unwrap();
    let t = get(&s, "r").transform;
    assert!(near(t.w, 140.0) && near(t.h, 50.0), "{t:?}");
    assert!(near(t.angle, 0.6));
    let after = corner(get(&s, "r"), 0);
    assert!((before - after).hypot() < 1e-9, "top-left stays: {before:?} {after:?}");
}

#[test]
fn edge_handles_stretch_one_axis() {
    let mut s = scene(vec![rect("r", 0.0, 0.0, 100.0, 50.0)]);
    let spec = ResizeSpec {
        handle: BoxHandle::Edge(Edge::Right),
        delta: Vec2::new(30.0, 99.0),
        keep_aspect: false,
        from_center: false,
    };
    run(&mut s, |tx| transform::resize(tx, &ids(&["r"]), spec)).unwrap();
    let t = get(&s, "r").transform;
    assert!(near(t.w, 130.0) && near(t.h, 50.0));
}

#[test]
fn dragging_past_the_opposite_side_mirrors_and_the_size_never_drops_below_one() {
    let mut s = scene(vec![rect("r", 0.0, 0.0, 100.0, 100.0)]);
    run(&mut s, |tx| transform::resize(tx, &ids(&["r"]), br(-150.0, 0.0))).unwrap();
    let t = get(&s, "r").transform;
    assert!(near(t.w, 50.0) && near(t.x, -50.0), "{t:?}");
    let mut s = scene(vec![rect("r", 0.0, 0.0, 100.0, 100.0)]);
    run(&mut s, |tx| transform::resize(tx, &ids(&["r"]), br(-100.0, -100.0))).unwrap();
    let t = get(&s, "r").transform;
    assert!(t.w >= 1.0 && t.h >= 1.0, "{t:?}");
}

#[test]
fn the_rotate_handle_does_not_resize() {
    let mut s = scene(vec![rect("r", 0.0, 0.0, 100.0, 100.0)]);
    let spec = ResizeSpec { handle: BoxHandle::Rotate, ..br(1.0, 1.0) };
    assert!(matches!(run(&mut s, |tx| transform::resize(tx, &ids(&["r"]), spec)), Err(TransformError::RotateHandle)));
}

#[test]
fn a_multi_selection_scales_as_a_unit() {
    let mut s = scene(vec![rect("a", 0.0, 0.0, 50.0, 50.0), rect("b", 100.0, 100.0, 50.0, 50.0)]);
    // Common box 0..150: drag the bottom-right corner by +150 doubles everything.
    run(&mut s, |tx| transform::resize(tx, &ids(&["a", "b"]), br(150.0, 150.0))).unwrap();
    let (a, b) = (get(&s, "a").transform, get(&s, "b").transform);
    assert!(near(a.w, 100.0) && near(a.x, 0.0));
    assert!(near(b.x, 200.0) && near(b.y, 200.0) && near(b.w, 100.0));
}

#[test]
fn a_group_resizes_as_a_unit_and_keeps_its_ratio() {
    let mut s =
        scene(vec![rect("a", 0.0, 0.0, 50.0, 50.0).grouped(&["g"]), rect("b", 100.0, 0.0, 50.0, 50.0).grouped(&["g"])]);
    // A lopsided drag: group members force the aspect ratio (Excalidraw rule).
    run(&mut s, |tx| transform::resize(tx, &ids(&["a", "b"]), br(150.0, 10.0))).unwrap();
    let (a, b) = (get(&s, "a").transform, get(&s, "b").transform);
    assert!(near(a.w, a.h) && near(b.w, b.h), "{a:?} {b:?}");
    assert!(near(a.w, 100.0), "{a:?}");
    assert!(near(b.x, 200.0));
}

#[test]
fn a_rotated_member_forces_uniform_scale_in_a_multi_selection() {
    let mut s = scene(vec![rect("a", 0.0, 0.0, 50.0, 50.0).angle(0.4), rect("b", 100.0, 0.0, 50.0, 50.0)]);
    run(&mut s, |tx| transform::resize(tx, &ids(&["a", "b"]), br(100.0, 0.0))).unwrap();
    let (a, b) = (get(&s, "a").transform, get(&s, "b").transform);
    assert!(near(a.w, a.h) && near(b.w, b.h), "no shear: {a:?} {b:?}");
    assert!(near(a.angle, 0.4));
}

#[test]
fn text_scales_its_font_on_a_corner_and_wraps_on_a_side() {
    let mut s = scene(vec![text("t", 0.0, 0.0, 100.0, 20.0, "hello")]);
    run(&mut s, |tx| transform::resize(tx, &ids(&["t"]), br(100.0, 0.0))).unwrap();
    let e = get(&s, "t");
    assert!(near(e.transform.w, 200.0) && near(e.transform.h, 40.0), "{:?}", e.transform);
    assert!(near(e.text_data().unwrap().font.size, 40.0));
    assert!(e.text_data().unwrap().auto_resize);

    let mut s = scene(vec![text("t", 0.0, 0.0, 100.0, 20.0, "hello")]);
    let spec = ResizeSpec {
        handle: BoxHandle::Edge(Edge::Right),
        delta: Vec2::new(50.0, 0.0),
        keep_aspect: false,
        from_center: false,
    };
    run(&mut s, |tx| transform::resize(tx, &ids(&["t"]), spec)).unwrap();
    let e = get(&s, "t");
    assert!(near(e.transform.w, 150.0) && near(e.transform.h, 20.0));
    assert!(near(e.text_data().unwrap().font.size, 20.0), "a side handle does not change the font");
    assert!(!e.text_data().unwrap().auto_resize, "fixed width, the text wraps");
}

#[test]
fn math_scales_its_font_size() {
    let m = Element::new(id("m"), ElementKind::Math(MathData::new("x^2")), ZIndex::first())
        .with_bounds(0.0, 0.0, 40.0, 20.0);
    let mut s = scene(vec![m]);
    run(&mut s, |tx| transform::resize(tx, &ids(&["m"]), br(40.0, 0.0))).unwrap();
    let e = get(&s, "m");
    assert!(near(e.transform.w, 80.0) && near(e.transform.h, 40.0));
    if let ElementKind::Math(d) = &e.kind {
        assert!(near(d.font_size, 40.0));
    }
}

#[test]
fn bound_text_follows_a_resized_container() {
    let mut s = scene(vec![rect("a", 0.0, 0.0, 100.0, 50.0), label("la", "a", 25.0, 15.0, 50.0, 20.0)]);
    run(&mut s, |tx| transform::resize(tx, &ids(&["a"]), br(100.0, 50.0))).unwrap();
    let l = get(&s, "la").transform;
    let c = l.center();
    assert!(near(c.x, 100.0) && near(c.y, 50.0), "the label stays centred: {l:?}");
    assert!(near(l.w, 50.0), "its size is the text layer's business");
}

// ---- rotate ----------------------------------------------------------------------------------------------------

#[test]
fn a_single_element_rotates_about_its_own_centre() {
    let mut s = scene(vec![rect("r", 0.0, 0.0, 100.0, 50.0)]);
    run(&mut s, |tx| transform::rotate(tx, &ids(&["r"]), FRAC_PI_2, RotateMode::By, None)).unwrap();
    let t = get(&s, "r").transform;
    assert!(near(t.angle, FRAC_PI_2));
    assert_eq!((t.x, t.y, t.w, t.h), (0.0, 0.0, 100.0, 50.0), "the unrotated box does not move");
}

#[test]
fn old_group_rotation_is_real_for_every_member() {
    // The old group rotation was cosmetic: bounds and hit tests ignored it. Now each member really turns, so the
    // world bounds follow what is drawn.
    let mut s =
        scene(vec![rect("a", 0.0, 0.0, 10.0, 10.0).grouped(&["g"]), rect("b", 90.0, 0.0, 10.0, 10.0).grouped(&["g"])]);
    run(&mut s, |tx| transform::rotate(tx, &ids(&["a", "b"]), FRAC_PI_2, RotateMode::By, None)).unwrap();
    let (a, b) = (get(&s, "a"), get(&s, "b"));
    assert!(near(a.transform.angle, FRAC_PI_2) && near(b.transform.angle, FRAC_PI_2));
    // Pivot (50, 5): a's centre (5,5) goes to (50, -40); b's centre (95,5) to (50, 50).
    let (ca, cb) = (a.transform.center(), b.transform.center());
    assert!(near(ca.x, 50.0) && near(ca.y, -40.0), "{ca:?}");
    assert!(near(cb.x, 50.0) && near(cb.y, 50.0), "{cb:?}");
    let ba = world_bounds(a);
    assert!(near(ba.x0, 45.0) && near(ba.y0, -45.0) && near(ba.x1, 55.0) && near(ba.y1, -35.0), "{ba:?}");
    // Rotating back restores the original geometry.
    run(&mut s, |tx| transform::rotate(tx, &ids(&["a", "b"]), -FRAC_PI_2, RotateMode::By, None)).unwrap();
    let (a, b) = (get(&s, "a").transform, get(&s, "b").transform);
    assert!(near(a.x, 0.0) && near(a.y, 0.0) && near(b.x, 90.0) && near(b.y, 0.0));
    assert!(near(a.angle, 0.0) && near(b.angle, 0.0));
}

#[test]
fn rotate_to_sets_an_absolute_angle_for_one_element() {
    let mut s = scene(vec![rect("r", 0.0, 0.0, 10.0, 10.0).angle(1.0)]);
    run(&mut s, |tx| transform::rotate(tx, &ids(&["r"]), 0.0, RotateMode::To, None)).unwrap();
    assert_eq!(get(&s, "r").transform.angle, 0.0);
    run(&mut s, |tx| transform::rotate(tx, &ids(&["r"]), 2.0, RotateMode::To, None)).unwrap();
    assert!(near(get(&s, "r").transform.angle, 2.0));
    let mut s = scene(vec![rect("a", 0.0, 0.0, 10.0, 10.0), rect("b", 20.0, 0.0, 10.0, 10.0)]);
    assert!(matches!(
        run(&mut s, |tx| transform::rotate(tx, &ids(&["a", "b"]), 1.0, RotateMode::To, None)),
        Err(TransformError::RotateToNeedsOne)
    ));
}

#[test]
fn rotation_snaps() {
    let mut s = scene(vec![rect("r", 0.0, 0.0, 10.0, 10.0)]);
    let step = 15.0_f64.to_radians();
    run(&mut s, |tx| transform::rotate(tx, &ids(&["r"]), 20.0_f64.to_radians(), RotateMode::By, Some(step))).unwrap();
    assert!(near(get(&s, "r").transform.angle, step));
    run(&mut s, |tx| transform::rotate(tx, &ids(&["r"]), 100.0_f64.to_radians(), RotateMode::To, Some(step))).unwrap();
    assert!(near(get(&s, "r").transform.angle.to_degrees(), 105.0));
}

#[test]
fn frames_and_elbow_arrows_cannot_rotate_and_the_whole_call_is_refused() {
    let mut elbow = arrow("e", &[(0.0, 0.0), (50.0, 50.0)]);
    if let ElementKind::Arrow(d) = &mut elbow.kind {
        d.path = PathType::Elbow;
    }
    let mut s = scene(vec![frame("f", 0.0, 0.0, 100.0, 100.0), elbow, rect("r", 200.0, 0.0, 10.0, 10.0)]);
    let rev = s.revision();
    let r = run(&mut s, |tx| transform::rotate(tx, &ids(&["f", "r"]), 1.0, RotateMode::By, None));
    assert!(matches!(r, Err(TransformError::NotRotatable { ref ids, .. }) if ids == &self::ids(&["f"])));
    let r = run(&mut s, |tx| transform::rotate(tx, &ids(&["e", "r"]), 1.0, RotateMode::By, None));
    assert!(matches!(r, Err(TransformError::NotRotatable { ref ids, .. }) if ids == &self::ids(&["e"])));
    assert_eq!(s.revision(), rev, "nothing is skipped silently");
}

#[test]
fn bound_text_turns_with_its_container() {
    let mut s = scene(vec![rect("a", 0.0, 0.0, 100.0, 50.0), label("la", "a", 25.0, 15.0, 50.0, 20.0)]);
    run(&mut s, |tx| transform::rotate(tx, &ids(&["a"]), 0.5, RotateMode::By, None)).unwrap();
    assert!(near(get(&s, "la").transform.angle, 0.5));
    let c = get(&s, "la").transform.center();
    assert!(near(c.x, 50.0) && near(c.y, 25.0));
}

#[test]
fn grouping_then_rotating_equals_rotating_the_members() {
    let mut s = scene(vec![rect("a", 0.0, 0.0, 10.0, 10.0), rect("b", 50.0, 20.0, 10.0, 10.0)]);
    run(&mut s, |tx| group::group(tx, &ids(&["a", "b"]), None, gid("g"))).unwrap();
    run(&mut s, |tx| transform::rotate(tx, &ids(&["a", "b"]), PI / 3.0, RotateMode::By, None)).unwrap();
    let ca = get(&s, "a").transform.center();
    let pivot = Point::new(30.0, 15.0);
    let expect = drafftink_geom::transform::rotate_point(Point::new(5.0, 5.0), pivot, PI / 3.0);
    assert!((ca - expect).hypot() < 1e-9);
}

// ---- set_bounds ------------------------------------------------------------------------------------------------

#[test]
fn set_bounds_sets_the_unrotated_box() {
    let mut s = scene(vec![rect("r", 0.0, 0.0, 10.0, 10.0).angle(0.4), freehand("f", &[(0.0, 0.0), (10.0, 10.0)])]);
    run(&mut s, |tx| {
        transform::set_bounds(tx, &id("r"), BoundsSpec { x: Some(5.0), w: Some(30.0), ..BoundsSpec::default() })
    })
    .unwrap();
    let t = get(&s, "r").transform;
    assert_eq!((t.x, t.y, t.w, t.h), (5.0, 0.0, 30.0, 10.0));
    assert_eq!(t.angle, 0.4);
    run(&mut s, |tx| {
        transform::set_bounds(tx, &id("f"), BoundsSpec { w: Some(40.0), h: Some(20.0), ..BoundsSpec::default() })
    })
    .unwrap();
    let b = world_bounds(get(&s, "f"));
    assert!(near(b.width(), 40.0) && near(b.height(), 20.0));
    assert!(matches!(
        run(&mut s, |tx| transform::set_bounds(tx, &id("r"), BoundsSpec { w: Some(-1.0), ..BoundsSpec::default() })),
        Err(TransformError::BadBounds)
    ));
}

#[test]
fn hostile_numbers_never_panic_and_never_break_the_scene() {
    use drafftink_edit::arrange::{self, Axis, DistributeOpts};
    use drafftink_edit::frame::{self, WrapOpts};
    use drafftink_edit::lifecycle::{self, DuplicateOpts};
    let bad =
        [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e300, -1e300, 1e150, 1e9, -1e9, 0.0, -0.0, f64::MIN_POSITIVE];
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 10.0, 10.0),
        rect("b", 50.0, 0.0, 10.0, 10.0),
        rect("c", 100.0, 0.0, 10.0, 10.0),
        text("t", 0.0, 50.0, 40.0, 20.0, "x"),
        line("l", &[(0.0, 100.0), (50.0, 130.0)]),
    ]);
    let all = ids(&["a", "b", "c", "t", "l"]);
    let mut src = SeqIds::new();
    for &v in &bad {
        for &w in &bad {
            let d = Vec2::new(v, w);
            let _ = run(&mut s, |tx| transform::move_by(tx, &all, d));
            let _ = run(&mut s, |tx| transform::nudge(tx, &all, v, w));
            for handle in [
                BoxHandle::Corner(Corner::TopLeft),
                BoxHandle::Corner(Corner::BottomRight),
                BoxHandle::Edge(Edge::Top),
                BoxHandle::Edge(Edge::Left),
            ] {
                for ids in [&all[..1], &all[3..4], &all[..]] {
                    let spec = ResizeSpec { handle, delta: d, keep_aspect: v > 0.0, from_center: w > 0.0 };
                    let _ = run(&mut s, |tx| transform::resize(tx, ids, spec));
                }
            }
            let _ = run(&mut s, |tx| transform::rotate(tx, &all, v, RotateMode::By, Some(w)));
            let _ = run(&mut s, |tx| transform::rotate(tx, &all[..1], v, RotateMode::To, Some(w)));
            let spec = BoundsSpec { x: Some(v), y: Some(w), w: Some(v), h: Some(w) };
            let _ = run(&mut s, |tx| transform::set_bounds(tx, &id("a"), spec));
            let _ = run(&mut s, |tx| arrange::distribute(tx, &all, Axis::X, DistributeOpts { gap: Some(v) }, None));
            let _ = run(&mut s, |tx| {
                lifecycle::duplicate(tx, &all[..2], &DuplicateOpts { offset: d, editing: None }, &mut src)
            });
            let _ = run(&mut s, |tx| frame::wrap(tx, &all[..2], &mut src, &WrapOpts { name: None, padding: v }));
            let report = validate_scene(&s);
            assert!(!report.has_errors(), "{v} {w}: {:?}", report.errors().collect::<Vec<_>>());
        }
    }
}

// ---- with the real binding solver ------------------------------------------------------------------------------

fn gap_of(s: &MemoryStore, arrow: &str) -> f64 {
    drafftink_geom::binding::binding_gap(get(s, arrow).style.stroke_width)
}

fn ends(s: &MemoryStore, arrow: &str) -> Vec<Point> {
    world_points(get(s, arrow)).unwrap()
}

/// Distance from `p` to the outline of `target`.
fn outline_distance(s: &MemoryStore, target: &str, p: Point) -> f64 {
    use drafftink_geom::Outline;
    drafftink_edit::transform::geometry::primitive(get(s, target)).unwrap().outline_distance(p)
}

/// Target `t` at (0,0,100,100); arrow from its right side to (300, 50), bound at the start, orbit.
fn solved_pair() -> MemoryStore {
    let mut a = arrow("arr", &[(100.0, 50.0), (300.0, 50.0)]);
    bind(&mut a, true, "t", [1.0, 0.5]);
    let mut s = scene(vec![rect("t", 0.0, 0.0, 100.0, 100.0), a]);
    run(&mut s, |tx| drafftink_edit::binding::update_bound_arrows(tx, &ids(&["t"]))).unwrap();
    s
}

#[test]
fn a_moved_target_drags_the_bound_end_along() {
    let mut s = solved_pair();
    let g = gap_of(&s, "arr");
    assert!((outline_distance(&s, "t", ends(&s, "arr")[0]) - g).abs() < 1e-6);
    let r = run(&mut s, |tx| transform::move_by(tx, &ids(&["t"]), Vec2::new(40.0, 30.0))).unwrap();
    assert_eq!(r.arrows, ids(&["arr"]));
    let p = ends(&s, "arr");
    assert!((outline_distance(&s, "t", p[0]) - g).abs() < 1e-6, "still one gap from the moved outline: {p:?}");
    assert!(p[0].x > 140.0 && p[0].y > 60.0, "it followed the target: {p:?}");
    assert!(near(p[1].x, 300.0) && near(p[1].y, 50.0), "the free end stays");
    assert_valid(&s);
}

#[test]
fn a_resized_or_rotated_target_keeps_the_end_on_its_outline() {
    let mut s = solved_pair();
    let g = gap_of(&s, "arr");
    run(&mut s, |tx| transform::resize(tx, &ids(&["t"]), br(100.0, 0.0))).unwrap();
    let p = ends(&s, "arr");
    assert!((p[0].x - (200.0 + g)).abs() < 1e-6, "{p:?}");
    let mut s = solved_pair();
    run(&mut s, |tx| transform::rotate(tx, &ids(&["t"]), 0.5, RotateMode::By, None)).unwrap();
    let p = ends(&s, "arr");
    assert!((outline_distance(&s, "t", p[0]) - g).abs() < 1e-6, "{p:?}");
    assert!(get(&s, "arr").linear_data().unwrap().start_binding.is_some());
}

#[test]
fn flipping_only_the_target_keeps_the_arrow_bound() {
    let mut s = solved_pair();
    let g = gap_of(&s, "arr");
    // A lone square is its own mirror image, so the end stays on the same side.
    run(&mut s, |tx| drafftink_edit::arrange::flip(tx, &ids(&["t"]), drafftink_edit::arrange::FlipAxis::Horizontal))
        .unwrap();
    assert!(get(&s, "arr").linear_data().unwrap().start_binding.is_some());
    assert!((ends(&s, "arr")[0].x - (100.0 + g)).abs() < 1e-6);
}

#[test]
fn moving_the_arrow_alone_frees_its_end_and_leaves_it_there() {
    let mut s = solved_pair();
    let before = ends(&s, "arr");
    let r = run(&mut s, |tx| transform::move_by(tx, &ids(&["arr"]), Vec2::new(0.0, 60.0))).unwrap();
    assert_eq!(r.unbound, vec![(id("arr"), End::Start)]);
    let after = ends(&s, "arr");
    assert!(near(after[0].x, before[0].x) && near(after[0].y, before[0].y + 60.0));
    // The target moving later no longer drags the freed end.
    run(&mut s, |tx| transform::move_by(tx, &ids(&["t"]), Vec2::new(500.0, 0.0))).unwrap();
    assert_eq!(ends(&s, "arr"), after);
}

#[test]
fn moving_target_and_arrow_together_keeps_the_pair_rigid() {
    let mut s = solved_pair();
    let before = ends(&s, "arr");
    let r = run(&mut s, |tx| transform::move_by(tx, &ids(&["t", "arr"]), Vec2::new(25.0, -10.0))).unwrap();
    assert!(r.unbound.is_empty());
    let after = ends(&s, "arr");
    assert!((after[0].x - (before[0].x + 25.0)).abs() < 1e-6 && (after[0].y - (before[0].y - 10.0)).abs() < 1e-6);
    assert!(near(after[1].x, before[1].x + 25.0));
}

#[test]
fn a_frame_moves_arrows_bound_to_its_children() {
    let mut a = arrow("arr", &[(30.0, 20.0), (300.0, 20.0)]);
    bind(&mut a, true, "kid", [1.0, 0.5]);
    let mut s = scene(vec![rect("kid", 10.0, 10.0, 20.0, 20.0).in_frame("f"), frame("f", 0.0, 0.0, 100.0, 100.0), a]);
    run(&mut s, |tx| drafftink_edit::binding::update_bound_arrows(tx, &ids(&["kid"]))).unwrap();
    let g = gap_of(&s, "arr");
    run(&mut s, |tx| transform::move_by(tx, &ids(&["f"]), Vec2::new(0.0, 200.0))).unwrap();
    let after = ends(&s, "arr")[0];
    assert!((outline_distance(&s, "kid", after) - g).abs() < 1e-6, "{after:?}");
    assert!(after.y > 200.0, "{after:?}");
    assert!(get(&s, "arr").linear_data().unwrap().start_binding.is_some());
}
