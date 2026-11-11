//! Input types for text layout: styles, spans, alignment, wrapping.
//!
//! These types are owned by this crate on purpose. `scene` converts its element data into a
//! [`TextSpec`]; this crate never reads a document.

use std::ops::Range;

/// An 8-bit sRGB colour with straight (non-premultiplied) alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct Rgba8 {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
    /// Alpha, 255 is opaque.
    pub a: u8,
}

impl Rgba8 {
    /// Opaque black.
    pub const BLACK: Self = Self { r: 0, g: 0, b: 0, a: 255 };

    /// An opaque colour.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// A colour with alpha.
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// The same colour as a `peniko` colour.
    pub fn to_color(self) -> peniko::Color {
        peniko::Color::from_rgba8(self.r, self.g, self.b, self.a)
    }
}

impl From<[u8; 4]> for Rgba8 {
    fn from([r, g, b, a]: [u8; 4]) -> Self {
        Self { r, g, b, a }
    }
}

/// A font family. The first five are bundled; [`FontFamily::Custom`] names a family added with
/// [`crate::Typesetter::register_font`].
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub enum FontFamily {
    /// GelPen, the hand-drawn default.
    #[default]
    GelPen,
    /// GelPen Serif.
    GelPenSerif,
    /// Vanilla Extract.
    VanillaExtract,
    /// Noto Sans (bundled with the `noto-sans` feature, on by default).
    NotoSans,
    /// IBM Plex Sans, a variable font with real light, regular and bold weights (an [`crate::ExtraFont`]: built in
    /// with the `plex-sans` feature, or provided by the host).
    PlexSans,
    /// A family registered at run time. Unknown names fall back to GelPen.
    Custom(String),
}

/// Font weight. Bundled families have three weights (Noto Sans has regular and bold only, Vanilla
/// Extract one, IBM Plex Sans draws all three from one variable file); a request for a missing weight uses the nearest face.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub enum FontWeight {
    /// Light.
    Light,
    /// Regular (GelPen Serif calls this face "Medium").
    #[default]
    Regular,
    /// Heavy (bold for Noto Sans).
    Heavy,
}

/// Horizontal alignment of lines inside the layout box.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Align {
    /// Left edge (the old app's only mode).
    #[default]
    Left,
    /// Centred.
    Center,
    /// Right edge.
    Right,
}

/// Line height.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum LineHeight {
    /// A multiple of the font's own line height (ascent + descent + gap). `Metrics(1.0)` is what
    /// the old app used.
    Metrics(f32),
    /// A multiple of the font size, as in CSS and Excalidraw (which uses 1.25).
    FontSize(f32),
    /// The font's own line height. Same as `Metrics(1.0)`.
    #[default]
    Normal,
}

/// Wrapping.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Wrap {
    /// Break lines only at `\n`. The box grows with the text ("autosize").
    #[default]
    None,
    /// Break lines at this width. Words longer than the width are split.
    Width(f32),
}

/// The style of a run of text.
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    /// Font family.
    pub family: FontFamily,
    /// Weight.
    pub weight: FontWeight,
    /// Italic. Families without an italic face get a synthetic slant.
    pub italic: bool,
    /// Font size in layout units (pixels at zoom 1).
    pub size: f32,
    /// Fill colour.
    pub color: Rgba8,
    /// Underline.
    pub underline: bool,
    /// Strikethrough.
    pub strike: bool,
}

impl TextStyle {
    /// The old app's default size.
    pub const DEFAULT_SIZE: f32 = 20.0;

    /// Default style with a size.
    pub fn with_size(size: f32) -> Self {
        Self { size, ..Self::default() }
    }

    /// Style for a legacy `(family, weight)` pair of the old app. The old app drew Noto Sans
    /// "Light" as the italic face and ignored the weight for Vanilla Extract.
    pub fn from_legacy(family: FontFamily, weight: FontWeight, size: f32, color: Rgba8) -> Self {
        let (weight, italic) = match (&family, weight) {
            (FontFamily::NotoSans, FontWeight::Light) => (FontWeight::Regular, true),
            (FontFamily::VanillaExtract, _) => (FontWeight::Regular, false),
            (_, w) => (w, false),
        };
        Self { family, weight, italic, size, color, underline: false, strike: false }
    }
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            family: FontFamily::default(),
            weight: FontWeight::default(),
            italic: false,
            size: Self::DEFAULT_SIZE,
            color: Rgba8::BLACK,
            underline: false,
            strike: false,
        }
    }
}

/// Fields a span overrides. `None` keeps the value from the base style or an earlier span.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SpanStyle {
    /// Family override.
    pub family: Option<FontFamily>,
    /// Weight override.
    pub weight: Option<FontWeight>,
    /// Italic override.
    pub italic: Option<bool>,
    /// Size override.
    pub size: Option<f32>,
    /// Colour override.
    pub color: Option<Rgba8>,
    /// Underline override.
    pub underline: Option<bool>,
    /// Strikethrough override.
    pub strike: Option<bool>,
}

impl SpanStyle {
    /// A span that only changes the colour (the old app's only per-character style).
    pub fn color(color: Rgba8) -> Self {
        Self { color: Some(color), ..Self::default() }
    }

    /// The style of a run of an edited text: an optional colour, and the heavy and italic faces when asked for.
    pub fn run(color: Option<Rgba8>, bold: bool, italic: bool) -> Self {
        Self { color, weight: bold.then_some(FontWeight::Heavy), italic: italic.then_some(true), ..Self::default() }
    }
}

/// A styled byte range of the text. Later spans win over earlier ones where they overlap.
/// Ranges that are empty, out of bounds or not on `char` boundaries are ignored.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    /// Byte range into [`TextSpec::text`].
    pub range: Range<usize>,
    /// What the span overrides.
    pub style: SpanStyle,
}

impl Span {
    /// Turn the old per-character colour list into spans. `colors[i]` is the colour of the i-th
    /// `char`; `None` and missing entries keep the base colour. Neighbouring chars with the same
    /// colour share one span.
    pub fn from_char_colors(text: &str, colors: &[Option<Rgba8>]) -> Vec<Span> {
        let mut out: Vec<Span> = Vec::new();
        for (i, (byte, ch)) in text.char_indices().enumerate() {
            let Some(Some(color)) = colors.get(i) else { continue };
            let end = byte + ch.len_utf8();
            match out.last_mut() {
                Some(last) if last.range.end == byte && last.style.color == Some(*color) => last.range.end = end,
                _ => out.push(Span { range: byte..end, style: SpanStyle::color(*color) }),
            }
        }
        out
    }
}

/// Whether `$...$` in the text is typeset as inline math.
///
/// The rules for what counts as a formula are in [`crate::inline`] (Pandoc's: the opening `$` is followed by a
/// non-space, the closing `$` is preceded by one and not followed by a digit; `\$` is a literal dollar).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum InlineMath {
    /// The text is plain: every character is drawn as it is, `\$` included.
    #[default]
    Off,
    /// Matched `$...$` pairs are formulas and `\$` is a dollar sign.
    Auto,
}

/// Everything that determines a text layout.
#[derive(Clone, Debug, PartialEq)]
pub struct TextSpec {
    /// The text. `\n` ends a paragraph.
    pub text: String,
    /// Base style.
    pub style: TextStyle,
    /// Per-range overrides.
    pub spans: Vec<Span>,
    /// Alignment inside the box. With [`Wrap::None`] the box is as wide as the longest line.
    pub align: Align,
    /// Wrapping.
    pub wrap: Wrap,
    /// Line height.
    pub line_height: LineHeight,
    /// Whether `$...$` is typeset as math.
    pub math: InlineMath,
    /// The formula whose source is shown instead of its typeset form: the one with a byte index of the text strictly
    /// inside its `$...$`. An editor sets this to the caret index while the caret is inside a formula (see
    /// [`crate::TextLayout::reveal_index`]); `None` typesets every formula.
    pub reveal: Option<usize>,
}

impl TextSpec {
    /// Plain text in the default style, no wrapping.
    pub fn new(text: impl Into<String>) -> Self {
        Self::styled(text, TextStyle::default())
    }

    /// Plain text with one style, no wrapping.
    pub fn styled(text: impl Into<String>, style: TextStyle) -> Self {
        Self {
            text: text.into(),
            style,
            spans: Vec::new(),
            align: Align::Left,
            wrap: Wrap::None,
            line_height: LineHeight::Normal,
            math: InlineMath::Off,
            reveal: None,
        }
    }

    /// Typeset `$...$` as math.
    pub fn with_inline_math(mut self, math: InlineMath) -> Self {
        self.math = math;
        self
    }

    /// Wrap at `width`.
    pub fn wrapped(mut self, width: f32) -> Self {
        self.wrap = Wrap::Width(width);
        self
    }

    /// Set the alignment.
    pub fn aligned(mut self, align: Align) -> Self {
        self.align = align;
        self
    }
}
