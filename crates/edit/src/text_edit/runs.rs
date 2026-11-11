//! Style of stretches of the text being edited (colour, bold, italic), kept over byte ranges.
//!
//! The scene stores style runs in characters ([`TextRun`]); the typesetter and the caret speak bytes. The
//! editor works in bytes and converts at the two edges (opening and finishing an edit).

use std::ops::Range;

use drafftink_scene::{CharStyle, Color, TextRun};

/// One styled stretch of the text, in bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleSpan {
    /// The bytes it covers.
    pub range: Range<usize>,
    /// What they look like on top of the element's own style. Never plain.
    pub style: CharStyle,
}

/// Styled stretches, sorted, never empty, never overlapping, and equal neighbours merged. Text outside every span
/// looks like the element says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StyleRuns(Vec<StyleSpan>);

impl StyleRuns {
    /// The runs of a stored text. Ranges past the end of the text are cut.
    pub fn from_text_runs(text: &str, runs: &[TextRun]) -> Self {
        let mut at: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
        at.push(text.len());
        let byte = |char_index: u32| at.get(char_index as usize).copied().unwrap_or(text.len());
        let mut out = Self::default();
        for run in runs {
            // Later runs win where they overlap, like in the scene.
            out.restyle(byte(run.start)..byte(run.end), |_| run.style());
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
                out.push(TextRun::new(start, end, span.style));
            }
        }
        out
    }

    /// The stretches, in text order.
    pub fn spans(&self) -> &[StyleSpan] {
        &self.0
    }

    /// Whether no character has a style of its own.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The style of the character that covers byte `at` (plain when it has none of its own).
    pub fn style_at(&self, at: usize) -> CharStyle {
        self.0.iter().find(|s| s.range.contains(&at)).map(|s| s.style).unwrap_or_default()
    }

    /// The colour of the character that covers byte `at`, if it has one of its own.
    pub fn color_at(&self, at: usize) -> Option<Color> {
        self.style_at(at).color
    }

    /// Gives `range` the colour `color` (`None`: the element's colour). Bold and italic stay.
    pub fn paint(&mut self, range: Range<usize>, color: Option<Color>) {
        self.restyle(range, |style| CharStyle { color, ..style });
    }

    /// Changes the style of every character of `range` by `change`, splitting spans at the edges of the range.
    pub fn restyle(&mut self, range: Range<usize>, change: impl Fn(CharStyle) -> CharStyle) {
        if range.is_empty() {
            return;
        }
        let mut out: Vec<StyleSpan> = Vec::with_capacity(self.0.len() + 2);
        let inside = |out: &mut Vec<StyleSpan>, bytes: Range<usize>, style: CharStyle| {
            let style = change(style);
            if bytes.start < bytes.end && !style.is_plain() {
                out.push(StyleSpan { range: bytes, style });
            }
        };
        // Everything before `cursor` inside `range` is done.
        let mut cursor = range.start;
        for span in self.0.drain(..) {
            if span.range.end <= range.start {
                out.push(span);
            } else if span.range.start >= range.end {
                inside(&mut out, cursor..range.end, CharStyle::default());
                cursor = range.end;
                out.push(span);
            } else {
                if span.range.start < range.start {
                    out.push(StyleSpan { range: span.range.start..range.start, style: span.style });
                }
                let (from, to) = (span.range.start.max(range.start), span.range.end.min(range.end));
                inside(&mut out, cursor..from, CharStyle::default());
                inside(&mut out, from..to, span.style);
                cursor = to;
                if span.range.end > range.end {
                    out.push(StyleSpan { range: range.end..span.range.end, style: span.style });
                }
            }
        }
        inside(&mut out, cursor..range.end, CharStyle::default());
        self.0 = out;
        self.merge();
    }

    /// Replaces the bytes `range` with `new_len` bytes of style `style`. Spans after the range move with the text.
    pub fn replace(&mut self, range: Range<usize>, new_len: usize, style: CharStyle) {
        let Range { start, end } = range.clone();
        self.cut(range);
        // After the cut every span lies before `start` or from `end` on; the second kind moves with the text.
        for span in self.0.iter_mut().filter(|s| s.range.start >= end) {
            span.range = (span.range.start + new_len - (end - start))..(span.range.end + new_len - (end - start));
        }
        if !style.is_plain() && new_len > 0 {
            let at = self.0.partition_point(|s| s.range.start < start);
            self.0.insert(at, StyleSpan { range: start..start + new_len, style });
        }
        self.merge();
    }

    /// Removes the style from `range`, splitting spans that cross its edges.
    fn cut(&mut self, range: Range<usize>) {
        let mut out = Vec::with_capacity(self.0.len() + 1);
        for span in self.0.drain(..) {
            if span.range.end <= range.start || span.range.start >= range.end {
                out.push(span);
                continue;
            }
            if span.range.start < range.start {
                out.push(StyleSpan { range: span.range.start..range.start, style: span.style });
            }
            if span.range.end > range.end {
                out.push(StyleSpan { range: range.end..span.range.end, style: span.style });
            }
        }
        self.0 = out;
    }

    fn merge(&mut self) {
        let mut out: Vec<StyleSpan> = Vec::with_capacity(self.0.len());
        for span in self.0.drain(..) {
            match out.last_mut() {
                Some(last) if last.range.end == span.range.start && last.style == span.style => {
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
    const BOLD: CharStyle = CharStyle { color: None, bold: true, italic: false };

    fn coloured(color: Option<Color>) -> CharStyle {
        CharStyle { color, ..CharStyle::default() }
    }

    fn spans(r: &StyleRuns) -> Vec<(Range<usize>, Color)> {
        r.spans().iter().filter_map(|s| Some((s.range.clone(), s.style.color?))).collect()
    }

    fn styles(r: &StyleRuns) -> Vec<(Range<usize>, CharStyle)> {
        r.spans().iter().map(|s| (s.range.clone(), s.style)).collect()
    }

    #[test]
    fn paint_splits_merges_and_clears() {
        let mut r = StyleRuns::default();
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
        let mut r = StyleRuns::default();
        r.paint(4..8, Some(RED));
        // Typing 3 bytes at 2: the red word moves right.
        r.replace(2..2, 3, coloured(None));
        assert_eq!(spans(&r), vec![(7..11, RED)]);
        // Typing inside the word with its colour keeps one span.
        r.replace(9..9, 2, coloured(Some(RED)));
        assert_eq!(spans(&r), vec![(7..13, RED)]);
        // Typing inside the word without a colour splits it.
        r.replace(9..9, 1, coloured(None));
        assert_eq!(spans(&r), vec![(7..9, RED), (10..14, RED)]);
        // Deleting across the gap joins what is left.
        r.replace(9..10, 0, coloured(None));
        assert_eq!(spans(&r), vec![(7..13, RED)]);
    }

    #[test]
    fn replace_over_a_range_that_straddles_a_span_edge() {
        let mut r = StyleRuns::default();
        r.paint(2..10, Some(RED));
        r.replace(0..4, 1, coloured(None));
        assert_eq!(spans(&r), vec![(1..7, RED)]);
        r.replace(5..9, 0, coloured(None));
        assert_eq!(spans(&r), vec![(1..5, RED)]);
    }

    #[test]
    fn runs_convert_between_characters_and_bytes() {
        let text = "aé€b😀c";
        let runs = [
            TextRun { start: 1, end: 3, color: Some(RED), ..Default::default() },
            TextRun { start: 4, end: 5, color: Some(BLUE), bold: true, italic: true },
        ];
        let r = StyleRuns::from_text_runs(text, &runs);
        assert_eq!(spans(&r), vec![(1..6, RED), (7..11, BLUE)]);
        assert_eq!(r.to_text_runs(text), runs.to_vec());
        assert_eq!(r.color_at(2), Some(RED));
        assert_eq!(r.color_at(6), None);
        assert_eq!(r.style_at(7), CharStyle { color: Some(BLUE), bold: true, italic: true });
    }

    #[test]
    fn runs_past_the_end_of_the_text_are_cut() {
        let r =
            StyleRuns::from_text_runs("ab", &[TextRun { start: 1, end: 99, color: Some(RED), ..Default::default() }]);
        assert_eq!(spans(&r), vec![(1..2, RED)]);
    }

    #[test]
    fn restyle_keeps_what_it_does_not_touch_and_fills_the_gaps() {
        let mut r = StyleRuns::default();
        r.paint(2..4, Some(RED));
        r.restyle(0..6, |s| CharStyle { bold: true, ..s });
        assert_eq!(
            styles(&r),
            vec![(0..2, BOLD), (2..4, CharStyle { color: Some(RED), bold: true, italic: false }), (4..6, BOLD),]
        );
        // Painting a bold stretch keeps it bold, and un-bolding the middle splits the run.
        r.paint(0..6, None);
        r.restyle(1..5, |s| CharStyle { bold: false, ..s });
        assert_eq!(styles(&r), vec![(0..1, BOLD), (5..6, BOLD)]);
    }

    #[test]
    fn typed_bytes_take_the_bold_of_the_style_they_are_given() {
        let mut r = StyleRuns::default();
        r.restyle(0..3, |s| CharStyle { bold: true, ..s });
        r.replace(3..3, 2, BOLD);
        assert_eq!(styles(&r), vec![(0..5, BOLD)], "typing after a bold word continues the span");
        r.replace(5..5, 1, CharStyle::default());
        assert_eq!(styles(&r), vec![(0..5, BOLD)]);
    }
}
