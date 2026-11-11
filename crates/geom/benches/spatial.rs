//! Spatial index and hit-test benchmarks at 1k, 10k and 100k elements.
//!
//! The scene is the one in the old baseline bench (`research/bench-baseline`): a grid with
//! 120 x 80 spacing, half rectangles 100 x 60, a quarter ellipses 100 x 60, a quarter lines;
//! id = grid index. The old numbers to compare with (`research/baseline-perf.md`):
//!
//! | operation | 1k | 10k | 100k |
//! |---|---|---|---|
//! | `shapes_at_point` (linear scan) | 60 us | 0.70 ms | 9.1 ms |
//! | `shapes_in_rect` 800x600 | 66 us | 0.77 ms | 9.6 ms |
//! | smart guide input: collect bounds + detect | 157 us | 1.67 ms | 17.0 ms |

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use drafftink_geom::guides::detect_smart_guides;
use drafftink_geom::kurbo::{Point, Rect, Size};
use drafftink_geom::snap::MAX_SNAP_CANDIDATES;
use drafftink_geom::{EllipseShape, Fill, HitOpts, OrientedBox, Outline, Polyline, Primitive, RectShape, SpatialIndex};

const SIZES: [usize; 3] = [1_000, 10_000, 100_000];

fn make_shapes(n: usize) -> Vec<Primitive> {
    let cols = (n as f64).sqrt().ceil() as usize;
    (0..n)
        .map(|i| {
            let (cx, cy) = ((i % cols) as f64 * 120.0, (i / cols) as f64 * 80.0);
            match i % 4 {
                0 | 1 => RectShape::from_rect(Rect::new(cx, cy, cx + 100.0, cy + 60.0)).into(),
                2 => EllipseShape::new(OrientedBox::new(Point::new(cx + 50.0, cy + 30.0), Size::new(100.0, 60.0), 0.0))
                    .into(),
                _ => Polyline::new(vec![Point::new(cx, cy), Point::new(cx + 100.0, cy + 60.0)], false).into(),
            }
        })
        .collect()
}

fn centre(n: usize) -> Point {
    let cols = (n as f64).sqrt().ceil();
    Point::new(cols * 60.0, cols * 40.0)
}

fn build_index(shapes: &[Primitive]) -> SpatialIndex<u32> {
    SpatialIndex::bulk_load(shapes.iter().enumerate().map(|(i, s)| (i as u32, s.bounds_with_stroke(2.0))))
}

fn bench(c: &mut Criterion) {
    let mut g = c.benchmark_group("geom");
    g.sample_size(20);
    for &n in &SIZES {
        let shapes = make_shapes(n);
        let index = build_index(&shapes);
        let p = centre(n);
        let opts = HitOpts::stroke(5.0, 2.0);

        // Index candidates only.
        g.bench_with_input(BenchmarkId::new("index_query_point", n), &n, |b, _| {
            b.iter(|| black_box(index.query_point(black_box(p), 5.0).count()))
        });

        // The whole pipeline that replaces `shapes_at_point`: candidates, then exact hit tests,
        // topmost (highest id) first.
        g.bench_with_input(BenchmarkId::new("hit_test_point", n), &n, |b, _| {
            b.iter(|| {
                let mut hits: Vec<u32> = index
                    .query_point(black_box(p), 5.0)
                    .map(|(id, _)| *id)
                    .filter(|id| shapes[*id as usize].hit_test(p, opts))
                    .collect();
                hits.sort_unstable_by(|a, b| b.cmp(a));
                black_box(hits)
            })
        });

        let marquee = Rect::new(p.x, p.y, p.x + 800.0, p.y + 600.0);
        g.bench_with_input(BenchmarkId::new("index_query_rect_800x600", n), &n, |b, _| {
            b.iter(|| black_box(index.query_rect(black_box(&marquee)).count()))
        });
        g.bench_with_input(BenchmarkId::new("marquee_exact_800x600", n), &n, |b, _| {
            b.iter(|| {
                let hits: Vec<u32> = index
                    .query_rect(black_box(&marquee))
                    .map(|(id, _)| *id)
                    .filter(|id| shapes[*id as usize].intersects_rect(&marquee, Fill::Stroke))
                    .collect();
                black_box(hits)
            })
        });

        g.bench_with_input(BenchmarkId::new("nearest", n), &n, |b, _| {
            b.iter(|| black_box(index.nearest(black_box(Point::new(p.x + 3.0, p.y + 7.0)))))
        });

        // Smart-guide input: candidates in a 400 unit zone around the dragged box, then detect.
        let drag = Rect::new(p.x, p.y, p.x + 100.0, p.y + 60.0);
        g.bench_with_input(BenchmarkId::new("smart_guides_query+detect", n), &n, |b, _| {
            b.iter(|| {
                let zone = Rect::new(drag.x0 - 400.0, drag.y0 - 400.0, drag.x1 + 400.0, drag.y1 + 400.0);
                let others: Vec<Rect> = index.query_rect(&zone).map(|(_, r)| r).take(MAX_SNAP_CANDIDATES).collect();
                black_box(detect_smart_guides(drag, &others, 5.0))
            })
        });

        // Updates: move one element.
        g.bench_with_input(BenchmarkId::new("update_one", n), &n, |b, _| {
            let mut ix = index.clone();
            let id = (n / 2) as u32;
            let base = shapes[id as usize].bounds_with_stroke(2.0);
            let mut flip = false;
            b.iter(|| {
                flip = !flip;
                let d = if flip { 5.0 } else { -5.0 };
                let _ = ix.update(&id, Rect::new(base.x0 + d, base.y0, base.x1 + d, base.y1));
            })
        });
        g.bench_with_input(BenchmarkId::new("remove_insert_one", n), &n, |b, _| {
            let mut ix = index.clone();
            let id = (n / 3) as u32;
            let r = shapes[id as usize].bounds_with_stroke(2.0);
            b.iter(|| {
                ix.remove(&id);
                let _ = ix.insert(id, r);
            })
        });
    }
    g.finish();

    let mut g = c.benchmark_group("geom_build");
    g.sample_size(10);
    for &n in &SIZES {
        let shapes = make_shapes(n);
        g.bench_with_input(BenchmarkId::new("bulk_load", n), &n, |b, _| b.iter(|| black_box(build_index(&shapes))));
        if n <= 10_000 {
            g.bench_with_input(BenchmarkId::new("insert_one_by_one", n), &n, |b, _| {
                b.iter(|| {
                    let mut ix = SpatialIndex::new();
                    for (i, s) in shapes.iter().enumerate() {
                        let _ = ix.insert(i as u32, s.bounds_with_stroke(2.0));
                    }
                    black_box(ix)
                })
            });
        }
    }
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
