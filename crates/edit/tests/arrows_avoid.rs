#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Elbow arrows go around the other shapes: the router with obstacles, and the arrows of a scene when a shape
//! moves in or out of their way.

mod arrows_common;

use arrows_common::*;
use drafftink_edit::binding::{End, update_bound_arrows, update_bound_arrows_near};
use drafftink_edit::elbow::{
    self, AVOID_PADDING, Bounds, ElbowEnd, ElbowTarget, MAX_OBSTACLES, RouteOpts, obstacle_box,
};
use drafftink_geom::kurbo::{Point, Rect};
use drafftink_scene::{ElementId, Head, SceneStore, SceneTx};

/// The pure router between the facing sides of two shapes of a scene.
fn pure_route(s: &Scene, a: &ElementId, b: &ElementId, obstacles: &[Bounds]) -> Vec<Point> {
    let ra = s.get(a).transform.rect();
    let rb = s.get(b).transform.rect();
    let (ta, tb) = (s.target_of(a), s.target_of(b));
    let gap = 6.0;
    let start = ElbowEnd::bound(
        Point::new(ra.x1 + gap, ra.center().y),
        ElbowTarget { id: a.clone(), target: ta, gap },
        Head::None,
    );
    let end = ElbowEnd::bound(
        Point::new(rb.x0 - gap, rb.center().y),
        ElbowTarget { id: b.clone(), target: tb, gap },
        Head::Arrow,
    );
    elbow::route(&start, &end, RouteOpts::default().around(obstacles)).unwrap()
}

fn bounds(x: f64, y: f64, w: f64, h: f64) -> Bounds {
    [x, y, x + w, y + h]
}

/// True when some leg of `route` runs through the inside of the box grown by `pad`.
fn crosses(route: &[Point], b: &Bounds, pad: f64) -> bool {
    let (x0, y0, x1, y1) = (b[0] - pad, b[1] - pad, b[2] + pad, b[3] + pad);
    route.windows(2).any(|w| {
        let (lx0, lx1) = (w[0].x.min(w[1].x), w[0].x.max(w[1].x));
        let (ly0, ly1) = (w[0].y.min(w[1].y), w[0].y.max(w[1].y));
        lx0 < x1 && lx1 > x0 && ly0 < y1 && ly1 > y0
    })
}

/// Two shapes side by side, level with each other.
fn pair() -> (Scene, ElementId, ElementId) {
    let mut s = Scene::new();
    let a = s.rect("a", 0.0, 0.0, 100.0, 100.0);
    let b = s.rect("b", 500.0, 0.0, 100.0, 100.0);
    (s, a, b)
}

#[test]
fn a_shape_between_the_ends_is_gone_around() {
    let (s, a, b) = pair();
    let wall = bounds(250.0, 20.0, 100.0, 60.0);
    let straight = pure_route(&s, &a, &b, &[]);
    assert_eq!(straight.len(), 2, "no obstacle: a straight line {straight:?}");
    let around = pure_route(&s, &a, &b, &[wall]);
    assert_clean_orthogonal(&around);
    assert!(around.len() > 2, "{around:?}");
    assert!(!crosses(&around, &wall, 0.0), "route crosses the shape: {around:?}");
    assert!(!crosses(&around, &wall, AVOID_PADDING - 0.5), "route comes closer than the padding: {around:?}");
    assert_eq!(around.first(), straight.first());
    assert_eq!(around.last(), straight.last());
}

#[test]
fn a_shape_that_is_not_in_the_way_leaves_the_route_alone() {
    let (s, a, b) = pair();
    let plain = pure_route(&s, &a, &b, &[]);
    for far in
        [bounds(250.0, 200.0, 100.0, 60.0), bounds(-300.0, -300.0, 50.0, 50.0), bounds(250.0, -90.0, 100.0, 40.0)]
    {
        assert_eq!(pure_route(&s, &a, &b, &[far]), plain, "{far:?}");
    }
}

#[test]
fn the_route_is_the_same_every_time_and_for_any_order_of_the_shapes() {
    let (s, a, b) = pair();
    let mut walls = vec![
        bounds(220.0, 10.0, 60.0, 80.0),
        bounds(320.0, -60.0, 60.0, 90.0),
        bounds(300.0, 70.0, 60.0, 90.0),
        bounds(150.0, 120.0, 40.0, 40.0),
    ];
    let first = pure_route(&s, &a, &b, &walls);
    assert!(first.len() > 2);
    for _ in 0..3 {
        assert_eq!(pure_route(&s, &a, &b, &walls), first);
    }
    walls.reverse();
    assert_eq!(pure_route(&s, &a, &b, &walls), first, "the order of the shapes does not matter");
    walls.rotate_left(1);
    assert_eq!(pure_route(&s, &a, &b, &walls), first);
    for w in &walls {
        assert!(!crosses(&first, w, 0.0), "{first:?} crosses {w:?}");
    }
}

#[test]
fn with_no_way_through_the_route_is_the_one_without_obstacles() {
    let (s, a, b) = pair();
    // A wall of boxes across the whole corridor, so close together that their paddings overlap: no route.
    let wall: Vec<Bounds> = (-40..40).map(|i| bounds(300.0, f64::from(i) * 50.0, 30.0, 50.0)).collect();
    let plain = pure_route(&s, &a, &b, &[]);
    let got = pure_route(&s, &a, &b, &wall);
    assert_clean_orthogonal(&got);
    // Only the nearest boxes are looked at, and none of them can be passed: the result is the plain route.
    assert_eq!(got, plain);
}

#[test]
fn a_ring_around_the_target_is_a_fallback_not_an_error() {
    let (s, a, b) = pair();
    let ring =
        [bounds(380.0, -80.0, 200.0, 20.0), bounds(380.0, 160.0, 200.0, 20.0), bounds(380.0, -80.0, 20.0, 260.0)];
    let plain = pure_route(&s, &a, &b, &[]);
    let got = pure_route(&s, &a, &b, &ring);
    assert_clean_orthogonal(&got);
    assert_eq!(got.first(), plain.first());
    assert_eq!(got.last(), plain.last());
}

#[test]
fn a_box_that_holds_an_end_is_not_an_obstacle() {
    let (s, a, b) = pair();
    let plain = pure_route(&s, &a, &b, &[]);
    // A big shape both ends sit inside, and one around the start only.
    assert_eq!(pure_route(&s, &a, &b, &[bounds(-200.0, -200.0, 1000.0, 500.0)]), plain);
    assert_eq!(pure_route(&s, &a, &b, &[bounds(-50.0, -50.0, 200.0, 200.0)]), plain);
}

#[test]
fn more_obstacles_than_the_limit_still_give_a_route_and_a_fixed_one() {
    let (s, a, b) = pair();
    let mut field: Vec<Bounds> = Vec::new();
    for i in 0..30 {
        for j in 0..30 {
            field.push(bounds(150.0 + f64::from(i) * 12.0, -200.0 + f64::from(j) * 14.0, 6.0, 6.0));
        }
    }
    assert!(field.len() > 10 * MAX_OBSTACLES);
    let one = pure_route(&s, &a, &b, &field);
    assert_clean_orthogonal(&one);
    field.reverse();
    assert_eq!(pure_route(&s, &a, &b, &field), one);
}

#[test]
fn hostile_boxes_are_ignored() {
    let (s, a, b) = pair();
    let plain = pure_route(&s, &a, &b, &[]);
    let junk = [
        [f64::NAN, 0.0, 10.0, 10.0],
        [0.0, 0.0, f64::INFINITY, 10.0],
        [100.0, 100.0, 0.0, 0.0],
        [1.0e12, 1.0e12, 1.0e12 + 1.0, 1.0e12 + 1.0],
    ];
    assert_eq!(pure_route(&s, &a, &b, &junk), plain);
}

/// A scene with an elbow arrow between `a` and `b` and a shape `wall` that can be moved in and out.
fn arrow_scene() -> (Scene, ElementId, ElementId, ElementId, ElementId) {
    let (mut s, a, b) = pair();
    let wall = s.rect("wall", 250.0, 600.0, 100.0, 60.0);
    let arrow = s.elbow_between("arrow", p(106.0, 50.0), p(494.0, 50.0));
    (s, a, b, wall, arrow)
}

fn move_wall(s: &mut Scene, wall: &ElementId, x: f64, y: f64) -> Vec<ElementId> {
    let before = obstacle_box(&s.get(wall));
    let mut tx = s.store.begin();
    tx.update(wall, |e| {
        e.transform.x = x;
        e.transform.y = y;
    })
    .unwrap();
    let out = update_bound_arrows_near(&mut tx, std::slice::from_ref(wall), &[before]).unwrap();
    tx.commit().unwrap();
    out
}

#[test]
fn an_arrow_leaves_a_shape_that_is_dropped_in_its_way_and_comes_back_when_it_leaves() {
    let (mut s, _a, _b, wall, arrow) = arrow_scene();
    let straight = s.world(&arrow);
    assert_eq!(straight.len(), 2, "{straight:?}");

    let changed = move_wall(&mut s, &wall, 250.0, 20.0);
    assert_eq!(changed, std::slice::from_ref(&arrow));
    let routed = s.world(&arrow);
    let wall_box = obstacle_box(&s.get(&wall));
    assert_clean_orthogonal(&routed);
    assert!(routed.len() > 2, "{routed:?}");
    assert!(!crosses(&routed, &[wall_box.x0, wall_box.y0, wall_box.x1, wall_box.y1], 0.0), "{routed:?}");
    assert!(drafftink_edit::binding::stale_arrows(&s.store).is_empty());

    let changed = move_wall(&mut s, &wall, 250.0, 600.0);
    assert_eq!(changed, std::slice::from_ref(&arrow), "the detour that the shape caused goes with it");
    assert_eq!(s.world(&arrow), straight);
}

#[test]
fn moving_a_shape_re_routes_only_the_arrows_near_it() {
    let (mut s, _a, _b, wall, arrow) = arrow_scene();
    // A second pair far away, with its own arrow.
    s.rect("c", 0.0, 2000.0, 100.0, 100.0);
    s.rect("d", 500.0, 2000.0, 100.0, 100.0);
    let far = s.elbow_between("far", p(106.0, 2050.0), p(494.0, 2050.0));
    let far_version = s.get(&far).version;
    let near_version = s.get(&arrow).version;

    let changed = move_wall(&mut s, &wall, 250.0, 20.0);
    assert_eq!(changed, std::slice::from_ref(&arrow));
    assert_eq!(s.get(&far).version, far_version, "an arrow far from the move was not touched");
    assert_ne!(s.get(&arrow).version, near_version);
}

#[test]
fn a_shape_beside_the_arrow_but_not_in_its_way_changes_nothing() {
    let (mut s, _a, _b, wall, arrow) = arrow_scene();
    let before = s.get(&arrow);
    let changed = move_wall(&mut s, &wall, 250.0, 130.0);
    assert!(changed.is_empty(), "{changed:?}");
    assert_eq!(s.get(&arrow), before);
}

#[test]
fn a_deleted_shape_no_longer_bends_the_route() {
    let (mut s, _a, _b, wall, arrow) = arrow_scene();
    move_wall(&mut s, &wall, 250.0, 20.0);
    assert!(s.world(&arrow).len() > 2);
    let before = obstacle_box(&s.get(&wall));
    let mut tx = s.store.begin();
    tx.update(&wall, |e| e.deleted = true).unwrap();
    update_bound_arrows_near(&mut tx, std::slice::from_ref(&wall), &[before]).unwrap();
    tx.commit().unwrap();
    assert_eq!(s.world(&arrow).len(), 2);
}

#[test]
fn a_new_shape_dropped_on_the_arrow_is_gone_around() {
    let (mut s, _a, _b, _wall, arrow) = arrow_scene();
    let extra = s.ellipse("extra", 260.0, 10.0, 80.0, 80.0);
    let changed = s.solve(&[&extra]);
    assert_eq!(changed, std::slice::from_ref(&arrow));
    let w = s.world(&arrow);
    assert!(w.len() > 2, "{w:?}");
    let bx = obstacle_box(&s.get(&extra));
    assert!(!crosses(&w, &[bx.x0, bx.y0, bx.x1, bx.y1], 0.0));
}

#[test]
fn the_same_scene_gives_the_same_stored_route_however_it_got_there() {
    // Peers that hold the same shapes compute the same arrow: nothing about the route depends on history.
    let (mut one, _, _, wall1, arrow1) = arrow_scene();
    move_wall(&mut one, &wall1, 250.0, 20.0);

    let (mut two, _, _, wall2, arrow2) = arrow_scene();
    move_wall(&mut two, &wall2, 250.0, 300.0);
    move_wall(&mut two, &wall2, 250.0, 20.0);
    assert_eq!(one.world(&arrow1), two.world(&arrow2));

    // And a fresh solve of the merged scene (the repair pass after a merge, which knows only what changed).
    let before = two.world(&arrow2);
    two.solve(&[&wall2]);
    assert_eq!(two.world(&arrow2), before);
}

#[test]
fn pinned_and_free_arrows_and_lines_are_left_alone() {
    let (mut s, _a, _b, wall, arrow) = arrow_scene();
    // A pinned arrow keeps the route its user made.
    let pinned = s.elbow_between("pinned", p(106.0, 50.0), p(494.0, 50.0));
    s.data(&pinned, |d| {
        d.fixed_segments.push(drafftink_scene::FixedSegment { index: 1, start: p(0.0, 0.0), end: p(0.0, 0.0) });
    });
    let pinned_before = s.get(&pinned);
    // An elbow arrow with two free ends goes where it was drawn.
    let free = s.arrow("free", &[(100.0, 40.0), (500.0, 60.0)]);
    s.set_path(&free, drafftink_scene::PathType::Elbow);
    let free_before = s.get(&free);
    // A line is not an obstacle.
    let line = s.line("line", &[(250.0, 0.0), (250.0, 100.0)]);
    s.solve(&[&line]);
    assert_eq!(s.world(&arrow).len(), 2, "a line across the way does not bend the route");

    move_wall(&mut s, &wall, 250.0, 20.0);
    assert_eq!(s.get(&pinned), pinned_before);
    assert_eq!(s.get(&free), free_before);
}

#[test]
fn the_labels_of_the_ends_and_frames_do_not_block_the_route() {
    let (mut s, _a, _b, _wall, arrow) = arrow_scene();
    let frame = s.add(
        drafftink_scene::Element::new(
            id("frame"),
            drafftink_scene::ElementKind::Frame(drafftink_scene::FrameData::default()),
            drafftink_scene::ZIndex::first(),
        )
        .with_bounds(-50.0, -50.0, 800.0, 300.0),
    );
    s.solve(&[&frame]);
    assert_eq!(s.world(&arrow).len(), 2, "an arrow inside a frame is not routed around the frame");
}

#[test]
fn unrelated_updates_through_the_solver_do_not_change_arrows_that_are_clear() {
    let (mut s, a, _b, _wall, arrow) = arrow_scene();
    let before = s.get(&arrow);
    s.solve(&[&a]);
    assert_eq!(s.get(&arrow), before);
    let mut tx = s.store.begin();
    assert!(update_bound_arrows(&mut tx, &[]).unwrap().is_empty());
}

#[test]
fn dragging_an_end_to_a_shape_behind_a_wall_routes_around_the_wall() {
    let (mut s, _a, _b, wall, _arrow) = arrow_scene();
    move_wall(&mut s, &wall, 250.0, 20.0);
    // Drawn from somewhere else and dropped on the two shapes, so that its ends move.
    let arrow2 = s.arrow("arrow2", &[(200.0, 300.0), (300.0, 300.0)]);
    s.set_path(&arrow2, drafftink_scene::PathType::Elbow);
    s.drag_end(&arrow2, End::Start, p(106.0, 70.0));
    s.drag_end(&arrow2, End::End, p(494.0, 70.0));
    let w = s.world(&arrow2);
    let bx = obstacle_box(&s.get(&wall));
    assert_clean_orthogonal(&w);
    assert!(!crosses(&w, &[bx.x0, bx.y0, bx.x1, bx.y1], 0.0), "{w:?}");
    assert_eq!(End::Start.other(), End::End);
}

#[test]
fn rotated_shapes_count_by_their_rotated_box() {
    let (mut s, _a, _b, wall, arrow) = arrow_scene();
    // A tall thin bar, rotated flat, lies across the line.
    s.edit(&wall, |e| {
        e.transform.x = 270.0;
        e.transform.y = -40.0;
        e.transform.w = 20.0;
        e.transform.h = 180.0;
        e.transform.angle = 0.0;
    });
    s.solve(&[&wall]);
    let upright = s.world(&arrow);
    assert!(upright.len() > 2);
    s.rotate(&wall, std::f64::consts::FRAC_PI_2);
    let flat = s.world(&arrow);
    let rb: Rect = obstacle_box(&s.get(&wall));
    assert!(!crosses(&flat, &[rb.x0, rb.y0, rb.x1, rb.y1], 0.0), "{flat:?}");
}

#[test]
fn an_arrow_made_elbow_between_two_shapes_goes_around_the_shape_between_them_at_once() {
    let (mut s, _a, _b) = pair();
    s.rect("wall", 250.0, 20.0, 100.0, 60.0);
    let arrow = s.arrow("arrow", &[(106.0, 50.0), (494.0, 50.0)]);
    s.drag_end(&arrow, End::Start, p(106.0, 50.0));
    s.drag_end(&arrow, End::End, p(494.0, 50.0));
    let mut tx = s.store.begin();
    drafftink_edit::linear::set_path(&mut tx, &arrow, drafftink_scene::PathType::Elbow).unwrap();
    tx.commit().unwrap();
    let w = s.world(&arrow);
    let wall = obstacle_box(&s.get(&id("wall")));
    assert!(w.len() > 2, "{w:?}");
    assert!(!crosses(&w, &[wall.x0, wall.y0, wall.x1, wall.y1], 0.0), "{w:?}");
}
