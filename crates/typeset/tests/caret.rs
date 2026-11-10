//! Caret, selection and word geometry: round trips and motions.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use drafftink_typeset::*;
use kurbo::Point;

fn boundaries(s: &str) -> Vec<usize> {
    s.char_indices().map(|(i, _)| i).chain(std::iter::once(s.len())).collect()
}

fn centre_right_of(l: &TextLayout, c: Caret) -> Point {
    let r = l.caret_rect(c, 0.0);
    Point::new(r.x0 + 0.01, (r.y0 + r.y1) / 2.0)
}

#[test]
fn point_from_cursor_and_cursor_from_point_round_trip_on_unwrapped_text() {
    let mut ts = Typesetter::new();
    for text in ["Hello world", "multi\nline\ntext here", "tabs\tand  double  spaces", "é ü ñ", "a\n\nb", "trailing\n"]
    {
        let l = ts.text(&plain(text, 22.0));
        for i in boundaries(text) {
            let c = l.selection_at_index(i, Affinity::Downstream).focus();
            let back = l.caret_at_point(centre_right_of(&l, c));
            assert_eq!(back.index, i, "{text:?}: index {i} -> {:?} -> {back:?}", l.caret_rect(c, 0.0));
        }
    }
}

#[test]
fn round_trip_on_wrapped_text_up_to_soft_break_ambiguity() {
    let mut ts = Typesetter::new();
    let text = "The quick brown fox jumps over the lazy dog and keeps running far away";
    let l = ts.text(&plain(text, 20.0).wrapped(150.0));
    assert!(l.lines.len() >= 3);
    let mut exact = 0;
    for i in boundaries(text) {
        let c = l.selection_at_index(i, Affinity::Downstream).focus();
        let back = l.caret_at_point(centre_right_of(&l, c));
        // A caret at a soft break sits at the start of the next line (downstream). Spaces that
        // hang past the wrap edge have no room, so the point lands on the line end instead.
        let near_break = l.lines.iter().any(|line| i.abs_diff(line.text_range.end) <= 1);
        if back.index == i {
            exact += 1;
        } else {
            assert!(near_break, "index {i} came back as {back:?}");
            assert!(back.index.abs_diff(i) <= 1);
        }
    }
    assert!(exact * 10 >= boundaries(text).len() * 8, "{exact} exact");
}

#[test]
fn caret_rect_is_inside_the_line_box_and_ordered() {
    let mut ts = Typesetter::new();
    let text = "abc\ndef";
    let l = ts.text(&plain(text, 20.0));
    let mut last = (-1.0, -1.0);
    for i in boundaries(text) {
        let r = l.caret_rect(l.selection_at_index(i, Affinity::Downstream).focus(), 1.5);
        assert!((r.width() - 1.5).abs() < 1e-6);
        let line = &l.lines[l.line_of_index(i)];
        assert!(r.y0 >= f64::from(line.top) - 0.01 && r.y1 <= f64::from(line.bottom) + 0.01, "{r:?} {line:?}");
        assert!((r.y0, r.x0) > last || i == 0, "carets move forward in reading order");
        last = (r.y0, r.x0);
    }
}

#[test]
fn caret_at_the_far_corners_clamps_to_the_text() {
    let mut ts = Typesetter::new();
    let text = "first\nsecond";
    let l = ts.text(&plain(text, 20.0));
    assert_eq!(l.caret_at_point(Point::new(-50.0, -50.0)).index, 0);
    assert_eq!(l.caret_at_point(Point::new(1.0e6, 1.0e6)).index, text.len());
    assert_eq!(l.caret_at_point(Point::new(1.0e6, 5.0)).index, "first".len());
    assert_eq!(l.caret_at_point(Point::new(f64::NAN, f64::NAN)).index, 0);
}

#[test]
fn empty_text_has_a_caret_at_the_origin() {
    let mut ts = Typesetter::new();
    let l = ts.text(&plain("", 20.0));
    let r = l.caret_rect(l.selection_at_index(0, Affinity::Downstream).focus(), 1.5);
    assert_eq!((r.x0, r.y0), (0.0, 0.0));
    assert!(r.height() > 15.0);
    assert_eq!(l.caret_at_point(Point::new(30.0, 10.0)).index, 0);
    assert!(l.selection_rects(&l.select_all()).is_empty());
}

#[test]
fn stale_and_mid_character_indices_are_clamped() {
    let mut ts = Typesetter::new();
    let text = "日本語";
    let l = ts.text(&plain(text, 20.0));
    for i in [0, 1, 2, 3, 4, 100, usize::MAX] {
        let sel = l.selection_at_index(i, Affinity::Downstream);
        let idx = sel.focus().index;
        assert!(idx <= text.len() && text.is_char_boundary(idx), "{i} -> {idx}");
        let _ = l.caret_rect(sel.focus(), 1.0);
    }
    let sel = l.selection_from_range(1, 1000);
    let r = sel.range();
    assert!(text.is_char_boundary(r.start) && r.end == text.len());
}

#[test]
fn selection_rects_cover_the_selected_text() {
    let mut ts = Typesetter::new();
    let text = "Hello wide world";
    let l = ts.text(&plain(text, 22.0));
    let sel = l.selection_from_range(2, 9);
    let rects = l.selection_rects(&sel);
    assert_eq!(rects.len(), 1);
    let (a, b) = (
        l.caret_rect(l.selection_at_index(2, Affinity::Downstream).focus(), 0.0),
        l.caret_rect(l.selection_at_index(9, Affinity::Downstream).focus(), 0.0),
    );
    assert!((rects[0].x0 - a.x0).abs() < 0.5 && (rects[0].x1 - b.x0).abs() < 0.5, "{:?} {a:?} {b:?}", rects[0]);
    assert!((rects[0].y0 - f64::from(l.lines[0].top)).abs() < 0.5);
    // Selecting everything in a three-line text gives one rectangle per line, stacked.
    let multi = ts.text(&plain("one\ntwo\nthree", 20.0));
    let all = multi.selection_rects(&multi.select_all());
    assert_eq!(all.len(), 3);
    assert!(all.windows(2).all(|w| w[0].y1 <= w[1].y0 + 0.01));
    // Backwards selection gives the same rectangles.
    let fwd = multi.selection_from_range(1, 9);
    let back = multi.selection_from_range(9, 1);
    assert_eq!(multi.selection_rects(&fwd), multi.selection_rects(&back));
    assert_eq!(fwd.range(), back.range());
    assert_eq!(back.focus().index, 1);
}

#[test]
fn selection_rects_of_a_wrapped_paragraph_have_one_rect_per_visual_line() {
    let mut ts = Typesetter::new();
    let l = ts.text(&plain("alpha beta gamma delta epsilon zeta eta theta", 20.0).wrapped(110.0));
    let rects = l.selection_rects(&l.select_all());
    assert_eq!(rects.len(), l.lines.len());
    for (r, line) in rects.iter().zip(&l.lines) {
        assert!(r.x1 <= f64::from(line.x + line.width) + 8.0, "{r:?} {line:?}");
    }
}

#[test]
fn double_click_selects_a_word_and_triple_click_a_paragraph() {
    let mut ts = Typesetter::new();
    let text = "hello big world\nsecond paragraph";
    let l = ts.text(&plain(text, 22.0));
    let big = text.find("big").expect("word");
    let p = centre_right_of(&l, l.selection_at_index(big + 1, Affinity::Downstream).focus());
    let word = l.word_at_point(p).range();
    assert!(word.start == big, "{word:?}");
    assert!(word.end == big + 3 || word.end == big + 4, "the word, with or without its trailing space: {word:?}");
    assert_eq!(l.word_range_at(big + 1), word);
    let para = l.paragraph_at_point(p).range();
    assert_eq!(&text[para], "hello big world");
    let second = centre_right_of(&l, l.selection_at_index(text.len() - 2, Affinity::Downstream).focus());
    assert_eq!(&text[l.paragraph_at_point(second).range()], "second paragraph");
}

#[test]
fn right_and_left_walk_every_character_once() {
    let mut ts = Typesetter::new();
    let text = "añb😀c";
    let l = ts.text(&plain(text, 20.0));
    let mut sel = l.selection_at_index(0, Affinity::Downstream);
    let mut seen = vec![0usize];
    for _ in 0..text.chars().count() + 2 {
        sel = l.move_selection(&sel, Motion::Right, false);
        if seen.last() != Some(&sel.focus().index) {
            seen.push(sel.focus().index);
        }
    }
    assert_eq!(seen, boundaries(text));
    for expected in boundaries(text).into_iter().rev().skip(1) {
        sel = l.move_selection(&sel, Motion::Left, false);
        assert_eq!(sel.focus().index, expected);
    }
}

#[test]
fn shift_motions_extend_and_plain_motions_collapse() {
    let mut ts = Typesetter::new();
    let l = ts.text(&plain("one two three", 20.0));
    let start = l.selection_at_index(0, Affinity::Downstream);
    let ext = l.move_selection(&l.move_selection(&start, Motion::WordRight, true), Motion::WordRight, true);
    assert_eq!(ext.anchor().index, 0);
    assert!(ext.focus().index >= 7, "{:?}", ext.focus());
    assert!(!ext.is_collapsed());
    let collapsed = l.move_selection(&ext, Motion::Right, false);
    assert!(collapsed.is_collapsed());
    let end = l.move_selection(&start, Motion::TextEnd, false);
    assert_eq!(end.focus().index, "one two three".len());
    assert_eq!(l.move_selection(&end, Motion::TextStart, false).focus().index, 0);
    let all = l.move_selection(&start, Motion::TextEnd, true);
    assert_eq!(all.range(), 0.."one two three".len());
}

#[test]
fn up_and_down_keep_the_horizontal_position() {
    let mut ts = Typesetter::new();
    let text = "aaaaaaaaaa\nbb\nWWWWWWWWWW";
    let l = ts.text(&plain(text, 20.0));
    let x_of = |s: &TextSelection| l.caret_rect(s.focus(), 0.0).x0;
    let start = l.selection_at_index(8, Affinity::Downstream);
    let x0 = x_of(&start);
    let down = l.move_selection(&start, Motion::Down, false);
    assert!(down.focus().index > 10 && down.focus().index <= 13, "short middle line: {:?}", down.focus());
    let down2 = l.move_selection(&down, Motion::Down, false);
    // 'W' is 18 px wide, so the nearest caret slot is at most half of that away.
    assert!((x_of(&down2) - x0).abs() < 9.5, "the original column returns on the long line: {} vs {x0}", x_of(&down2));
    assert!(
        down2.focus().index > 14 && down2.focus().index < text.len(),
        "not clamped to a line end: {:?}",
        down2.focus()
    );
    let up = l.move_selection(&down2, Motion::Up, false);
    assert_eq!(up.focus().index, down.focus().index);
}

#[test]
fn line_and_paragraph_home_end() {
    let mut ts = Typesetter::new();
    let text = "alpha beta gamma delta epsilon\nnext";
    let l = ts.text(&plain(text, 20.0).wrapped(120.0));
    let mid = l.selection_at_index(text.find("gamma").expect("gamma") + 2, Affinity::Downstream);
    let ls = l.move_selection(&mid, Motion::LineStart, false).focus().index;
    let le = l.move_selection(&mid, Motion::LineEnd, false).focus().index;
    let line = &l.lines[l.line_of_index(mid.focus().index)];
    assert_eq!(ls, line.text_range.start);
    assert!(le <= line.text_range.end && le >= line.text_range.end - 1);
    assert_eq!(l.move_selection(&mid, Motion::ParagraphStart, false).focus().index, 0);
    assert_eq!(l.move_selection(&mid, Motion::ParagraphEnd, false).focus().index, text.find('\n').expect("newline"));
}

#[test]
fn drag_extends_from_the_press_point() {
    let mut ts = Typesetter::new();
    let l = ts.text(&plain("drag across this text", 22.0));
    let press = l.selection_at_point(Point::new(1.0, 10.0));
    let at = |i| centre_right_of(&l, l.selection_at_index(i, Affinity::Downstream).focus());
    let dragged = l.extend_to_point(&press, at(8));
    assert_eq!(dragged.range(), 0..8);
    let further = l.extend_to_point(&dragged, at(15));
    assert_eq!(further.range(), 0..15);
    let back = l.extend_to_point(&further, at(3));
    assert_eq!(back.range(), 0..3);
    let shift = l.shift_click(&press, at(10));
    assert_eq!(shift.range(), 0..10);
}

#[test]
fn alignment_moves_carets_with_the_text() {
    let mut ts = Typesetter::new();
    let l = ts.text(&plain("ab\nabcdefgh", 20.0).wrapped(300.0).aligned(Align::Right));
    let first_end = l.caret_rect(l.selection_at_index(2, Affinity::Upstream).focus(), 0.0);
    let second_end = l.caret_rect(l.selection_at_index(11, Affinity::Downstream).focus(), 0.0);
    assert!((first_end.x0 - 300.0).abs() < 1.0 && (second_end.x0 - 300.0).abs() < 1.0, "{first_end:?} {second_end:?}");
    let back = l.caret_at_point(Point::new(299.0, 5.0));
    assert!(back.index >= 1);
}

/// Regression: Parley's visual word motion looped forever on a one-pixel-wide wrap of mixed-script
/// text, and its selection geometry asserted when a combining mark followed a newline.
#[test]
fn word_motions_end_on_degenerate_wraps_and_newline_marks_do_not_panic() {
    let mut ts = Typesetter::new();
    // Mixed scripts: the paragraph may be right-to-left, so "right" can mean "earlier in the text".
    // What must hold is that every call returns and stays inside the text.
    let text = "\n\n$\n\n𞸧C\n\nᜭH\n\u{d63}:\n𞸫{\nᜁ\nୋ\nS\n\n]ᇐ\n!\u{ac4}\n\nP\n𑌯װW�/\n\nᝥ\n?\n\u{10a05}\n\nD&7\n\n=𐞹\n\">\\à\n\n僚\nJ\n\n1F";
    for wrap in [0.0, 1.0, 5.0] {
        let l = ts.text(&plain(text, 20.0).wrapped(wrap));
        for start in (0..=text.len()).filter(|&i| text.is_char_boundary(i)) {
            for motion in [Motion::WordRight, Motion::WordLeft] {
                let mut sel = l.selection_at_index(start, Affinity::Downstream);
                for _ in 0..text.len() + 2 {
                    let next = l.move_selection(&sel, motion, false);
                    assert!(next.focus().index <= text.len());
                    if next.focus().index == sel.focus().index {
                        break;
                    }
                    sel = next;
                }
            }
        }
    }
    // Left-to-right text on the same degenerate wraps still walks to both ends.
    let ltr = "ab cd\n\nef gh ij\nk";
    for wrap in [1.0, 5.0] {
        let l = ts.text(&plain(ltr, 20.0).wrapped(wrap));
        let mut sel = l.selection_at_index(0, Affinity::Downstream);
        for _ in 0..ltr.len() {
            sel = l.move_selection(&sel, Motion::WordRight, false);
        }
        assert_eq!(sel.focus().index, ltr.len(), "wrap {wrap}");
        for _ in 0..ltr.len() {
            sel = l.move_selection(&sel, Motion::WordLeft, false);
        }
        assert_eq!(sel.focus().index, 0, "wrap {wrap}");
    }
    let l = ts.text(&plain("𑼂ׯ\n\u{fb1e}a", 20.0));
    assert!(!l.selection_rects(&l.select_all()).is_empty());
}
