//! The real measure: `drafftink-typeset` behind the three traits.

use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use drafftink_edit::text_bind::TextMeasure as EditMeasure;
use drafftink_io::svg::MathSvg;
use drafftink_io::typeset_measure::math_layout_with;
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

impl EditMeasure for TypesetMeasure {
    fn line_width(&self, line: &str, font: &scene::TextStyle) -> f64 {
        self.text_size(line, font.family, font.weight, font.size, font.line_height).0
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

    fn math_size(&self, latex: &str, size: f64) -> Option<MathBox> {
        TypesetterMeasure(&self.ts).math_size(latex, size)
    }
}

impl MathSvg for TypesetMeasure {
    fn math_svg(&self, latex: &str, size: f64) -> Option<String> {
        TypesetterMeasure(&self.ts).math_svg(latex, size)
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
