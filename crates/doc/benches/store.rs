//! Criterion benches for the Loro store against the old baseline (docs/performance.md, "Sync"):
//! old `sync_to_crdt` (every local edit) 64 ms at 1k shapes and 427 ms at 10k;
//! old `sync_from_crdt` (every received message) 2.05 s at 1k shapes and over 80 s at 10k.
//!
//! Run: `cargo bench -p drafftink-doc`. Set `DOC_BENCH_SIZES=1000,10000` to choose sizes.

#![allow(clippy::expect_used)]

use std::hint::black_box;

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use drafftink_doc::{LoroConfig, LoroStore};
use drafftink_scene::*;
use kurbo::Point;

fn sizes() -> Vec<usize> {
    std::env::var("DOC_BENCH_SIZES")
        .ok()
        .map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![1_000, 10_000, 100_000])
}

fn id(i: usize) -> ElementId {
    ElementId::new(&format!("e{i:07}")).expect("generated ids are valid")
}

/// A mixed scene: 60% rectangles, 20% text, 10% arrows, 10% freehand strokes of 30 points.
fn scene(n: usize) -> Vec<Element> {
    let keys = ZIndex::rebalance(n).unwrap_or_default();
    (0..n)
        .map(|i| {
            let (x, y) = (
                f64::from(u32::try_from(i % 300).unwrap_or(0)) * 140.0,
                f64::from(u32::try_from(i / 300).unwrap_or(0)) * 140.0,
            );
            let key = keys.get(i).cloned().unwrap_or_else(ZIndex::first);
            let mut e = match i % 10 {
                0 | 1 => Element::new(id(i), ElementKind::Text(TextData::new("A label of average length")), key)
                    .with_bounds(x, y, 120.0, 25.0),
                2 => Element::linear(
                    id(i),
                    Kind::Arrow,
                    key.clone(),
                    &[Point::new(x, y), Point::new(x + 80.0, y + 40.0)],
                )
                .unwrap_or_else(|| Element::new(id(i), ElementKind::Rectangle, key)),
                3 => {
                    let pts: Vec<Point> = (0..30).map(|k| Point::new(x + f64::from(k), y + f64::from(k % 7))).collect();
                    let mut f = Element::new(id(i), ElementKind::Freehand(FreehandData::new(pts)), key);
                    f.refit_points();
                    f
                }
                _ => Element::new(id(i), ElementKind::Rectangle, key).with_bounds(x, y, 100.0, 60.0),
            };
            e.style.stroke_color = Color::rgb(u8::try_from(i % 251).unwrap_or(0), 99, 241);
            e
        })
        .collect()
}

fn loaded(n: usize, peer: u64) -> LoroStore {
    let mut s = LoroStore::with_config(LoroConfig { peer_id: Some(peer), ..LoroConfig::default() });
    let mut tx = s.begin();
    for e in scene(n) {
        let _ = tx.insert(e);
    }
    let _ = tx.commit();
    s.clear_history();
    s
}

fn bench_local_edit(c: &mut Criterion) {
    let mut g = c.benchmark_group("local_edit_commit");
    g.sample_size(30);
    for n in sizes() {
        let mut s = loaded(n, 1);
        let target = id(n / 2);
        let mut x = 0.0;
        g.bench_function(BenchmarkId::new("move_one_element", n), |b| {
            b.iter(|| {
                x += 1.0;
                let mut tx = s.begin();
                let _ = tx.update(&target, |e| e.transform.x = x);
                black_box(tx.commit())
            });
        });
        let mut hue = 0u8;
        g.bench_function(BenchmarkId::new("recolor_one_element", n), |b| {
            b.iter(|| {
                hue = hue.wrapping_add(1);
                let mut tx = s.begin();
                let _ = tx.update(&target, |e| e.style.stroke_color = Color::rgb(hue, 1, 2));
                black_box(tx.commit())
            });
        });
    }
    g.finish();
}

fn bench_apply_remote(c: &mut Criterion) {
    let mut g = c.benchmark_group("apply_remote_single_element");
    g.sample_size(30);
    for n in sizes() {
        let mut a = loaded(n, 1);
        let mut b = match LoroStore::from_snapshot_with(
            &a.export_snapshot().unwrap_or_default(),
            LoroConfig { peer_id: Some(2), ..LoroConfig::default() },
        ) {
            Ok(s) => s,
            Err(_) => return,
        };
        let target = id(n / 3);
        let mut x = 0.0;
        g.bench_function(BenchmarkId::new("move", n), |bench| {
            bench.iter_batched(
                || {
                    x += 1.0;
                    // `b` is in sync with `a` before every edit, so a's version before the edit is b's version.
                    let before = a.version_vector();
                    let mut tx = a.begin();
                    let _ = tx.update(&target, |e| e.transform.x = x);
                    let _ = tx.commit();
                    a.export_updates_since(&before).unwrap_or_default()
                },
                |bytes| black_box(b.apply_remote(&bytes)),
                BatchSize::PerIteration,
            );
        });
    }
    g.finish();
}

fn bench_snapshots(c: &mut Criterion) {
    let mut g = c.benchmark_group("snapshot");
    g.sample_size(10);
    for n in sizes() {
        let s = loaded(n, 1);
        let full = s.export_snapshot().unwrap_or_default();
        let state = s.export_state_only().unwrap_or_default();
        eprintln!("n={n}: full snapshot {} KiB, state-only {} KiB", full.len() / 1024, state.len() / 1024);
        g.bench_function(BenchmarkId::new("export_full", n), |b| b.iter(|| black_box(s.export_snapshot())));
        g.bench_function(BenchmarkId::new("export_state_only", n), |b| b.iter(|| black_box(s.export_state_only())));
        g.bench_function(BenchmarkId::new("import_full", n), |b| b.iter(|| black_box(LoroStore::from_snapshot(&full))));
        g.bench_function(BenchmarkId::new("import_state_only", n), |b| {
            b.iter(|| black_box(LoroStore::from_snapshot(&state)))
        });
    }
    g.finish();
}

fn bench_bulk_and_undo(c: &mut Criterion) {
    let mut g = c.benchmark_group("other");
    g.sample_size(10);
    for n in sizes().into_iter().filter(|n| *n <= 10_000) {
        g.bench_function(BenchmarkId::new("bulk_load_commit", n), |b| {
            b.iter_batched(
                || scene(n),
                |els| {
                    let mut s = LoroStore::new();
                    let mut tx = s.begin();
                    for e in els {
                        let _ = tx.insert(e);
                    }
                    black_box(tx.commit())
                },
                BatchSize::PerIteration,
            );
        });
    }
    for n in sizes() {
        let mut s = loaded(n, 1);
        let target = id(n / 2);
        let mut x = 0.0;
        g.bench_function(BenchmarkId::new("edit_undo_redo", n), |b| {
            b.iter(|| {
                x += 1.0;
                let mut tx = s.begin();
                let _ = tx.update(&target, |e| e.transform.x = x);
                let _ = tx.commit();
                let _ = black_box(s.undo());
                black_box(s.redo())
            });
        });
    }
    g.finish();
}

criterion_group!(benches, bench_local_edit, bench_apply_remote, bench_snapshots, bench_bulk_and_undo);
criterion_main!(benches);
