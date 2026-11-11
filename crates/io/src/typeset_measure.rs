//! The real text engine behind [`TextMeasure`] and [`MathSvg`](crate::svg::MathSvg).
//!
//! `io` keeps its formats pure by asking a trait for sizes. This module is the one adapter over
//! [`Typesetter`] that the engine and the stage both use, so a text measured for an SVG export has the size the
//! editor gave it. The typesetter itself belongs to the caller, who lends it through [`TypesetterHandle`]: the
//! engine shares one behind a lock, the stage owns one.

use std::cell::RefCell;
use std::fmt::Write as _;
use std::sync::Arc;

use drafftink_scene as scene;
use drafftink_typeset::{
    ColouredPath, FONT_SIZE_RANGE, FontFamily, FontWeight, LineHeight, MATH_SIZE_RANGE, MathError, MathLayout,
    MathSpec, Rgba8, TextSpec, TextStyle, Typesetter,
};

use crate::measure::{MathBox, TextMeasure};
use crate::svg::MathSvg;

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

    fn math_size(&self, latex: &str, size: f64) -> Option<MathBox> {
        let l = self.0.with_typesetter(|ts| math_layout_with(ts, latex, size)).ok()?;
        Some(MathBox { width: clean(l.width), height: clean(l.height), depth: clean(-l.depth) })
    }
}

impl<H: TypesetterHandle> MathSvg for TypesetterMeasure<H> {
    fn math_svg(&self, latex: &str, size: f64) -> Option<String> {
        let layout = self.0.with_typesetter(|ts| math_layout_with(ts, latex, size)).ok()?;
        let mut out = String::new();
        for ColouredPath { color, path } in layout.outline(SENTINEL) {
            let d = path.to_svg();
            if color == SENTINEL {
                let _ = write!(out, r#"<path d="{d}" fill="currentColor"/>"#);
            } else {
                // A colour the formula sets itself with `\color{..}` stays.
                let (r, g, b) = (color.r, color.g, color.b);
                let _ = write!(out, r##"<path d="{d}" fill="#{r:02x}{g:02x}{b:02x}""##);
                if color.a != 255 {
                    let _ = write!(out, r#" fill-opacity="{:.3}""#, f64::from(color.a) / 255.0);
                }
                out.push_str("/>");
            }
        }
        Some(out)
    }
}
