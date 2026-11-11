//! Colour of stretches of the text being edited, kept over byte ranges.
//!
//! The scene stores colour runs in characters ([`TextRun`]); the typesetter and the caret speak bytes. The
//! editor works in bytes and converts at the two edges (opening and finishing an edit).

use std::ops::Range;

use drafftink_scene::{Color, TextRun};

/// One coloured stretch of the text, in bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorSpan {
    /// The bytes it covers.
    pub range: Range<usize>,
    /// Their colour.
    pub color: Color,
}

/// Coloured stretches, sorted, never empty, never overlapping, and equal neighbours merged. Text outside every
/// span has the colour of the element.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ColorRuns(Vec<ColorSpan>);

impl ColorRuns {
    /// The runs of a stored text. Ranges past the end of the text are cut.
    pub fn from_text_runs(text: &str, runs: &[TextRun]) -> Self {
        let mut at: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
        at.push(text.len());
        let byte = |char_index: u32| at.get(char_index as usize).copied().unwrap_or(text.len());
        let mut out = Self::default();
        for run in runs {
            if let Some(color) = run.color {
                out.paint(byte(run.start)..byte(run.end), Some(color));
            }
        }
        out
    }

    /// The runs as the scene stores them, in characters.
    pub fn to_text_runs(&self, text: &str) -> Vec<TextRun> {
        let mut out = Vec::with_capacity(self.0.len());
        let (mut chars, mut at) = (0u32, 0usize);
        let mut advance = |to: usize| {
            let step = text.get(at..to).map_or(0, |s| s.chars().count());
            chars = chars.saturating_add(u32::try_from(step).unwrap_or(u32::MAX));
            at = to;
            chars
        };
        for span in &self.0 {
            let start = advance(span.range.start);
            let end = advance(span.range.end);
            if start < end {
                out.push(TextRun { start, end, color: Some(span.color) });
            }
        }
        out
    }

    /// The stretches, in text order.
    pub fn spans(&self) -> &[ColorSpan] {
        &self.0
    }

    /// Whether no character has a colour of its own.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The colour of the character that covers byte `at`, if it has one of its own.
    pub fn color_at(&self, at: usize) -> Option<Color> {
        self.0.iter().find(|s| s.range.contains(&at)).map(|s| s.color)
    }

    /// Gives `range` the colour `color` (`None`: the element's colour).
    pub fn paint(&mut self, range: Range<usize>, color: Option<Color>) {
        if range.is_empty() {
            return;
        }
        self.cut(range.clone());
        if let Some(color) = color {
            let at = self.0.partition_point(|s| s.range.start < range.start);
            self.0.insert(at, ColorSpan { range, color });
        }
        self.merge();
    }

    /// Replaces the bytes `range` with `new_len` bytes of colour `color`. Spans after the range move with the text.
    pub fn replace(&mut self, range: Range<usize>, new_len: usize, color: Option<Color>) {
        let Range { start, end } = range.clone();
        self.cut(range);
        // After the cut every span lies before `start` or from `end` on; the second kind moves with the text.
        for span in self.0.iter_mut().filter(|s| s.range.start >= end) {
            span.range = (span.range.start + new_len - (end - start))..(span.range.end + new_len - (end - start));
        }
        if let (Some(color), true) = (color, new_len > 0) {
            let at = self.0.partition_point(|s| s.range.start < start);
            self.0.insert(at, ColorSpan { range: start..start + new_len, color });
        }
        self.merge();
    }

    /// Removes the colour from `range`, splitting spans that cross its edges.
    fn cut(&mut self, range: Range<usize>) {
        let mut out = Vec::with_capacity(self.0.len() + 1);
        for span in self.0.drain(..) {
            if span.range.end <= range.start || span.range.start >= range.end {
                out.push(span);
                continue;
            }
            if span.range.start < range.start {
                out.push(ColorSpan { range: span.range.start..range.start, color: span.color });
            }
            if span.range.end > range.end {
                out.push(ColorSpan { range: range.end..span.range.end, color: span.color });
            }
        }
        self.0 = out;
    }

    fn merge(&mut self) {
        let mut out: Vec<ColorSpan> = Vec::with_capacity(self.0.len());
        for span in self.0.drain(..) {
            match out.last_mut() {
                Some(last) if last.range.end == span.range.start && last.color == span.color => {
                    last.range.end = span.range.end;
                }
                _ => out.push(span),
            }
        }
        self.0 = out;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Color = Color::rgb(255, 0, 0);
    const BLUE: Color = Color::rgb(0, 0, 255);

    fn spans(r: &ColorRuns) -> Vec<(Range<usize>, Color)> {
        r.spans().iter().map(|s| (s.range.clone(), s.color)).collect()
    }

    #[test]
    fn paint_splits_merges_and_clears() {
        let mut r = ColorRuns::default();
        r.paint(2..6, Some(RED));
        r.paint(4..8, Some(BLUE));
        assert_eq!(spans(&r), vec![(2..4, RED), (4..8, BLUE)]);
        r.paint(4..6, Some(RED));
        assert_eq!(spans(&r), vec![(2..6, RED), (6..8, BLUE)]);
        r.paint(3..7, None);
        assert_eq!(spans(&r), vec![(2..3, RED), (7..8, BLUE)]);
    }

    #[test]
    fn replace_moves_what_follows_and_gives_the_new_bytes_a_colour() {
        let mut r = ColorRuns::default();
        r.paint(4..8, Some(RED));
        // Typing 3 bytes at 2: the red word moves right.
        r.replace(2..2, 3, None);
        assert_eq!(spans(&r), vec![(7..11, RED)]);
        // Typing inside the word with its colour keeps one span.
        r.replace(9..9, 2, Some(RED));
        assert_eq!(spans(&r), vec![(7..13, RED)]);
        // Typing inside the word without a colour splits it.
        r.replace(9..9, 1, None);
        assert_eq!(spans(&r), vec![(7..9, RED), (10..14, RED)]);
        // Deleting across the gap joins what is left.
        r.replace(9..10, 0, None);
        assert_eq!(spans(&r), vec![(7..13, RED)]);
    }

    #[test]
    fn replace_over_a_range_that_straddles_a_span_edge() {
        let mut r = ColorRuns::default();
        r.paint(2..10, Some(RED));
        r.replace(0..4, 1, None);
        assert_eq!(spans(&r), vec![(1..7, RED)]);
        r.replace(5..9, 0, None);
        assert_eq!(spans(&r), vec![(1..5, RED)]);
    }

    #[test]
    fn runs_convert_between_characters_and_bytes() {
        let text = "aé€b😀c";
        let runs = [TextRun { start: 1, end: 3, color: Some(RED) }, TextRun { start: 4, end: 5, color: Some(BLUE) }];
        let r = ColorRuns::from_text_runs(text, &runs);
        assert_eq!(spans(&r), vec![(1..6, RED), (7..11, BLUE)]);
        assert_eq!(r.to_text_runs(text), runs.to_vec());
        assert_eq!(r.color_at(2), Some(RED));
        assert_eq!(r.color_at(6), None);
    }

    #[test]
    fn runs_past_the_end_of_the_text_are_cut() {
        let r = ColorRuns::from_text_runs("ab", &[TextRun { start: 1, end: 99, color: Some(RED) }]);
        assert_eq!(spans(&r), vec![(1..2, RED)]);
    }
}
