//! Math layout with ReX.
//!
//! The old app parsed the 683 KB math font, built a codepoint map of its whole cmap and ran the
//! parser and layout again for every equation on every frame. Here the font and the map are
//! built once per [`MathEngine`], and the result is a plain [`MathLayout`] that the cache keeps.
//!
//! ReX reports no error positions, so [`MathError::span`] is found by looking for the offending
//! token in the source. It is exact for unknown commands, symbols, environments, colours and
//! dimensions, and the best guess (an unmatched brace, or the end of the input) for the rest.

use std::collections::HashMap;
use std::ops::{Range, RangeInclusive};
use std::sync::Arc;

use kurbo::{Rect, Size};
use rex::font::backend::ttf_parser::TtfMathFont;
use rex::font::common::GlyphId;
use rex::layout::engine::LayoutBuilder;
use rex::parser::error::ParseError;
use rex::render::{Backend, Cursor, FontBackend, GraphicsBackend, RGBA, Renderer};
use ttf_parser::gsub::{SingleSubstitution, SubstitutionSubtable};

use crate::fonts;
use crate::outline::ColouredPath;
use crate::style::Rgba8;

/// Longest LaTeX source accepted, in bytes.
pub const MAX_LATEX_BYTES: usize = 4096;
/// Deepest nesting: open groups, `\left`...`\right` pairs and environments, plus runs of
/// prefix commands such as `\sqrt\sqrt\sqrt`.
pub const MAX_NESTING: usize = 48;
/// Most structural tokens (`\`, `{`, `^`, `_`, `&`) in one formula. Parser and layout recurse once
/// per token at worst, and wasm has a 1 MB stack.
pub const MAX_TOKENS: usize = 700;
/// Smallest and largest font size.
pub const MATH_SIZE_RANGE: RangeInclusive<f64> = 1.0..=1024.0;
/// Largest laid-out extent in layout units; larger results are an error.
pub const MAX_EXTENT: f64 = 1.0e6;

/// Everything that determines a math layout. The colour is not part of it: glyphs carry `None`
/// unless the formula sets `\color{..}`, and the renderer fills `None` with the shape colour.
#[derive(Clone, Debug, PartialEq)]
pub struct MathSpec {
    /// LaTeX source (math mode, no `$`).
    pub latex: String,
    /// Font size in layout units.
    pub size: f64,
    /// Draw letters and digits with the GelPen text font when it has them, like the old app did,
    /// so equations look like the surrounding handwriting. Symbols GelPen lacks use STIX Two Math.
    pub text_font_fallback: bool,
}

impl MathSpec {
    /// The old app's formula size, also used when a size is not a finite number.
    pub const DEFAULT_SIZE: f64 = 20.0;

    /// A formula with the old app's defaults (size 20, GelPen fallback on).
    pub fn new(latex: impl Into<String>) -> Self {
        Self { latex: latex.into(), size: Self::DEFAULT_SIZE, text_font_fallback: true }
    }

    /// Set the size.
    pub fn with_size(mut self, size: f64) -> Self {
        self.size = size;
        self
    }
}

/// What went wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MathErrorKind {
    /// The source is longer than [`MAX_LATEX_BYTES`].
    TooLong,
    /// Nesting deeper than [`MAX_NESTING`] or more than [`MAX_TOKENS`] structural tokens.
    TooComplex,
    /// `\name` is not a command ReX knows.
    UnknownCommand(String),
    /// A character with no math meaning.
    UnknownSymbol(char),
    /// `\begin{name}` names no supported environment.
    UnknownEnvironment(String),
    /// A colour name or code that does not parse.
    UnknownColor(String),
    /// A bad dimension such as `\rule{1xx}{2pt}`.
    BadDimension(String),
    /// A brace, `\left`/`\right` or `\begin`/`\end` has no partner.
    Unbalanced,
    /// A command lacks an argument, or `^` / `_` has nothing after it.
    MissingArgument,
    /// Any other syntax error; the message says which.
    Syntax(String),
    /// The font has no glyph, or the layout failed.
    Layout(String),
    /// The layout is larger than [`MAX_EXTENT`] or not a finite number.
    TooLarge,
    /// A bug in the math engine (a panic was caught). The message is the panic text.
    Internal(String),
    /// The math font (STIX Two Math) is not loaded yet. The host was asked for it; the formula is laid out
    /// again when it arrives. This error is not cached.
    FontLoading,
}

/// A math error with the byte range of the source it points at.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[error("{message} (bytes {}..{})", span.start, span.end)]
pub struct MathError {
    /// What went wrong, for code.
    pub kind: MathErrorKind,
    /// Where: a byte range of the LaTeX source on `char` boundaries, possibly empty (a point).
    pub span: Range<usize>,
    /// Plain-words text for the user.
    pub message: String,
}

impl MathError {
    fn new(kind: MathErrorKind, span: Range<usize>, message: impl Into<String>) -> Self {
        Self { kind, span, message: message.into() }
    }

    /// The error of a formula asked for before the math font arrived.
    pub(crate) fn font_loading() -> Self {
        Self::new(
            MathErrorKind::FontLoading,
            0..0,
            "The math font is still loading. The formula appears when it has arrived.",
        )
    }

    /// The error's line (0-based) and column (0-based, in `char`s) in `latex`.
    pub fn line_col(&self, latex: &str) -> (usize, usize) {
        let head = latex.get(..self.span.start.min(latex.len())).unwrap_or_default();
        let line = head.matches('\n').count();
        let col = head.rsplit('\n').next().map_or(0, |l| l.chars().count());
        (line, col)
    }
}

/// Which font a glyph comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MathFontSlot {
    /// STIX Two Math.
    Math,
    /// GelPen (the text font).
    Text,
}

/// One positioned glyph. Coordinates are relative to the baseline origin: x to the right, y down,
/// so glyphs above the baseline have negative y.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MathGlyph {
    /// Which font.
    pub font: MathFontSlot,
    /// Glyph id in that font.
    pub id: u16,
    /// Origin x.
    pub x: f64,
    /// Origin y (the glyph's baseline).
    pub y: f64,
    /// Font size to draw with (em size in layout units).
    pub size: f64,
    /// Colour set by `\color{..}`, or `None` for the shape colour.
    pub color: Option<Rgba8>,
}

/// A filled rectangle: a fraction bar, a root bar, an overline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MathRule {
    /// Rectangle, same coordinates as [`MathGlyph`].
    pub rect: Rect,
    /// Colour override.
    pub color: Option<Rgba8>,
}

/// The result of laying out a formula.
#[derive(Clone)]
pub struct MathLayout {
    /// Unique per layout built by one process.
    pub id: u64,
    /// Glyphs in paint order.
    pub glyphs: Vec<MathGlyph>,
    /// Rules.
    pub rules: Vec<MathRule>,
    /// Advance width.
    pub width: f64,
    /// Distance from the baseline up to the top of the box (usually positive).
    pub height: f64,
    /// ReX's depth: the signed distance from the baseline to the bottom of the box, negative when
    /// the box extends below the baseline. The old app stored it the same way.
    pub depth: f64,
    /// The math font.
    pub math_font: peniko::FontData,
    /// The text font used for [`MathFontSlot::Text`] glyphs.
    pub text_font: peniko::FontData,
}

impl std::fmt::Debug for MathLayout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MathLayout")
            .field("id", &self.id)
            .field("glyphs", &self.glyphs.len())
            .field("rules", &self.rules.len())
            .field("width", &self.width)
            .field("height", &self.height)
            .field("depth", &self.depth)
            .finish_non_exhaustive()
    }
}

impl MathLayout {
    /// The box relative to the baseline origin: `x` from 0 to `width`, `y` from `-height` to
    /// `-depth`. This is the old app's math bounds with the origin at `(0, 0)`.
    pub fn bounds(&self) -> Rect {
        Rect::new(0.0, -self.height, self.width, -self.depth)
    }

    /// Size of [`Self::bounds`].
    pub fn size(&self) -> Size {
        self.bounds().size()
    }

    /// Distance from the top of the box to the baseline.
    pub fn baseline(&self) -> f64 {
        self.height
    }

    /// The font file of a slot.
    pub fn font(&self, slot: MathFontSlot) -> &peniko::FontData {
        match slot {
            MathFontSlot::Math => &self.math_font,
            MathFontSlot::Text => &self.text_font,
        }
    }

    /// Rough heap size, for the cache budget.
    pub fn weight(&self) -> usize {
        self.glyphs.len() * std::mem::size_of::<MathGlyph>() + self.rules.len() * std::mem::size_of::<MathRule>() + 256
    }

    /// Filled paths for exports, grouped by colour in paint order. `base` fills glyphs that have
    /// no colour of their own.
    pub fn outline(&self, base: Rgba8) -> Vec<ColouredPath> {
        crate::outline::math_outline(self, base)
    }
}

// ----------------------------------------------------------------------------------------------
// Engine

pub(crate) struct MathEngine {
    font: TtfMathFont<'static>,
    text_face: Option<ttf_parser::Face<'static>>,
    glyph_to_char: HashMap<u16, char>,
    math_data: peniko::FontData,
    text_data: peniko::FontData,
}

impl MathEngine {
    /// An engine over the bytes of STIX Two Math. They live for the rest of the process: the font is parsed
    /// once and ReX borrows it.
    pub fn new(math_font: &'static [u8]) -> Result<Self, MathError> {
        let bad = |what: &str| {
            MathError::new(MathErrorKind::Layout(what.to_owned()), 0..0, format!("The math font is unusable: {what}."))
        };
        let face = ttf_parser::Face::parse(math_font, 0).map_err(|e| bad(&e.to_string()))?;
        let font = TtfMathFont::new(face).map_err(|e| bad(&e.to_string()))?;
        let text_face = ttf_parser::Face::parse(fonts::GELPEN_REGULAR, 0).ok();
        let mut glyph_to_char = HashMap::new();
        for sub in font.font().tables().cmap.iter().flat_map(|c| c.subtables) {
            if sub.is_unicode() {
                sub.codepoints(|cp| {
                    if let (Some(c), Some(g)) = (char::from_u32(cp), sub.glyph_index(cp)) {
                        glyph_to_char.entry(g.0).or_insert(c);
                    }
                });
            }
        }
        for (form, base) in script_forms(font.font()) {
            if let Some(&c) = glyph_to_char.get(&base) {
                glyph_to_char.insert(form, c);
            }
        }
        let data = |bytes: &'static [u8]| peniko::FontData::new(peniko::Blob::new(Arc::new(bytes)), 0);
        Ok(Self { font, text_face, glyph_to_char, math_data: data(math_font), text_data: data(fonts::GELPEN_REGULAR) })
    }

    pub fn layout(&self, spec: &MathSpec, id: u64) -> Result<MathLayout, MathError> {
        let latex = spec.latex.as_str();
        precheck(latex)?;
        let size = if spec.size.is_finite() {
            spec.size.clamp(*MATH_SIZE_RANGE.start(), *MATH_SIZE_RANGE.end())
        } else {
            MathSpec::DEFAULT_SIZE
        };
        let run = || self.run(spec, latex, size, id);
        // ReX is not written to be panic-free on hostile input. On targets that unwind, turn a
        // panic into an error. (With `panic = "abort"` this cannot help, which is why `precheck`
        // and the fuzz tests exist.)
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)) {
            Ok(r) => r,
            Err(payload) => {
                let text = payload
                    .downcast_ref::<&str>()
                    .map(|s| (*s).to_owned())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_default();
                Err(MathError::new(
                    MathErrorKind::Internal(text),
                    0..latex.len(),
                    "The math engine failed on this formula.",
                ))
            }
        }
    }

    fn run(&self, spec: &MathSpec, latex: &str, size: f64, id: u64) -> Result<MathLayout, MathError> {
        let nodes = rex::parser::parse(latex).map_err(|e| convert_parse_error(&e, latex))?;
        let engine = LayoutBuilder::new(&self.font).font_size(size).build();
        let layout = engine.layout(&nodes).map_err(|e| {
            let rex::error::LayoutError::Font(font_err) = e;
            MathError::new(
                MathErrorKind::Layout(font_err.to_string()),
                0..latex.len(),
                format!("The formula cannot be laid out: {font_err}."),
            )
        })?;
        let dims = layout.size();
        let ok = |v: f64| v.is_finite() && v.abs() <= MAX_EXTENT;
        if !(ok(dims.width) && ok(dims.height) && ok(dims.depth)) {
            return Err(MathError::new(MathErrorKind::TooLarge, 0..latex.len(), "The formula is too large to draw."));
        }
        let mut rec = Recorder::new(self, spec.text_font_fallback);
        Renderer::new().render(&layout, &mut rec);
        Ok(MathLayout {
            id,
            glyphs: rec.glyphs,
            rules: rec.rules,
            width: dims.width,
            height: dims.height,
            depth: dims.depth,
            math_font: self.math_data.clone(),
            text_font: self.text_data.clone(),
        })
    }

    /// The text font (GelPen) used for [`MathFontSlot::Text`] glyphs; `None` if it did not parse.
    pub fn text_face(&self) -> Option<&ttf_parser::Face<'static>> {
        self.text_face.as_ref()
    }
}

/// The base glyph behind each script-size form (the `ssty` feature) of the math font: `(form, base)`.
///
/// ReX draws the glyphs of a superscript or subscript with these forms (`two.ssty`, and a second, smaller one
/// for scripts of scripts). Some forms are also the glyph of a compatibility character (`two.ssty` is U+00B2),
/// others have no code point at all. Either way [`Recorder::symbol`] must find the character of the *base* glyph
/// to draw the script in the text font like the rest of the formula.
fn script_forms(face: &ttf_parser::Face<'_>) -> Vec<(u16, u16)> {
    let Some(gsub) = face.tables().gsub else { return Vec::new() };
    let tag = ttf_parser::Tag::from_bytes(b"ssty");
    let subtables = gsub
        .features
        .into_iter()
        .filter(|feature| feature.tag == tag)
        .flat_map(|feature| feature.lookup_indices)
        .filter_map(|index| gsub.lookups.get(index))
        .flat_map(|lookup| lookup.subtables.into_iter::<SubstitutionSubtable>());
    let mut forms = Vec::new();
    for subtable in subtables {
        let bases = (0..face.number_of_glyphs()).map(ttf_parser::GlyphId);
        match subtable {
            SubstitutionSubtable::Single(SingleSubstitution::Format1 { coverage, delta }) => {
                forms.extend(
                    bases
                        .filter(|&base| coverage.contains(base))
                        .map(|base| (base.0.wrapping_add_signed(delta), base.0)),
                );
            }
            SubstitutionSubtable::Single(SingleSubstitution::Format2 { coverage, substitutes }) => {
                forms.extend(bases.filter_map(|base| Some((substitutes.get(coverage.get(base)?)?.0, base.0))));
            }
            SubstitutionSubtable::Alternate(alt) => {
                for base in bases {
                    let set = alt.coverage.get(base).and_then(|index| alt.alternate_sets.get(index));
                    forms.extend(set.into_iter().flat_map(|set| set.alternates).map(|form| (form.0, base.0)));
                }
            }
            _ => {}
        }
    }
    forms
}

/// Map the Unicode math alphanumerics (italic, bold, sans, monospace) to plain ASCII so the text
/// font can draw them. Ported from the old `rex_backend.rs`.
pub(crate) fn math_to_ascii(c: char) -> Option<char> {
    let cp = c as u32;
    let offset = |base: u32, first: u8| Some(char::from(first + (cp - base) as u8));
    match cp {
        0x1D434..=0x1D44D => offset(0x1D434, b'A'),
        0x1D44E..=0x1D454 => offset(0x1D44E, b'a'),
        0x1D456..=0x1D467 => Some(char::from(b'i' + (cp - 0x1D456) as u8)),
        0x210E => Some('h'),
        0x1D400..=0x1D419 => offset(0x1D400, b'A'),
        0x1D41A..=0x1D433 => offset(0x1D41A, b'a'),
        0x1D468..=0x1D481 => offset(0x1D468, b'A'),
        0x1D482..=0x1D49B => offset(0x1D482, b'a'),
        0x1D5A0..=0x1D5B9 => offset(0x1D5A0, b'A'),
        0x1D5BA..=0x1D5D3 => offset(0x1D5BA, b'a'),
        0x1D670..=0x1D689 => offset(0x1D670, b'A'),
        0x1D68A..=0x1D6A3 => offset(0x1D68A, b'a'),
        0x1D7CE..=0x1D7D7 => offset(0x1D7CE, b'0'),
        0x1D7D8..=0x1D7E1 => offset(0x1D7D8, b'0'),
        0x1D7E2..=0x1D7EB => offset(0x1D7E2, b'0'),
        0x1D7EC..=0x1D7F5 => offset(0x1D7EC, b'0'),
        0x1D7F6..=0x1D7FF => offset(0x1D7F6, b'0'),
        _ => None,
    }
}

struct Recorder<'e> {
    engine: &'e MathEngine,
    use_text: bool,
    glyphs: Vec<MathGlyph>,
    rules: Vec<MathRule>,
    colors: Vec<Option<Rgba8>>,
    current: Option<Rgba8>,
}

impl<'e> Recorder<'e> {
    fn new(engine: &'e MathEngine, use_text: bool) -> Self {
        Self { engine, use_text, glyphs: Vec::new(), rules: Vec::new(), colors: Vec::new(), current: None }
    }
}

impl<'f> FontBackend<TtfMathFont<'f>> for Recorder<'_> {
    fn symbol(&mut self, pos: Cursor, gid: GlyphId, scale: f64, _ctx: &TtfMathFont<'f>) {
        let math_gid: u16 = gid.into();
        if self.use_text
            && let (Some(face), Some(&cp)) = (self.engine.text_face(), self.engine.glyph_to_char.get(&math_gid))
            && let Some(g) = face.glyph_index(math_to_ascii(cp).unwrap_or(cp))
        {
            // Slightly smaller than the math font, to match the text tool (old app: 0.75).
            self.glyphs.push(MathGlyph {
                font: MathFontSlot::Text,
                id: g.0,
                x: pos.x,
                y: pos.y,
                size: scale * 0.75,
                color: self.current,
            });
            return;
        }
        self.glyphs.push(MathGlyph {
            font: MathFontSlot::Math,
            id: math_gid,
            x: pos.x,
            y: pos.y,
            size: scale,
            color: self.current,
        });
    }
}

impl GraphicsBackend for Recorder<'_> {
    fn rule(&mut self, pos: Cursor, width: f64, height: f64) {
        self.rules.push(MathRule { rect: Rect::new(pos.x, pos.y, pos.x + width, pos.y + height), color: self.current });
    }

    fn begin_color(&mut self, RGBA(r, g, b, a): RGBA) {
        self.colors.push(self.current);
        self.current = Some(Rgba8::new(r, g, b, a));
    }

    fn end_color(&mut self) {
        if let Some(prev) = self.colors.pop() {
            self.current = prev;
        }
    }
}

impl<'f> Backend<TtfMathFont<'f>> for Recorder<'_> {}

// ----------------------------------------------------------------------------------------------
// Input checks and error positions

/// Reject input that would make ReX recurse too deep or run too long. One cheap pass.
///
/// ReX's parser and layout recurse once per nested group and once per prefix command (`\sqrt`,
/// `\frac`, `^`): `\sqrt\sqrt\sqrt...` needs no braces and overflowed a 1 MiB stack (the wasm
/// default) at about 300 levels in a release build and 100 in a debug build. So the depth counted
/// here is the number of open groups plus the length of every run of consecutive commands and
/// scripts on the way down. Each of these is limited to [`MAX_NESTING`] in total.
fn precheck(latex: &str) -> Result<(), MathError> {
    if latex.len() > MAX_LATEX_BYTES {
        let cut = (0..=MAX_LATEX_BYTES).rev().find(|&i| latex.is_char_boundary(i)).unwrap_or(0);
        return Err(MathError::new(
            MathErrorKind::TooLong,
            cut..latex.len(),
            format!("The formula is longer than {MAX_LATEX_BYTES} bytes."),
        ));
    }
    let too_complex = |at: usize, len: usize, what: String| {
        MathError::new(MathErrorKind::TooComplex, at..(at + len).min(latex.len()), what)
    };
    // `levels[i]` is the command-run length that was pending when group i opened.
    let mut levels: Vec<usize> = Vec::new();
    let mut pending: usize = 0; // sum of `levels`
    let mut run = 0usize;
    let mut tokens = 0usize;
    let mut chars = latex.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let mut width = c.len_utf8();
        match c {
            '\\' => {
                tokens += 1;
                let mut name = String::new();
                while let Some(&(_, n)) = chars.peek() {
                    if n.is_ascii_alphabetic() {
                        name.push(n);
                        width += 1;
                        chars.next();
                    } else {
                        break;
                    }
                }
                if name.is_empty() {
                    // `\\`, `\{`, `\,`: a control symbol; skip its one character.
                    if let Some((_, n)) = chars.next() {
                        width += n.len_utf8();
                    }
                    run = 0;
                } else if name == "left" || name == "begin" {
                    levels.push(run);
                    pending += run;
                    run = 0;
                } else if name == "right" || name == "end" {
                    pending -= levels.pop().map_or(0, |r| r.min(pending));
                    run = 0;
                } else {
                    run += 1;
                }
            }
            '^' | '_' => {
                tokens += 1;
                run += 1;
            }
            '{' => {
                tokens += 1;
                levels.push(run);
                pending += run;
                run = 0;
            }
            '}' => {
                pending -= levels.pop().map_or(0, |r| r.min(pending));
                run = 0;
            }
            '&' => {
                tokens += 1;
                run = 0;
            }
            c if c.is_whitespace() => {}
            _ => run = 0,
        }
        let depth = levels.len() + pending + run;
        if depth > MAX_NESTING {
            return Err(too_complex(i, width, format!("The formula nests deeper than {MAX_NESTING} levels.")));
        }
        if tokens > MAX_TOKENS {
            return Err(too_complex(
                i,
                width,
                format!("The formula has more than {MAX_TOKENS} commands, braces and scripts."),
            ));
        }
    }
    Ok(())
}

/// Find `needle` in `hay`; the byte range, or `None`.
fn find(hay: &str, needle: &str) -> Option<Range<usize>> {
    if needle.is_empty() {
        return None;
    }
    hay.find(needle).map(|s| s..s + needle.len())
}

/// The range of `\name` where the name is not a prefix of a longer command.
fn find_command(latex: &str, name: &str) -> Option<Range<usize>> {
    let pat = format!("\\{name}");
    let mut from = 0;
    while let Some(off) = latex.get(from..).and_then(|s| s.find(&pat)) {
        let start = from + off;
        let end = start + pat.len();
        if !latex.get(end..).and_then(|s| s.chars().next()).is_some_and(|c| c.is_ascii_alphabetic()) {
            return Some(start..end);
        }
        from = end;
    }
    None
}

/// Position of the first unmatched `}` or, failing that, the last unmatched `{`; otherwise the end.
fn find_unbalanced(latex: &str) -> Range<usize> {
    let mut stack: Vec<usize> = Vec::new();
    let mut prev_backslash = false;
    for (i, c) in latex.char_indices() {
        let escaped = prev_backslash;
        prev_backslash = c == '\\' && !escaped;
        match c {
            '{' if !escaped => stack.push(i),
            '}' if !escaped && stack.pop().is_none() => return i..i + 1,
            '}' => {}
            _ => {}
        }
    }
    match stack.last() {
        Some(&i) => i..i + 1,
        None => latex.len()..latex.len(),
    }
}

fn convert_parse_error(e: &ParseError, latex: &str) -> MathError {
    let end = latex.len()..latex.len();
    let whole = 0..latex.len();
    let (kind, span, message) = match e {
        ParseError::UnrecognizedControlSequence(n) => (
            MathErrorKind::UnknownCommand(n.to_string()),
            find_command(latex, n).unwrap_or(whole),
            format!("Unknown command \\{n}."),
        ),
        ParseError::UnrecognizedSymbol(c) => (
            MathErrorKind::UnknownSymbol(*c),
            find(latex, &c.to_string()).unwrap_or(whole),
            format!("The symbol '{c}' is not supported."),
        ),
        ParseError::UnrecognizedEnvironment(n) => (
            MathErrorKind::UnknownEnvironment(n.to_string()),
            find(latex, n).unwrap_or(whole),
            format!("Unknown environment '{n}'."),
        ),
        ParseError::UnrecognizedColor(n) => (
            MathErrorKind::UnknownColor(n.to_string()),
            find(latex, n).unwrap_or(whole),
            format!("'{n}' is not a colour. Use a name such as red, or #rrggbb."),
        ),
        ParseError::UnrecognizedDimension(n) => (
            MathErrorKind::BadDimension(n.to_string()),
            find(latex, n).unwrap_or(whole),
            format!("'{n}' is not a dimension. Use a number followed by pt or em."),
        ),
        ParseError::UnmatchedBrackets | ParseError::UnexpectedEndGroup { .. } => (
            MathErrorKind::Unbalanced,
            find_unbalanced(latex),
            "A brace, \\left/\\right or \\begin/\\end has no partner.".to_owned(),
        ),
        ParseError::MissingArgForCommand(n) => (
            MathErrorKind::MissingArgument,
            find_command(latex, n).unwrap_or(whole),
            format!("\\{n} needs an argument in braces."),
        ),
        ParseError::MissingSubSuperScript => {
            let at = latex.rfind(['^', '_']).map_or(end, |i| i..i + 1);
            (MathErrorKind::MissingArgument, at, "^ or _ needs something after it.".to_owned())
        }
        ParseError::ExpectedToken | ParseError::MissingColFormatForArrayEnvironment => {
            (MathErrorKind::MissingArgument, end, "The formula ends too early.".to_owned())
        }
        other => (MathErrorKind::Syntax(other.to_string()), whole, format!("Syntax error: {other}.")),
    };
    MathError::new(kind, span, message)
}
