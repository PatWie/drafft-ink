//! Measuring and wrapping text.
//!
//! [`TextMeasure`] says how wide a line is. The engine adapts the typesetter to it; tests use
//! [`MonospaceMeasure`]. Wrapping is a function of the measure, so the shape that is drawn and the shape that
//! was sized agree as long as the renderer wraps at the same width with the same measure.

use drafftink_geom::kurbo::Size;
use drafftink_scene::TextStyle;

/// How wide and tall text is. The engine implements it over the typesetter; tests use [`MonospaceMeasure`].
///
/// Only [`TextMeasure::line_width`] is required. The wrapping default is a greedy word wrap that breaks a word
/// character by character when it does not fit on a line by itself.
pub trait TextMeasure {
    /// Advance width of one line of text (no line breaks in it), in world units.
    fn line_width(&self, line: &str, font: &TextStyle) -> f64;

    /// Height of one line.
    fn line_height(&self, font: &TextStyle) -> f64 {
        font.size * font.line_height
    }

    /// Height of the wrapped line `line`. A line with an inline formula is as tall as the formula needs, so a measure
    /// that typesets inline math (`TextStyle::inline_math`) overrides this; the default is [`Self::line_height`].
    fn line_height_of(&self, _line: &str, font: &TextStyle) -> f64 {
        self.line_height(font)
    }

    /// Wraps `text` (hard breaks are `\n`) so that no line is wider than `max_width` where that is possible. A
    /// non-finite or negative `max_width` keeps only the hard breaks.
    fn wrap(&self, text: &str, font: &TextStyle, max_width: f64) -> Vec<String> {
        wrap_text(self, text, font, max_width)
    }
}

/// A deterministic stand-in for a real font: every character is `advance_em` times the font size wide (wide East
/// Asian characters count double). For tests and headless runs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonospaceMeasure {
    /// Width of one narrow character as a multiple of the font size.
    pub advance_em: f64,
}

impl Default for MonospaceMeasure {
    fn default() -> Self {
        Self { advance_em: 0.5 }
    }
}

fn is_wide(c: char) -> bool {
    matches!(c as u32, 0x1100..=0x115F | 0x2E80..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE6F | 0xFF00..=0xFF60 | 0x1F300..=0x1FAFF)
}

impl TextMeasure for MonospaceMeasure {
    fn line_width(&self, line: &str, font: &TextStyle) -> f64 {
        let cells: usize = line.chars().map(|c| if is_wide(c) { 2 } else { 1 }).sum();
        cells as f64 * self.advance_em * font.size
    }
}

/// The wrapping the trait uses by default.
pub fn wrap_text<M: TextMeasure + ?Sized>(m: &M, text: &str, font: &TextStyle, max_width: f64) -> Vec<String> {
    let mut out = Vec::new();
    let limit = max_width.is_finite() && max_width >= 0.0;
    for hard in text.split('\n') {
        if !limit || m.line_width(hard, font) <= max_width {
            out.push(hard.to_string());
            continue;
        }
        wrap_line(m, hard, font, max_width, &mut out);
    }
    out
}

/// Splits a line into tokens: a word with the spaces after it. Leading spaces form a token of their own.
fn tokens(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut in_space = false;
    let mut seen_word = false;
    for (i, c) in line.char_indices() {
        let space = c.is_whitespace();
        if in_space && !space {
            if seen_word || start < i {
                out.push(line.get(start..i).unwrap_or_default());
            }
            start = i;
            in_space = false;
            seen_word = true;
        } else if !in_space && space {
            in_space = true;
        }
        if !space {
            seen_word = true;
        }
    }
    if start < line.len() {
        out.push(line.get(start..).unwrap_or_default());
    }
    out
}

fn wrap_line<M: TextMeasure + ?Sized>(m: &M, line: &str, font: &TextStyle, max: f64, out: &mut Vec<String>) {
    let mut cur = String::new();
    for token in tokens(line) {
        let mut token = token.to_string();
        loop {
            let candidate = format!("{cur}{}", token.trim_end());
            if m.line_width(&candidate, font) <= max {
                cur.push_str(&token);
                break;
            }
            if cur.trim().is_empty() {
                // The token alone is too wide: cut it character by character.
                let mut piece = String::new();
                let mut rest = token.as_str();
                while let Some(c) = rest.chars().next() {
                    let mut next = piece.clone();
                    next.push(c);
                    if !piece.is_empty() && m.line_width(next.trim_end(), font) > max {
                        out.push(std::mem::take(&mut piece));
                    }
                    piece.push(c);
                    rest = rest.get(c.len_utf8()..).unwrap_or_default();
                }
                cur = piece;
                break;
            }
            out.push(cur.trim_end().to_string());
            cur.clear();
            // Retry the same token on the new line.
            token = token.trim_start().to_string();
            if token.is_empty() {
                break;
            }
        }
    }
    out.push(cur.trim_end().to_string());
}

/// Width and height of already wrapped lines. An empty line counts as a line of the normal height.
pub fn measure_lines<M: TextMeasure + ?Sized>(m: &M, lines: &[String], font: &TextStyle) -> Size {
    let w = lines.iter().map(|l| m.line_width(l, font)).fold(0.0, f64::max);
    let h = if lines.is_empty() { m.line_height(font) } else { lines.iter().map(|l| m.line_height_of(l, font)).sum() };
    Size::new(w, h)
}

/// The size of `text` as one box, wrapped at `max_width` (`None`: only at hard breaks).
pub fn measure_text<M: TextMeasure + ?Sized>(m: &M, text: &str, font: &TextStyle, max_width: Option<f64>) -> Size {
    let lines = m.wrap(text, font, max_width.unwrap_or(f64::INFINITY));
    measure_lines(m, &lines, font)
}
