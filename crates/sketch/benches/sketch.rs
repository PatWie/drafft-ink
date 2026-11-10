//! Benches for the expensive cases.
//!
//! The old app did all of these on every frame: hachure generation for patterned fills, the noise
//! pass for open lines and arrows, and the pressure outline of freehand strokes. Here the same work
//! is a cache miss, and a hit is a hash plus an `Arc` clone.

use std::hint::black_box;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use drafftink_sketch::{
    Arrowhead, Curve, FillStyle, PressureProfile, Roundness, SketchCache, SketchInput, SketchParams, SketchShape,
    sketch, sketch_key,
};
use kurbo::Point;

fn rect(w: f64, h: f64, fill: FillStyle) -> SketchInput {
    SketchInput::new(
        SketchShape::Rect { width: w, height: h, roundness: Roundness::Sharp },
        SketchParams { fill, seed: 7, ..SketchParams::default() },
    )
}

fn stroke_points(n: usize) -> Vec<Point> {
    (0..n)
        .map(|i| {
            let t = i as f64;
            Point::new(t * 0.8, 40.0 * (t * 0.05).sin() + 6.0 * (t * 0.9).cos())
        })
        .collect()
}

fn freehand(n: usize, pressure: bool) -> SketchInput {
    SketchInput::new(
        SketchShape::Freehand {
            points: stroke_points(n),
            pressures: if pressure {
                (0..n).map(|i| 0.3 + 0.7 * ((i as f64) * 0.02).sin().abs()).collect()
            } else {
                Vec::new()
            },
            closed: false,
            profile: PressureProfile::default(),
        },
        SketchParams { seed: 7, ..SketchParams::default() },
    )
}

fn bench(c: &mut Criterion) {
    // Hachure on a large rectangle: gap 8 (stroke width 2) over 4000 x 3000 needs ~875 lines.
    let big = rect(4000.0, 3000.0, FillStyle::Hachure);
    c.bench_function("hachure_fill_4000x3000", |b| b.iter(|| sketch(black_box(&big))));
    let cross = rect(4000.0, 3000.0, FillStyle::CrossHatch);
    c.bench_function("crosshatch_fill_4000x3000", |b| b.iter(|| sketch(black_box(&cross))));
    let typical = rect(300.0, 200.0, FillStyle::Hachure);
    c.bench_function("hachure_fill_300x200", |b| b.iter(|| sketch(black_box(&typical))));
    let dots = rect(1000.0, 1000.0, FillStyle::Dots);
    c.bench_function("dots_fill_1000x1000", |b| b.iter(|| sketch(black_box(&dots))));
    let plain = rect(300.0, 200.0, FillStyle::None);
    c.bench_function("rect_outline_300x200", |b| b.iter(|| sketch(black_box(&plain))));

    // Long freehand strokes.
    for n in [1_000, 10_000, 50_000] {
        let with = freehand(n, true);
        c.bench_function(&format!("freehand_pressure_{n}_points"), |b| b.iter(|| sketch(black_box(&with))));
        let without = freehand(n, false);
        c.bench_function(&format!("freehand_sketchy_{n}_points"), |b| b.iter(|| sketch(black_box(&without))));
    }

    // Arrow with a bent shaft.
    let arrow = SketchInput::new(
        SketchShape::Arrow {
            points: stroke_points(8),
            curve: Curve::Smooth,
            start: Arrowhead::NONE,
            end: Arrowhead::TRIANGLE,
            head_size: 15.0,
        },
        SketchParams { seed: 7, ..SketchParams::default() },
    );
    c.bench_function("arrow_8_points", |b| b.iter(|| sketch(black_box(&arrow))));

    // Cache: what a drag frame costs (hit) against what a resize frame costs (miss).
    let mut cache = SketchCache::new(64);
    cache.get(&big);
    c.bench_function("cache_hit_hachure_4000x3000", |b| b.iter(|| cache.get(black_box(&big))));
    let long = freehand(10_000, true);
    c.bench_function("key_10000_points", |b| b.iter(|| sketch_key(black_box(&long))));
    c.bench_function("cache_hit_freehand_10000_points", |b| {
        let mut cache = SketchCache::new(64);
        cache.get(&long);
        b.iter(|| cache.get(black_box(&long)))
    });
    // Resize drag: every frame has a new width, so every frame is a miss.
    c.bench_function("resize_drag_frame_300x200_hachure", |b| {
        let mut w = 300.0;
        b.iter_batched(
            || {
                w += 1.0;
                rect(w, 200.0, FillStyle::Hachure)
            },
            |i| {
                let mut cache = SketchCache::new(64);
                cache.get(&i)
            },
            BatchSize::SmallInput,
        )
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
