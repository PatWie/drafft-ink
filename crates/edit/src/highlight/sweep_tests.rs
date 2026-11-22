#![allow(clippy::single_range_in_vec_init)]
//! Which characters a path crosses on a real layout.

use drafftink_geom::kurbo::Point;
use drafftink_typeset::{Affinity, TextLayout, TextSpec, TextStyle, Typesetter};

use super::*;

#[test]
fn byte_ranges_become_char_ranges() {
    let text = "héllo wörld";
    assert_eq!(byte_range_to_chars(text, &(0..3)), 0..2, "é is two bytes");
    assert_eq!(byte_range_to_chars(text, &(7..text.len())), 6..11);
}

// ---- the path on a real layout -----------------------------------------------------------------------------

const SIZE: f32 = 24.0;

fn layout(text: &str, wrap: Option<f32>) -> std::sync::Arc<TextLayout> {
    let mut ts = Typesetter::new();
    let mut spec = TextSpec::styled(text, TextStyle::with_size(SIZE));
    if let Some(w) = wrap {
        spec = spec.wrapped(w);
    }
    ts.text(&spec)
}

/// The point at the left edge of the character that starts at byte `at`, in the middle of its line.
fn edge(l: &TextLayout, at: usize) -> Point {
    let r = l.caret_rect(l.selection_at_index(at, Affinity::Downstream).focus(), 0.0);
    Point::new(r.x0, (r.y0 + r.y1) / 2.0)
}

/// Feeds the straight path `from` to `to` in small steps.
fn drag(sweep: &mut CharSweep, l: &TextLayout, from: Point, to: Point) {
    for k in 0..=200 {
        sweep.probe(l, from.lerp(to, f64::from(k) / 200.0), 4.0);
    }
}

#[test]
fn a_path_along_a_line_crosses_the_characters_between_its_ends() {
    let text = "hello big world";
    let l = layout(text, None);
    let mut sweep = CharSweep::new();
    drag(&mut sweep, &l, edge(&l, 6), edge(&l, 9));
    assert_eq!(sweep.byte_ranges(text), [6..9], "`big`");
    // Going the other way crosses the same characters.
    let mut back = CharSweep::new();
    drag(&mut back, &l, edge(&l, 9), edge(&l, 6));
    assert_eq!(back.byte_ranges(text), [6..9]);
}

#[test]
fn a_path_over_wrapped_text_crosses_each_line_it_passes_up_to_the_end_of_that_line() {
    let text = "the quick brown fox jumps over the lazy dog";
    let l = layout(text, Some(140.0));
    assert!(l.lines.len() >= 3, "{} lines", l.lines.len());
    let (first, last) = (&l.lines[0], &l.lines[2]);
    let mut sweep = CharSweep::new();
    // From inside the first line down to the third and to the right, in a diagonal.
    let from = edge(&l, first.text_range.start + 4);
    let to = edge(&l, last.text_range.start + 8);
    drag(&mut sweep, &l, from, to);
    let ranges = sweep.byte_ranges(text);
    assert_eq!(ranges.len(), 3, "one range per line: {ranges:?}");
    assert_eq!(ranges[0].start, first.text_range.start + 4);
    assert!(ranges[0].end <= first.text_range.end && ranges[2].start >= last.text_range.start);
    assert_eq!(ranges[2].end, last.text_range.start + 8);
    assert!(ranges.iter().all(|r| r.start < r.end), "{ranges:?}");
}

#[test]
fn a_path_that_misses_the_text_crosses_nothing() {
    let text = "hello big world";
    let l = layout(text, None);
    let mut sweep = CharSweep::new();
    let below = Point::new(5.0, f64::from(l.lines[0].bottom) + 40.0);
    drag(&mut sweep, &l, below, below + drafftink_geom::kurbo::Vec2::new(100.0, 0.0));
    drag(&mut sweep, &l, Point::new(-80.0, 5.0), Point::new(-40.0, 5.0));
    assert!(sweep.is_empty(text));
    assert!(sweep.byte_ranges(text).is_empty());
}

#[test]
fn leaving_the_text_and_coming_back_makes_two_ranges() {
    let text = "hello big world";
    let l = layout(text, None);
    let mut sweep = CharSweep::new();
    drag(&mut sweep, &l, edge(&l, 0), edge(&l, 3));
    sweep.probe(&l, Point::new(40.0, 400.0), 4.0);
    drag(&mut sweep, &l, edge(&l, 10), edge(&l, 14));
    assert_eq!(sweep.byte_ranges(text), [0..3, 10..14]);
}

#[test]
fn a_path_that_stays_inside_one_character_crosses_nothing() {
    let text = "hello";
    let l = layout(text, None);
    let mut sweep = CharSweep::new();
    let p = edge(&l, 2) + drafftink_geom::kurbo::Vec2::new(1.0, 0.0);
    drag(&mut sweep, &l, p, p + drafftink_geom::kurbo::Vec2::new(1.0, 0.0));
    assert!(sweep.byte_ranges(text).is_empty());
}

#[test]
fn a_line_break_is_not_highlighted() {
    let text = "one\ntwo";
    let l = layout(text, None);
    let mut sweep = CharSweep::new();
    let from = edge(&l, 0);
    let to = Point::new(from.x + 400.0, from.y);
    drag(&mut sweep, &l, from, to);
    assert_eq!(sweep.byte_ranges(text), [0..3]);
}
