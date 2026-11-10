//! Services the engine owns besides the document: the text and formula engine behind three measuring traits.
//!
//! Other crates below the engine need the size of a text or a formula but cannot depend on `typeset`:
//! `edit` asks a [`drafftink_edit::text_bind::TextMeasure`] (wrapping, bound text, container growth), `io`
//! asks a [`drafftink_io::TextMeasure`] (import of old documents, Mermaid layout) and draws formulas with a
//! [`drafftink_io::svg::MathSvg`] (SVG export). One [`Measure`] implements all three, so one text engine with
//! one set of caches answers every caller.
//!
//! * [`Services::new`] is the real thing: [`TypesetMeasure`] over a [`drafftink_typeset::Typesetter`] with its LRU
//!   caches. The typesetter is built on first use (it registers the bundled fonts, a few milliseconds), so a session
//!   that never measures never pays.
//! * [`Services::fake`] is [`FakeMeasure`]: the monospace measure of `edit` and the estimate of `io`. It is exact
//!   and font-free, so tests that check numbers do not depend on glyph shapes.
//! * [`Services::with`] takes any [`Measure`].
//!
//! The formula layout ([`drafftink_typeset::MathLayout`]) is computed once per distinct formula and size and then
//! served from the typesetter's cache: [`Services::math_layout`].

mod fake;
mod typeset;

use std::sync::Arc;

use drafftink_edit::text_bind::TextMeasure as EditMeasure;
use drafftink_io::TextMeasure as IoMeasure;
use drafftink_io::svg::MathSvg;
use drafftink_typeset::{MathError, MathLayout, TypesetStats};

pub use fake::FakeMeasure;
pub use typeset::{LazyTypesetter, SharedTypesetter, TypesetMeasure};

/// One text and formula engine, seen through the three traits the lower crates define.
pub trait Measure: EditMeasure + IoMeasure + MathSvg + Send {
    /// The layout of a formula at a size. The engine behind it caches the result, so asking again for the same
    /// formula is a lookup. An error carries a position and plain words.
    fn math_layout(&self, latex: &str, size: f64) -> Result<Arc<MathLayout>, MathError>;

    /// Cache counters, for engines that have caches.
    fn stats(&self) -> Option<TypesetStats> {
        None
    }
}

/// What a [`Session`](crate::Session) owns beyond tabs and settings.
pub struct Services {
    measure: Box<dyn Measure>,
}

impl Default for Services {
    fn default() -> Self {
        Self::new()
    }
}

impl Services {
    /// The real text engine (built on first use).
    pub fn new() -> Self {
        Self::with(TypesetMeasure::new())
    }

    /// The same real engine, sharing an existing typesetter with its caches (the renderer holds the other handle).
    pub fn sharing(typesetter: SharedTypesetter) -> Self {
        Self::with(TypesetMeasure::sharing(typesetter))
    }

    /// Deterministic measuring without fonts, for tests.
    pub fn fake() -> Self {
        Self::with(FakeMeasure::default())
    }

    /// Any measure.
    pub fn with(measure: impl Measure + 'static) -> Self {
        Self { measure: Box::new(measure) }
    }

    /// The measure for functions of `drafftink-edit` (`refresh_labels`, `bind_text`, ...).
    pub fn edit(&self) -> &dyn EditMeasure {
        &*self.measure
    }

    /// The measure for functions of `drafftink-io` (import, Mermaid layout, SVG export).
    pub fn io(&self) -> &dyn IoMeasure {
        &*self.measure
    }

    /// The formula drawer for SVG export.
    pub fn math_svg(&self) -> &dyn MathSvg {
        &*self.measure
    }

    /// The layout of a formula. Compute it once when the formula is edited; later calls with the same text and size
    /// are served from the cache.
    pub fn math_layout(&self, latex: &str, size: f64) -> Result<Arc<MathLayout>, MathError> {
        self.measure.math_layout(latex, size)
    }

    /// Cache counters, when the engine has caches.
    pub fn stats(&self) -> Option<TypesetStats> {
        self.measure.stats()
    }
}

impl std::fmt::Debug for Services {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Services").finish_non_exhaustive()
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
