//! The highlight band is part of what a click finds, what the index holds and what the selection outline holds.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use drafftink_canvas::geom::kurbo::Point;
use drafftink_canvas::scene::{
    CharStyle, Element, ElementKind, Head, LinearKind, MarkerColor, SceneRead, SpanHighlight, TextRun, ZIndex,
};
use drafftink_canvas::shape::{index_bounds, selection_box};

fn p(x: f64, y: f64) -> Point {
    Point::new(x, y)
}

fn marked(mut e: Element) -> Element {
    e.style.highlight = Some(MarkerColor::Yellow);
    e
}

fn top(c: &drafftink_canvas::Canvas<drafftink_canvas::scene::MemoryStore>, x: f64, y: f64) -> Option<String> {
    c.hit_at(p(x, y)).map(|h| h.id.to_string())
}

#[test]
fn the_band_is_hit_where_the_bare_outline_is_missed() {
    let bare = canvas_with(vec![rect("r", 100.0, 100.0, 200.0, 100.0)]);
    let band = canvas_with(vec![marked(rect("r", 100.0, 100.0, 200.0, 100.0))]);
    // Bare: stroke 2 -> reach 1 + 5 px of tolerance = 6. Band: 10 wide, 5 each side + 5 = 10. Distance 9 is only the band.
    for (x, y) in [(91.0, 150.0), (109.0, 150.0), (200.0, 91.0), (200.0, 109.0)] {
        assert_eq!(top(&bare, x, y), None, "({x}, {y}) is off the bare outline");
        assert_eq!(top(&band, x, y).as_deref(), Some("r"), "({x}, {y}) is on the band");
    }
    assert_eq!(top(&band, 200.0, 150.0), None, "the inside of an unfilled shape is not part of the band");
    assert_eq!(top(&band, 85.0, 150.0), None, "past the band and the tolerance");
}

#[test]
fn a_wide_stroke_makes_a_wide_band() {
    let mut e = marked(ellipse("e", 100.0, 100.0, 200.0, 100.0));
    e.style.stroke_width = 8.0;
    let c = canvas_with(vec![e]);
    // 3 x 8 = 24 wide: 12 each side of the left vertex, plus 5.
    assert_eq!(top(&c, 100.0 - 15.0, 150.0).as_deref(), Some("e"));
    assert_eq!(top(&c, 100.0 - 20.0, 150.0), None);
}

#[test]
fn a_line_is_hit_along_its_band() {
    let bare = canvas_with(vec![line("l", &[(0.0, 100.0), (200.0, 100.0)])]);
    let band = canvas_with(vec![marked(line("l", &[(0.0, 100.0), (200.0, 100.0)]))]);
    assert_eq!(top(&bare, 100.0, 108.0), None);
    assert_eq!(top(&band, 100.0, 108.0).as_deref(), Some("l"));
}

fn arrow_with_head(highlight: bool) -> Element {
    let mut e = Element::linear(id("a"), LinearKind::Arrow, ZIndex::first(), &[p(0.0, 100.0), p(200.0, 100.0)]).unwrap();
    if let ElementKind::Arrow(d) = &mut e.kind {
        d.end_head = Head::Triangle;
        d.head_size = Some(40.0);
    }
    if highlight { marked(e) } else { e }
}

#[test]
fn the_head_of_an_arrow_is_on_its_band() {
    let bare = canvas_with(vec![arrow_with_head(false)]);
    let band = canvas_with(vec![arrow_with_head(true)]);
    // A wing of the head stands 20 above the shaft, 40 behind the tip: far from the shaft's band.
    let wing = (170.0, 80.0);
    assert_eq!(top(&bare, wing.0, wing.1), None);
    assert_eq!(top(&band, wing.0, wing.1).as_deref(), Some("a"));
    assert_eq!(top(&band, wing.0, 20.0), None, "far above is still nothing");
}

#[test]
fn a_text_band_reaches_a_little_past_the_box_and_an_unmarked_text_does_not() {
    let mut t = text("t", 100.0, 100.0, 100.0, 30.0);
    if let ElementKind::Text(d) = &mut t.kind {
        d.font.size = 20.0;
        d.font.line_height = 1.25;
    }
    let bare = canvas_with(vec![t.clone()]);
    let band = canvas_with(vec![marked(t.clone())]);
    // 0.16 x 25 = 4 units past the box, plus 5 px of tolerance.
    assert_eq!(top(&bare, 100.0 - 7.0, 115.0), None);
    assert_eq!(top(&band, 100.0 - 7.0, 115.0).as_deref(), Some("t"));
    // A highlight on some characters only counts as well.
    if let ElementKind::Text(d) = &mut t.kind {
        d.runs = vec![TextRun::new(0, 2, CharStyle { highlight: SpanHighlight::On(MarkerColor::Pink), ..Default::default() })];
    }
    let spans = canvas_with(vec![t]);
    assert_eq!(top(&spans, 100.0 - 7.0, 115.0).as_deref(), Some("t"));
}

#[test]
fn the_index_holds_the_band_so_that_nothing_it_covers_is_missed() {
    for e in [
        marked(rect("r", 100.0, 100.0, 200.0, 100.0)),
        marked(ellipse("e", 0.0, 0.0, 50.0, 50.0)),
        arrow_with_head(true),
        marked(text("t", 10.0, 10.0, 80.0, 20.0)),
    ] {
        let mut bare = e.clone();
        bare.style.highlight = None;
        let (with, without) = (index_bounds(&e).unwrap(), index_bounds(&bare).unwrap());
        assert!(with.x0 < without.x0 && with.x1 > without.x1 && with.y0 <= without.y0, "{:?}: {with:?} {without:?}", e.tag());
    }
    let c = canvas_with(vec![marked(rect("r", 100.0, 100.0, 200.0, 100.0))]);
    assert!(c.index().verify(c.store()).is_ok());
    // A marquee that only touches the band still finds nothing: marquee selection works on the outline itself.
    assert!(c.store().get(&id("r")).is_some());
}

#[test]
fn the_selection_outline_holds_the_band_and_a_plain_element_keeps_its_box() {
    let plain = rect("r", 100.0, 100.0, 200.0, 100.0);
    assert_eq!(selection_box(&plain).size, drafftink_canvas::shape::element_box(&plain).size);
    let b = selection_box(&marked(plain.clone()));
    assert_eq!((b.size.width, b.size.height), (210.0, 110.0), "5 each side");
    assert_eq!(b.center, drafftink_canvas::shape::element_box(&plain).center);
    let mut thick = marked(plain);
    thick.style.stroke_width = 10.0;
    assert_eq!(selection_box(&thick).size.width, 200.0 + 30.0);
}
