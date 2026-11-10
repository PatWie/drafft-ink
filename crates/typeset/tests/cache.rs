//! The layout cache: counters, key sensitivity, eviction, bounds.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;

use common::*;
use drafftink_profile::{Counter, WorkScope};
use drafftink_typeset::*;

fn labels(n: usize) -> Vec<TextSpec> {
    (0..n).map(|i| plain(&format!("label {i} with some words"), 16.0)).collect()
}

#[test]
fn cold_pass_misses_warm_pass_hits_and_returns_the_same_layouts() {
    let mut ts = Typesetter::new();
    let specs = labels(1000);
    let first: Vec<_> = specs.iter().map(|s| ts.text(s)).collect();
    let s1 = ts.stats();
    assert_eq!((s1.text_misses, s1.text_hits, s1.text_entries), (1000, 0, 1000));
    let second: Vec<_> = specs.iter().map(|s| ts.text(s)).collect();
    let s2 = ts.stats();
    assert_eq!((s2.text_misses, s2.text_hits), (1000, 1000));
    assert!(first.iter().zip(&second).all(|(a, b)| Arc::ptr_eq(a, b)));
    assert_eq!(s2.text_hit_rate(), Some(0.5));
}

#[test]
fn profile_counters_follow_misses_not_hits() {
    let mut ts = Typesetter::new();
    let scope = WorkScope::begin();
    let spec = plain("counted", 20.0);
    let _ = ts.text(&spec);
    let _ = ts.text(&spec);
    let _ = ts.text(&spec);
    let _ = ts.math(&MathSpec::new("x^2"));
    let _ = ts.math(&MathSpec::new("x^2"));
    assert_eq!(scope.count(Counter::Relayouts), 2, "one text layout and one math layout actually ran");
    assert_eq!(scope.count(Counter::CacheMiss), 2);
    assert_eq!(scope.count(Counter::CacheHit), 3);
}

#[test]
fn every_input_that_changes_the_layout_changes_the_key() {
    let mut ts = Typesetter::new();
    let base = plain("keyed text here", 20.0);
    let _ = ts.text(&base);
    let variants: Vec<(&str, TextSpec)> = vec![
        ("text", plain("keyed text herE", 20.0)),
        ("size", plain("keyed text here", 20.5)),
        (
            "family",
            TextSpec { style: TextStyle { family: FontFamily::GelPenSerif, ..base.style.clone() }, ..base.clone() },
        ),
        ("weight", TextSpec { style: TextStyle { weight: FontWeight::Heavy, ..base.style.clone() }, ..base.clone() }),
        ("italic", TextSpec { style: TextStyle { italic: true, ..base.style.clone() }, ..base.clone() }),
        ("colour", TextSpec { style: TextStyle { color: Rgba8::rgb(1, 0, 0), ..base.style.clone() }, ..base.clone() }),
        (
            "alpha",
            TextSpec { style: TextStyle { color: Rgba8::new(0, 0, 0, 128), ..base.style.clone() }, ..base.clone() },
        ),
        ("underline", TextSpec { style: TextStyle { underline: true, ..base.style.clone() }, ..base.clone() }),
        ("strike", TextSpec { style: TextStyle { strike: true, ..base.style.clone() }, ..base.clone() }),
        (
            "span colour",
            TextSpec {
                spans: vec![Span { range: 0..3, style: SpanStyle::color(Rgba8::rgb(5, 5, 5)) }],
                ..base.clone()
            },
        ),
        (
            "span range",
            TextSpec {
                spans: vec![Span { range: 0..4, style: SpanStyle::color(Rgba8::rgb(5, 5, 5)) }],
                ..base.clone()
            },
        ),
        ("align", base.clone().aligned(Align::Center)),
        ("wrap", base.clone().wrapped(100.0)),
        ("wrap 2", base.clone().wrapped(101.0)),
        ("line height", TextSpec { line_height: LineHeight::FontSize(1.5), ..base.clone() }),
        ("line height 2", TextSpec { line_height: LineHeight::Metrics(1.5), ..base.clone() }),
    ];
    for (name, v) in &variants {
        let before = ts.stats();
        let _ = ts.text(v);
        let after = ts.stats();
        assert_eq!(after.text_misses, before.text_misses + 1, "changing the {name} must miss");
    }
    // Every variant is now cached under its own key.
    let before = ts.stats();
    for (_, v) in &variants {
        let _ = ts.text(v);
    }
    assert_eq!(ts.stats().text_hits, before.text_hits + variants.len() as u64);
}

/// The old cache key left the alpha out, so changing only the opacity could show stale text.
#[test]
fn opacity_only_change_gives_a_layout_with_the_new_alpha() {
    let mut ts = Typesetter::new();
    let opaque =
        ts.text(&TextSpec::styled("fade", TextStyle { color: Rgba8::new(10, 20, 30, 255), ..TextStyle::default() }));
    let faint =
        ts.text(&TextSpec::styled("fade", TextStyle { color: Rgba8::new(10, 20, 30, 40), ..TextStyle::default() }));
    assert_eq!(opaque.runs[0].color.a, 255);
    assert_eq!(faint.runs[0].color.a, 40);
    assert!(!Arc::ptr_eq(&opaque, &faint));
}

#[test]
fn registering_a_font_invalidates_by_version() {
    let mut ts = Typesetter::new();
    let spec = plain("versioned", 20.0);
    let a = ts.text(&spec);
    let bytes =
        std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts/GelPen.ttf")).expect("font");
    ts.register_font(bytes).expect("registers");
    let b = ts.text(&spec);
    assert!(!Arc::ptr_eq(&a, &b));
    assert_eq!(ts.stats().text_misses, 2);
    assert_eq!(ts.stats().font_set_version, 2);
}

#[test]
fn least_recently_used_entries_go_first() {
    let mut ts = Typesetter::with_config(CacheConfig { text_entries: 8, ..CacheConfig::default() });
    let specs = labels(20);
    for s in &specs[..8] {
        let _ = ts.text(s);
    }
    // Touch the oldest so it becomes the newest.
    let _ = ts.text(&specs[0]);
    for s in &specs[8..12] {
        let _ = ts.text(s);
    }
    let st = ts.stats();
    assert_eq!(st.text_entries, 8);
    assert_eq!(st.evictions, 4);
    let before = ts.stats();
    let _ = ts.text(&specs[0]);
    assert_eq!(ts.stats().text_hits, before.text_hits + 1, "the touched entry survived");
    let _ = ts.text(&specs[1]);
    assert_eq!(ts.stats().text_misses, before.text_misses + 1, "the oldest untouched entry was evicted");
}

#[test]
fn the_byte_budget_bounds_memory() {
    let mut ts = Typesetter::with_config(CacheConfig { text_bytes: 200_000, ..CacheConfig::default() });
    let long = "lorem ipsum dolor sit amet ".repeat(40);
    for i in 0..300 {
        let _ = ts.text(&plain(&format!("{i} {long}"), 14.0));
        assert!(ts.stats().bytes <= 200_000 + 100_000, "after {i}: {}", ts.stats().bytes);
    }
    let st = ts.stats();
    assert!(st.evictions > 200, "{st:?}");
    assert!(st.text_entries < 100);
}

#[test]
fn one_entry_larger_than_the_budget_is_still_served() {
    let mut ts = Typesetter::with_config(CacheConfig { text_bytes: 10, text_entries: 1, ..CacheConfig::default() });
    let spec = plain("this layout alone is bigger than ten bytes", 20.0);
    let a = ts.text(&spec);
    let b = ts.text(&spec);
    assert!(Arc::ptr_eq(&a, &b));
    assert_eq!(ts.stats().text_entries, 1);
}

#[test]
fn math_cache_counts_and_bounds() {
    let mut ts = Typesetter::with_config(CacheConfig { math_entries: 16, ..CacheConfig::default() });
    for i in 0..40 {
        let _ = ts.math(&MathSpec::new(format!("x^{i} + {i}")));
    }
    let st = ts.stats();
    assert_eq!(st.math_misses, 40);
    assert_eq!(st.math_entries, 16);
    let spec = MathSpec::new("x^39 + 39");
    let a = ts.math(&spec).expect("cached");
    let b = ts.math(&spec).expect("cached");
    assert!(Arc::ptr_eq(&a, &b));
    // Size and fallback flag are part of the key.
    let mut other = spec.clone();
    other.text_font_fallback = false;
    let before = ts.stats();
    let _ = ts.math(&other);
    let _ = ts.math(&spec.clone().with_size(21.0));
    assert_eq!(ts.stats().math_misses, before.math_misses + 2);
}

#[test]
fn clear_caches_empties_but_keeps_counters() {
    let mut ts = Typesetter::new();
    let _ = ts.text(&plain("a", 20.0));
    let _ = ts.math(&MathSpec::new("a"));
    ts.clear_caches();
    let st = ts.stats();
    assert_eq!((st.text_entries, st.math_entries, st.bytes), (0, 0, 0));
    assert_eq!((st.text_misses, st.math_misses), (1, 1));
}
