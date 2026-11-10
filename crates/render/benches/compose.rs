#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Frame composition at 1k, 10k and 100k elements.
//!
//! `cargo bench -p drafftink-render --bench compose`. Results and the comparison with the old renderer's
//! `build_scene` are in `docs/render.md`. Cold means an empty fragment cache (every visible element is encoded;
//! the typesetter and the sketch cache are as warm as an editor would have them, because text is measured when it
//! is edited). Warm means the cache holds the visible elements.

#[path = "../tests/common/mod.rs"]
mod common;

use common::*;
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use drafftink_render::*;
use drafftink_scene::*;
use drafftink_typeset::Typesetter;
use kurbo::{Size, Vec2};
use vello::Scene;

struct World {
    store: MemoryStore,
    index: PaintIndex,
    ts: Typesetter,
    side: f64,
}

fn world(n: usize) -> World {
    let mut ts = Typesetter::new();
    let store = store_of(mixed_elements(n, 42, &mut ts));
    let index = build_paint_index(&store);
    let side = (n as f64).sqrt().ceil() * 140.0;
    World { store, index, ts, side }
}

fn no_lod() -> RenderOptions {
    RenderOptions { lod: LodConfig::OFF, ..RenderOptions::default() }
}

fn zoomed_in(w: &World, step: f64) -> View {
    // 800 x 600 logical pixels at zoom 1 in the middle of the scene, nudged by `step`.
    let c = w.side / 2.0 + step;
    View::new(1.0, Vec2::new(-c, -c), Size::new(800.0, 600.0))
}

fn fit_all(w: &World) -> View {
    View::fit(kurbo::Rect::new(0.0, 0.0, w.side, w.side), Size::new(1600.0, 900.0), 10.0)
}

fn bench(c: &mut Criterion) {
    let mut g = c.benchmark_group("compose");
    g.sample_size(20);
    for (label, n) in [("1k", 1_000usize), ("10k", 10_000), ("100k", 100_000)] {
        let mut w = world(n);
        let mut scene = Scene::new();

        // Zoomed in: a few hundred elements visible.
        let v = zoomed_in(&w, 0.0);
        let mut warm = SceneComposer::new();
        warm.set_options(no_lod());
        let rep = warm.compose(&w.store, &w.index, &v, &mut w.ts, &mut scene);
        eprintln!("{label} zoomed in: {rep:?}");
        g.bench_function(BenchmarkId::new("zoomed_in_warm", label), |b| {
            b.iter(|| warm.compose(&w.store, &w.index, &v, &mut w.ts, &mut scene));
        });
        g.bench_function(BenchmarkId::new("zoomed_in_cold", label), |b| {
            b.iter_batched(
                || {
                    let mut c = SceneComposer::new();
                    c.set_options(no_lod());
                    c
                },
                |mut c| c.compose(&w.store, &w.index, &v, &mut w.ts, &mut scene),
                BatchSize::LargeInput,
            );
        });
        // Panning: the view moves every frame, the cache is warm after the first pass.
        let mut step = 0.0;
        g.bench_function(BenchmarkId::new("pan_warm", label), |b| {
            b.iter(|| {
                step = (step + 7.0) % 200.0;
                let v = zoomed_in(&w, step);
                warm.compose(&w.store, &w.index, &v, &mut w.ts, &mut scene)
            });
        });

        // Dragging one element: edit, update the index, compose.
        let mut x = 0.0;
        let c0 = w.side as usize / 2;
        let dragged = format!("e{}", (c0 / 140) * ((n as f64).sqrt().ceil() as usize) + c0 / 140);
        g.bench_function(BenchmarkId::new("drag_one_element", label), |b| {
            b.iter(|| {
                x += 1.0;
                let cs = update(&mut w.store, &dragged, |e| e.transform.x += 1.0);
                update_paint_index(&mut w.index, &w.store, &cs);
                warm.compose(&w.store, &w.index, &v, &mut w.ts, &mut scene)
            });
        });
        let _ = x;

        // Everything visible, every element in full (the old benchmark's "everything visible").
        if n <= 10_000 {
            let fit = fit_all(&w);
            let mut full = SceneComposer::new();
            full.set_options(no_lod());
            let rep = full.compose(&w.store, &w.index, &fit, &mut w.ts, &mut scene);
            eprintln!("{label} all visible, full detail: {rep:?}");
            g.bench_function(BenchmarkId::new("all_visible_full_warm", label), |b| {
                b.iter(|| full.compose(&w.store, &w.index, &fit, &mut w.ts, &mut scene));
            });
            g.bench_function(BenchmarkId::new("all_visible_full_cold", label), |b| {
                b.iter_batched(
                    || {
                        let mut c = SceneComposer::new();
                        c.set_options(no_lod());
                        c
                    },
                    |mut c| c.compose(&w.store, &w.index, &fit, &mut w.ts, &mut scene),
                    BatchSize::LargeInput,
                );
            });
        }

        // Everything visible with the default level of detail (the zoomed-out view of a big document).
        let fit = fit_all(&w);
        let mut lod = SceneComposer::new();
        let rep = lod.compose(&w.store, &w.index, &fit, &mut w.ts, &mut scene);
        eprintln!("{label} all visible, level of detail: {rep:?}");
        g.bench_function(BenchmarkId::new("all_visible_lod_warm", label), |b| {
            b.iter(|| lod.compose(&w.store, &w.index, &fit, &mut w.ts, &mut scene));
        });
        g.bench_function(BenchmarkId::new("all_visible_lod_cold", label), |b| {
            b.iter_batched(
                SceneComposer::new,
                |mut c| c.compose(&w.store, &w.index, &fit, &mut w.ts, &mut scene),
                BatchSize::LargeInput,
            );
        });
    }
    g.finish();

    let mut g = c.benchmark_group("overlay");
    let v = View::new(1.0, Vec2::ZERO, Size::new(1600.0, 900.0));
    let o = Overlay {
        selection: vec![SelectionBox {
            obox: drafftink_geom::OrientedBox::from_xywh(100.0, 100.0, 300.0, 200.0, 0.2),
            handles: drafftink_geom::select::HandleSet::ALL,
            dashed: true,
        }],
        marquee: Some(kurbo::Rect::new(0.0, 0.0, 300.0, 300.0)),
        ..Overlay::default()
    };
    let mut scene = Scene::new();
    g.bench_function("selection_and_marquee", |b| {
        b.iter(|| paint_overlay(&mut scene, &o, &v, &OverlayStyle::default()));
    });
    g.finish();

    let mut g = c.benchmark_group("grid");
    let mut scene = Scene::new();
    for (label, zoom) in [("zoom_1", 1.0), ("zoom_0.1", 0.1), ("zoom_0.01", 0.01)] {
        let v = View::new(zoom, Vec2::ZERO, Size::new(1920.0, 1080.0));
        for (sl, style) in [("lines", GridStyle::Lines), ("dots", GridStyle::Dots)] {
            g.bench_function(format!("{sl}/{label}"), |b| {
                b.iter(|| paint_grid(&mut scene, &v, &GridSpec::new(style), Theme::Light));
            });
        }
    }
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
