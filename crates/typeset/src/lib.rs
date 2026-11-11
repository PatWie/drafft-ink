//! Text and math layout for Drafft.ink (layer L1).
//!
//! `Typesetter` lays out [`TextSpec`] with Parley and [`MathSpec`] with ReX into immutable
//! layouts (glyph runs, rules, bounds, caret geometry) and keeps them in bounded LRU caches.
//! Bounds and hit tests come from the layout, measured once when the content changes and never
//! during render. See `docs/typeset.md`.

#![warn(missing_docs)]

mod bound;
mod cache;
mod caret;
mod fonts;
mod inline;
mod layout;
mod math;
mod outline;
mod style;
mod typesetter;

pub use bound::{BOUND_TEXT_PADDING, BoundText, Container, ContainerKind, VerticalAlign};
pub use caret::{Affinity, Caret, Motion, TextSelection};
pub use fonts::{ExtraFont, font_file};
pub use inline::{MAX_FORMULAS, formula_ranges, has_inline_math};
pub use layout::{
    Decoration, DecorationKind, FONT_SIZE_RANGE, FORMULA_ERROR_COLOR, FormulaError, Glyph, GlyphRun, InlineFormula,
    LineInfo, MAX_SPANS, MAX_TEXT_BYTES, MIN_WRAP_WIDTH, RunStyle, TextLayout,
};
pub use math::{
    MATH_SIZE_RANGE, MAX_EXTENT, MAX_LATEX_BYTES, MAX_NESTING, MAX_TOKENS, MathError, MathErrorKind, MathFontSlot,
    MathGlyph, MathLayout, MathRule, MathSpec,
};
pub use outline::ColouredPath;
pub use style::{
    Align, FontFamily, FontWeight, InlineMath, LineHeight, Rgba8, Span, SpanStyle, TextSpec, TextStyle, Wrap,
};
#[cfg(feature = "bench")]
pub use typesetter::bench_support;
pub use typesetter::{CacheConfig, FontError, MAX_FONT_BYTES, TypesetStats, Typesetter};
