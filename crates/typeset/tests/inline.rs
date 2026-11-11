//! Inline math in text: layout, bounds, wrapping, carets, selections, errors, caching, hostile input.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{ink_bbox, plain};
use drafftink_typeset::*;
use kurbo::{Point, Rect};

fn auto(text: &str, size: f32) -> TextSpec {
    plain(text, size).with_inline_math(InlineMath::Auto)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.01
}

#[test]
fn text_without_formulas_lays_out_as_plain_text() {
    let mut ts = Typesetter::new();
    for t in ["plain", "it costs $5 and $10", "a $ b", "two\nlines $", ""] {
        let (a, p) = (ts.text(&auto(t, 20.0)), ts.text(&plain(t, 20.0)));
        assert_eq!(a.size, p.size, "{t:?}");
        assert_eq!(a.glyph_count(), p.glyph_count(), "{t:?}");
        assert!(a.formulas.is_empty() && a.formula_errors.is_empty(), "{t:?}");
        assert_eq!(a.text_len, t.len());
    }
}

#[test]
fn off_mode_draws_dollars_and_escapes_as_written() {
    let mut ts = Typesetter::new();
    let off = ts.text(&plain(r"Energy $E = mc^2$ \$", 20.0));
    assert!(off.formulas.is_empty());
    let auto = ts.text(&auto(r"Energy $E = mc^2$ \$", 20.0));
    assert_eq!(auto.formulas.len(), 1);
    assert!(off.glyph_count() > auto.glyph_count());
}

#[test]
fn a_formula_sits_on_the_baseline_of_its_line() {
    let mut ts = Typesetter::new();
    let l = ts.text(&auto("Energy $E = mc^2$ here", 20.0));
    let f = l.formulas.first().expect("one formula");
    assert_eq!(f.src, 7..17);
    let line = &l.lines[f.line as usize];
    assert!(close(f.origin.y, f64::from(line.baseline)), "{:?} {line:?}", f.origin);
    // The box is as wide as the formula and as tall as its extent.
    assert!(close(f.rect.width(), f.math.width));
    assert!(close(f.rect.height(), f.math.height - f.math.depth));
    // The text on both sides keeps its place: the text before ends where the formula starts.
    let before = ts.measure(&plain("Energy ", 20.0)).width;
    assert!(close(f.rect.x0, before), "{} vs {before}", f.rect.x0);
    assert!(close(l.size.width, before + f.rect.width() + ts.measure(&plain(" here", 20.0)).width));
}

#[test]
fn a_tall_formula_makes_its_line_taller_and_lines_stay_in_order() {
    let mut ts = Typesetter::new();
    let flat = ts.text(&plain("a b", 20.0));
    let tall = ts.text(&auto("a $\\frac{x}{y}$ b", 20.0));
    assert!(tall.size.height > flat.size.height + 10.0, "{:?} {:?}", tall.size, flat.size);
    let f = &tall.formulas[0];
    let line = &tall.lines[0];
    assert!(f.rect.y0 >= f64::from(line.top) - 0.01 && f.rect.y1 <= f64::from(line.bottom) + 0.01);
    // Several lines, one tall: the others keep the normal height and the layout is the sum.
    let l = ts.text(&auto("one\n$\\frac{x}{y}$\nthree", 20.0));
    assert_eq!(l.lines.len(), 3);
    let normal = l.lines[0].bottom - l.lines[0].top;
    assert!(l.lines[1].bottom - l.lines[1].top > normal + 10.0);
    assert!(close(f64::from(l.lines[2].bottom - l.lines[2].top), f64::from(normal)));
    assert!(close(l.size.height, f64::from(l.lines[2].bottom)));
}

#[test]
fn formulas_follow_the_alignment_of_their_line() {
    let mut ts = Typesetter::new();
    let text = "long line of text\n$x$";
    let left = ts.text(&auto(text, 20.0));
    let right = ts.text(&auto(text, 20.0).aligned(Align::Right));
    let centre = ts.text(&auto(text, 20.0).aligned(Align::Center));
    let (l, r, c) = (&left.formulas[0], &right.formulas[0], &centre.formulas[0]);
    assert!(close(l.rect.x0, 0.0));
    assert!(close(r.rect.x1, right.size.width), "{:?} {:?}", r.rect, right.size);
    assert!(close((c.rect.x0 + c.rect.x1) / 2.0, centre.size.width / 2.0));
}

#[test]
fn wrapping_never_splits_a_formula_and_respects_the_width() {
    let mut ts = Typesetter::new();
    let text = "alpha beta $a + b + c$ gamma delta $x^2$ epsilon zeta eta";
    let l = ts.text(&auto(text, 20.0).wrapped(170.0));
    assert!(l.lines.len() >= 3);
    assert_eq!(l.formulas.len(), 2);
    for f in &l.formulas {
        let line = &l.lines[f.line as usize];
        assert!(line.text_range.start <= f.src.start && f.src.end <= line.text_range.end, "{f:?} {line:?}");
        assert!(f.rect.x1 <= 170.01, "{:?}", f.rect);
    }
    assert!(l.size.width <= 170.01, "{:?}", l.size);
    // The lines cover the text without gaps.
    let mut at = 0;
    for line in &l.lines {
        assert_eq!(line.text_range.start, at);
        at = line.text_range.end;
    }
    assert_eq!(at, text.len());
}

#[test]
fn a_formula_wider_than_the_box_overflows_and_widens_the_bounds() {
    let mut ts = Typesetter::new();
    let l = ts.text(&auto("$a + b + c + d + e + f + g$", 20.0).wrapped(40.0));
    let f = &l.formulas[0];
    assert!(f.rect.width() > 40.0);
    assert!(l.size.width >= f.rect.width() - 0.01);
    assert!(l.box_size.width >= l.size.width);
}

#[test]
fn a_broken_formula_is_its_source_in_the_warning_colour() {
    let mut ts = Typesetter::new();
    let l = ts.text(&auto(r"ok $\bogus$ and $x$", 20.0));
    assert_eq!(l.formulas.len(), 1, "the good formula is typeset");
    let [e] = l.formula_errors.as_slice() else { panic!("one error: {:?}", l.formula_errors) };
    assert_eq!(e.src, 3..11);
    assert!(matches!(e.error.kind, MathErrorKind::UnknownCommand(_)));
    // The source, dollar signs included, is drawn.
    let red: usize = l.runs.iter().filter(|r| r.color == FORMULA_ERROR_COLOR).map(|r| r.glyphs.len()).sum();
    assert_eq!(red, 8);
    // Other text keeps its colour.
    assert!(l.runs.iter().any(|r| r.color == Rgba8::BLACK));
    // The error does not move the caret mapping: text_len is the source length.
    assert_eq!(l.text_len, r"ok $\bogus$ and $x$".len());
}

#[test]
fn hostile_formulas_become_errors_and_never_fail_the_layout() {
    let mut ts = Typesetter::new();
    let deep = format!("${}$", "\\sqrt".repeat(200));
    let braces = format!("${}x{}$", "{".repeat(100), "}".repeat(100));
    let long = format!("${}$", "x".repeat(MAX_LATEX_BYTES + 10));
    let many = "$x$ ".repeat(MAX_FORMULAS + 50);
    for t in [deep, braces, long, many, "$\\frac{$".to_owned(), "$\\left($".to_owned()] {
        let l = ts.text(&auto(&t, 16.0));
        assert!(l.size.width.is_finite() && l.size.height.is_finite());
        assert_eq!(l.text_len, t.len());
    }
    let l = ts.text(&auto(&"$x$ ".repeat(MAX_FORMULAS + 50), 16.0));
    assert_eq!(l.formulas.len(), MAX_FORMULAS);
}

#[test]
fn an_escaped_dollar_is_drawn_as_a_dollar() {
    let mut ts = Typesetter::new();
    let l = ts.text(&auto(r"\$5 \$x\$", 20.0));
    let plain_l = ts.text(&plain("$5 $x$", 20.0));
    assert!(l.formulas.is_empty());
    assert_eq!(l.size, plain_l.size);
    assert_eq!(l.text_len, r"\$5 \$x\$".len());
    // Carets are indexes of the source: after the first escape is index 2.
    let c = l.caret_rect(Caret { index: 2, affinity: Affinity::Downstream }, 1.0);
    let p = plain_l.caret_rect(Caret { index: 1, affinity: Affinity::Downstream }, 1.0);
    assert!(close(c.x0, p.x0));
}

#[test]
fn a_colour_span_colours_the_formula_it_covers() {
    let mut ts = Typesetter::new();
    let mut spec = auto("a $x$ b", 20.0);
    let blue = Rgba8::rgb(0, 0, 255);
    spec.spans.push(Span { range: 2..5, style: SpanStyle::color(blue) });
    let l = ts.text(&spec);
    assert_eq!(l.formulas[0].color, blue);
    let none = ts.text(&auto("a $x$ b", 20.0));
    assert_eq!(none.formulas[0].color, Rgba8::BLACK);
    // The size of the surrounding text is the size of the formula.
    spec.spans.push(Span { range: 2..5, style: SpanStyle { size: Some(40.0), ..SpanStyle::default() } });
    let big = ts.text(&spec);
    assert!(big.formulas[0].rect.height() > 1.8 * l.formulas[0].rect.height());
}

// ---------------------------------------------------------------------------------------------
// Caret and selection

fn caret(index: usize) -> Caret {
    Caret { index, affinity: Affinity::Downstream }
}

#[test]
fn the_caret_is_at_the_edges_of_a_formula() {
    let mut ts = Typesetter::new();
    let l = ts.text(&auto("ab $E=mc^2$ cd", 20.0));
    let f = &l.formulas[0];
    assert_eq!(f.src, 3..11);
    let left = l.caret_rect(caret(3), 1.5);
    let right = l.caret_rect(caret(11), 1.5);
    assert!(close(left.x0, f.rect.x0), "{left:?} {:?}", f.rect);
    assert!(close(right.x0, f.rect.x1), "{right:?} {:?}", f.rect);
    // Inside the formula the nearer edge.
    assert!(close(l.caret_rect(caret(4), 1.5).x0, f.rect.x0));
    assert!(close(l.caret_rect(caret(10), 1.5).x0, f.rect.x1));
    // The caret spans the line.
    assert!(close(left.y0, 0.0) && close(left.y1, f64::from(l.lines[0].bottom)));
}

#[test]
fn clicking_a_formula_picks_the_nearer_edge() {
    let mut ts = Typesetter::new();
    let l = ts.text(&auto("ab $E=mc^2$ cd", 20.0));
    let f = &l.formulas[0];
    let y = (f.rect.y0 + f.rect.y1) / 2.0;
    let quarter = Point::new(f.rect.x0 + f.rect.width() * 0.25, y);
    let three_quarters = Point::new(f.rect.x0 + f.rect.width() * 0.75, y);
    assert_eq!(l.caret_at_point(quarter).index, 3);
    assert_eq!(l.caret_at_point(three_quarters).index, 11);
    // Text around it still works.
    assert_eq!(l.caret_at_point(Point::new(0.5, y)).index, 0);
    assert_eq!(l.caret_at_point(Point::new(l.size.width + 50.0, y)).index, l.text_len);
}

#[test]
fn one_step_crosses_a_formula() {
    let mut ts = Typesetter::new();
    let l = ts.text(&auto("ab $E=mc^2$ cd", 20.0));
    let at = |i| l.selection_at_index(i, Affinity::Downstream);
    let right = l.move_selection(&at(3), Motion::Right, false);
    assert_eq!(right.focus().index, 11);
    let left = l.move_selection(&at(11), Motion::Left, false);
    assert_eq!(left.focus().index, 3);
    // Extending selects the whole formula, which is what Backspace deletes.
    let sel = l.move_selection(&at(11), Motion::Left, true);
    assert_eq!(sel.range(), 3..11);
    // Neighbours are single steps.
    assert_eq!(l.move_selection(&at(2), Motion::Right, false).focus().index, 3);
    assert_eq!(l.move_selection(&at(12), Motion::Left, false).focus().index, 11);
    // Text start and end.
    assert_eq!(l.move_selection(&at(5), Motion::TextEnd, false).focus().index, l.text_len);
}

#[test]
fn a_formula_at_the_start_and_end_of_the_text_has_two_distinct_edges() {
    let mut ts = Typesetter::new();
    for text in ["$x^2$", "$x^2$ and", "and $x^2$", "$a$ $b$", "x$a$ $b$y"] {
        let l = ts.text(&auto(text, 20.0));
        for f in &l.formulas {
            let left = l.caret_rect(caret(f.src.start), 1.0).x0;
            let right = l.caret_rect(caret(f.src.end), 1.0).x0;
            assert!(close(left, f.rect.x0) && close(right, f.rect.x1), "{text:?} {f:?} {left} {right}");
        }
    }
}

#[test]
fn selection_rectangles_cover_a_selected_formula() {
    let mut ts = Typesetter::new();
    let l = ts.text(&auto("ab $E=mc^2$ cd", 20.0));
    let f = &l.formulas[0];
    let rects = l.selection_rects(&l.selection_from_range(3, 11));
    assert_eq!(rects.len(), 1);
    assert!(close(rects[0].x0, f.rect.x0) && close(rects[0].x1, f.rect.x1), "{rects:?} {:?}", f.rect);
    // A selection from the middle of the text before it to the middle of the text after it is one rectangle.
    let wide = l.selection_rects(&l.selection_from_range(1, 13));
    assert_eq!(wide.len(), 1);
    assert!(wide[0].x0 < f.rect.x0 && wide[0].x1 > f.rect.x1);
    // Select all covers the whole line.
    let all = l.selection_rects(&l.select_all());
    assert!(close(all[0].x1 - all[0].x0, l.size.width));
    // The selection of a formula is reported in source indexes.
    assert_eq!(l.selection_from_range(3, 11).range(), 3..11);
    assert_eq!(l.select_all().range(), 0..l.text_len);
}

#[test]
fn double_clicking_a_formula_selects_it() {
    let mut ts = Typesetter::new();
    let l = ts.text(&auto("ab $E=mc^2$ cd", 20.0));
    let f = &l.formulas[0];
    let mid = Point::new((f.rect.x0 + f.rect.x1) / 2.0, (f.rect.y0 + f.rect.y1) / 2.0);
    assert_eq!(l.word_at_point(mid).range(), 3..11);
    assert_eq!(l.word_range_at(3), 3..11);
    // Dragging from before the formula to its right half selects it.
    let start = l.selection_at_point(Point::new(0.5, mid.y));
    let drag = l.extend_to_point(&start, Point::new(f.rect.x0 + f.rect.width() * 0.9, mid.y));
    assert_eq!(drag.range(), 0..11);
}

#[test]
fn formula_lookup_by_index_and_point() {
    let mut ts = Typesetter::new();
    let l = ts.text(&auto("ab $x$ cd", 20.0));
    assert!(l.formula_at_index(3).is_some() && l.formula_at_index(6).is_some() && l.formula_at_index(4).is_some());
    assert!(l.formula_at_index(2).is_none() && l.formula_at_index(7).is_none());
    let f = &l.formulas[0];
    assert!(l.formula_at_point(f.rect.center()).is_some());
    assert!(l.formula_at_point(Point::new(1.0, 1.0)).is_none());
}

#[test]
fn revealing_a_formula_shows_its_source_and_lets_the_caret_in() {
    let mut ts = Typesetter::new();
    let text = "ab $E=mc^2$ cd";
    let l = ts.text(&auto(text, 20.0));
    let at = l.reveal_index(&l.formulas[0]);
    assert_eq!(at, 4);
    let mut spec = auto(text, 20.0);
    spec.reveal = Some(at);
    let r = ts.text(&spec);
    assert!(r.formulas.is_empty() && r.formula_errors.is_empty());
    assert_eq!(r.size, ts.measure(&plain(text, 20.0)));
    // The caret can be inside now.
    let inside = r.caret_rect(caret(6), 1.0).x0;
    let outside = r.caret_rect(caret(3), 1.0).x0;
    assert!(inside > outside);
    // A caret outside the formula does not reveal it.
    for i in [0, 3, 11, 14] {
        spec.reveal = Some(i);
        assert_eq!(ts.text(&spec).formulas.len(), 1, "{i}");
    }
}

#[test]
fn lines_report_source_ranges() {
    let mut ts = Typesetter::new();
    let text = "ab $x$\n\\$c $yy$";
    let l = ts.text(&auto(text, 20.0));
    assert_eq!(l.lines.len(), 2);
    assert_eq!(l.lines[0].text_range, 0..7);
    assert_eq!(l.lines[1].text_range, 7..text.len());
    assert_eq!(l.line_of_index(5), 0);
    assert_eq!(l.line_of_index(9), 1);
}

// ---------------------------------------------------------------------------------------------
// Outline and cache

#[test]
fn the_outline_has_the_ink_of_the_formula() {
    let mut ts = Typesetter::new();
    let l = ts.text(&auto("ab $\\sum_{i}^{n} i$", 24.0));
    let f = &l.formulas[0];
    let bbox = ink_bbox(&l.outline()).expect("ink");
    let formula_ink = ink_bbox(&f.math.outline(Rgba8::BLACK)).expect("formula ink");
    let moved = Rect::new(
        formula_ink.x0 + f.origin.x,
        formula_ink.y0 + f.origin.y,
        formula_ink.x1 + f.origin.x,
        formula_ink.y1 + f.origin.y,
    );
    assert!(
        bbox.x1 >= moved.x1 - 0.01 && bbox.y0 <= moved.y0 + 0.01 && bbox.y1 >= moved.y1 - 0.01,
        "{bbox:?} {moved:?}"
    );
    // The formula's ink is inside its box (plus a little overhang).
    assert!(moved.x0 >= f.rect.x0 - 2.0 && moved.x1 <= f.rect.x1 + 2.0, "{moved:?} {:?}", f.rect);
}

#[test]
fn the_cache_key_covers_the_math() {
    let mut ts = Typesetter::new();
    let a = ts.text(&auto("a $x^2$ b", 20.0));
    let again = ts.text(&auto("a $x^2$ b", 20.0));
    assert!(std::sync::Arc::ptr_eq(&a, &again));
    let math_misses = ts.stats().math_misses;
    // The same formula in another text reuses the math layout.
    let _ = ts.text(&auto("c $x^2$ d", 20.0));
    assert_eq!(ts.stats().math_misses, math_misses);
    // Each of these is a different layout.
    let other = [
        auto("a $x^3$ b", 20.0),
        auto("a $x^2$ b", 21.0),
        plain("a $x^2$ b", 20.0),
        TextSpec { reveal: Some(4), ..auto("a $x^2$ b", 20.0) },
    ];
    for spec in other {
        let l = ts.text(&spec);
        assert!(!std::sync::Arc::ptr_eq(&a, &l), "{spec:?}");
    }
    // A different size is a different formula layout.
    assert!(ts.stats().math_misses > math_misses);
}

#[test]
fn layouts_with_formulas_are_deterministic() {
    let text = "Energy $E = mc^2$ and $\\frac{a}{b}$ here";
    let (mut a, mut b) = (Typesetter::new(), Typesetter::new());
    let (la, lb) = (a.text(&auto(text, 18.0).wrapped(150.0)), b.text(&auto(text, 18.0).wrapped(150.0)));
    assert_eq!(la.size, lb.size);
    assert_eq!(la.lines, lb.lines);
    assert_eq!(
        la.formulas.iter().map(|f| f.rect).collect::<Vec<_>>(),
        lb.formulas.iter().map(|f| f.rect).collect::<Vec<_>>()
    );
}

#[test]
fn inline_superscript_is_smaller_and_raised() {
    let mut ts = Typesetter::new();
    let l = ts.text(&auto("$x^2$", 20.0));
    let [formula] = l.formulas.as_slice() else { panic!("{:?}", l.formulas) };
    let [base, script] = formula.math.glyphs.as_slice() else { panic!("{:?}", formula.math.glyphs) };
    assert!(script.size < 0.8 * base.size, "{} vs {}", script.size, base.size);
    assert!(script.y < base.y - 5.0, "script baseline {} must be above {}", script.y, base.y);
    assert_eq!(script.font, base.font);
    // The exports draw the same: the superscript's ink is above the base's x-height.
    let ink = ink_bbox(&l.outline()).expect("ink");
    assert!(ink.y0 < formula.origin.y - 15.0, "{ink:?} {:?}", formula.origin);
}
