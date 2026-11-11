//! Hostile input: nothing here may panic, overflow the stack, hang or return a bad layout.
//!
//! Every formula runs on a small stack. Release builds use 1 MiB, the default on wasm32; this is
//! the run that proves the limits in `precheck`. Debug frames are several times larger (ReX
//! overflows 1 MiB at 60 nested matrices in a debug build and at over 120 in release), so debug
//! builds use 4 MiB. Run `cargo test --release -p drafftink-typeset --test hostile` for the strict
//! check. `TYPESET_FUZZ_ITERS` raises the number of random formulas (default 3000).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::Lcg;
use drafftink_typeset::*;
use proptest::prelude::*;

const SMALL_STACK: usize = if cfg!(debug_assertions) { 4 << 20 } else { 1 << 20 };

fn on_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(SMALL_STACK)
        .spawn(f)
        .expect("spawn")
        .join()
        .expect("the thread must not panic")
}

fn check_math(ts: &mut Typesetter, latex: &str) {
    let started = drafftink_profile::now_ns();
    let r = ts.math(&MathSpec::new(latex));
    let took_ms = (drafftink_profile::now_ns() - started) / 1_000_000;
    assert!(
        took_ms < 5000,
        "{} bytes took {took_ms} ms: {:?}",
        latex.len(),
        latex.chars().take(80).collect::<String>()
    );
    match r {
        Ok(l) => {
            assert!(l.width.is_finite() && l.height.is_finite() && l.depth.is_finite());
            assert!(l.width.abs() <= MAX_EXTENT && l.height.abs() <= MAX_EXTENT && l.depth.abs() <= MAX_EXTENT);
            assert!(l.glyphs.iter().all(|g| g.x.is_finite() && g.y.is_finite() && g.size.is_finite()));
            assert!(l.rules.iter().all(|r| r.rect.x0.is_finite() && r.rect.y1.is_finite()));
        }
        Err(e) => {
            assert!(!matches!(e.kind, MathErrorKind::Internal(_)), "ReX panicked on {latex:?}: {e:?}");
            assert!(e.span.start <= e.span.end && e.span.end <= latex.len(), "{e:?}");
            assert!(latex.is_char_boundary(e.span.start) && latex.is_char_boundary(e.span.end), "{e:?}");
            assert!(!e.message.is_empty());
        }
    }
}

const VOCAB: &[&str] = &[
    "a",
    "b",
    "x",
    "1",
    "2",
    "0",
    " ",
    "+",
    "-",
    "=",
    "(",
    ")",
    "[",
    "]",
    "|",
    "!",
    "'",
    "\\,",
    "\\;",
    "\\\\",
    "\\{",
    "\\}",
    "{",
    "}",
    "{",
    "}",
    "^",
    "_",
    "&",
    "\\frac",
    "\\sqrt",
    "\\sum",
    "\\int",
    "\\prod",
    "\\lim",
    "\\infty",
    "\\alpha",
    "\\pi",
    "\\left(",
    "\\right)",
    "\\left.",
    "\\right|",
    "\\middle|",
    "\\begin{matrix}",
    "\\end{matrix}",
    "\\begin{pmatrix}",
    "\\end{pmatrix}",
    "\\begin{array}{cc}",
    "\\end{array}",
    "\\begin{cases}",
    "\\end{cases}",
    "\\color{red}",
    "\\color{#12345678}",
    "\\color{zzz}",
    "\\mathrm",
    "\\mathbf",
    "\\text",
    "\\mathbb",
    "\\mathcal",
    "\\over",
    "\\choose",
    "\\binom",
    "\\overline",
    "\\underline",
    "\\overbrace",
    "\\underbrace",
    "\\hat",
    "\\vec",
    "\\bar",
    "\\dot",
    "\\widehat",
    "\\rule{1pt}{2pt}",
    "\\rule{9e9pt}{1em}",
    "\\hspace{1em}",
    "\\kern",
    "\\big",
    "\\Bigg",
    "\\limits",
    "\\nolimits",
    "\\displaystyle",
    "\\scriptstyle",
    "\\newcommand",
    "#1",
    "\\bogus",
    "é",
    "日",
    "😀",
    "\u{0}",
    "\u{202e}",
    "\u{1d7ff}",
    "\\\u{e9}",
    "%",
    "$",
    "\n",
    "\t",
    "\\begin",
    "\\end",
    "\\left",
    "\\right",
    "\\sqrt[3]",
    "\\not",
    "\\stackrel",
    "\\substack",
];

fn random_formula(rng: &mut Lcg, tokens: usize) -> String {
    (0..tokens).map(|_| VOCAB[rng.below(VOCAB.len())]).collect()
}

#[test]
fn random_token_soup_never_panics() {
    let iters: u64 = std::env::var("TYPESET_FUZZ_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(3000);
    on_small_stack(move || {
        let mut ts = Typesetter::new();
        let mut rng = Lcg(0x00D1_AFF7);
        for i in 0..iters {
            let n = 1 + rng.below(if i % 10 == 0 { 400 } else { 40 });
            check_math(&mut ts, &random_formula(&mut rng, n));
        }
    });
}

/// A random formula from a small grammar; almost all of them are valid.
fn grammar(rng: &mut Lcg, depth: usize) -> String {
    let atoms = ["a", "x", "12", "\\alpha", "\\pi", "+", "=", "\\infty", "(", ")", "\\sum", "\\int"];
    if depth == 0 {
        return atoms[rng.below(atoms.len())].to_owned();
    }
    let d = depth - 1;
    match rng.below(14) {
        0 => format!("\\frac{{{}}}{{{}}}", grammar(rng, d), grammar(rng, d)),
        1 => format!("\\sqrt{{{}}}", grammar(rng, d)),
        2 => format!("x^{{{}}}", grammar(rng, d)),
        3 => format!("x_{{{}}}^{{{}}}", grammar(rng, d), grammar(rng, d)),
        4 => format!("\\left( {} \\right)", grammar(rng, d)),
        5 => format!(
            "\\begin{{matrix}} {} & {} \\\\ {} & {} \\end{{matrix}}",
            grammar(rng, d),
            grammar(rng, d),
            grammar(rng, d),
            grammar(rng, d)
        ),
        6 => format!("\\color{{red}}{{{}}}", grammar(rng, d)),
        7 => format!("\\mathbf{{{}}}", grammar(rng, d)),
        8 => format!("\\overline{{{}}}", grammar(rng, d)),
        9 => format!("{{{}}}", grammar(rng, d)),
        10 => format!("\\binom{{{}}}{{{}}}", grammar(rng, d), grammar(rng, d)),
        _ => format!("{} {}", grammar(rng, d), grammar(rng, d)),
    }
}

#[test]
fn grammar_formulas_lay_out() {
    on_small_stack(|| {
        let mut ts = Typesetter::new();
        let mut rng = Lcg(5);
        let mut ok = 0;
        for _ in 0..600 {
            let d = 1 + rng.below(6);
            let src = grammar(&mut rng, d);
            if src.len() <= MAX_LATEX_BYTES && ts.math(&MathSpec::new(&src)).is_ok() {
                ok += 1;
            }
            check_math(&mut ts, &src);
        }
        assert!(ok > 450, "only {ok} of 600 grammar formulas laid out");
    });
}

#[test]
fn balanced_random_formulas_never_panic() {
    on_small_stack(|| {
        let mut ts = Typesetter::new();
        let mut rng = Lcg(77);
        for _ in 0..800 {
            // Wrap pieces in braces so most are balanced.
            let parts: Vec<String> = (0..1 + rng.below(8))
                .map(|_| {
                    let n = 1 + rng.below(6);
                    format!("{{{}}}", random_formula(&mut rng, n))
                })
                .collect();
            let src = parts.join("^");
            check_math(&mut ts, &src);
        }
    });
}

#[test]
fn pathological_shapes_stay_within_stack_and_time() {
    on_small_stack(|| {
        let mut ts = Typesetter::new();
        // `deep(k)` repeats a construct up to the depth limit (it must lay out or fail cleanly);
        // `over` repeats it far past the limit (it must be refused, not overflow).
        let depth = MAX_NESTING;
        let over = MAX_TOKENS - 2;
        let rep =
            |open: &str, core: &str, close: &str, n: usize| format!("{}{core}{}", open.repeat(n), close.repeat(n));
        let cases: Vec<(&str, String, bool)> = vec![
            ("sqrt chain", rep("\\sqrt", "{x}", "", depth - 1), false),
            ("sqrt chain over", rep("\\sqrt", "{x}", "", over), true),
            ("frac chain over", rep("\\frac", "{a}{b}", "", over / 2), true),
            ("frac nested", rep("\\frac{1}{", "x", "}", depth / 2 - 1), false),
            ("frac nested over", rep("\\frac{1}{", "x", "}", depth), true),
            ("scripts nested", rep("x^{", "x", "}", depth - 1), false),
            ("subscripts nested", rep("x_{", "x", "}", depth - 1), false),
            ("scripts over", rep("x^{", "x", "}", depth * 4), true),
            ("left right", rep("\\left(", "x", "\\right)", depth - 1), false),
            ("left right over", rep("\\left(", "x", "\\right)", depth * 4), true),
            ("matrices", rep("\\begin{matrix}", "x", "\\end{matrix}", depth - 2), false),
            ("matrices over", rep("\\begin{matrix}", "x", "\\end{matrix}", depth * 4), true),
            ("braces", rep("{", "x", "}", depth), false),
            ("braces over", rep("{", "x", "}", depth * 100), true),
            ("prime chain", "x".to_owned() + &"'".repeat(MAX_LATEX_BYTES - 10), false),
            ("script chain", "x".to_owned() + &"^a".repeat(over / 2), false),
            ("script chain sqrt", "x".to_owned() + &"^\\sqrt".repeat(depth / 2 - 1) + "{x}", false),
            ("big matrix", format!("\\begin{{matrix}}{}\\end{{matrix}}", "a & ".repeat(over / 2)), false),
            ("overline chain", "\\overline".repeat(over) + "{x}", true),
            ("huge rule", "\\rule{99999999999pt}{99999999999pt}".to_owned(), false),
            ("huge hspace", "\\hspace{99999999999em} x".to_owned(), false),
            ("many symbols", "\\alpha ".repeat(MAX_TOKENS - 1), false),
            ("symbol run", "\\alpha".repeat(depth * 2), true),
            ("only braces", "{}".repeat(MAX_TOKENS / 2 - 1), false),
            ("unicode", "日本語😀é".repeat(300), false),
            ("control chars", "\u{0}\u{1}\u{7f}\u{ffff}".repeat(200), false),
            ("newlines", "a\n".repeat(2000), false),
            ("lone backslashes", "\\".repeat(2000), false),
            ("percent comments", "% hi\n".repeat(500), false),
        ];
        for (name, src, must_refuse) in cases {
            eprint!("{name:20} ");
            let started = drafftink_profile::now_ns();
            check_math(&mut ts, &src);
            eprintln!("{:>6} us", (drafftink_profile::now_ns() - started) / 1000);
            if must_refuse {
                let e = ts.math(&MathSpec::new(&src)).expect_err(name);
                assert!(matches!(e.kind, MathErrorKind::TooComplex | MathErrorKind::TooLong), "{name}: {e:?}");
            }
        }
    });
}

#[test]
fn nan_infinite_and_extreme_sizes_are_clamped() {
    let mut ts = Typesetter::new();
    for size in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0, -5.0, 1e300, 1e-300] {
        let l = ts.math(&MathSpec::new("\\frac{a}{b}").with_size(size)).expect("clamped, not an error");
        assert!(l.width.is_finite() && l.width < 1e6 && l.width > 0.0, "{size}: {}", l.width);
    }
}

fn arb_text() -> impl Strategy<Value = String> {
    prop_oneof![
        ".{0,200}",
        "[a-zA-Z0-9 \\n\\t]{0,300}",
        "(\\PC|\\n){0,120}",
        proptest::collection::vec(
            prop_oneof![
                Just("日本語"),
                Just("😀"),
                Just("é"),
                Just("\u{200d}"),
                Just("\u{202e}"),
                Just("\r\n"),
                Just("\u{0}")
            ],
            0..60
        )
        .prop_map(|v| v.concat()),
    ]
}

/// Calls every geometry method of the caret API. None may panic, and the answers must be sane.
fn exercise_caret_apis(l: &TextLayout, len: usize) {
    use kurbo::Point;
    let w = l.size.width;
    let h = l.size.height;
    let points = [
        Point::new(0.0, 0.0),
        Point::new(w / 2.0, h / 2.0),
        Point::new(w, h),
        Point::new(w * 0.9, 1.0),
        Point::new(-5.0, h * 0.3),
        Point::new(w + 100.0, h + 100.0),
        Point::new(f64::NAN, f64::INFINITY),
        Point::new(f64::NEG_INFINITY, 1e300),
    ];
    let indices = [0, 1, 2, 3, len / 2, len.saturating_sub(1), len, len + 1, usize::MAX];
    let motions = [
        Motion::Left,
        Motion::Right,
        Motion::WordLeft,
        Motion::WordRight,
        Motion::Up,
        Motion::Down,
        Motion::LineStart,
        Motion::LineEnd,
        Motion::ParagraphStart,
        Motion::ParagraphEnd,
        Motion::TextStart,
        Motion::TextEnd,
    ];
    let mut sels = vec![l.select_all()];
    for &i in &indices {
        for &j in &indices {
            sels.push(l.selection_from_range(i, j));
        }
        sels.push(l.selection_at_index(i, Affinity::Upstream));
        sels.push(l.selection_at_index(i, Affinity::Downstream));
        let _ = l.word_range_at(i);
        let _ = l.line_of_index(i);
        let c = l.selection_at_index(i, Affinity::Downstream).focus();
        let r = l.caret_rect(c, 1.5);
        assert!(r.x0.is_finite() && r.y0.is_finite() && r.x1.is_finite() && r.y1.is_finite(), "{r:?}");
        let _ = l.caret_point(c);
    }
    for &p in &points {
        let c = l.caret_at_point(p);
        assert!(c.index <= len, "{c:?} > {len}");
        sels.push(l.selection_at_point(p));
        sels.push(l.word_at_point(p));
        sels.push(l.line_at_point(p));
        sels.push(l.paragraph_at_point(p));
        let _ = l.hit_test(p, 2.0);
    }
    for sel in sels.clone() {
        for m in motions {
            for extend in [false, true] {
                let moved = l.move_selection(&sel, m, extend);
                assert!(moved.range().end <= len);
            }
        }
        for &p in &points {
            assert!(l.extend_to_point(&sel, p).range().end <= len);
            assert!(l.shift_click(&sel, p).range().end <= len);
        }
        for r in l.selection_rects(&sel) {
            assert!(r.x0.is_finite() && r.x1.is_finite() && r.y0.is_finite() && r.y1.is_finite(), "{r:?}");
        }
        assert!(sel.range().end <= len && sel.range().start <= sel.range().end);
    }
}

/// Text with dollar signs, formulas good and broken, escapes, line breaks and wide characters.
fn arb_math_text() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            Just("word "),
            Just("$x$"),
            Just("$\\frac{a}{b}$"),
            Just("$E = mc^2$"),
            Just("$\\bogus$"),
            Just("$x^{$"),
            Just("$\\sqrt\\sqrt\\sqrt{x}$"),
            Just("\\$"),
            Just("\\"),
            Just("$"),
            Just("$$"),
            Just(" $ "),
            Just("\n"),
            Just("é"),
            Just("日本語"),
            Just("😀"),
            Just("\u{200d}"),
            Just("5"),
        ],
        0..40,
    )
    .prop_map(|v| v.concat())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn inline_math_text_never_panics_and_keeps_source_indexes(
        text in arb_math_text(),
        wrap in prop_oneof![Just(None), Just(Some(1.0f32)), (0.0f32..300.0).prop_map(Some)],
        reveal in prop_oneof![Just(None), (0usize..200).prop_map(Some)],
        spans in proptest::collection::vec((0usize..200, 0usize..200), 0..8),
    ) {
        let mut spec = TextSpec::styled(text.clone(), TextStyle::with_size(16.0)).with_inline_math(InlineMath::Auto);
        if let Some(w) = wrap { spec.wrap = Wrap::Width(w); }
        spec.reveal = reveal;
        for (a, b) in spans {
            spec.spans.push(Span { range: a..b, style: SpanStyle::color(Rgba8::rgb(9, 9, 9)) });
        }
        let mut ts = Typesetter::new();
        let l = ts.text(&spec);
        prop_assert!(l.size.width.is_finite() && l.size.height.is_finite());
        prop_assert!(l.text_len == text.len());
        prop_assert!(!l.lines.is_empty());
        for line in &l.lines {
            prop_assert!(line.text_range.start <= line.text_range.end && line.text_range.end <= text.len());
        }
        // Formulas are on char boundaries of the source, in order, and inside their line.
        let mut at = 0;
        for f in &l.formulas {
            prop_assert!(text.is_char_boundary(f.src.start) && text.is_char_boundary(f.src.end));
            prop_assert!(f.src.start >= at);
            at = f.src.end;
            prop_assert!(f.rect.x0.is_finite() && f.rect.y1.is_finite());
        }
        let _ = l.outline();
        exercise_caret_apis(&l, text.len());
    }

    #[test]
    fn arbitrary_latex_never_panics(src in arb_text()) {
        let mut ts = Typesetter::new();
        check_math(&mut ts, &src);
    }

    #[test]
    fn arbitrary_text_with_arbitrary_spans_never_panics(
        text in arb_text(),
        size in prop_oneof![Just(f32::NAN), Just(0.0f32), Just(-3.0f32), Just(1e30f32), 0.5f32..400.0],
        wrap in prop_oneof![Just(None), Just(Some(f32::NAN)), Just(Some(0.0f32)), Just(Some(-5.0f32)), Just(Some(f32::INFINITY)), (0.0f32..500.0).prop_map(Some)],
        spans in proptest::collection::vec((0usize..400, 0usize..400, 0u8..8), 0..20),
    ) {
        let mut spec = TextSpec::styled(text.clone(), TextStyle { size, ..TextStyle::default() });
        if let Some(w) = wrap { spec.wrap = Wrap::Width(w); }
        for (a, b, kind) in spans {
            let mut st = SpanStyle::default();
            match kind {
                0 => st.color = Some(Rgba8::rgb(1, 2, 3)),
                1 => st.size = Some(f32::NAN),
                2 => st.size = Some(1e9),
                3 => st.weight = Some(FontWeight::Heavy),
                4 => st.italic = Some(true),
                5 => st.underline = Some(true),
                6 => st.family = Some(FontFamily::Custom("No Such Font".into())),
                _ => st.family = Some(FontFamily::NotoSans),
            }
            spec.spans.push(Span { range: a..b, style: st });
        }
        let mut ts = Typesetter::new();
        let l = ts.text(&spec);
        prop_assert!(l.size.width.is_finite() && l.size.height.is_finite());
        prop_assert!(l.size.width >= 0.0 && l.size.height >= 0.0);
        prop_assert!(!l.lines.is_empty());
        prop_assert!(l.text_len == text.len());
        for run in &l.runs {
            for g in &run.glyphs { prop_assert!(g.x.is_finite() && g.y.is_finite()); }
        }
        let _ = l.outline();
        // Every caret API, on whatever came out, with hostile indices and points.
        exercise_caret_apis(&l, text.len());
    }
}
