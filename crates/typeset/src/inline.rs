//! Inline math: which parts of a text are formulas, and how the text as written maps to the text laid out.
//!
//! # What is a formula
//!
//! The rules are Pandoc's `tex_math_dollars`, which keep prices out of the maths ("it costs $5 and $10"):
//!
//! * A formula is `$latex$` on one line. The `$` that opens it is followed by a character that is not white space.
//!   The `$` that closes it is the next unescaped `$`; it must follow a character that is not white space and must not
//!   be followed by a digit. When the next `$` does not close, the opening `$` is an ordinary character and the search
//!   goes on after it.
//! * `\$` is a literal dollar sign, in the text and inside a formula (where it reaches the formula as `\$`).
//! * Two or more `$` in a row (`$$`) are ordinary characters. There is no display math.
//! * At most [`MAX_FORMULAS`] formulas per text; later ones stay ordinary text.
//!
//! # The laid-out text
//!
//! The text as written (the *source*) is what the document stores, what the caret indexes and what exports write.
//! The text handed to the shaper (the *laid-out text*) replaces each formula with one zero-width space and each
//! `\$` with `$`; the formula itself is an inline box placed in front of its zero-width space. [`TextMap`] converts
//! byte indexes between the two. A formula is one caret step: the source indexes inside it map to its edges.

use std::ops::Range;

/// Most formulas typeset in one text. Later `$...$` pairs stay ordinary text.
pub const MAX_FORMULAS: usize = 256;

/// The character laid out in place of a formula: zero width, and a line break may follow it.
pub(crate) const PLACEHOLDER: char = '\u{200b}';

/// One part of a source text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Piece {
    /// Ordinary text, drawn as written.
    Plain(Range<usize>),
    /// `\$`: a dollar sign.
    Escape(Range<usize>),
    /// `$latex$`.
    Formula {
        /// The whole formula with its dollar signs.
        src: Range<usize>,
        /// The LaTeX between them.
        latex: Range<usize>,
    },
}

/// The formulas of `text` as written: each `$...$` range, dollar signs included.
pub fn formula_ranges(text: &str) -> Vec<Range<usize>> {
    scan(text)
        .into_iter()
        .filter_map(|p| match p {
            Piece::Formula { src, .. } => Some(src),
            Piece::Plain(_) | Piece::Escape(_) => None,
        })
        .collect()
}

/// Whether `text` has a formula or an escaped dollar, that is, whether [`InlineMath::Auto`](crate::InlineMath::Auto)
/// would lay it out differently from plain text.
pub fn has_inline_math(text: &str) -> bool {
    text.contains('$') && scan(text).iter().any(|p| !matches!(p, Piece::Plain(_)))
}

/// Splits `text` into pieces that cover it without gaps.
pub(crate) fn scan(text: &str) -> Vec<Piece> {
    let b = text.as_bytes();
    let mut pieces = Vec::new();
    if !b.contains(&b'$') {
        if !b.is_empty() {
            pieces.push(Piece::Plain(0..b.len()));
        }
        return pieces;
    }
    let (mut i, mut plain_start, mut formulas) = (0usize, 0usize, 0usize);
    let flush = |pieces: &mut Vec<Piece>, from: usize, to: usize| {
        if from < to {
            pieces.push(Piece::Plain(from..to));
        }
    };
    while let Some(&c) = b.get(i) {
        match c {
            b'\\' if b.get(i + 1) == Some(&b'$') => {
                flush(&mut pieces, plain_start, i);
                pieces.push(Piece::Escape(i..i + 2));
                i += 2;
                plain_start = i;
            }
            b'$' => {
                let run = b.get(i..).map_or(0, |rest| rest.iter().take_while(|&&c| c == b'$').count());
                if run == 1
                    && formulas < MAX_FORMULAS
                    && let Some(close) = closing_dollar(text, i)
                {
                    flush(&mut pieces, plain_start, i);
                    pieces.push(Piece::Formula { src: i..close + 1, latex: i + 1..close });
                    formulas += 1;
                    i = close + 1;
                    plain_start = i;
                } else {
                    i += run.max(1);
                }
            }
            _ => i += 1,
        }
    }
    flush(&mut pieces, plain_start, b.len());
    pieces
}

/// The byte index of the `$` that closes the formula opened by the `$` at `open`, if there is one.
fn closing_dollar(text: &str, open: usize) -> Option<usize> {
    let b = text.as_bytes();
    let first = text.get(open + 1..)?.chars().next()?;
    if first.is_whitespace() {
        return None;
    }
    let mut j = open + 1;
    while let Some(&c) = b.get(j) {
        match c {
            b'\n' => return None,
            // The next character is escaped (a line break cannot be).
            b'\\' if b.get(j + 1) == Some(&b'\n') => return None,
            b'\\' => j += 2,
            b'$' => {
                let before = text.get(..j)?.chars().next_back()?;
                let digit_follows = b.get(j + 1).is_some_and(u8::is_ascii_digit);
                let doubled = b.get(j + 1) == Some(&b'$');
                return (!before.is_whitespace() && !digit_follows && !doubled).then_some(j);
            }
            _ => j += 1,
        }
    }
    None
}

// ----------------------------------------------------------------------------------------------
// The index map

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SegKind {
    /// Same bytes on both sides.
    Plain,
    /// `\$` in the source, `$` laid out.
    Escape,
    /// `$latex$` in the source, one placeholder laid out.
    Formula,
}

#[derive(Clone, Debug)]
struct Seg {
    src: Range<usize>,
    lay: Range<usize>,
    kind: SegKind,
}

/// Converts byte indexes between the text as written and the text laid out. An empty map is the identity.
#[derive(Clone, Debug, Default)]
pub(crate) struct TextMap {
    segs: Vec<Seg>,
    src_len: usize,
    lay_len: usize,
}

impl TextMap {
    /// The map of a text that is laid out as written.
    pub fn identity(len: usize) -> Self {
        Self { segs: Vec::new(), src_len: len, lay_len: len }
    }

    /// Length of the text laid out.
    #[cfg(test)]
    pub fn lay_len(&self) -> usize {
        self.lay_len
    }

    /// Length of the text as written.
    pub fn src_len(&self) -> usize {
        self.src_len
    }

    /// The laid-out index for a source index (clamped). Inside an escape or a formula it is the nearer edge.
    pub fn to_lay(&self, src: usize) -> usize {
        let src = src.min(self.src_len);
        if self.segs.is_empty() {
            return src;
        }
        let at = self.segs.partition_point(|s| s.src.end <= src);
        let Some(seg) = self.segs.get(at) else { return self.lay_len };
        match seg.kind {
            SegKind::Plain => seg.lay.start + (src - seg.src.start),
            SegKind::Escape | SegKind::Formula => {
                if src - seg.src.start <= (seg.src.end - seg.src.start) / 2 {
                    seg.lay.start
                } else {
                    seg.lay.end
                }
            }
        }
    }

    /// The source index for a laid-out index (clamped).
    pub fn to_src(&self, lay: usize) -> usize {
        let lay = lay.min(self.lay_len);
        if self.segs.is_empty() {
            return lay;
        }
        let at = self.segs.partition_point(|s| s.lay.end <= lay);
        let Some(seg) = self.segs.get(at) else { return self.src_len };
        match seg.kind {
            SegKind::Plain => seg.src.start + (lay - seg.lay.start),
            SegKind::Escape | SegKind::Formula => {
                if lay <= seg.lay.start {
                    seg.src.start
                } else {
                    seg.src.end
                }
            }
        }
    }

    /// A laid-out range as a source range.
    pub fn range_to_src(&self, lay: Range<usize>) -> Range<usize> {
        self.to_src(lay.start)..self.to_src(lay.end)
    }
}

/// Builds the laid-out text and its [`TextMap`] from pieces.
#[derive(Default)]
pub(crate) struct Assembly {
    text: String,
    map: TextMap,
}

impl Assembly {
    /// Appends ordinary source text.
    pub fn plain(&mut self, src: &str, range: Range<usize>) {
        let text = src.get(range.clone()).unwrap_or_default();
        let lay = self.text.len()..self.text.len() + text.len();
        self.text.push_str(text);
        self.map.segs.push(Seg { src: range, lay, kind: SegKind::Plain });
    }

    /// Appends `\$` as `$`.
    pub fn escape(&mut self, range: Range<usize>) {
        let lay = self.text.len()..self.text.len() + 1;
        self.text.push('$');
        self.map.segs.push(Seg { src: range, lay, kind: SegKind::Escape });
    }

    /// Appends the placeholder of a formula and returns its laid-out index.
    pub fn formula(&mut self, range: Range<usize>) -> usize {
        let at = self.text.len();
        self.text.push(PLACEHOLDER);
        self.map.segs.push(Seg { src: range, lay: at..self.text.len(), kind: SegKind::Formula });
        at
    }

    /// The laid-out length so far.
    pub fn len(&self) -> usize {
        self.text.len()
    }

    /// The finished text and map. `src_len` is the length of the source.
    pub fn finish(mut self, src_len: usize) -> (String, TextMap) {
        self.map.src_len = src_len;
        self.map.lay_len = self.text.len();
        (self.text, self.map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn formulas(text: &str) -> Vec<&str> {
        formula_ranges(text).into_iter().filter_map(|r| text.get(r)).collect()
    }

    #[test]
    fn a_formula_needs_a_non_space_after_the_opening_and_before_the_closing_dollar() {
        assert_eq!(formulas("Energy $E = mc^2$ here"), ["$E = mc^2$"]);
        assert!(formulas("a $ x$ b").is_empty());
        assert!(formulas("a $x $ b").is_empty());
        assert_eq!(formulas("$x$"), ["$x$"]);
    }

    #[test]
    fn prices_are_not_formulas() {
        assert!(formulas("it costs $5 and $10").is_empty());
        assert!(formulas("$5-$6").is_empty());
        assert!(formulas("between $20 and $30 today").is_empty());
        // A price before a real formula.
        assert_eq!(formulas("costs $5 and $x$"), ["$x$"]);
    }

    #[test]
    fn an_escaped_dollar_is_text_and_cannot_close() {
        assert!(formulas(r"\$x\$").is_empty());
        assert_eq!(formulas(r"$a\$b$"), [r"$a\$b$"]);
        assert_eq!(formulas(r"\$ and $x$"), ["$x$"]);
    }

    #[test]
    fn doubled_dollars_are_text() {
        assert!(formulas("$$x$$").is_empty());
        assert!(formulas("a $$ b").is_empty());
        assert!(formulas("$a$$b$").is_empty());
    }

    #[test]
    fn a_formula_stays_on_its_line() {
        assert!(formulas("$a\nb$").is_empty());
        assert_eq!(formulas("$a$\n$b$"), ["$a$", "$b$"]);
        assert!(formulas("$a\\\nb$").is_empty());
    }

    #[test]
    fn pieces_cover_the_text_without_gaps() {
        for text in ["", "plain", "a $x$ b \\$ c $ d", "é$ü$é", "$$", "\\", "$", "\\$", "$x$$y$ $z$"] {
            let mut at = 0;
            for p in scan(text) {
                let r = match &p {
                    Piece::Plain(r) | Piece::Escape(r) => r.clone(),
                    Piece::Formula { src, .. } => src.clone(),
                };
                assert_eq!(r.start, at, "{text:?}");
                at = r.end;
            }
            assert_eq!(at, text.len(), "{text:?}");
        }
    }

    #[test]
    fn at_most_max_formulas_are_found() {
        let text = "$x$ ".repeat(MAX_FORMULAS + 10);
        assert_eq!(formula_ranges(&text).len(), MAX_FORMULAS);
    }

    #[test]
    fn has_inline_math_sees_formulas_and_escapes() {
        assert!(has_inline_math("a $x$"));
        assert!(has_inline_math(r"a \$"));
        assert!(!has_inline_math("a $ b"));
        assert!(!has_inline_math("plain"));
    }

    fn assembled(text: &str) -> (String, TextMap) {
        let mut a = Assembly::default();
        for p in scan(text) {
            match p {
                Piece::Plain(r) => a.plain(text, r),
                Piece::Escape(r) => a.escape(r),
                Piece::Formula { src, .. } => {
                    a.formula(src);
                }
            }
        }
        a.finish(text.len())
    }

    #[test]
    fn the_map_converts_indexes_both_ways() {
        let text = r"ab $x$ c\$d";
        let (lay, map) = assembled(text);
        assert_eq!(lay, "ab \u{200b} c$d");
        // Before the formula, at its edges, and after it.
        assert_eq!(map.to_lay(3), 3);
        assert_eq!(map.to_lay(6), 6);
        assert_eq!(map.to_src(3), 3);
        assert_eq!(map.to_src(6), 6);
        // Inside the formula the nearer edge.
        assert_eq!(map.to_lay(4), 3);
        assert_eq!(map.to_lay(5), 6);
        // After the escape.
        assert_eq!(map.to_lay(text.len()), lay.len());
        assert_eq!(map.to_src(lay.len()), text.len());
        // Round trip on every index that is a boundary of a piece.
        for src in [0, 1, 2, 3, 6, 7, 8, 10, 11] {
            assert_eq!(map.to_src(map.to_lay(src)), src, "{src}");
        }
    }

    #[test]
    fn the_map_clamps_out_of_range_indexes() {
        let (_, map) = assembled("a $x$");
        assert_eq!(map.to_lay(usize::MAX), map.lay_len());
        assert_eq!(map.to_src(usize::MAX), 5);
        let id = TextMap::identity(4);
        assert_eq!(id.to_lay(9), 4);
        assert_eq!(id.to_src(9), 4);
    }
}
