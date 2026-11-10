//! Hit test, query and index update benchmarks at 1k, 10k and 100k elements.
//!
//! The scene is the one of the old baseline and of the `geom` bench: a grid with 120 x 80 spacing, half rectangles
//! 100 x 60, a quarter ellipses, a quarter lines; every other rectangle filled. The old app scanned every shape for a
//! hit: 60 us at 1k, 0.70 ms at 10k, 9.1 ms at 100k (`research/baseline-perf.md`).
//!
//! `cargo bench -p drafftink-canvas`. Results are summarised in `docs/canvas-api.md`.

#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::hint::black_box;

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use drafftink_canvas::geom::kurbo::{Point, Rect, Size};
use drafftink_canvas::scene::{Color, Element, ElementId, ElementKind, Kind, MemoryStore, SceneStore, SceneTx, ZIndex};
use drafftink_canvas::{Canvas, HitOptions, InputEvent, MarqueeMode, PointerEvent, PointerPhase, SceneIndex, Viewport};

const SIZES: [usize; 3] = [1_000, 10_000, 100_000];

fn build_store(n: usize) -> MemoryStore {
    let cols = (n as f64).sqrt().ceil() as usize;
    let keys = ZIndex::n_between(None, None, n).unwrap_or_default();
    let mut store = MemoryStore::new();
    let mut tx = store.begin();
    for (i, z) in keys.into_iter().enumerate() {
        let (x, y) = ((i % cols) as f64 * 120.0, (i / cols) as f64 * 80.0);
        let id = ElementId::new(&format!("e{i}")).unwrap_or_else(|_| ElementId::new("x").unwrap());
        let mut e = match i % 4 {
            0 | 1 => Element::new(id, ElementKind::Rectangle, z).with_bounds(x, y, 100.0, 60.0),
            2 => Element::new(id, ElementKind::Ellipse, z).with_bounds(x, y, 100.0, 60.0),
            _ => Element::linear(id, Kind::Line, z, &[Point::new(x, y), Point::new(x + 100.0, y + 60.0)]).unwrap(),
        };
        if i % 2 == 0 {
            e.style.fill_color = Some(Color::rgb(10, 20, 30));
        }
        let _ = tx.insert(e);
    }
    let _ = tx.commit();
    store
}

fn centre(n: usize) -> Point {
    let cols = (n as f64).sqrt().ceil();
    Point::new(cols * 60.0 + 50.0, cols * 40.0 + 30.0)
}

fn bench(c: &mut Criterion) {
    let mut g = c.benchmark_group("canvas");
    g.sample_size(20);
    for &n in &SIZES {
        let store = build_store(n);
        let index = SceneIndex::build(&store);
        let p = centre(n);
        let opts = HitOptions::default();

        g.bench_with_input(BenchmarkId::new("index_build_bulk", n), &n, |b, _| {
            b.iter(|| black_box(SceneIndex::build(black_box(&store)).len()))
        });
        g.bench_with_input(BenchmarkId::new("hit", n), &n, |b, _| {
            b.iter(|| black_box(index.hit(&store, black_box(p), 1.0, &opts)))
        });
        g.bench_with_input(BenchmarkId::new("hit_all", n), &n, |b, _| {
            b.iter(|| black_box(index.hit_all(&store, black_box(p), 1.0, &opts).len()))
        });
        g.bench_with_input(BenchmarkId::new("hit_miss_far_away", n), &n, |b, _| {
            b.iter(|| black_box(index.hit(&store, black_box(Point::new(-5000.0, -5000.0)), 1.0, &opts)))
        });
        let view = Rect::new(p.x - 400.0, p.y - 300.0, p.x + 400.0, p.y + 300.0);
        g.bench_with_input(BenchmarkId::new("culling_query_800x600", n), &n, |b, _| {
            b.iter(|| black_box(index.in_rect(&store, black_box(&view)).len()))
        });
        g.bench_with_input(BenchmarkId::new("marquee_intersect_800x600", n), &n, |b, _| {
            b.iter(|| black_box(index.marquee(&store, black_box(&view), MarqueeMode::Intersect, false).len()))
        });

        // Incremental update: one element moves. The change set comes from the store; only `apply` is timed.
        let target = ElementId::new("e0").unwrap();
        g.bench_with_input(BenchmarkId::new("index_apply_one_move", n), &n, |b, _| {
            let store = RefCell::new(build_store(n));
            let mut index = SceneIndex::build(&*store.borrow());
            let mut flip = false;
            b.iter_batched(
                || {
                    flip = !flip;
                    let dx = if flip { 1000.0 } else { -1000.0 };
                    let mut store = store.borrow_mut();
                    let mut tx = store.begin();
                    let _ = tx.update(&target, |e| e.transform.x += dx);
                    tx.commit().unwrap_or_default()
                },
                |cs| black_box(index.apply(&*store.borrow(), &cs)),
                BatchSize::SmallInput,
            )
        });

        // The whole path a host takes: transaction, subscription, index update.
        g.bench_with_input(BenchmarkId::new("canvas_transact_one_move", n), &n, |b, _| {
            let mut canvas = Canvas::new(build_store(n), Viewport::new(Size::new(800.0, 600.0), 1.0));
            let mut dx = 1.0;
            b.iter(|| {
                dx = -dx;
                let _ = canvas.transact(|tx| tx.update(&target, |e| e.transform.y += dx));
            })
        });

        // A hover move through the whole stack (pick under the pointer, no change of target).
        g.bench_with_input(BenchmarkId::new("canvas_hover_move", n), &n, |b, _| {
            let mut canvas = Canvas::new(build_store(n), Viewport::new(Size::new(800.0, 600.0), 1.0));
            canvas.set_camera(drafftink_canvas::Camera { scroll: p, zoom: 1.0 });
            let mut t = 0.0;
            b.iter(|| {
                t += 1.0;
                let x = 400.0 + (t % 7.0);
                black_box(
                    canvas
                        .handle_input(&InputEvent::Pointer(PointerEvent::mouse(PointerPhase::Move, x, 300.0, t)))
                        .len(),
                )
            })
        });
    }
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
