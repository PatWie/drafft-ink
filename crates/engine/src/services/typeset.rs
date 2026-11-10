//! The real measure: `drafftink-typeset` behind the three traits.

use std::fmt::Write as _;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use drafftink_edit::text_bind::TextMeasure as EditMeasure;
use drafftink_io::svg::MathSvg;
use drafftink_io::{MathBox, TextMeasure as IoMeasure};
use drafftink_scene as scene;
use drafftink_typeset::{
    FONT_SIZE_RANGE, FontFamily, FontWeight, LineHeight, MathError, MathLayout, MathSpec, Rgba8, TextSpec, TextStyle,
    TypesetStats, Typesetter,
};

use super::Measure;

/// A typesetter built on first use and shared by everyone who holds a handle (the session's measure, the renderer).
#[derive(Default)]
pub struct LazyTypesetter {
    cell: OnceLock<Mutex<Typesetter>>,
}

/// A shared handle on the session's typesetter.
pub type SharedTypesetter = Arc<LazyTypesetter>;

impl LazyTypesetter {
    /// A handle that builds the typesetter on first use.
    pub fn shared() -> SharedTypesetter {
        Arc::new(Self::default())
    }

    /// Runs `f` on the typesetter, building it first if needed. A poisoned lock (a panic in an earlier call) is
    /// ignored: layouts are immutable values and the caches stay consistent.
    pub fn with<R>(&self, f: impl FnOnce(&mut Typesetter) -> R) -> R {
        let m = self.cell.get_or_init(|| Mutex::new(Typesetter::new()));
        let mut g: MutexGuard<'_, Typesetter> = m.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut g)
    }
}

/// `drafftink-typeset` as an [`EditMeasure`], an [`IoMeasure`] and a [`MathSvg`].
pub struct TypesetMeasure {
    ts: SharedTypesetter,
}

impl Default for TypesetMeasure {
    fn default() -> Self {
        Self::new()
    }
}

/// Used when the stored line height is not a usable number.
const DEFAULT_LINE_HEIGHT: f64 = 1.25;
/// The colour handed to the outline for "the shape colour". The SVG then says `currentColor` for it.
const SENTINEL: Rgba8 = Rgba8::new(1, 2, 3, 4);

impl TypesetMeasure {
    /// A measure over a new, lazily built typesetter.
    pub fn new() -> Self {
        Self { ts: LazyTypesetter::shared() }
    }

    /// A measure over an existing typesetter.
    pub fn sharing(ts: SharedTypesetter) -> Self {
        Self { ts }
    }

    /// The handle, to share with the renderer.
    pub fn handle(&self) -> SharedTypesetter {
        Arc::clone(&self.ts)
    }

    fn size_of(&self, spec: &TextSpec) -> (f64, f64) {
        let s = self.ts.with(|t| t.measure(spec));
        (clean(s.width), clean(s.height))
    }
}

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
fn spec_for(text: &str, f: scene::FontFamily, w: scene::FontWeight, size: f64, line_height: f64) -> TextSpec {
    let (lo, hi) = FONT_SIZE_RANGE;
    let size = if size.is_finite() { (size as f32).clamp(lo, hi) } else { TextStyle::DEFAULT_SIZE };
    let lh = if line_height.is_finite() && line_height > 0.0 { line_height } else { DEFAULT_LINE_HEIGHT };
    let mut spec = TextSpec::styled(text, TextStyle::from_legacy(family(f), weight(w), size, Rgba8::BLACK));
    spec.line_height = LineHeight::FontSize(lh.clamp(0.1, 10.0) as f32);
    spec
}

impl EditMeasure for TypesetMeasure {
    fn line_width(&self, line: &str, font: &scene::TextStyle) -> f64 {
        self.size_of(&spec_for(line, font.family, font.weight, font.size, font.line_height)).0
    }
}

impl IoMeasure for TypesetMeasure {
    fn text_size(
        &self,
        text: &str,
        family: scene::FontFamily,
        weight: scene::FontWeight,
        size: f64,
        line_height: f64,
    ) -> (f64, f64) {
        self.size_of(&spec_for(text, family, weight, size, line_height))
    }

    fn math_size(&self, latex: &str, size: f64) -> Option<MathBox> {
        let l = self.math_layout(latex, size).ok()?;
        Some(MathBox { width: clean(l.width), height: clean(l.height), depth: clean(-l.depth) })
    }
}

impl MathSvg for TypesetMeasure {
    fn math_svg(&self, latex: &str, size: f64) -> Option<String> {
        let layout = self.math_layout(latex, size).ok()?;
        let mut out = String::new();
        for (color, path) in layout.outline(SENTINEL) {
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

impl Measure for TypesetMeasure {
    fn math_layout(&self, latex: &str, size: f64) -> Result<Arc<MathLayout>, MathError> {
        let (lo, hi) = drafftink_typeset::MATH_SIZE_RANGE;
        let size = if size.is_finite() { size.clamp(lo, hi) } else { 20.0 };
        self.ts.with(|t| t.math(&MathSpec::new(latex).with_size(size)))
    }

    fn stats(&self) -> Option<TypesetStats> {
        Some(self.ts.with(|t| t.stats()))
    }
}
