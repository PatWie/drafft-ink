#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! `update_bound_arrows` at scale: 1,000 arrows bound to 1,000 shapes, 100 shapes moved in one transaction.
//!
//! Three solvers are timed on the same scene:
//! * `scan`: `update_bound_arrows`, which finds the arrows by scanning the transaction's elements;
//! * `indexed`: `update_bound_arrows_indexed`, which uses the committed reverse index;
//! * `elbow`: the same with every arrow an elbow arrow (A* routing per arrow).
//!
//! A second group, `elbow_around_shapes_1k_shapes`, routes elbow arrows around the shapes between their ends.

use std::hint::black_box;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use drafftink_edit::binding::{update_bound_arrows, update_bound_arrows_indexed, update_bound_arrows_near};
use drafftink_edit::elbow::{self, RouteOpts};
use drafftink_geom::kurbo::Point;
use drafftink_scene::{
    BindMode, Binding, Element, ElementId, ElementKind, LinearData, LinearKind, MemoryStore, PathType, SceneRead,
    SceneStore, SceneTx, ZIndex,
};

const SHAPES: usize = 1_000;
const MOVED: usize = 100;

fn id(prefix: &str, i: usize) -> ElementId {
    ElementId::new(&format!("{prefix}{i}")).expect("valid id")
}

fn scene(elbow: bool) -> (MemoryStore, Vec<ElementId>) {
    let mut store = MemoryStore::new();
    let mut tx = store.begin();
    let cols = 40usize;
    let mut z: Option<ZIndex> = None;
    let next = |z: &mut Option<ZIndex>| {
        let k = ZIndex::between(z.as_ref(), None).expect("z index");
        *z = Some(k.clone());
        k
    };
    let mut shapes = Vec::new();
    for i in 0..SHAPES {
        let (cx, cy) = ((i % cols) as f64 * 260.0, (i / cols) as f64 * 200.0);
        let e = Element::new(id("s", i), ElementKind::Rectangle, next(&mut z)).with_bounds(cx, cy, 120.0, 80.0);
        shapes.push(e.id.clone());
        tx.insert(e).expect("insert shape");
    }
    for i in 0..SHAPES {
        let (cx, cy) = ((i % cols) as f64 * 260.0, (i / cols) as f64 * 200.0);
        // From the right side of shape i, 100 units to the right, bound at the end to shape i.
        // Elbow arrows start below and to the right, so each one needs a bend or two.
        let from =
            if elbow { Point::new(cx + 120.0 + 100.0, cy + 160.0) } else { Point::new(cx + 120.0 + 100.0, cy + 40.0) };
        let pts = [from, Point::new(cx + 126.0, cy + 40.0)];
        let mut e = Element::linear(id("a", i), LinearKind::Arrow, next(&mut z), &pts).expect("arrow");
        if let ElementKind::Arrow(d) = &mut e.kind {
            let d: &mut LinearData = d;
            d.end_binding = Some(Binding { target: id("s", i), mode: BindMode::Orbit, fixed_point: [0.5, 0.5] });
            if elbow {
                d.path = PathType::Elbow;
            }
        }
        tx.insert(e).expect("insert arrow");
    }
    tx.commit().expect("commit");
    // Settle the arrows (and route the elbows).
    let mut tx = store.begin();
    update_bound_arrows(&mut tx, &shapes).expect("settle");
    tx.commit().expect("commit settle");
    (store, shapes)
}

fn move_some(tx: &mut impl SceneTx, shapes: &[ElementId]) -> Vec<ElementId> {
    let moved: Vec<ElementId> = shapes.iter().step_by(SHAPES / MOVED).cloned().collect();
    for id in &moved {
        tx.update(id, |e| {
            e.transform.x += 17.0;
            e.transform.y += 9.0;
        })
        .expect("move");
    }
    moved
}

fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("update_bound_arrows_1k_arrows_1k_shapes_100_moved");
    group.sample_size(30);

    let (mut simple, shapes) = scene(false);
    {
        // Pre-flight: the benchmark must really re-solve the 100 arrows of the 100 moved shapes.
        let mut tx = simple.begin();
        let moved = move_some(&mut tx, &shapes);
        let n = update_bound_arrows(&mut tx, &moved).expect("solve");
        assert_eq!(n.len(), MOVED);
    }
    group.bench_function("scan", |b| {
        b.iter_batched(
            || (),
            |()| {
                let mut tx = simple.begin();
                let moved = move_some(&mut tx, &shapes);
                let n = update_bound_arrows(&mut tx, &moved).expect("solve");
                black_box(n.len());
                // Dropping the transaction rolls it back, so every iteration starts from the same scene.
            },
            BatchSize::PerIteration,
        )
    });
    let derived = simple.derived().clone();
    group.bench_function("indexed", |b| {
        b.iter_batched(
            || (),
            |()| {
                let mut tx = simple.begin();
                let moved = move_some(&mut tx, &shapes);
                let n = update_bound_arrows_indexed(&mut tx, &moved, &derived).expect("solve");
                black_box(n.len());
            },
            BatchSize::PerIteration,
        )
    });

    let (mut elbows, shapes) = scene(true);
    {
        let mut tx = elbows.begin();
        let moved = move_some(&mut tx, &shapes);
        let n = update_bound_arrows(&mut tx, &moved).expect("solve");
        assert_eq!(n.len(), MOVED);
    }
    group.bench_function("elbow", |b| {
        b.iter_batched(
            || (),
            |()| {
                let mut tx = elbows.begin();
                let moved = move_some(&mut tx, &shapes);
                let n = update_bound_arrows(&mut tx, &moved).expect("solve");
                black_box(n.len());
            },
            BatchSize::PerIteration,
        )
    });
    group.finish();
}

/// 1,000 shapes in 40 columns and 100 elbow arrows, each from a shape to the one three columns to its right, with
/// two shapes in the way. Settled, so every arrow has its route around the two shapes.
fn around_scene() -> (MemoryStore, Vec<ElementId>) {
    let (mut store, shapes) = scene(false);
    let mut tx = store.begin();
    let mut z: Option<ZIndex> = tx.top_z();
    let mut arrows = Vec::new();
    for k in 0..100usize {
        let (row, col) = (k / 10 * 2, k % 10 * 4);
        let (from, to) = (row * 40 + col, row * 40 + col + 3);
        let start = Point::new((col as f64) * 260.0 + 120.0 + 6.0, (row as f64) * 200.0 + 40.0);
        let end = Point::new(((col + 3) as f64) * 260.0 - 6.0, (row as f64) * 200.0 + 40.0);
        let key = ZIndex::between(z.as_ref(), None).expect("z index");
        z = Some(key.clone());
        let mut e = Element::linear(id("e", k), LinearKind::Arrow, key, &[start, end]).expect("arrow");
        if let ElementKind::Arrow(d) = &mut e.kind {
            d.path = PathType::Elbow;
            d.start_binding =
                Some(Binding { target: shapes[from].clone(), mode: BindMode::Orbit, fixed_point: [1.0, 0.5] });
            d.end_binding =
                Some(Binding { target: shapes[to].clone(), mode: BindMode::Orbit, fixed_point: [0.0, 0.5] });
        }
        arrows.push(e.id.clone());
        tx.insert(e).expect("insert");
    }
    tx.commit().expect("commit");
    let mut tx = store.begin();
    for a in &arrows {
        elbow::reroute_forced(&mut tx, a, RouteOpts::default()).expect("route");
    }
    tx.commit().expect("commit routes");
    (store, arrows)
}

fn bench_around(c: &mut Criterion) {
    let mut group = c.benchmark_group("elbow_around_shapes_1k_shapes");
    group.sample_size(30);
    let (mut store, arrows) = around_scene();
    let first = arrows[0].clone();
    // The second shape of the first row lies between the ends of `e0`.
    let blocker = id("s", 1);

    // One arrow routed again, in a fresh transaction (the box index answers).
    group.bench_function("route_one_arrow", |b| {
        b.iter_batched(
            || (),
            |()| {
                let mut tx = store.begin();
                black_box(elbow::reroute_forced(&mut tx, &first, RouteOpts::default()).expect("route"));
            },
            BatchSize::PerIteration,
        )
    });

    // A shape moves out of the way and back: the commit, then the pass in a fresh transaction, as the editor does it.
    group.bench_function("move_one_shape_and_back", |b| {
        b.iter(|| {
            for dy in [3000.0, -3000.0] {
                let old = elbow::obstacle_box(store.get(&blocker).expect("shape"));
                let mut tx = store.begin();
                tx.update(&blocker, |e| e.transform.y += dy).expect("move");
                tx.commit().expect("commit move");
                let mut tx = store.begin();
                let n = update_bound_arrows_near(&mut tx, std::slice::from_ref(&blocker), &[old]).expect("solve");
                black_box(n.len());
                tx.commit().expect("commit solve");
            }
        })
    });
    // The worst case in this scene: from the first shape of a row to the last, 38 shapes between them in a row of
    // 40, so more shapes lie in the way than a search starts with.
    {
        let (a, b) = (store.get(&id("s", 0)).expect("shape").clone(), store.get(&id("s", 39)).expect("shape").clone());
        let key = ZIndex::between(store.top_z().as_ref(), None).expect("z index");
        let mut tx = store.begin();
        let pts = [
            Point::new(a.transform.x + a.transform.w + 6.0, a.transform.y + 40.0),
            Point::new(b.transform.x - 6.0, b.transform.y + 40.0),
        ];
        let mut e = Element::linear(id("wide", 0), LinearKind::Arrow, key, &pts).expect("arrow");
        if let ElementKind::Arrow(d) = &mut e.kind {
            d.path = PathType::Elbow;
            d.start_binding = Some(Binding { target: a.id.clone(), mode: BindMode::Orbit, fixed_point: [1.0, 0.5] });
            d.end_binding = Some(Binding { target: b.id.clone(), mode: BindMode::Orbit, fixed_point: [0.0, 0.5] });
        }
        tx.insert(e).expect("insert");
        tx.commit().expect("commit");
    }
    let wide = id("wide", 0);
    group.bench_function("route_across_a_row_of_38_shapes", |b| {
        b.iter_batched(
            || (),
            |()| {
                let mut tx = store.begin();
                black_box(elbow::reroute_forced(&mut tx, &wide, RouteOpts::default()).expect("route"));
            },
            BatchSize::PerIteration,
        )
    });

    group.finish();
}

criterion_group!(benches, bench, bench_around);
criterion_main!(benches);
