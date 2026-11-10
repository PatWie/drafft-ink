//! Layout cost: 1000 labels and a set of equations, cold (cache empty) and warm (cache full).
//!
//! `math/old_app_style` rebuilds the math font and its codepoint map for every formula, which is
//! what the old app did on every frame for every visible equation.

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use drafftink_typeset::bench_support::math_with_fresh_engine;
use drafftink_typeset::*;

fn labels(n: usize) -> Vec<TextSpec> {
    (0..n)
        .map(|i| {
            let text = match i % 4 {
                0 => format!("Step {i}: validate input"),
                1 => format!("Service {i}\nreads queue {}", i % 7),
                2 => format!("A longer note about item {i} that needs wrapping inside its box"),
                _ => format!("{i}"),
            };
            let mut spec = TextSpec::styled(text, TextStyle::with_size(14.0 + (i % 5) as f32 * 2.0));
            if i % 4 == 2 {
                spec = spec.wrapped(160.0);
            }
            if i % 3 == 0 {
                spec.spans.push(Span { range: 0..2, style: SpanStyle::color(Rgba8::rgb(200, 30, 30)) });
            }
            spec
        })
        .collect()
}

const FORMULAS: &[&str] = &[
    "E = mc^2",
    "\\frac{a}{b} + \\sqrt{x^2 + y^2}",
    "\\sum_{i=0}^{n} i = \\frac{n(n+1)}{2}",
    "\\int_0^\\infty e^{-x^2}\\,dx = \\frac{\\sqrt{\\pi}}{2}",
    "\\alpha + \\beta = \\gamma",
    "x_{1,2} = \\frac{-b \\pm \\sqrt{b^2-4ac}}{2a}",
    "\\begin{matrix} a & b \\\\ c & d \\end{matrix}",
    "\\frac{1}{2}mv^2",
];

fn text(c: &mut Criterion) {
    let specs = labels(1000);
    let mut g = c.benchmark_group("text_1k_labels");
    g.sample_size(20);
    g.bench_function("first_ever_new_typesetter", |b| {
        b.iter_batched(
            Typesetter::new,
            |mut ts| {
                for s in &specs {
                    std::hint::black_box(ts.text(s));
                }
            },
            BatchSize::PerIteration,
        );
    });
    let mut ts = Typesetter::new();
    for s in &specs {
        let _ = ts.text(s);
    }
    g.bench_function("cold_cache_warm_fonts", |b| {
        b.iter(|| {
            ts.clear_caches();
            for s in &specs {
                std::hint::black_box(ts.text(s));
            }
        });
    });
    for s in &specs {
        let _ = ts.text(s);
    }
    g.bench_function("warm", |b| {
        b.iter(|| {
            for s in &specs {
                std::hint::black_box(ts.text(s));
            }
        });
    });
    g.finish();
}

fn math(c: &mut Criterion) {
    let specs: Vec<MathSpec> = (0..100)
        .map(|i| MathSpec::new(FORMULAS[i % FORMULAS.len()]).with_size(16.0 + (i / FORMULAS.len()) as f64))
        .collect();
    let mut g = c.benchmark_group("math_100_equations");
    g.sample_size(20);
    let mut ts = Typesetter::new();
    let _ = ts.math(&specs[0]);
    g.bench_function("cold_cache", |b| {
        b.iter(|| {
            ts.clear_caches();
            for s in &specs {
                std::hint::black_box(ts.math(s).ok());
            }
        });
    });
    for s in &specs {
        let _ = ts.math(s);
    }
    g.bench_function("warm", |b| {
        b.iter(|| {
            for s in &specs {
                std::hint::black_box(ts.math(s).ok());
            }
        });
    });
    g.bench_function("old_app_style_per_frame", |b| {
        b.iter(|| {
            for s in &specs {
                std::hint::black_box(math_with_fresh_engine(s).ok());
            }
        });
    });
    g.finish();
}

fn caret(c: &mut Criterion) {
    let mut ts = Typesetter::new();
    let l = ts.text(&TextSpec::new("The quick brown fox jumps over the lazy dog ".repeat(20)).wrapped(400.0));
    let mut g = c.benchmark_group("caret");
    g.bench_function("point_to_caret_and_back", |b| {
        b.iter(|| {
            let c = l.caret_at_point(kurbo::Point::new(200.0, 150.0));
            std::hint::black_box(l.caret_rect(c, 1.5));
        });
    });
    g.bench_function("select_all_rects", |b| b.iter(|| std::hint::black_box(l.selection_rects(&l.select_all()))));
    g.finish();
}

criterion_group!(benches, text, math, caret);
criterion_main!(benches);
