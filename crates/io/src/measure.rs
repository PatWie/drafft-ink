//! Text measurement, as a trait.
//!
//! `io` is pure and sits below the engine that owns the real text engine (`typeset`). Where an import needs the size
//! of a text or a formula (old documents did not store it; a Mermaid layout needs it), it asks a [`TextMeasure`].
//! The engine passes an adapter over `typeset`; tests use [`EstimateMeasure`], which reproduces the old app's
//! fallback estimate (`docs/formats.md` sections 1.6 and 7.3).

use drafftink_scene::{FontFamily, FontWeight};

/// The size of a laid-out formula in world units, relative to its baseline origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MathBox {
    /// Advance width.
    pub width: f64,
    /// Height above the baseline.
    pub height: f64,
    /// Depth below the baseline (0 or more).
    pub depth: f64,
}

/// Measures text and formulas.
pub trait TextMeasure {
    /// Size `(width, height)` of `text` (may contain `\n`) in the given font. Never negative or NaN.
    fn text_size(&self, text: &str, family: FontFamily, weight: FontWeight, size: f64, line_height: f64) -> (f64, f64);

    /// Size of a LaTeX formula, or `None` when it does not parse (the caller keeps a fallback box).
    fn math_size(&self, latex: &str, size: f64) -> Option<MathBox>;
}

/// The old app's fallback estimate: width is the longest line in bytes times the font size times a factor per
/// font and weight (at least 20), height is lines times size times 1.2 (the `line_height` argument is ignored, as
/// the old estimate had a fixed 1.2). Formulas: `bytes * size * 0.5` wide, `size` high, `0.3 * size` deep.
#[derive(Debug, Clone, Copy, Default)]
pub struct EstimateMeasure;

fn char_width_factor(family: FontFamily, weight: FontWeight) -> f64 {
    match (family, weight) {
        (FontFamily::GelPen, FontWeight::Light) => 0.50,
        (FontFamily::GelPen, FontWeight::Regular) => 0.55,
        (FontFamily::GelPen, FontWeight::Heavy) => 0.60,
        (FontFamily::NotoSans, FontWeight::Light) => 0.50,
        (FontFamily::NotoSans, FontWeight::Regular) => 0.52,
        (FontFamily::NotoSans, FontWeight::Heavy) => 0.55,
        (FontFamily::GelPenSerif, FontWeight::Light) => 0.55,
        (FontFamily::GelPenSerif, FontWeight::Regular) => 0.58,
        (FontFamily::GelPenSerif, FontWeight::Heavy) => 0.60,
        (FontFamily::VanillaExtract, _) => 0.50,
    }
}

fn clean(v: f64, fallback: f64) -> f64 {
    if v.is_finite() { v.max(0.0) } else { fallback }
}

impl TextMeasure for EstimateMeasure {
    fn text_size(
        &self,
        text: &str,
        family: FontFamily,
        weight: FontWeight,
        size: f64,
        _line_height: f64,
    ) -> (f64, f64) {
        let (size, line_height) = (clean(size, 0.0), 1.2);
        let longest = text.lines().map(str::len).max().unwrap_or(0) as f64;
        let lines = text.lines().count().max(1) + usize::from(text.ends_with('\n'));
        ((longest * size * char_width_factor(family, weight)).max(20.0), lines as f64 * size * line_height)
    }

    fn math_size(&self, latex: &str, size: f64) -> Option<MathBox> {
        let size = clean(size, 0.0);
        Some(MathBox { width: latex.len() as f64 * size * 0.5, height: size, depth: size * 0.3 })
    }
}
