//! Caret, selection and word geometry on a [`TextLayout`], for the text editor.
//!
//! The editor owns the text and the selection state; this module answers geometric questions
//! about one laid-out text. All indices are byte offsets into the text the layout was built from
//! and are clamped, so a stale index from an older text never panics. All points are in layout
//! space (origin at the top-left of the layout box). The editor subtracts the shape position and
//! undoes the shape rotation before calling.
//!
//! Selections wrap Parley's, which remember the horizontal position for Up/Down and the word or
//! line a double or triple click started on.

use std::ops::Range;

use kurbo::{Point, Rect};
use parley::editing::{Cursor, Selection};

use crate::layout::TextLayout;

pub use parley::Affinity;

/// A caret position: a byte index and the side of a line break it attaches to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caret {
    /// Byte index into the text, on a `char` boundary.
    pub index: usize,
    /// Which side of a soft line break the caret sits on.
    pub affinity: Affinity,
}

/// A selection: an anchor and a focus caret. The focus is the end that moves.
#[derive(Clone, Debug)]
pub struct TextSelection(Selection);

/// How a caret moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    /// One grapheme to the left (visual order).
    Left,
    /// One grapheme to the right (visual order).
    Right,
    /// One word to the left.
    WordLeft,
    /// One word to the right.
    WordRight,
    /// One line up, keeping the horizontal position.
    Up,
    /// One line down, keeping the horizontal position.
    Down,
    /// Start of the visual line.
    LineStart,
    /// End of the visual line.
    LineEnd,
    /// Start of the paragraph (after the previous `\n`).
    ParagraphStart,
    /// End of the paragraph (before the next `\n`).
    ParagraphEnd,
    /// Start of the text.
    TextStart,
    /// End of the text.
    TextEnd,
}

fn finite(v: f64) -> f32 {
    if v.is_finite() { v.clamp(-1.0e7, 1.0e7) as f32 } else { 0.0 }
}

/// Move the focus one word to the right (`right`) or left, in screen terms.
///
/// Parley's `next_visual_word` / `previous_visual_word` loop until the cursor stops moving. With
/// text that wraps into one cluster per line they never stop (found by the hostile test: a 1 px
/// wrap width over a text of mixed scripts). The logical variants walk clusters and always end.
/// In right-to-left text the visual right is the logical previous word.
fn word_step(layout: &crate::layout::PLayout, sel: &Selection, right: bool, extend: bool) -> Selection {
    let focus = sel.focus();
    let [upstream, downstream] = focus.logical_clusters(layout);
    let rtl = downstream.or(upstream).is_some_and(|c| c.is_rtl());
    let to = if right != rtl { focus.next_logical_word(layout) } else { focus.previous_logical_word(layout) };
    if extend { sel.extend(to) } else { Selection::from(to) }
}

fn caret_of(c: Cursor) -> Caret {
    Caret { index: c.index(), affinity: c.affinity() }
}

impl TextSelection {
    /// The fixed end.
    pub fn anchor(&self) -> Caret {
        caret_of(self.0.anchor())
    }

    /// The moving end.
    pub fn focus(&self) -> Caret {
        caret_of(self.0.focus())
    }

    /// Whether anchor and focus are at the same index.
    pub fn is_collapsed(&self) -> bool {
        self.0.is_collapsed()
    }

    /// The selected byte range, start before end.
    pub fn range(&self) -> Range<usize> {
        self.0.text_range()
    }

    /// A collapsed selection at the focus.
    pub fn collapse(&self) -> Self {
        Self(self.0.collapse())
    }
}

impl TextLayout {
    /// A collapsed selection at byte `index` (clamped to the text).
    pub fn selection_at_index(&self, index: usize, affinity: Affinity) -> TextSelection {
        TextSelection(Selection::from_byte_index(&self.layout, index, affinity))
    }

    /// A selection from `anchor` to `focus`, both clamped.
    pub fn selection_from_range(&self, anchor: usize, focus: usize) -> TextSelection {
        let a = Cursor::from_byte_index(&self.layout, anchor, Affinity::Downstream);
        let f = Cursor::from_byte_index(&self.layout, focus, Affinity::Downstream);
        TextSelection(Selection::new(a, f))
    }

    /// Select everything.
    pub fn select_all(&self) -> TextSelection {
        self.selection_from_range(0, usize::MAX)
    }

    /// The caret under a point (click). Points past the end of a line give the line end; points
    /// below the last line give the text end.
    pub fn caret_at_point(&self, p: Point) -> Caret {
        caret_of(Cursor::from_point(&self.layout, finite(p.x), finite(p.y)))
    }

    /// A collapsed selection under a point.
    pub fn selection_at_point(&self, p: Point) -> TextSelection {
        TextSelection(Selection::from_point(&self.layout, finite(p.x), finite(p.y)))
    }

    /// The word under a point (double click).
    pub fn word_at_point(&self, p: Point) -> TextSelection {
        TextSelection(Selection::word_from_point(&self.layout, finite(p.x), finite(p.y)))
    }

    /// The visual line under a point.
    pub fn line_at_point(&self, p: Point) -> TextSelection {
        TextSelection(Selection::line_from_point(&self.layout, finite(p.x), finite(p.y)))
    }

    /// The paragraph (text between `\n`) under a point (triple click).
    pub fn paragraph_at_point(&self, p: Point) -> TextSelection {
        TextSelection(Selection::hard_line_from_point(&self.layout, finite(p.x), finite(p.y)))
    }

    /// The word containing byte `index`, as a byte range. Whitespace runs count as words.
    pub fn word_range_at(&self, index: usize) -> Range<usize> {
        let caret = self.caret_rect(self.selection_at_index(index, Affinity::Downstream).focus(), 0.0);
        let p = Point::new(caret.x0 + 0.25, (caret.y0 + caret.y1) / 2.0);
        self.word_at_point(p).range()
    }

    /// Drag: extend `sel` so its focus is under `p`.
    pub fn extend_to_point(&self, sel: &TextSelection, p: Point) -> TextSelection {
        TextSelection(sel.0.extend_to_point(&self.layout, finite(p.x), finite(p.y)))
    }

    /// Shift-click: move the focus under `p`, keeping the anchor.
    pub fn shift_click(&self, sel: &TextSelection, p: Point) -> TextSelection {
        TextSelection(sel.0.shift_click_extension(&self.layout, finite(p.x), finite(p.y)))
    }

    /// Move the focus (and the anchor too unless `extend`).
    pub fn move_selection(&self, sel: &TextSelection, motion: Motion, extend: bool) -> TextSelection {
        let l = &self.layout;
        let s = &sel.0;
        TextSelection(match motion {
            Motion::Left => s.previous_visual(l, extend),
            Motion::Right => s.next_visual(l, extend),
            Motion::WordLeft => word_step(l, s, false, extend),
            Motion::WordRight => word_step(l, s, true, extend),
            Motion::Up => s.previous_line(l, extend),
            Motion::Down => s.next_line(l, extend),
            Motion::LineStart => s.line_start(l, extend),
            Motion::LineEnd => s.line_end(l, extend),
            Motion::ParagraphStart => s.hard_line_start(l, extend),
            Motion::ParagraphEnd => s.hard_line_end(l, extend),
            Motion::TextStart => {
                let to = Cursor::from_byte_index(l, 0, Affinity::Downstream);
                if extend { s.extend(to) } else { Selection::from(to) }
            }
            Motion::TextEnd => {
                let to = Cursor::from_byte_index(l, usize::MAX, Affinity::Downstream);
                if extend { s.extend(to) } else { Selection::from(to) }
            }
        })
    }

    /// The rectangle of a caret, `width` wide (the old app drew 1.5). It spans the line box.
    /// For empty text it is at the origin with the height of one line.
    pub fn caret_rect(&self, caret: Caret, width: f64) -> Rect {
        if self.text_len == 0 {
            let l = self.lines.first();
            let (top, bottom) = l.map_or((0.0, 0.0), |l| (f64::from(l.top), f64::from(l.bottom)));
            return Rect::new(0.0, top, width, bottom);
        }
        let c = Cursor::from_byte_index(&self.layout, caret.index, caret.affinity);
        let b = c.geometry(&self.layout, width as f32);
        Rect::new(b.x0, b.y0, b.x1, b.y1)
    }

    /// The caret's insertion point: top of the caret line.
    pub fn caret_point(&self, caret: Caret) -> Point {
        let r = self.caret_rect(caret, 0.0);
        Point::new(r.x0, r.y0)
    }

    /// Rectangles that cover the selected text, one or more per line, in layout space. A newline
    /// inside the selection adds a little width at the end of its line, as in most editors.
    ///
    /// This is our own version of Parley's `Selection::geometry`, which has a `debug_assert!` that
    /// fails when a combining mark follows a newline (the newline then shares a cluster with the
    /// mark). The result is the same as Parley's for text without inline boxes.
    pub fn selection_rects(&self, sel: &TextSelection) -> Vec<Rect> {
        const NEWLINE_WIDTH_RATIO: f64 = 0.25;
        let range = sel.range();
        let mut out = Vec::new();
        if range.is_empty() {
            return out;
        }
        for line in self.layout.lines() {
            let lr = line.text_range();
            // A line with an empty range is the empty last line after a trailing newline.
            if lr.end <= range.start || lr.start >= range.end {
                continue;
            }
            let m = line.metrics();
            let top = f64::from(m.block_min_coord.min(m.content_block_min_coord));
            let bottom = f64::from(m.block_max_coord.max(m.content_block_max_coord));
            let x0 = f64::from(m.offset) + f64::from(m.inline_min_coord);
            // The selection runs on past this line's hard break: show the newline as a little space.
            let newline_space = if line.break_reason() == parley::BreakReason::Explicit && range.end >= lr.end {
                line.runs().last().map_or(0.0, |r| {
                    let fm = r.font_metrics();
                    (f64::from(fm.ascent) + f64::from(fm.descent)) * NEWLINE_WIDTH_RATIO
                })
            } else {
                0.0
            };
            if range.start <= lr.start && range.end >= lr.end {
                let r = Rect::new(x0, top, x0 + f64::from(m.advance) + newline_space, bottom);
                if r.width() > 0.0 {
                    out.push(r);
                }
                continue;
            }
            // Partly selected line: walk the clusters in visual order and emit one rectangle per
            // contiguous selected stretch.
            let mut cur_x = x0;
            // Where the current selected stretch began, if one is open.
            let mut stretch_start: Option<f64> = None;
            let mut selected = 0usize;
            for run in line.runs() {
                for cluster in run.visual_clusters() {
                    let advance = f64::from(cluster.advance());
                    if range.contains(&cluster.text_range().start) {
                        selected += 1;
                        stretch_start.get_or_insert(cur_x);
                    } else if let Some(start_x) = stretch_start.take()
                        && cur_x > start_x
                    {
                        out.push(Rect::new(start_x, top, cur_x, bottom));
                    }
                    cur_x += advance;
                }
            }
            let start_x = stretch_start.unwrap_or(cur_x);
            let end_x = if range.end >= lr.end && selected > 0 { cur_x + newline_space } else { cur_x };
            if end_x > start_x {
                out.push(Rect::new(start_x, top, end_x, bottom));
            }
        }
        out
    }

    /// Index of the line that contains byte `index`.
    pub fn line_of_index(&self, index: usize) -> usize {
        let i = index.min(self.text_len);
        self.lines
            .iter()
            .position(|l| i >= l.text_range.start && i < l.text_range.end)
            .unwrap_or(self.lines.len().saturating_sub(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TextSpec, Typesetter};

    fn same(a: &[Rect], b: &[Rect]) -> bool {
        a.len() == b.len()
            && a.iter().zip(b).all(|(p, q)| {
                (p.x0 - q.x0).abs() < 1e-3
                    && (p.x1 - q.x1).abs() < 1e-3
                    && (p.y0 - q.y0).abs() < 1e-3
                    && (p.y1 - q.y1).abs() < 1e-3
            })
    }

    /// Our selection rectangles equal Parley's on every range of ordinary text (the only case
    /// where Parley does not assert), including wrapped, multi-line and right-to-left text.
    #[test]
    fn selection_rects_match_parley_on_ordinary_text() {
        let mut ts = Typesetter::new();
        let cases = [
            TextSpec::new("Hello wide world"),
            TextSpec::new("one\ntwo\n\nfour"),
            TextSpec::new("trailing\n"),
            TextSpec::new("alpha beta gamma delta epsilon zeta eta theta").wrapped(110.0),
            TextSpec::new("mixed שלום עולם text\nline two").wrapped(150.0),
            TextSpec::new("é ü ñ 日本語 😀 end"),
        ];
        for spec in cases {
            let l = ts.text(&spec);
            let idx: Vec<usize> = spec.text.char_indices().map(|(i, _)| i).chain([spec.text.len()]).collect();
            for &a in &idx {
                for &b in &idx {
                    let sel = l.selection_from_range(a, b);
                    let ours = l.selection_rects(&sel);
                    let theirs: Vec<Rect> =
                        sel.0.geometry(&l.layout).into_iter().map(|(r, _)| Rect::new(r.x0, r.y0, r.x1, r.y1)).collect();
                    assert!(same(&ours, &theirs), "{:?} {a}..{b}\n ours   {ours:?}\n parley {theirs:?}", spec.text);
                }
            }
        }
    }

    /// A newline followed by a combining mark shares a cluster with it; Parley asserts on this.
    #[test]
    fn newline_followed_by_a_combining_mark_does_not_panic() {
        let mut ts = Typesetter::new();
        for text in ["𑼂ׯ\n\u{fb1e}a", "a\n\u{301}b", "\n\u{200d}\n", "a\r\n\u{301}"] {
            let l = ts.text(&TextSpec::new(text));
            let all = l.select_all();
            let rects = l.selection_rects(&all);
            assert!(!rects.is_empty(), "{text:?}");
            assert!(rects.iter().all(|r| r.x0.is_finite() && r.x1.is_finite() && r.y1 > r.y0));
        }
    }
}
