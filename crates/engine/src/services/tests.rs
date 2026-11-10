//! The measures: the fake is exact, the real one measures with fonts and caches formulas.

use drafftink_scene::{FontFamily, FontWeight, TextStyle};

use crate::Session;
use crate::services::Services;
use crate::testing;

fn style(size: f64) -> TextStyle {
    TextStyle { size, ..TextStyle::default() }
}

#[test]
fn the_fake_measures_exactly_and_has_no_formula_engine() {
    let s = Services::fake();
    // Monospace: half the font size per character.
    assert_eq!(s.edit().line_width("abcd", &style(20.0)), 40.0);
    let (w, h) = s.io().text_size("ab\ncd", FontFamily::GelPen, FontWeight::Regular, 10.0, 1.25);
    assert!(w > 0.0 && h > 0.0);
    let svg = s.math_svg().math_svg("x^2", 20.0).unwrap();
    assert!(svg.starts_with("<rect"), "{svg}");
    let e = s.math_layout("x^2", 20.0).unwrap_err();
    assert!(e.message.contains("Services::new"), "{}", e.message);
    assert!(s.stats().is_none());
}

#[test]
fn the_real_measure_sizes_text_with_fonts() {
    let s = Services::new();
    let one = s.edit().line_width("a", &style(20.0));
    let three = s.edit().line_width("aaa", &style(20.0));
    assert!(one > 0.0, "{one}");
    assert!(three > 2.0 * one, "three letters ({three}) are wider than two of one ({})", 2.0 * one);
    assert!(s.edit().line_width("aaa", &style(40.0)) > 1.9 * three, "twice the size, about twice the width");
    assert_eq!(s.edit().line_width("", &style(20.0)), 0.0);

    let (w1, h1) = s.io().text_size("hello", FontFamily::GelPen, FontWeight::Regular, 20.0, 1.25);
    let (w2, h2) = s.io().text_size("hello\nhello", FontFamily::GelPen, FontWeight::Regular, 20.0, 1.25);
    assert_eq!(w1, w2, "a second line does not widen the text");
    assert!((h2 / h1 - 2.0).abs() < 0.05, "two lines are twice as tall: {h1} {h2}");
    // The edit crate's line height is size times the factor, and the layout agrees with it.
    let factor = TextStyle::default().line_height;
    assert!((h1 - 20.0 * factor).abs() < 0.01, "{h1}");
}

#[test]
fn hostile_numbers_give_finite_sizes() {
    let s = Services::new();
    for size in [f64::NAN, f64::INFINITY, -5.0, 0.0, 1e300] {
        let (w, h) = s.io().text_size("abc", FontFamily::NotoSans, FontWeight::Heavy, size, f64::NAN);
        assert!(w.is_finite() && h.is_finite() && w >= 0.0 && h >= 0.0, "size {size}: {w} {h}");
        assert!(s.edit().line_width("abc", &style(size)).is_finite());
        if let Some(b) = s.io().math_size("x", size) {
            assert!(b.width.is_finite() && b.height.is_finite() && b.depth >= 0.0);
        }
    }
    let long = "x".repeat(10_000);
    assert!(s.edit().line_width(&long, &style(20.0)).is_finite());
}

#[test]
fn a_formula_is_laid_out_once_and_then_served_from_the_cache() {
    let s = Services::new();
    let before = s.stats().unwrap();
    let a = s.math_layout("\\frac{a}{b}", 24.0).unwrap();
    let b = s.math_layout("\\frac{a}{b}", 24.0).unwrap();
    assert!(std::sync::Arc::ptr_eq(&a, &b), "the same layout, not a second one");
    // The size and the SVG use the same cache entry.
    let m = s.io().math_size("\\frac{a}{b}", 24.0).unwrap();
    assert_eq!(m.width, a.width);
    assert!(s.math_svg().math_svg("\\frac{a}{b}", 24.0).is_some());
    let after = s.stats().unwrap();
    assert_eq!(after.math_misses - before.math_misses, 1, "one layout for four requests");
    assert_eq!(after.math_hits - before.math_hits, 3);
    let other = s.math_layout("\\frac{a}{b}", 48.0).unwrap();
    assert!(other.width > 1.9 * a.width, "a larger size is a different layout");
}

#[test]
fn formula_boxes_and_svg_come_from_the_layout() {
    let s = Services::new();
    let l = s.math_layout("x^2", 20.0).unwrap();
    let b = s.io().math_size("x^2", 20.0).unwrap();
    assert_eq!((b.width, b.height), (l.width, l.height));
    assert!(b.depth >= 0.0);
    let svg = s.math_svg().math_svg("x^2", 20.0).unwrap();
    assert!(svg.contains("<path d=\"M") && svg.contains("fill=\"currentColor\""), "{svg}");
    // A colour the formula sets itself is kept.
    let red = s.math_svg().math_svg("\\color{red}{x}", 20.0).unwrap();
    assert!(red.contains("fill=\"#"), "{red}");
}

#[test]
fn a_formula_that_does_not_parse_gives_none_and_a_positioned_error() {
    let s = Services::new();
    assert!(s.io().math_size("\\notacommand", 20.0).is_none());
    assert!(s.math_svg().math_svg("\\notacommand", 20.0).is_none());
    let e = s.math_layout("\\notacommand", 20.0).unwrap_err();
    assert!(!e.message.is_empty());
    // The failure is cached too: asking again does not parse again.
    let before = s.stats().unwrap();
    assert!(s.math_layout("\\notacommand", 20.0).is_err());
    assert_eq!(s.stats().unwrap().math_misses, before.math_misses);
}

#[test]
fn a_session_owns_its_services_and_tests_can_inject_the_fake() {
    let (s, _) = testing::session();
    assert_eq!(s.services.edit().line_width("abcd", &style(20.0)), 40.0, "test sessions measure with the fake");
    let fakes = crate::ports::fake::Fakes::new();
    let s = Session::new(fakes.headless_ports());
    assert!(s.services.stats().is_some(), "a plain session uses the real engine");
    let s = Session::with_services(fakes.headless_ports(), Services::fake());
    assert!(s.services.stats().is_none());
}

#[test]
fn sessions_can_share_one_typesetter() {
    let a = crate::services::TypesetMeasure::new();
    let shared = a.handle();
    let one = Services::with(a);
    let two = Services::sharing(shared);
    one.math_layout("y", 20.0).unwrap();
    let before = two.stats().unwrap();
    two.math_layout("y", 20.0).unwrap();
    assert_eq!(two.stats().unwrap().math_hits, before.math_hits + 1, "the second owner hits the first one's cache");
}
