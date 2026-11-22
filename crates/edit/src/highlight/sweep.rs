//! Which characters of a laid-out text a path crosses.
//!
//! The marker works like a text selection that follows the path: on each line the path passes, the characters
//! between where it came in and where it went out are crossed. The positions are Parley's: the caret under a
//! point ([`TextLayout::caret_at_point`]) and the line boxes of the layout, so wrapping, alignment, bold runs and
//! inline formulas are what the user sees.
//!
//! A [`CharSweep`] is fed the points of the path one after the other (in the layout's space: origin at the
//! top-left of the text box) and can be asked for the ranges at any time, so a drag shows its preview without
//! starting over.

use std::ops::Range;

use drafftink_geom::kurbo::Point;
use drafftink_typeset::TextLayout;

/// The span of one line that the path has crossed, while it stays on that line.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pass {
    line: usize,
    bytes: Range<usize>,
}

/// The characters of one text that a path has crossed so far.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CharSweep {
    passes: Vec<Pass>,
    /// The last pass belongs to the stretch of path that is still on the text.
    on_text: bool,
}

impl CharSweep {
    /// An empty sweep.
    pub fn new() -> Self {
        Self::default()
    }

    /// The path is at `p` (layout space). A point is on the text when it is within `reach` of a line box; the
    /// path then crosses the characters between its caret and the last one on that line. A point that is not on
    /// the text ends the stretch that was on it.
    pub fn probe(&mut self, layout: &TextLayout, p: Point, reach: f64) {
        let Some(line) = line_at(layout, p, reach) else {
            self.on_text = false;
            return;
        };
        let Some(info) = layout.lines.get(line) else {
            self.on_text = false;
            return;
        };
        let at = layout.caret_at_point(p).index.clamp(info.text_range.start, info.text_range.end);
        match self.passes.last_mut() {
            Some(last) if self.on_text && last.line == line => {
                last.bytes = last.bytes.start.min(at)..last.bytes.end.max(at);
            }
            _ => self.passes.push(Pass { line, bytes: at..at }),
        }
        self.on_text = true;
    }

    /// Ends the stretch that is on the text (the pointer jumped, or left it).
    pub fn lift(&mut self) {
        self.on_text = false;
    }

    /// The crossed characters as byte ranges of the text: sorted, apart, none empty, and without the line break
    /// that ends a line.
    pub fn byte_ranges(&self, text: &str) -> Vec<Range<usize>> {
        let trimmed: Vec<Range<usize>> = self
            .passes
            .iter()
            .map(|p| p.bytes.start..p.bytes.end.min(text.len()))
            .map(|r| r.start..trim_break(text, r.clone()))
            .filter(|r| r.start < r.end)
            .collect();
        merge(&trimmed)
    }

    /// True when nothing was crossed.
    pub fn is_empty(&self, text: &str) -> bool {
        self.byte_ranges(text).is_empty()
    }
}

/// `range.end` without a line break at its end.
fn trim_break(text: &str, range: Range<usize>) -> usize {
    let slice = text.get(range.clone()).unwrap_or("");
    range.start + slice.trim_end_matches(['\n', '\r']).len()
}

/// The line whose box `p` is on or within `reach` of: the nearest one, and none when `p` is further than `reach`
/// from every line or from the sides of the text.
fn line_at(layout: &TextLayout, p: Point, reach: f64) -> Option<usize> {
    if !(p.x.is_finite() && p.y.is_finite()) {
        return None;
    }
    let width = layout.size.width.max(layout.box_size.width);
    if p.x < -reach || p.x > width + reach {
        return None;
    }
    layout
        .lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let (top, bottom) = (f64::from(l.top), f64::from(l.bottom));
            (i, (top - p.y).max(p.y - bottom).max(0.0))
        })
        .filter(|(_, gap)| *gap <= reach)
        // Lines share their edges: a point on one takes the lower line, as Parley's hit test does.
        .min_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))
        .map(|(i, _)| i)
}

/// A byte range of `text` as a range of `char` indices.
pub fn byte_range_to_chars(text: &str, bytes: &Range<usize>) -> Range<usize> {
    let chars_before = |at: usize| text.char_indices().take_while(|(i, _)| *i < at).count();
    chars_before(bytes.start)..chars_before(bytes.end)
}

/// The ranges sorted, with the ones that touch or overlap joined.
fn merge(ranges: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut sorted = ranges.to_vec();
    sorted.sort_by_key(|r| (r.start, r.end));
    let mut out: Vec<Range<usize>> = Vec::with_capacity(sorted.len());
    for r in sorted {
        match out.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

#[cfg(test)]
#[path = "sweep_tests.rs"]
mod tests;
