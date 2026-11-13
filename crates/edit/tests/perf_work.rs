#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Work-counter tests of the edit rules at 1k, 10k and (in release builds) 100k elements.
//!
//! They assert on work, never on time (`docs/performance.md` section 2.5): the number of elements a rule visits
//! with a pass over the whole scene (`Counter::ElementsScanned`) and what it touched. A rule that serves a
//! gesture frame must visit 0 elements by scan and touch the same few elements at every scene size. `cargo xtask
//! perf` runs this file in release mode, which adds the 100k scene.

use std::collections::BTreeSet;

use drafftink_edit::binding::{
    BindOpts, End, arrows_bound_to, binding_reach, drag_end, drag_end_among, find_hovered, find_hovered_among,
    update_bound_arrows,
};
use drafftink_edit::frame::{frame_at, update_membership_after_move};
use drafftink_edit::style::{self, StyleProps};
use drafftink_edit::text_bind::{GrowAnchor, MonospaceMeasure, label_of, refresh_labels};
use drafftink_edit::transform::geometry::world_bounds;
use drafftink_edit::transform::{self, ResizeSpec, RotateMode};
use drafftink_geom::SpatialIndex;
use drafftink_geom::kurbo::{Point, Vec2};
use drafftink_geom::select::{BoxHandle, Corner};
use drafftink_profile::{Counter, WorkScope};
use drafftink_scene::{
    BindMode, Binding, Document, Element, ElementId, ElementKind, FrameData, Head, LinearKind, MemoryStore, SceneRead,
    SceneStore, SceneTx, TextData, ZIndex,
};

/// Scene sizes. Debug builds stop at 10k: building 100k elements unoptimized takes tens of seconds.
fn sizes() -> &'static [usize] {
    if cfg!(debug_assertions) { &[1_000, 10_000] } else { &[1_000, 10_000, 100_000] }
}

fn id(s: &str) -> ElementId {
    ElementId::new(s).unwrap()
}

const CELL: f64 = 140.0;

fn columns(n: usize) -> usize {
    (n as f64).sqrt().ceil() as usize
}

/// `n` rectangles `r<i>` on a grid. Rectangle `r<i>` with `i % 10 == 0` has an arrow `a<i>` bound to it and to
/// `r<i+1>`, and every 20th has a bound text `t<i>`. Three frames `f0..f2` sit in the first cells. The scene
/// holds the rectangles first, then the arrows, texts and frames.
fn scene(n: usize) -> MemoryStore {
    let side = columns(n);
    let origin = |i: usize| ((i % side) as f64 * CELL, (i / side) as f64 * CELL);
    let mut elements: Vec<Element> = Vec::new();
    let mut keys = ZIndex::rebalance(n * 2 + 8).unwrap().into_iter();
    let mut next_key = || keys.next().unwrap();
    for i in 0..n {
        let (x, y) = origin(i);
        elements
            .push(Element::new(id(&format!("r{i}")), ElementKind::Rectangle, next_key()).with_bounds(x, y, 80.0, 60.0));
    }
    for i in (0..n.saturating_sub(1)).step_by(10) {
        let (a, b) = (&elements[i], &elements[i + 1]);
        let start = Point::new(a.transform.x + 80.0, a.transform.y + 30.0);
        let end = Point::new(b.transform.x, b.transform.y + 30.0);
        let mut arrow = Element::linear(id(&format!("a{i}")), LinearKind::Arrow, next_key(), &[start, end]).unwrap();
        if let ElementKind::Arrow(d) = &mut arrow.kind {
            d.start_binding = Some(Binding { target: a.id.clone(), mode: BindMode::Orbit, fixed_point: [1.0, 0.5] });
            d.end_binding = Some(Binding { target: b.id.clone(), mode: BindMode::Orbit, fixed_point: [0.0, 0.5] });
            d.end_head = Head::Arrow;
        }
        elements.push(arrow);
    }
    for i in (0..n).step_by(20) {
        let (x, y) = origin(i);
        let mut data = TextData::new("label");
        data.container_id = Some(id(&format!("r{i}")));
        let mut text = Element::new(id(&format!("t{i}")), ElementKind::Text(data), next_key());
        text.transform = drafftink_scene::Transform::new(x + 10.0, y + 20.0, 40.0, 20.0);
        elements.push(text);
    }
    for k in 0..3 {
        let (x, y) = origin(k * 3);
        elements.push(
            Element::new(id(&format!("f{k}")), ElementKind::Frame(FrameData { name: None, clip: true }), next_key())
                .with_bounds(x - 10.0, y - 10.0, 300.0, 200.0),
        );
    }
    let mut doc = Document::new();
    doc.elements = elements;
    MemoryStore::from_document(&doc).unwrap()
}

fn scanned(work: &WorkScope) -> u64 {
    work.count(Counter::ElementsScanned)
}

#[test]
fn reading_and_writing_the_style_of_a_selection_scans_nothing() {
    // The properties panel reads the style of the selection every time the selection changes; a scan here was an
    // 0.8 s stall at the start of a drag in a 100,000-element document.
    for &n in sizes() {
        let mut store = scene(n);
        let ids: Vec<ElementId> = (0..100).map(|i| id(&format!("r{}", i * 2))).collect();
        let work = WorkScope::begin();
        let styles = style::read(&store, &ids, true).unwrap();
        assert_eq!(styles.len(), 100);
        assert_eq!(scanned(&work), 0, "style read at {n}: {}", work.summary());
        let mut tx = store.begin();
        let work = WorkScope::begin();
        let props = StyleProps { font_size: Some(24.0), ..StyleProps::default() };
        style::apply(&mut tx, &ids, &props, true).unwrap();
        assert_eq!(scanned(&work), 0, "style apply with a text property at {n}: {}", work.summary());
    }
}

#[test]
fn moving_a_bound_rectangle_scans_nothing_and_touches_two_elements() {
    for &n in sizes() {
        let mut store = scene(n);
        let mut tx = store.begin();
        let work = WorkScope::begin();
        let report = transform::move_by(&mut tx, &[id("r0")], Vec2::new(10.0, 5.0)).unwrap();
        assert_eq!(scanned(&work), 0, "move at {n}: {}", work.summary());
        assert_eq!(report.changed, [id("r0"), id("t0")], "move at {n}: the rectangle and its label");
        assert_eq!(report.arrows, [id("a0")], "move at {n}: only the arrow bound to the moved element is solved");
        let changes = tx.commit().unwrap();
        assert_eq!(changes.elements.len(), 3, "move at {n}: the rectangle, its arrow and its label");
    }
}

#[test]
fn resizing_and_rotating_scan_nothing() {
    for &n in sizes() {
        let mut store = scene(n);
        let mut tx = store.begin();
        let work = WorkScope::begin();
        let spec = ResizeSpec {
            handle: BoxHandle::Corner(Corner::BottomRight),
            delta: Vec2::new(12.0, 7.0),
            keep_aspect: false,
            from_center: false,
        };
        let resized = transform::resize(&mut tx, &[id("r0")], spec).unwrap();
        assert_eq!(scanned(&work), 0, "resize at {n}: {}", work.summary());
        assert_eq!(resized.arrows, [id("a0")], "resize at {n}");
        tx.commit().unwrap();

        let mut tx = store.begin();
        let work = WorkScope::begin();
        let turned = transform::rotate(&mut tx, &[id("r10")], 0.3, RotateMode::By, None).unwrap();
        assert_eq!(scanned(&work), 0, "rotate at {n}: {}", work.summary());
        assert_eq!(turned.arrows, [id("a10")], "rotate at {n}");
    }
}

#[test]
fn moving_one_of_many_selected_rectangles_scans_nothing() {
    for &n in sizes() {
        let mut store = scene(n);
        let ids: Vec<ElementId> = (0..100).map(|i| id(&format!("r{}", i * 3 + 1))).collect();
        let mut tx = store.begin();
        let work = WorkScope::begin();
        let report = transform::move_by(&mut tx, &ids, Vec2::new(1.0, 1.0)).unwrap();
        assert_eq!(scanned(&work), 0, "100 moved at {n}: {}", work.summary());
        let labelled = (0..100).filter(|i| (i * 3 + 1) % 20 == 0).count();
        assert_eq!(report.changed.len(), 100 + labelled, "100 moved at {n}: the rectangles and their labels");
    }
}

#[test]
fn updating_bound_arrows_follows_the_index_on_a_fresh_transaction_and_scans_on_a_dirty_one() {
    for &n in sizes() {
        let mut store = scene(n);
        let total = store.len_all() as u64;

        let mut tx = store.begin();
        let work = WorkScope::begin();
        let bound = arrows_bound_to(&tx, &[id("r1")]);
        assert_eq!(bound, BTreeSet::from([id("a0")]), "at {n}");
        let solved = update_bound_arrows(&mut tx, &[id("r1")]).unwrap();
        assert!(solved.iter().all(|a| *a == id("a0")), "only the arrow bound to r1 may change: {solved:?} at {n}");
        assert_eq!(scanned(&work), 0, "fresh transaction at {n}: {}", work.summary());
        drop(tx);

        // After a write the committed index no longer describes the transaction: the call scans, and finds the same.
        let mut tx = store.begin();
        tx.update(&id("r1"), |e| e.transform.x += 5.0).unwrap();
        let work = WorkScope::begin();
        assert_eq!(update_bound_arrows(&mut tx, &[id("r1")]).unwrap(), [id("a0")], "at {n}");
        assert_eq!(scanned(&work), total, "dirty transaction at {n} scans once: {}", work.summary());
    }
}

#[test]
fn the_frame_under_a_point_is_found_without_a_scan() {
    for &n in sizes() {
        let mut store = scene(n);
        let none = BTreeSet::new();
        let inside = Point::new(50.0, 50.0);
        let work = WorkScope::begin();
        assert_eq!(frame_at(&store, inside, &none), Some(id("f0")), "at {n}");
        assert_eq!(frame_at(&store, Point::new(-5000.0, -5000.0), &none), None, "at {n}");
        assert_eq!(scanned(&work), 0, "at {n}: {}", work.summary());

        // The scan, reached through a transaction that has written, answers the same.
        let mut tx = store.begin();
        tx.update(&id("r5"), |e| e.transform.x += 1.0).unwrap();
        let work = WorkScope::begin();
        assert_eq!(frame_at(&tx, inside, &none), Some(id("f0")), "scan at {n}");
        assert!(scanned(&work) > 0, "the dirty transaction must scan");
    }
}

#[test]
fn a_drop_over_a_frame_is_decided_without_a_scan() {
    for &n in sizes() {
        let mut store = scene(n);
        let mut tx = store.begin();
        let work = WorkScope::begin();
        let report = update_membership_after_move(&mut tx, &[id("r1")], None).unwrap();
        assert_eq!(scanned(&work), 0, "at {n}: {}", work.summary());
        assert_eq!(report.joined.len(), 1, "r1 lies in the first frame at {n}");
    }
}

#[test]
fn laying_out_the_label_of_a_moved_container_scans_nothing() {
    for &n in sizes() {
        let mut store = scene(n);
        let mut tx = store.begin();
        let work = WorkScope::begin();
        assert_eq!(label_of(&tx, &id("r0")), Some(id("t0")), "at {n}");
        let written = refresh_labels(&mut tx, &MonospaceMeasure::default(), &[id("r0")], GrowAnchor::Top).unwrap();
        assert_eq!(written, [id("t0")], "at {n}");
        assert_eq!(scanned(&work), 0, "at {n}: {}", work.summary());
    }
}

/// The index a caller keeps: world bounds grown by half the stroke, which is what `canvas` stores.
fn spatial(store: &MemoryStore) -> SpatialIndex<ElementId> {
    SpatialIndex::bulk_load(store.iter().map(|e| (e.id.clone(), world_bounds(e).inflate(1.0, 1.0))))
}

#[test]
fn the_shape_under_a_dragged_arrow_end_comes_from_the_index_and_equals_the_scan() {
    for &n in sizes() {
        let store = scene(n);
        let index = spatial(&store);
        let probes = [
            Point::new(40.0, 30.0),   // inside r0
            Point::new(83.0, 30.0),   // just right of r0
            Point::new(110.0, 30.0),  // in the gap between r0 and r1
            Point::new(-30.0, -30.0), // outside the scene
            Point::new(CELL * 3.0 + 40.0, 30.0),
        ];
        for zoom in [1.0, 0.25] {
            for &p in &probes {
                let work = WorkScope::begin();
                let want = find_hovered(&store, p, zoom, &[]);
                assert!(scanned(&work) > 0, "the scanning form scans");
                let near: Vec<ElementId> = index.query_point(p, binding_reach(zoom)).map(|(i, _)| i.clone()).collect();
                let work = WorkScope::begin();
                let got = find_hovered_among(&store, &near, p, zoom, &[]);
                assert_eq!(scanned(&work), 0, "indexed lookup at {n}: {}", work.summary());
                assert_eq!(got, want, "at {n}, {p:?}, zoom {zoom}");
                assert!(near.len() < 50, "the query returns the neighbourhood, not the scene: {} at {n}", near.len());
            }
        }
    }
}

#[test]
fn dragging_an_arrow_end_with_candidates_scans_nothing_and_matches_the_scan() {
    for &n in sizes() {
        let opts = BindOpts { zoom: 1.0, ..BindOpts::default() };
        let to = Point::new(CELL + 40.0, 30.0);
        let reach = binding_reach(opts.zoom);

        let mut scanned_store = scene(n);
        let mut tx = scanned_store.begin();
        drag_end(&mut tx, &id("a0"), End::End, to, &opts).unwrap();
        tx.commit().unwrap();

        let mut store = scene(n);
        let index = spatial(&store);
        let near: Vec<ElementId> = index.query_point(to, reach).map(|(i, _)| i.clone()).collect();
        let mut tx = store.begin();
        let work = WorkScope::begin();
        drag_end_among(&mut tx, &near, &id("a0"), End::End, to, &opts).unwrap();
        assert_eq!(scanned(&work), 0, "at {n}: {}", work.summary());
        let changes = tx.commit().unwrap();
        assert_eq!(changes.elements.len(), 1, "only the arrow is written at {n}");
        assert_eq!(store.get(&id("a0")), scanned_store.get(&id("a0")), "the same arrow as the scan makes at {n}");
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Elbow arrows around other shapes
// ---------------------------------------------------------------------------------------------------------------

/// `scene(n)` and one elbow arrow `long` from the right of `r0` to the left of `r<to>`, with the shapes between them in
/// the way.
fn scene_with_long_elbow(n: usize, to: usize) -> MemoryStore {
    let mut store = scene(n);
    let (a, b) = (store.get(&id("r0")).unwrap().clone(), store.get(&id(&format!("r{to}"))).unwrap().clone());
    let start = Point::new(a.transform.x + 80.0 + 6.0, a.transform.y + 30.0);
    let end = Point::new(b.transform.x - 6.0, b.transform.y + 30.0);
    let mut arrow = Element::linear(
        id("long"),
        LinearKind::Arrow,
        store.top_z().map(|z| ZIndex::between(Some(&z), None).unwrap()).unwrap(),
        &[start, end],
    )
    .unwrap();
    if let ElementKind::Arrow(d) = &mut arrow.kind {
        d.path = drafftink_scene::PathType::Elbow;
        d.start_binding = Some(Binding { target: a.id.clone(), mode: BindMode::Orbit, fixed_point: [1.0, 0.5] });
        d.end_binding = Some(Binding { target: b.id.clone(), mode: BindMode::Orbit, fixed_point: [0.0, 0.5] });
        d.end_head = Head::Arrow;
    }
    let mut tx = store.begin();
    tx.insert(arrow).unwrap();
    tx.commit().unwrap();
    store
}

/// True when some leg of the route crosses the inside of the shape's box.
fn crosses_shape(route: &[Point], shape: &Element) -> bool {
    let r = shape.transform.rect();
    route.windows(2).any(|w| {
        let (x0, x1) = (w[0].x.min(w[1].x), w[0].x.max(w[1].x));
        let (y0, y1) = (w[0].y.min(w[1].y), w[0].y.max(w[1].y));
        x0 < r.x1 && x1 > r.x0 && y0 < r.y1 && y1 > r.y0
    })
}

#[test]
fn routing_around_the_shapes_in_the_way_asks_the_index_and_its_work_does_not_grow_with_the_scene() {
    let mut expansions = Vec::new();
    for &n in sizes() {
        let mut store = scene_with_long_elbow(n, 6);
        let mut tx = store.begin();
        let work = WorkScope::begin();
        let changed =
            drafftink_edit::elbow::reroute_forced(&mut tx, &id("long"), drafftink_edit::elbow::RouteOpts::default())
                .unwrap();
        assert!(changed, "the straight line through five shapes is routed again at {n}");
        assert_eq!(scanned(&work), 0, "at {n}: {}", work.summary());
        assert!(work.count(Counter::SpatialQueries) >= 1, "the shapes come from the index at {n}: {}", work.summary());
        let steps = work.count(Counter::RouteExpansions);
        assert!(steps > 0 && steps <= drafftink_edit::elbow::MAX_EXPANSIONS as u64, "{steps} expansions at {n}");
        expansions.push(steps);

        let route = drafftink_edit::linear::world_points(tx.get(&id("long")).unwrap()).unwrap();
        for k in 1..=5 {
            let shape = tx.get(&id(&format!("r{k}"))).unwrap();
            assert!(!crosses_shape(&route, shape), "the route crosses r{k} at {n}: {route:?}");
        }
    }
    assert!(
        expansions.windows(2).all(|w| w[0] == w[1]),
        "the same neighbourhood costs the same at every size: {expansions:?}"
    );
}

#[test]
fn moving_a_shape_re_routes_the_few_elbow_arrows_near_it_and_scans_nothing() {
    for &n in sizes() {
        let mut store = scene_with_long_elbow(n, 2);
        let mut tx = store.begin();
        drafftink_edit::elbow::reroute_forced(&mut tx, &id("long"), drafftink_edit::elbow::RouteOpts::default())
            .unwrap();
        tx.commit().unwrap();
        let before = store.get(&id("long")).unwrap().clone();

        // r1 leaves the row: the detour around it is not needed any more. The move is committed, as the invariant
        // pass sees it, and the pass starts in a fresh transaction.
        let old = drafftink_edit::elbow::obstacle_box(store.get(&id("r1")).unwrap());
        let mut tx = store.begin();
        tx.update(&id("r1"), |e| e.transform.y += 3000.0).unwrap();
        tx.commit().unwrap();
        let mut tx = store.begin();
        let work = WorkScope::begin();
        let changed = drafftink_edit::binding::update_bound_arrows_near(&mut tx, &[id("r1")], &[old]).unwrap();
        // `a0` is bound to r1 and follows it; `long` passed it and loses its detour. No other arrow is looked at.
        assert_eq!(changed, [id("a0"), id("long")], "at {n}");
        assert_eq!(scanned(&work), 0, "at {n}: {}", work.summary());
        tx.commit().unwrap();
        assert_ne!(store.get(&id("long")).unwrap(), &before, "the route changed at {n}");
    }
}
