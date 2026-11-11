//! The real measure: `drafftink-typeset` behind the three traits.

use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use drafftink_edit::text_bind::{TextMeasure as EditMeasure, wrap_text};
use drafftink_io::svg::MathSvg;
use drafftink_io::typeset_measure::{font_wrapped_lines, math_layout_with};
use drafftink_io::{MathBox, TextMeasure as IoMeasure, TypesetterHandle, TypesetterMeasure};
use drafftink_scene as scene;
use drafftink_typeset::{MathError, MathLayout, TypesetStats, Typesetter};

use super::Measure;

/// A typesetter built on first use and shared by everyone who holds a handle (the session's measure, the renderer).
#[derive(Default)]
pub struct LazyTypesetter {
    cell: OnceLock<Mutex<Typesetter>>,
}

/// A shared handle on the session's typesetter.
pub type SharedTypesetter = Arc<LazyTypesetter>;

impl TypesetterHandle for LazyTypesetter {
    fn with_typesetter<R>(&self, f: impl FnOnce(&mut Typesetter) -> R) -> R {
        self.with(f)
    }
}

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
}

/// Whether `text` is laid out differently from plain text in `font`: it has an inline formula or an escaped dollar.
fn has_inline_math(text: &str, font: &scene::TextStyle) -> bool {
    font.inline_math == scene::InlineMath::Auto && drafftink_typeset::has_inline_math(text)
}

impl EditMeasure for TypesetMeasure {
    fn line_width(&self, line: &str, font: &scene::TextStyle) -> f64 {
        self.text_size_font(line, font).0
    }

    /// A line with a formula is as tall as the typesetter makes it.
    fn line_height_of(&self, line: &str, font: &scene::TextStyle) -> f64 {
        if has_inline_math(line, font) { self.text_size_font(line, font).1 } else { self.line_height(font) }
    }

    /// Text with a formula wraps as the typesetter wraps it, which never splits a formula. The greedy word wrap of
    /// the default would cut `$a + b$` at its spaces.
    fn wrap(&self, text: &str, font: &scene::TextStyle, max_width: f64) -> Vec<String> {
        if has_inline_math(text, font) {
            self.ts.with(|t| font_wrapped_lines(t, text, font, max_width))
        } else {
            wrap_text(self, text, font, max_width)
        }
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
        TypesetterMeasure(&self.ts).text_size(text, family, weight, size, line_height)
    }

    fn text_size_font(&self, text: &str, font: &scene::TextStyle) -> (f64, f64) {
        TypesetterMeasure(&self.ts).text_size_font(text, font)
    }

    fn math_size(&self, latex: &str, size: f64) -> Option<MathBox> {
        TypesetterMeasure(&self.ts).math_size(latex, size)
    }
}

impl MathSvg for TypesetMeasure {
    fn math_svg(&self, latex: &str, size: f64) -> Option<String> {
        TypesetterMeasure(&self.ts).math_svg(latex, size)
    }

    fn text_svg(&self, text: &scene::TextData, box_width: f64) -> Option<String> {
        TypesetterMeasure(&self.ts).text_svg(text, box_width)
    }
}

impl Measure for TypesetMeasure {
    fn math_layout(&self, latex: &str, size: f64) -> Result<Arc<MathLayout>, MathError> {
        self.ts.with(|t| math_layout_with(t, latex, size))
    }

    fn stats(&self) -> Option<TypesetStats> {
        Some(self.ts.with(|t| t.stats()))
    }
}
