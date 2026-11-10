//! Cost of the profiler's hot calls. Run: `cargo bench -p drafftink-profile`.
//!
//! The numbers that matter: a span with recording off (what every user pays), a span with
//! recording on, and the always-on counter and redraw calls.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use drafftink_profile as profile;
use profile::{Counter, Phase, RedrawReason};

fn spans(c: &mut Criterion) {
    let mut g = c.benchmark_group("span");

    profile::set_enabled(false);
    g.bench_function("off", |b| {
        b.iter(|| {
            let _s = profile::span(black_box("bench"));
        })
    });
    g.bench_function("phase_off", |b| {
        b.iter(|| {
            let _p = profile::phase(black_box(Phase::Encode));
        })
    });

    profile::set_enabled(true);
    profile::reset();
    g.bench_function("on", |b| {
        b.iter(|| {
            let _s = profile::span(black_box("bench"));
        })
    });
    g.bench_function("on_nested3", |b| {
        b.iter(|| {
            let _a = profile::span(black_box("a"));
            let _b = profile::span(black_box("b"));
            let _c = profile::span(black_box("c"));
        })
    });
    profile::begin_frame();
    g.bench_function("phase_on", |b| {
        b.iter(|| {
            let _p = profile::phase(black_box(Phase::Encode));
        })
    });
    profile::end_frame();
    profile::set_enabled(false);
    profile::reset();
    g.finish();
}

fn always_on(c: &mut Criterion) {
    let mut g = c.benchmark_group("always_on");
    g.bench_function("count", |b| b.iter(|| profile::count(black_box(Counter::ElementsEncoded), black_box(1))));
    g.bench_function("redraw", |b| b.iter(|| profile::redraw(black_box(RedrawReason::HoverChange))));
    g.bench_function("work_scope_begin", |b| b.iter(|| black_box(profile::WorkScope::begin())));
    g.finish();
    profile::reset();
}

fn frames(c: &mut Criterion) {
    let mut g = c.benchmark_group("frame");
    profile::set_enabled(true);
    g.bench_function("begin_end_on", |b| {
        b.iter(|| {
            profile::begin_frame();
            profile::end_frame();
        })
    });
    g.bench_function("snapshot_json", |b| b.iter(|| black_box(profile::snapshot().to_json())));
    profile::set_enabled(false);
    g.finish();
}

criterion_group!(benches, spans, always_on, frames);
criterion_main!(benches);
