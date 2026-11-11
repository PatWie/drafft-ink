#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! `update_bound_arrows` at scale: 1,000 arrows bound to 1,000 shapes, 100 shapes moved in one transaction.
//!
//! Three solvers are timed on the same scene:
//! * `scan`: `update_bound_arrows`, which finds the arrows by scanning the transaction's elements;
//! * `indexed`: `update_bound_arrows_indexed`, which uses the committed reverse index;
//! * `elbow`: the same with every arrow an elbow arrow (A* routing per arrow).

use std::hint::black_box;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use drafftink_edit::binding::{update_bound_arrows, update_bound_arrows_indexed};
use drafftink_geom::kurbo::Point;
use drafftink_scene::{
    BindMode, Binding, Element, ElementId, ElementKind, Kind, LinearData, MemoryStore, PathType, SceneStore, SceneTx,
    ZIndex,
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
        let mut e = Element::linear(id("a", i), Kind::Arrow, next(&mut z), &pts).expect("arrow");
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

criterion_group!(benches, bench);
criterion_main!(benches);
