#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Binding acquisition while an arrow end is dragged, and the highlight candidate.

mod arrows_common;

use arrows_common::*;
use drafftink_edit::binding::{BindOpts, End, EndChange, find_hovered, hover_candidate, plan_end_drag};
use drafftink_scene::{BindMode, Color, ElementKind, PathType};

fn free_arrow(s: &mut Scene) -> drafftink_scene::ElementId {
    s.arrow("a", &[(600.0, 600.0), (700.0, 650.0)])
}

#[test]
fn the_pointer_in_the_margin_highlights_with_orbit_and_inside_does_not_for_the_arrow_tool() {
    let mut s = Scene::new();
    let t = s.rect("t", 0.0, 0.0, 100.0, 100.0);
    let opts = BindOpts::default();
    let c = hover_candidate(&s.store, p(110.0, 20.0), PathType::Sharp, &opts).unwrap();
    assert_eq!((c.target.clone(), c.mode), (t.clone(), BindMode::Orbit));
    assert!(c.signed_distance > 0.0 && c.signed_distance < 15.0);
    assert!(hover_candidate(&s.store, p(50.0, 50.0), PathType::Sharp, &opts).is_none(), "inside: nothing is suggested");
    assert!(hover_candidate(&s.store, p(130.0, 50.0), PathType::Sharp, &opts).is_none(), "too far");
    assert!(hover_candidate(&s.store, p(110.0, 20.0), PathType::Sharp, &BindOpts { disabled: true, ..opts }).is_none());
}

#[test]
fn the_reach_grows_when_zoomed_out() {
    let mut s = Scene::new();
    let t = s.rect("t", 0.0, 0.0, 100.0, 100.0);
    assert!(find_hovered(&s.store, p(125.0, 50.0), 1.0, &[]).is_none());
    assert_eq!(find_hovered(&s.store, p(125.0, 50.0), 0.25, &[]).unwrap().id, t);
    assert!(find_hovered(&s.store, p(140.0, 50.0), 0.25, &[]).is_none());
}

#[test]
fn dropping_inside_binds_inside_at_that_spot() {
    let mut s = Scene::new();
    let t = s.rect("t", 0.0, 0.0, 100.0, 100.0);
    let a = free_arrow(&mut s);
    let c = s.drag_end(&a, End::End, p(25.0, 75.0)).unwrap();
    assert_eq!((c.target.clone(), c.mode), (t.clone(), BindMode::Inside));
    let b = s.binding(&a, End::End).unwrap();
    assert_eq!(b.mode, BindMode::Inside);
    assert!((b.fixed_point[0] - 0.25).abs() < 1e-9 && (b.fixed_point[1] - 0.75).abs() < 1e-9);
    assert_close(s.end(&a), p(25.0, 75.0));
}

#[test]
fn dropping_in_the_margin_binds_orbit_and_the_end_rides_the_outline() {
    let mut s = Scene::new();
    let t = s.rect("t", 0.0, 0.0, 100.0, 100.0);
    let a = free_arrow(&mut s);
    s.drag_end(&a, End::End, p(108.0, 30.0)).unwrap();
    let b = s.binding(&a, End::End).unwrap();
    assert_eq!(b.mode, BindMode::Orbit);
    assert!(orbit_error(&s, &t, s.end(&a), 6.0) < 1e-6, "{:?}", s.end(&a));
    // It keeps aiming at the same focus when the target moves.
    s.move_by(&t, 0.0, 50.0);
    assert!(orbit_error(&s, &t, s.end(&a), 6.0) < 1e-6);
}

#[test]
fn an_orbit_focus_snaps_to_a_side_midpoint() {
    let mut s = Scene::new();
    s.rect("t", 0.0, 0.0, 100.0, 100.0);
    let a = free_arrow(&mut s);
    let c = s.drag_end(&a, End::End, p(108.0, 56.0)).unwrap();
    assert_eq!(c.snap_midpoint, Some(p(100.0, 50.0)));
    let off = s
        .drag_end_with(&a, End::End, p(108.0, 56.0), &BindOpts { midpoint_snap: false, ..BindOpts::default() })
        .unwrap();
    assert!(off.snap_midpoint.is_none());
    let locked =
        s.drag_end_with(&a, End::End, p(108.0, 56.0), &BindOpts { angle_locked: true, ..BindOpts::default() }).unwrap();
    assert!(locked.snap_midpoint.is_none());
}

#[test]
fn alt_forces_an_inside_binding_from_the_margin() {
    let mut s = Scene::new();
    s.rect("t", 0.0, 0.0, 100.0, 100.0);
    let a = free_arrow(&mut s);
    let c =
        s.drag_end_with(&a, End::End, p(108.0, 30.0), &BindOpts { force_inside: true, ..BindOpts::default() }).unwrap();
    assert_eq!(c.mode, BindMode::Inside);
    assert_close(s.end(&a), p(108.0, 30.0));
}

#[test]
fn dragging_away_or_with_binding_off_frees_the_end() {
    let mut s = Scene::new();
    s.rect("t", 0.0, 0.0, 100.0, 100.0);
    let a = free_arrow(&mut s);
    s.drag_end(&a, End::End, p(50.0, 50.0)).unwrap();
    assert!(s.binding(&a, End::End).is_some());
    assert!(s.drag_end(&a, End::End, p(300.0, 300.0)).is_none());
    assert!(s.binding(&a, End::End).is_none());
    assert_close(s.end(&a), p(300.0, 300.0));
    s.drag_end(&a, End::End, p(50.0, 50.0)).unwrap();
    assert!(
        s.drag_end_with(&a, End::End, p(50.0, 50.0), &BindOpts { disabled: true, ..BindOpts::default() }).is_none()
    );
    assert!(s.binding(&a, End::End).is_none());
}

#[test]
fn the_smaller_shape_inside_a_bigger_one_wins_when_the_pointer_is_in_it() {
    let mut s = Scene::new();
    let _outer = s.rect("outer", 0.0, 0.0, 400.0, 400.0);
    let inner = s.rect("inner", 100.0, 100.0, 100.0, 100.0);
    assert_eq!(find_hovered(&s.store, p(150.0, 150.0), 1.0, &[]).unwrap().id, inner);
    // Away from the inner shape the outer one is the only candidate.
    assert_eq!(find_hovered(&s.store, p(350.0, 350.0), 1.0, &[]).unwrap().id.as_str(), "outer");
}

#[test]
fn an_opaque_shape_hides_what_is_behind_it_and_a_transparent_one_does_not() {
    let mut s = Scene::new();
    let back = s.rect("back", 0.0, 0.0, 200.0, 200.0);
    let front = s.rect("front", 50.0, 50.0, 300.0, 300.0);
    // Both are candidates near the corner of the front one only if it is transparent: the nearest outline wins.
    let near_back_edge = p(195.0, 100.0);
    assert_eq!(find_hovered(&s.store, near_back_edge, 1.0, &[]).unwrap().id, back);
    s.fill(&front, Color::rgb(200, 200, 255));
    assert_eq!(
        find_hovered(&s.store, near_back_edge, 1.0, &[]).unwrap().id,
        front,
        "the filled shape covers the other"
    );
}

#[test]
fn a_locked_shape_is_never_chosen_but_still_hides_what_is_behind() {
    let mut s = Scene::new();
    let back = s.rect("back", 0.0, 0.0, 200.0, 200.0);
    let front = s.rect("front", 50.0, 50.0, 300.0, 300.0);
    s.fill(&front, Color::rgb(0, 0, 0));
    s.lock(&front);
    assert!(find_hovered(&s.store, p(195.0, 100.0), 1.0, &[]).is_none());
    s.tombstone(&front, true);
    assert_eq!(find_hovered(&s.store, p(195.0, 100.0), 1.0, &[]).unwrap().id, back);
}

#[test]
fn arrows_lines_and_labels_are_not_targets() {
    let mut s = Scene::new();
    let other = s.arrow("other", &[(0.0, 0.0), (100.0, 100.0)]);
    let _ = other;
    assert!(find_hovered(&s.store, p(50.0, 50.0), 1.0, &[]).is_none());
}

#[test]
fn a_frame_can_be_bound_from_outside_only() {
    use drafftink_scene::{Element, FrameData};
    let mut s = Scene::new();
    let z = drafftink_scene::ZIndex::between(None, None).unwrap();
    let f =
        s.add(Element::new(id("f"), ElementKind::Frame(FrameData::default()), z).with_bounds(0.0, 0.0, 300.0, 300.0));
    assert!(find_hovered(&s.store, p(150.0, 150.0), 1.0, &[]).is_none(), "inside a frame");
    assert_eq!(find_hovered(&s.store, p(310.0, 150.0), 1.0, &[]).unwrap().id, f);
}

#[test]
fn both_ends_on_one_shape_bind_inside() {
    let mut s = Scene::new();
    s.rect("t", 0.0, 0.0, 100.0, 100.0);
    let a = free_arrow(&mut s);
    s.drag_end(&a, End::Start, p(20.0, 20.0)).unwrap();
    // The end is dropped in the margin of the same shape: both become inside.
    s.drag_end(&a, End::End, p(108.0, 80.0)).unwrap();
    assert_eq!(s.binding(&a, End::Start).unwrap().mode, BindMode::Inside);
    assert_eq!(s.binding(&a, End::End).unwrap().mode, BindMode::Inside);
    assert_close(s.start(&a), p(20.0, 20.0));
    assert_close(s.end(&a), p(108.0, 80.0));
}

#[test]
fn dragging_one_end_re_aims_the_other_orbit_end() {
    let mut s = Scene::new();
    let ta = s.rect("ta", 0.0, 0.0, 100.0, 100.0);
    s.rect("tb", 400.0, 0.0, 100.0, 100.0);
    let a = s.arrow("a", &[(110.0, 50.0), (390.0, 50.0)]);
    s.drag_end(&a, End::Start, p(110.0, 50.0)).unwrap();
    s.drag_end(&a, End::End, p(390.0, 50.0)).unwrap();
    let y0 = s.start(&a).y;
    s.drag_end(&a, End::End, p(390.0, 90.0)).unwrap();
    assert!(orbit_error(&s, &ta, s.start(&a), 6.0) < 1e-6);
    assert!(s.start(&a).y > y0 + 0.5, "the start aims lower: {:?}", s.start(&a));
}

#[test]
fn an_elbow_end_binds_orbit_and_snaps_to_the_outline_plus_gap() {
    let mut s = Scene::new();
    let t = s.rect("t", 0.0, 0.0, 100.0, 100.0);
    let a = s.arrow("a", &[(300.0, 250.0), (200.0, 150.0)]);
    s.set_path(&a, PathType::Elbow);
    let c = s.drag_end(&a, End::End, p(102.0, 40.0)).unwrap();
    assert_eq!(c.mode, BindMode::Orbit);
    assert_eq!(s.binding(&a, End::End).unwrap().mode, BindMode::Orbit);
    assert!((signed(&s, &t, s.end(&a)) - 6.0).abs() < 1e-3, "{:?}", s.end(&a));
    assert_clean_orthogonal(&s.world(&a));
}

#[test]
fn the_plan_is_a_value_that_does_not_touch_the_scene() {
    let mut s = Scene::new();
    s.rect("t", 0.0, 0.0, 100.0, 100.0);
    let a = free_arrow(&mut s);
    let before = s.world(&a);
    let plan = plan_end_drag(&s.store, &s.get(&a), End::End, p(50.0, 50.0), &BindOpts::default());
    assert!(matches!(plan.dragged, EndChange::Bind(_)));
    assert_eq!(plan.other, EndChange::Keep);
    assert_eq!(s.world(&a), before);
}

#[test]
fn hostile_input_is_an_error_not_a_panic() {
    use drafftink_edit::binding::{BindingError, drag_end};
    use drafftink_scene::SceneStore;
    let mut s = Scene::new();
    s.rect("t", 0.0, 0.0, 100.0, 100.0);
    let a = free_arrow(&mut s);
    let mut tx = s.store.begin();
    assert_eq!(drag_end(&mut tx, &a, End::End, p(f64::NAN, 0.0), &BindOpts::default()), Err(BindingError::NotFinite));
    assert!(matches!(
        drag_end(&mut tx, &id("nope"), End::End, p(0.0, 0.0), &BindOpts::default()),
        Err(BindingError::Missing(_))
    ));
    let t = id("t");
    assert!(matches!(
        drag_end(&mut tx, &t, End::End, p(0.0, 0.0), &BindOpts::default()),
        Err(BindingError::NotLinear(_))
    ));
    drop(tx);
    assert!(find_hovered(&s.store, p(f64::INFINITY, 0.0), 1.0, &[]).is_none());
    assert!(
        hover_candidate(
            &s.store,
            p(f64::NAN, 0.0),
            PathType::Sharp,
            &BindOpts { zoom: f64::NAN, ..BindOpts::default() }
        )
        .is_none()
    );
}
