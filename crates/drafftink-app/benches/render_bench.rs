//! Benchmark for build_scene performance

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use drafftink_core::{Canvas, canvas::CanvasDocument};
use drafftink_render::{RenderContext, Renderer, VelloRenderer};

const INTRO_JSON: &str = include_str!("../assets/intro.json");

fn bench_build_scene(c: &mut Criterion) {
    let doc = CanvasDocument::from_json(INTRO_JSON).expect("Failed to parse intro.json");
    let mut canvas = Canvas::new();
    canvas.document = doc;
    let size = kurbo::Size::new(1920.0, 1080.0);

    // Cold: fresh renderer each time (no cache)
    c.bench_function("build_scene_cold", |b| {
        b.iter(|| {
            let mut renderer = VelloRenderer::new();
            let ctx = RenderContext::new(black_box(&canvas), size);
            renderer.build_scene(&ctx);
        })
    });

    // Warm: reuse renderer (cache populated)
    let mut renderer = VelloRenderer::new();
    // Prime the cache
    let ctx = RenderContext::new(&canvas, size);
    renderer.build_scene(&ctx);

    c.bench_function("build_scene_warm", |b| {
        b.iter(|| {
            let ctx = RenderContext::new(black_box(&canvas), size);
            renderer.build_scene(&ctx);
        })
    });
}

criterion_group!(benches, bench_build_scene);
criterion_main!(benches);
