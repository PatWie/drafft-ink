//! The real text engine behind [`TextMeasure`] and [`MathSvg`](crate::svg::MathSvg).
//!
//! `io` keeps its formats pure by asking a trait for sizes. This module is the one adapter over
//! [`Typesetter`] that the engine and the stage both use, so a text measured for an SVG export has the size the
//! editor gave it. The typesetter itself belongs to the caller, who lends it through [`TypesetterHandle`]: the
//! engine shares one behind a lock, the stage owns one.

use std::cell::RefCell;
use std::fmt::Write as _;
use std::sync::{Arc, OnceLock};

use drafftink_scene as scene;
use drafftink_typeset::{
    Align, ColouredPath, FONT_SIZE_RANGE, FontFamily, FontWeight, InlineMath, LineHeight, MATH_SIZE_RANGE, MathError,
    MathLayout, MathSpec, Rgba8, Span, SpanStyle, TextSpec, TextStyle, Typesetter, Wrap, font_file,
};

use crate::measure::{MathBox, TextMeasure};
use crate::svg::{FontFace, MathSvg};

/// Something that can lend a [`Typesetter`] for the length of one call.
pub trait TypesetterHandle {
    /// Runs `f` on the typesetter.
    fn with_typesetter<R>(&self, f: impl FnOnce(&mut Typesetter) -> R) -> R;
}

impl<H: TypesetterHandle + ?Sized> TypesetterHandle for &H {
    fn with_typesetter<R>(&self, f: impl FnOnce(&mut Typesetter) -> R) -> R {
        (**self).with_typesetter(f)
    }
}

impl<H: TypesetterHandle + ?Sized> TypesetterHandle for Arc<H> {
    fn with_typesetter<R>(&self, f: impl FnOnce(&mut Typesetter) -> R) -> R {
        (**self).with_typesetter(f)
    }
}

/// A typesetter borrowed for a while (a stage that owns its typesetter lends it for one export).
impl TypesetterHandle for RefCell<&mut Typesetter> {
    fn with_typesetter<R>(&self, f: impl FnOnce(&mut Typesetter) -> R) -> R {
        f(&mut self.borrow_mut())
    }
}

/// [`TextMeasure`] and [`MathSvg`] over the typesetter that `H` lends.
#[derive(Debug, Clone)]
pub struct TypesetterMeasure<H>(pub H);

/// Used when the stored line height is not a usable number.
const DEFAULT_LINE_HEIGHT: f64 = 1.25;
/// The colour handed to the outline for "the shape colour". The SVG then says `currentColor` for it.
const SENTINEL: Rgba8 = Rgba8::new(1, 2, 3, 4);

fn clean(v: f64) -> f64 {
    if v.is_finite() { v.max(0.0) } else { 0.0 }
}

fn family(f: scene::FontFamily) -> FontFamily {
    match f {
        scene::FontFamily::GelPen => FontFamily::GelPen,
        scene::FontFamily::NotoSans => FontFamily::NotoSans,
        scene::FontFamily::GelPenSerif => FontFamily::GelPenSerif,
        scene::FontFamily::VanillaExtract => FontFamily::VanillaExtract,
    }
}

fn weight(w: scene::FontWeight) -> FontWeight {
    match w {
        scene::FontWeight::Light => FontWeight::Light,
        scene::FontWeight::Regular => FontWeight::Regular,
        scene::FontWeight::Heavy => FontWeight::Heavy,
    }
}

fn inline_math(m: scene::InlineMath) -> InlineMath {
    match m {
        scene::InlineMath::Auto => InlineMath::Auto,
        scene::InlineMath::Off => InlineMath::Off,
    }
}

/// The font files this build bundles, as faces for [`SvgOptions::fonts`](crate::svg::SvgOptions::fonts): GelPen and
/// GelPen Serif in three weights, Vanilla Extract, and Noto Sans (regular and bold) where it is built in (not on
/// the web, which loads it on demand). The bytes are the typesetter's statics; the list is built once.
pub fn bundled_svg_fonts() -> Vec<FontFace> {
    static FACES: OnceLock<Vec<FontFace>> = OnceLock::new();
    FACES
        .get_or_init(|| {
            use scene::{FontFamily as F, FontWeight as W};
            let wanted = [
                (F::GelPen, W::Light),
                (F::GelPen, W::Regular),
                (F::GelPen, W::Heavy),
                (F::GelPenSerif, W::Light),
                (F::GelPenSerif, W::Regular),
                (F::GelPenSerif, W::Heavy),
                // One face: the other weights use it (the canvas draws it the same way).
                (F::VanillaExtract, W::Regular),
                (F::NotoSans, W::Regular),
                (F::NotoSans, W::Heavy),
            ];
            wanted
                .into_iter()
                .filter_map(|(f, weight)| {
                    let data = font_file(&family(f), self::weight(weight))?;
                    Some(FontFace { family: f, weight, mime: "font/ttf".to_owned(), data: Arc::from(data) })
                })
                .collect()
        })
        .clone()
}

/// The layout input for plain text in a scene font: no wrapping, line height as a multiple of the size.
pub fn plain_text_spec(
    text: &str,
    font_family: scene::FontFamily,
    font_weight: scene::FontWeight,
    size: f64,
    line_height: f64,
) -> TextSpec {
    let size = if size.is_finite() {
        (size as f32).clamp(*FONT_SIZE_RANGE.start(), *FONT_SIZE_RANGE.end())
    } else {
        TextStyle::DEFAULT_SIZE
    };
    let lh = if line_height.is_finite() && line_height > 0.0 { line_height } else { DEFAULT_LINE_HEIGHT };
    let mut spec =
        TextSpec::styled(text, TextStyle::from_legacy(family(font_family), weight(font_weight), size, Rgba8::BLACK));
    spec.line_height = LineHeight::FontSize(lh.clamp(0.1, 10.0) as f32);
    spec
}

/// The size of plain text in a scene font, never negative or NaN.
pub fn text_size_with(
    ts: &mut Typesetter,
    text: &str,
    font_family: scene::FontFamily,
    font_weight: scene::FontWeight,
    size: f64,
    line_height: f64,
) -> (f64, f64) {
    let s = ts.measure(&plain_text_spec(text, font_family, font_weight, size, line_height));
    (clean(s.width), clean(s.height))
}

/// The layout input for `text` in the font settings of a text element, wrapped at `wrap` when given. Inline math is
/// on or off as the font says.
pub fn font_text_spec(text: &str, font: &scene::TextStyle, wrap: Option<f64>) -> TextSpec {
    let mut spec = plain_text_spec(text, font.family, font.weight, font.size, font.line_height);
    spec.math = inline_math(font.inline_math);
    if let Some(w) = wrap.filter(|w| w.is_finite() && *w >= 0.0) {
        spec.wrap = Wrap::Width(w as f32);
    }
    spec
}

/// The size of `text` in a text element's font settings, never negative or NaN.
pub fn font_text_size_with(ts: &mut Typesetter, text: &str, font: &scene::TextStyle) -> (f64, f64) {
    let s = ts.measure(&font_text_spec(text, font, None));
    (clean(s.width), clean(s.height))
}

/// `text` wrapped at `max_width` as the renderer wraps it: the source text of each line, without the line break.
/// A non-finite or negative `max_width` keeps only the hard breaks.
pub fn font_wrapped_lines(ts: &mut Typesetter, text: &str, font: &scene::TextStyle, max_width: f64) -> Vec<String> {
    let layout = ts.text(&font_text_spec(text, font, Some(max_width)));
    layout
        .lines
        .iter()
        .map(|l| {
            text.get(l.text_range.clone()).unwrap_or_default().trim_end_matches(['\n', '\r']).trim_end().to_owned()
        })
        .collect()
}

/// The layout input for a text element as drawn: colours, alignment and the wrap width of a fixed-width box. `stroke` is
/// the colour of the text.
pub fn element_text_spec(t: &scene::TextData, stroke: Rgba8, box_width: f64) -> TextSpec {
    let mut spec = font_text_spec(
        &t.content,
        &t.font,
        (!t.auto_resize || (t.container_id.is_some() && box_width > 0.0)).then_some(box_width),
    );
    spec.style.color = stroke;
    spec.spans = run_spans(t);
    spec.align = match t.font.align {
        scene::TextAlign::Left => Align::Left,
        scene::TextAlign::Center => Align::Center,
        scene::TextAlign::Right => Align::Right,
    };
    spec
}

/// The spans for the style runs of a text: colour, bold and italic, over byte ranges.
fn run_spans(t: &scene::TextData) -> Vec<Span> {
    if t.runs.is_empty() {
        return Vec::new();
    }
    let mut at: Vec<usize> = t.content.char_indices().map(|(i, _)| i).collect();
    at.push(t.content.len());
    let byte = |char_index: u32| at.get(char_index as usize).copied().unwrap_or(t.content.len());
    t.runs
        .iter()
        .filter(|run| !run.style().is_plain())
        .map(|run| Span {
            range: byte(run.start)..byte(run.end),
            style: SpanStyle::run(run.color.map(|c| Rgba8::new(c.r, c.g, c.b, c.a)), run.bold, run.italic),
        })
        .collect()
}

/// Lays out a formula at `size`, clamped to the sizes the typesetter accepts (a size that is not a number
/// counts as 20).
pub fn math_layout_with(ts: &mut Typesetter, latex: &str, size: f64) -> Result<Arc<MathLayout>, MathError> {
    let size = if size.is_finite() { size.clamp(*MATH_SIZE_RANGE.start(), *MATH_SIZE_RANGE.end()) } else { 20.0 };
    ts.math(&MathSpec::new(latex).with_size(size))
}

impl<H: TypesetterHandle> TextMeasure for TypesetterMeasure<H> {
    fn text_size(
        &self,
        text: &str,
        font_family: scene::FontFamily,
        font_weight: scene::FontWeight,
        size: f64,
        line_height: f64,
    ) -> (f64, f64) {
        self.0.with_typesetter(|ts| text_size_with(ts, text, font_family, font_weight, size, line_height))
    }

    fn text_size_font(&self, text: &str, font: &scene::TextStyle) -> (f64, f64) {
        self.0.with_typesetter(|ts| font_text_size_with(ts, text, font))
    }

    fn math_size(&self, latex: &str, size: f64) -> Option<MathBox> {
        let l = self.0.with_typesetter(|ts| math_layout_with(ts, latex, size)).ok()?;
        Some(MathBox { width: clean(l.width), height: clean(l.height), depth: clean(-l.depth) })
    }
}

/// Filled paths as SVG. `SENTINEL` is the shape colour (`currentColor`); any other colour stays.
fn outline_svg(paths: Vec<ColouredPath>) -> String {
    let mut out = String::new();
    for ColouredPath { color, path } in paths {
        let d = path.to_svg();
        if color == SENTINEL {
            let _ = write!(out, r#"<path d="{d}" fill="currentColor"/>"#);
        } else {
            // A colour the formula or the text sets itself stays.
            let (r, g, b) = (color.r, color.g, color.b);
            let _ = write!(out, r##"<path d="{d}" fill="#{r:02x}{g:02x}{b:02x}""##);
            if color.a != 255 {
                let _ = write!(out, r#" fill-opacity="{:.3}""#, f64::from(color.a) / 255.0);
            }
            out.push_str("/>");
        }
    }
    out
}

impl<H: TypesetterHandle> MathSvg for TypesetterMeasure<H> {
    fn math_svg(&self, latex: &str, size: f64) -> Option<String> {
        let layout = self.0.with_typesetter(|ts| math_layout_with(ts, latex, size)).ok()?;
        Some(outline_svg(layout.outline(SENTINEL)))
    }

    fn text_svg(&self, text: &scene::TextData, box_width: f64) -> Option<String> {
        let spec = element_text_spec(text, SENTINEL, box_width);
        let layout = self.0.with_typesetter(|ts| ts.text(&spec));
        Some(outline_svg(layout.outline()))
    }
}
