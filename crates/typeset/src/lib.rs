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
mod layout;
mod math;
mod outline;
mod style;
mod typesetter;

pub use bound::{BOUND_TEXT_PADDING, BoundText, Container, ContainerKind, VerticalAlign};
pub use caret::{Affinity, Caret, Motion, TextSelection};
pub use fonts::HAS_NOTO_SANS;
pub use layout::{
    Decoration, DecorationKind, FONT_SIZE_RANGE, Glyph, GlyphRun, LineInfo, MAX_SPANS, MAX_TEXT_BYTES, MIN_WRAP_WIDTH,
    RunStyle, TextLayout,
};
pub use math::{
    MATH_SIZE_RANGE, MAX_EXTENT, MAX_LATEX_BYTES, MAX_NESTING, MAX_TOKENS, MathError, MathErrorKind, MathFontSlot,
    MathGlyph, MathLayout, MathRule, MathSpec,
};
pub use style::{Align, FontFamily, FontWeight, LineHeight, Rgba8, Span, SpanStyle, TextSpec, TextStyle, Wrap};
pub use typesetter::{CacheConfig, FontError, MAX_FONT_BYTES, TypesetStats, Typesetter, bench_support};
