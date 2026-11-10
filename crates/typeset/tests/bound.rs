//! Bound text in containers (Excalidraw sizing, research/parts/02c §3).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use drafftink_typeset::*;

fn bound(ts: &mut Typesetter, text: &str, kind: ContainerKind, w: f64, h: f64, valign: VerticalAlign) -> BoundText {
    ts.bound_text(&plain(text, 20.0), Container::new(kind, w, h), valign)
}

#[test]
fn rectangle_wraps_to_width_minus_padding_and_centres_vertically() {
    let mut ts = Typesetter::new();
    let b = bound(&mut ts, "Hello", ContainerKind::Rectangle, 200.0, 100.0, VerticalAlign::Middle);
    assert_eq!(b.wrap_width, 190.0);
    assert_eq!(b.inner, kurbo::Rect::new(5.0, 5.0, 195.0, 95.0));
    let h = b.layout.size.height;
    assert!((b.origin.x - 5.0).abs() < 1e-9);
    assert!((b.origin.y - (5.0 + (90.0 - h) / 2.0)).abs() < 1e-9);
    assert!(!b.grew(kurbo::Size::new(200.0, 100.0)));
    assert_eq!(b.container_size, kurbo::Size::new(200.0, 100.0));
}

#[test]
fn vertical_alignment_top_middle_bottom() {
    let mut ts = Typesetter::new();
    let top = bound(&mut ts, "x", ContainerKind::Rectangle, 100.0, 100.0, VerticalAlign::Top);
    let mid = bound(&mut ts, "x", ContainerKind::Rectangle, 100.0, 100.0, VerticalAlign::Middle);
    let bot = bound(&mut ts, "x", ContainerKind::Rectangle, 100.0, 100.0, VerticalAlign::Bottom);
    let h = top.layout.size.height;
    assert_eq!(top.origin.y, 5.0);
    assert!((mid.origin.y - (5.0 + (90.0 - h) / 2.0)).abs() < 1e-9);
    assert!((bot.origin.y - (95.0 - h)).abs() < 1e-9);
}

#[test]
fn text_taller_than_the_container_grows_it_downward_and_never_shrinks() {
    let mut ts = Typesetter::new();
    let text = "one two three four five six seven eight nine ten eleven twelve";
    let b = bound(&mut ts, text, ContainerKind::Rectangle, 100.0, 40.0, VerticalAlign::Middle);
    let h = b.layout.size.height;
    assert!(b.layout.lines.len() > 3);
    assert_eq!(b.container_size.height, (h.ceil() + 10.0), "rectangle: ceil(text) + 2 * padding");
    assert_eq!(b.container_size.width, 100.0, "width only grows for unbreakable overflow");
    assert!(b.grew(kurbo::Size::new(100.0, 40.0)));
    // Now the text fits exactly: origin is the inner top (no negative offset).
    assert!((b.origin.y - 5.0).abs() < 0.6, "{:?}", b.origin);
    assert_eq!(b.min_container_height(ContainerKind::Rectangle), b.container_size.height);
    // A tall container is left alone.
    let big = bound(&mut ts, "hi", ContainerKind::Rectangle, 100.0, 500.0, VerticalAlign::Top);
    assert_eq!(big.container_size.height, 500.0);
}

#[test]
fn a_character_wider_than_the_box_widens_the_container() {
    let mut ts = Typesetter::new();
    let b = bound(&mut ts, "WW", ContainerKind::Rectangle, 12.0, 100.0, VerticalAlign::Top);
    assert!(b.container_size.width > 12.0, "{:?}", b.container_size);
    assert!(b.layout.size.width <= b.wrap_width + 0.01);
    assert!(b.container_size.width >= b.layout.size.width + 10.0 - 0.01);
}

#[test]
fn ellipse_and_diamond_use_the_inscribed_rectangle() {
    let sqrt2 = std::f64::consts::SQRT_2;
    let e = Container::new(ContainerKind::Ellipse, 200.0, 100.0);
    assert_eq!(e.max_text_width(20.0), (100.0 * sqrt2).round() - 10.0);
    assert_eq!(e.max_text_height(0.0), (50.0 * sqrt2).round() - 10.0);
    let o = e.inner_origin();
    assert!((o.x - (5.0 + 100.0 * (1.0 - sqrt2 / 2.0))).abs() < 1e-9);
    assert!((o.y - (5.0 + 50.0 * (1.0 - sqrt2 / 2.0))).abs() < 1e-9);
    let d = Container::new(ContainerKind::Diamond, 200.0, 100.0);
    assert_eq!(d.max_text_width(20.0), 90.0);
    assert_eq!(d.max_text_height(0.0), 40.0);
    assert_eq!(d.inner_origin(), kurbo::Point::new(55.0, 30.0));
}

#[test]
fn container_dimension_for_text_inverts_the_max_dimensions() {
    for kind in [ContainerKind::Rectangle, ContainerKind::Ellipse, ContainerKind::Diamond] {
        for d in 1..=600 {
            let d = f64::from(d) + 0.3;
            let dim = Container::dimension_for_text(kind, d);
            let c = Container::new(kind, dim, dim);
            assert!(
                c.max_text_width(20.0) >= d.ceil() - 1.0,
                "{kind:?} d={d}: dim {dim} gives {}",
                c.max_text_width(20.0)
            );
            assert!(c.max_text_height(0.0) >= d.ceil() - 1.0);
            // ...and it is not wasteful: at most a few pixels more than needed.
            assert!(c.max_text_width(20.0) <= d.ceil() + 1.0, "{kind:?} d={d}");
        }
    }
    assert_eq!(Container::dimension_for_text(ContainerKind::Arrow, 30.0), 30.0 + 80.0);
}

#[test]
fn arrow_labels_wrap_at_seventy_percent_or_eleven_em() {
    let a = Container::new(ContainerKind::Arrow, 1000.0, 10.0);
    assert_eq!(a.max_text_width(20.0), 700.0);
    let short = Container::new(ContainerKind::Arrow, 100.0, 10.0);
    assert_eq!(short.max_text_width(20.0), 220.0);
    assert_eq!(short.max_text_height(33.0), 33.0, "short arrow boxes never constrain the label");
    assert_eq!(Container::new(ContainerKind::Arrow, 100.0, 400.0).max_text_height(33.0), 400.0);
    let mut ts = Typesetter::new();
    let b = bound(&mut ts, "a label", ContainerKind::Arrow, 100.0, 10.0, VerticalAlign::Middle);
    assert_eq!(b.container_size, kurbo::Size::new(100.0, 10.0), "arrows never grow");
}

#[test]
fn lines_align_inside_the_inner_box() {
    let mut ts = Typesetter::new();
    let spec = plain("aa bb cc\nx", 20.0).aligned(Align::Center);
    let b = ts.bound_text(&spec, Container::new(ContainerKind::Rectangle, 300.0, 100.0), VerticalAlign::Middle);
    let line = &b.layout.lines[1];
    assert!((f64::from(line.x) - (290.0 - f64::from(line.width)) / 2.0).abs() < 0.6);
    assert_eq!(b.layout.box_size.width, 290.0);
}

#[test]
fn bound_text_uses_the_cache_and_its_wrap_ignores_the_spec_wrap() {
    let mut ts = Typesetter::new();
    let mut spec = plain("some bound text that needs wrapping to fit", 20.0);
    spec.wrap = Wrap::Width(5000.0);
    let c = Container::new(ContainerKind::Rectangle, 120.0, 60.0);
    let a = ts.bound_text(&spec, c, VerticalAlign::Middle);
    let before = ts.stats();
    let b = ts.bound_text(&spec, c, VerticalAlign::Middle);
    assert!(std::sync::Arc::ptr_eq(&a.layout, &b.layout));
    assert!(ts.stats().text_hits > before.text_hits);
    assert_eq!(ts.stats().text_misses, before.text_misses);
    assert!(a.layout.size.width <= 110.01);
}

#[test]
fn empty_text_in_a_container_does_not_grow_it_and_min_size_is_one_line() {
    let mut ts = Typesetter::new();
    let b = bound(&mut ts, "", ContainerKind::Rectangle, 100.0, 60.0, VerticalAlign::Middle);
    assert_eq!(b.container_size, kurbo::Size::new(100.0, 60.0));
    let min = ts.min_container_size(&plain("", 20.0));
    assert!(min.width > 10.0 && min.width < 60.0 && min.height > 30.0 && min.height < 45.0, "{min:?}");
}

#[test]
fn degenerate_containers_do_not_panic() {
    let mut ts = Typesetter::new();
    for (w, h) in [(0.0, 0.0), (-10.0, -10.0), (1.0, 1.0), (f64::NAN, f64::NAN), (1e12, 1e12), (f64::INFINITY, 5.0)] {
        for kind in [ContainerKind::Rectangle, ContainerKind::Ellipse, ContainerKind::Diamond, ContainerKind::Arrow] {
            let b =
                ts.bound_text(&plain("text in a weird box", 20.0), Container::new(kind, w, h), VerticalAlign::Middle);
            assert!(b.layout.size.width.is_finite());
        }
    }
}
