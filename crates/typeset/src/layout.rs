//! Text layout result and the code that builds it from a [`TextSpec`].

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::ops::{Range, RangeInclusive};
use std::sync::Arc;

use kurbo::{Point, Rect, Size};
use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily as PFamily, FontFamilyName, FontStyle, FontWeight as PWeight,
    LayoutContext, OverflowWrap, PositionedLayoutItem, StyleProperty,
};

use crate::fonts::{self, FALLBACK_FAMILY};
use crate::outline::ColouredPath;
use crate::style::{Align, FontFamily, FontWeight, LineHeight, Rgba8, Span, SpanStyle, TextSpec, TextStyle, Wrap};

/// Longest text laid out, in bytes. Longer text is cut at a `char` boundary and the layout is
/// marked [`TextLayout::truncated`].
pub const MAX_TEXT_BYTES: usize = 1 << 20;
/// Most spans honoured per layout. Later spans are ignored.
pub const MAX_SPANS: usize = 4096;
/// Smallest and largest font size, in layout units.
pub const FONT_SIZE_RANGE: RangeInclusive<f32> = 1.0..=2048.0;
/// Smallest wrap width.
pub const MIN_WRAP_WIDTH: f32 = 1.0;

/// The brush Parley carries: an index into [`TextLayout::styles`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct StyleId(pub u32);

pub(crate) type PLayout = parley::Layout<StyleId>;

/// A fully resolved style: the base style with the spans that cover a range applied.
#[derive(Clone, Debug, PartialEq)]
pub struct RunStyle {
    /// Family.
    pub family: FontFamily,
    /// Weight.
    pub weight: FontWeight,
    /// Italic.
    pub italic: bool,
    /// Font size.
    pub size: f32,
    /// Colour.
    pub color: Rgba8,
    /// Underline.
    pub underline: bool,
    /// Strikethrough.
    pub strike: bool,
}

/// One positioned glyph. Coordinates are in layout space: x to the right, y down, origin at the
/// top-left of the layout box. `y` is the glyph's baseline position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glyph {
    /// Glyph id in the run's font.
    pub id: u32,
    /// Horizontal position of the glyph origin.
    pub x: f32,
    /// Vertical position of the glyph origin (the baseline, plus any glyph offset).
    pub y: f32,
}

/// Glyphs of one font and one style on one line, ready for `draw_glyphs`.
#[derive(Clone, Debug)]
pub struct GlyphRun {
    /// The font file (a cheap handle).
    pub font: peniko::FontData,
    /// Font size in layout units.
    pub font_size: f32,
    /// Variation coordinates, normalised (empty for the static bundled fonts).
    pub normalized_coords: Vec<i16>,
    /// Horizontal shear for a synthetic italic: `tan(angle)`, `None` when the face is a real italic.
    pub skew: Option<f32>,
    /// A synthetic bold is wanted. The bundled faces never need it.
    pub embolden: bool,
    /// Positioned glyphs.
    pub glyphs: Vec<Glyph>,
    /// Index into [`TextLayout::styles`].
    pub style: u32,
    /// Fill colour (same as `styles[style].color`).
    pub color: Rgba8,
    /// Index into [`TextLayout::lines`].
    pub line: u32,
    /// Baseline of the line.
    pub baseline: f32,
    /// Horizontal start of the run.
    pub x: f32,
    /// Advance of the whole run.
    pub advance: f32,
}

/// What a decoration is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecorationKind {
    /// Underline.
    Underline,
    /// Strikethrough.
    Strike,
}

/// An underline or strikethrough, as a filled rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Decoration {
    /// Kind.
    pub kind: DecorationKind,
    /// The rectangle to fill, in layout space.
    pub rect: Rect,
    /// Colour.
    pub color: Rgba8,
}

/// Geometry of one line.
#[derive(Clone, Debug, PartialEq)]
pub struct LineInfo {
    /// Byte range of the text on the line (includes the trailing `\n`).
    pub text_range: Range<usize>,
    /// Baseline, measured from the top of the layout.
    pub baseline: f32,
    /// Top of the line box.
    pub top: f32,
    /// Bottom of the line box.
    pub bottom: f32,
    /// Distance from the line top to the baseline.
    pub ascent: f32,
    /// Distance from the baseline to the line bottom.
    pub descent: f32,
    /// Left edge of the content after alignment.
    pub x: f32,
    /// Width of the content, without hanging whitespace.
    pub width: f32,
}

impl LineInfo {
    /// The box of the line's content.
    pub fn rect(&self) -> Rect {
        Rect::new(f64::from(self.x), f64::from(self.top), f64::from(self.x + self.width), f64::from(self.bottom))
    }
}

/// The result of laying out a [`TextSpec`]. Immutable; the cache hands out `Arc<TextLayout>`.
///
/// All coordinates are relative to the top-left of the layout box. Callers place the box in the
/// world with a translation (and a rotation about its centre, if any).
pub struct TextLayout {
    /// Unique per layout built by one process. A changed layout has a new id, so a render
    /// fragment can be validated with it.
    pub id: u64,
    /// Content size: the longest line and the sum of the line heights. Bounds and hit tests use
    /// this, and it is measured here once, never during render.
    pub size: Size,
    /// Size of the box the text sits in: the wrap width when wrapping, otherwise equal to `size`.
    pub box_size: Size,
    /// Lines, top to bottom. Never empty: empty text has one empty line.
    pub lines: Vec<LineInfo>,
    /// Glyph runs in paint order.
    pub runs: Vec<GlyphRun>,
    /// Underlines and strikethroughs.
    pub decorations: Vec<Decoration>,
    /// Distinct resolved styles. `GlyphRun::style` indexes this; index 0 is the base style.
    pub styles: Vec<RunStyle>,
    /// Length of the laid-out text in bytes (all caret indices are `<=` this).
    pub text_len: usize,
    /// The input was longer than [`MAX_TEXT_BYTES`] or had more than [`MAX_SPANS`] spans.
    pub truncated: bool,
    pub(crate) layout: PLayout,
}

impl std::fmt::Debug for TextLayout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextLayout")
            .field("id", &self.id)
            .field("size", &self.size)
            .field("lines", &self.lines.len())
            .field("runs", &self.runs.len())
            .finish_non_exhaustive()
    }
}

impl TextLayout {
    /// The bounds of the content, `(0, 0)` to `size`.
    pub fn bounds(&self) -> Rect {
        Rect::from_origin_size(Point::ORIGIN, self.size)
    }

    /// Baseline of the first line, from the top of the box.
    pub fn first_baseline(&self) -> f32 {
        self.lines.first().map_or(0.0, |l| l.baseline)
    }

    /// Total number of glyphs.
    pub fn glyph_count(&self) -> usize {
        self.runs.iter().map(|r| r.glyphs.len()).sum()
    }

    /// Rough heap size, for the cache budget.
    pub fn weight(&self) -> usize {
        let glyphs = self.glyph_count() * std::mem::size_of::<Glyph>();
        glyphs
            + self.runs.len() * std::mem::size_of::<GlyphRun>()
            + self.lines.len() * std::mem::size_of::<LineInfo>()
            + self.text_len * 24
            + 512
    }

    /// Whether `p` is on the text: inside the box of one of the lines, grown by `tolerance`.
    /// This is tighter than the bounds for ragged or centred text.
    pub fn hit_test(&self, p: Point, tolerance: f64) -> bool {
        self.lines.iter().any(|l| l.rect().inflate(tolerance, tolerance).contains(p))
    }

    /// The glyph outlines and decorations as filled paths, grouped by colour in paint order.
    /// Used by exports (SVG, PNG without a glyph renderer) and tests. Glyphs the font cannot
    /// outline are skipped. Variable-font coordinates are ignored (the bundled fonts are static).
    pub fn outline(&self) -> Vec<ColouredPath> {
        crate::outline::text_outline(self)
    }
}

// ----------------------------------------------------------------------------------------------
// Building

pub(crate) struct Engine {
    pub font_cx: FontContext,
    pub layout_cx: LayoutContext<StyleId>,
}

impl Engine {
    pub fn new() -> Self {
        let mut font_cx = FontContext::new();
        for bytes in fonts::bundled() {
            font_cx.collection.register_fonts(peniko::Blob::new(Arc::new(bytes)), None);
        }
        Self { font_cx, layout_cx: LayoutContext::new() }
    }
}

/// The spec after sanitising: bounded sizes, valid spans, flattened styles.
struct Clean<'a> {
    text: &'a str,
    styles: Vec<RunStyle>,
    /// Styled segments `(byte range, style index)` whose style is not the base style.
    segments: Vec<(Range<usize>, u32)>,
    wrap: Option<f32>,
    truncated: bool,
}

fn clamp_size(s: f32) -> f32 {
    if s.is_finite() { s.clamp(*FONT_SIZE_RANGE.start(), *FONT_SIZE_RANGE.end()) } else { TextStyle::DEFAULT_SIZE }
}

fn cut_at_boundary(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    text.get(..text.floor_char_boundary(max)).unwrap_or_default()
}

pub(crate) fn sanitize_wrap(w: Wrap) -> Option<f32> {
    match w {
        Wrap::None => None,
        Wrap::Width(w) if w.is_finite() => Some(w.max(MIN_WRAP_WIDTH)),
        Wrap::Width(_) => None,
    }
}

fn clean(spec: &TextSpec) -> Clean<'_> {
    let text = cut_at_boundary(&spec.text, MAX_TEXT_BYTES);
    let mut truncated = text.len() != spec.text.len();
    let base = RunStyle {
        family: spec.style.family.clone(),
        weight: spec.style.weight,
        italic: spec.style.italic,
        size: clamp_size(spec.style.size),
        color: spec.style.color,
        underline: spec.style.underline,
        strike: spec.style.strike,
    };
    let spans = usable_spans(spec, text, &mut truncated);
    let (styles, segments) = flatten_spans(&base, &spans, text.len());
    Clean { text, styles, segments, wrap: sanitize_wrap(spec.wrap), truncated }
}

/// The spans that can be applied to `text`: not empty, inside it and on `char` boundaries, at most [`MAX_SPANS`]
/// of them (`truncated` is set when more were given).
fn usable_spans<'a>(spec: &'a TextSpec, text: &str, truncated: &mut bool) -> Vec<&'a crate::style::Span> {
    let mut spans = Vec::new();
    for s in &spec.spans {
        if spans.len() >= MAX_SPANS {
            *truncated = true;
            break;
        }
        let r = &s.range;
        if r.start < r.end && r.end <= text.len() && text.is_char_boundary(r.start) && text.is_char_boundary(r.end) {
            spans.push(s);
        }
    }
    spans
}

/// `base` with the fields `o` overrides.
fn overridden(base: &RunStyle, o: &SpanStyle) -> RunStyle {
    let mut st = base.clone();
    if let Some(v) = &o.family {
        st.family = v.clone();
    }
    st.weight = o.weight.unwrap_or(st.weight);
    st.italic = o.italic.unwrap_or(st.italic);
    st.size = o.size.map_or(st.size, clamp_size);
    st.color = o.color.unwrap_or(st.color);
    st.underline = o.underline.unwrap_or(st.underline);
    st.strike = o.strike.unwrap_or(st.strike);
    st
}

/// Turns overlapping spans into the table of distinct styles (index 0 is `base`) and the byte ranges that use a style
/// other than the base. Later spans win where they overlap.
fn flatten_spans(base: &RunStyle, spans: &[&Span], n: usize) -> (Vec<RunStyle>, Vec<(Range<usize>, u32)>) {
    let mut styles = vec![base.clone()];
    let mut segments: Vec<(Range<usize>, u32)> = Vec::new();
    if spans.is_empty() {
        return (styles, segments);
    }
    // Sweep over the span boundaries, keeping the spans that cover the current segment in input order.
    let mut points: BTreeSet<usize> = BTreeSet::from([0, n]);
    for s in spans {
        points.insert(s.range.start);
        points.insert(s.range.end);
    }
    let points: Vec<usize> = points.into_iter().collect();
    let mut by_start: Vec<usize> = (0..spans.len()).collect();
    by_start.sort_by_key(|&k| spans.get(k).map_or(0, |s| s.range.start));
    let mut next_start = 0usize;
    let mut active: BTreeSet<usize> = BTreeSet::new();
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        while let Some(&k) = by_start.get(next_start) {
            if spans.get(k).is_some_and(|s| s.range.start <= a) {
                active.insert(k);
                next_start += 1;
            } else {
                break;
            }
        }
        active.retain(|&k| spans.get(k).is_some_and(|s| s.range.end > a));
        let st = active.iter().filter_map(|&k| spans.get(k)).fold(base.clone(), |st, s| overridden(&st, &s.style));
        let idx = match styles.iter().position(|x| *x == st) {
            Some(i) => i,
            None => {
                styles.push(st);
                styles.len() - 1
            }
        };
        if idx != 0 {
            let idx = u32::try_from(idx).unwrap_or(u32::MAX);
            match segments.last_mut() {
                Some((r, i)) if *i == idx && r.end == a => r.end = b,
                _ => segments.push((a..b, idx)),
            }
        }
    }
    (styles, segments)
}

fn line_height(lh: LineHeight) -> parley::LineHeight {
    let sane = |v: f32| if v.is_finite() { v.clamp(0.1, 20.0) } else { 1.0 };
    match lh {
        LineHeight::Normal => parley::LineHeight::MetricsRelative(1.0),
        LineHeight::Metrics(v) => parley::LineHeight::MetricsRelative(sane(v)),
        LineHeight::FontSize(v) => parley::LineHeight::FontSizeRelative(sane(v)),
    }
}

fn pweight(style: &RunStyle) -> f32 {
    let face = fonts::face_for(&style.family, style.weight, style.italic);
    match (&style.family, style.weight) {
        (FontFamily::Custom(_), FontWeight::Light) => 300.0,
        _ => face.weight,
    }
}

/// The Parley properties that carry `style`.
fn properties(style: &RunStyle, id: u32) -> [StyleProperty<'static, StyleId>; 5] {
    let face = fonts::face_for(&style.family, style.weight, style.italic);
    let mut names: Vec<FontFamilyName<'static>> = vec![FontFamilyName::Named(Cow::Owned(face.name.clone()))];
    if fonts::HAS_NOTO_SANS && face.name != FALLBACK_FAMILY {
        names.push(FontFamilyName::Named(Cow::Borrowed(FALLBACK_FAMILY)));
    }
    [
        StyleProperty::FontFamily(PFamily::List(Cow::Owned(names))),
        StyleProperty::FontSize(style.size),
        StyleProperty::FontWeight(PWeight::new(pweight(style))),
        StyleProperty::FontStyle(if face.italic { FontStyle::Italic } else { FontStyle::Normal }),
        StyleProperty::Brush(StyleId(id)),
    ]
}

pub(crate) fn build(engine: &mut Engine, spec: &TextSpec, id: u64) -> TextLayout {
    let c = clean(spec);
    let mut builder = engine.layout_cx.ranged_builder(&mut engine.font_cx, c.text, 1.0, false);
    let base = c.styles.first().cloned().unwrap_or(RunStyle {
        family: FontFamily::GelPen,
        weight: FontWeight::Regular,
        italic: false,
        size: 20.0,
        color: Rgba8::BLACK,
        underline: false,
        strike: false,
    });
    for p in properties(&base, 0) {
        builder.push_default(p);
    }
    builder.push_default(StyleProperty::LineHeight(line_height(spec.line_height)));
    builder.push_default(StyleProperty::OverflowWrap(OverflowWrap::Anywhere));
    for (range, idx) in &c.segments {
        if let Some(st) = c.styles.get(*idx as usize) {
            for p in properties(st, *idx) {
                builder.push(p, range.clone());
            }
        }
    }
    let mut layout: PLayout = builder.build(c.text);
    layout.break_all_lines(c.wrap);
    let alignment = match spec.align {
        Align::Left => Alignment::Left,
        Align::Center => Alignment::Center,
        Align::Right => Alignment::Right,
    };
    layout.align(alignment, AlignmentOptions::default());
    finish(layout, &c, id, base.size)
}

fn line_info(line: &parley::Line<'_, StyleId>) -> LineInfo {
    let m = line.metrics();
    LineInfo {
        text_range: line.text_range(),
        baseline: m.baseline,
        top: m.block_min_coord,
        bottom: m.block_max_coord,
        ascent: m.baseline - m.block_min_coord,
        descent: m.block_max_coord - m.baseline,
        x: m.offset,
        width: (m.advance - m.hanging_advance).max(0.0),
    }
}

/// The underline and strikethrough of a run, if its style asks for them.
fn decorations_of(gr: &parley::GlyphRun<'_, StyleId>, style: &RunStyle, color: Rgba8) -> Vec<Decoration> {
    let fm = gr.run().font_metrics();
    let (x0, base_y) = (gr.offset(), gr.baseline());
    let x1 = x0 + gr.advance();
    let bar = |kind, top: f32, bottom: f32| Decoration {
        kind,
        rect: Rect::new(f64::from(x0), f64::from(top), f64::from(x1), f64::from(bottom)),
        color,
    };
    let mut out = Vec::new();
    if style.underline {
        let (off, size) = (fm.underline_offset, fm.underline_size.max(0.5));
        out.push(bar(DecorationKind::Underline, base_y - off, base_y - off + size));
    }
    if style.strike {
        let (off, size) = (fm.strikethrough_offset, fm.strikethrough_size.max(0.5));
        out.push(bar(DecorationKind::Strike, base_y - off - size / 2.0, base_y - off + size / 2.0));
    }
    out
}

fn glyph_run(
    gr: &parley::GlyphRun<'_, StyleId>,
    glyphs: Vec<Glyph>,
    style_idx: u32,
    color: Rgba8,
    line: usize,
) -> GlyphRun {
    let run = gr.run();
    let syn = run.synthesis();
    GlyphRun {
        font: run.font().clone(),
        font_size: run.font_size(),
        normalized_coords: run.normalized_coords().iter().map(|c| c.to_bits()).collect(),
        skew: syn.skew().map(|deg| deg.to_radians().tan()),
        embolden: syn.embolden(),
        glyphs,
        style: style_idx,
        color,
        line: u32::try_from(line).unwrap_or(u32::MAX),
        baseline: gr.baseline(),
        x: gr.offset(),
        advance: gr.advance(),
    }
}

fn finish(layout: PLayout, c: &Clean<'_>, id: u64, base_size: f32) -> TextLayout {
    let mut lines = Vec::new();
    let mut runs: Vec<GlyphRun> = Vec::new();
    let mut decorations = Vec::new();
    for (li, line) in layout.lines().enumerate() {
        lines.push(line_info(&line));
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(gr) = item else { continue };
            let style_idx = gr.style().brush.0;
            let style = c.styles.get(style_idx as usize);
            let color = style.map_or(Rgba8::BLACK, |s| s.color);
            if let Some(s) = style {
                decorations.extend(decorations_of(&gr, s, color));
            }
            let glyphs: Vec<Glyph> = gr.positioned_glyphs().map(|g| Glyph { id: g.id, x: g.x, y: g.y }).collect();
            if !glyphs.is_empty() {
                runs.push(glyph_run(&gr, glyphs, style_idx, color, li));
            }
        }
    }
    if c.text.is_empty() {
        // Parley lays out a lone space for empty text so that line metrics exist. Keep its
        // height (a caret needs it) but not its width or glyphs.
        runs.clear();
        decorations.clear();
        for l in &mut lines {
            l.text_range = 0..0;
            l.width = 0.0;
        }
    }
    if lines.is_empty() {
        // Empty text: one empty line so a caret and bounds exist.
        let h = base_size * 1.2;
        lines.push(LineInfo {
            text_range: 0..0,
            baseline: base_size,
            top: 0.0,
            bottom: h,
            ascent: base_size,
            descent: h - base_size,
            x: 0.0,
            width: 0.0,
        });
    }
    let height = lines.last().map_or(0.0, |l| l.bottom).max(layout.height());
    let width = if c.text.is_empty() {
        0.0
    } else if c.wrap.is_some() {
        layout.width()
    } else {
        layout.full_width()
    };
    let size = Size::new(f64::from(width.max(0.0)), f64::from(height));
    let box_w = c.wrap.map_or(size.width, |w| f64::from(w).max(size.width));
    TextLayout {
        id,
        size,
        box_size: Size::new(box_w, size.height),
        lines,
        runs,
        decorations,
        styles: c.styles.clone(),
        text_len: c.text.len(),
        truncated: c.truncated,
        layout,
    }
}
