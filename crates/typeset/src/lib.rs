//! Text and math layout for Drafft.ink (layer L1).
//!
//! `Typesetter` lays out [`TextSpec`] with Parley and [`MathSpec`] with ReX into immutable
//! layouts (glyph runs, rules, bounds, caret geometry) and keeps them in bounded LRU caches.
//! Bounds and hit tests come from the layout, measured once when the content changes and never
//! during render. See `docs/typeset.md`.

mod bound;
mod cache;
mod caret;
mod fonts;
mod layout;
mod math;
mod outline;
mod style;
mod typesetter;

pub use bound::*;
pub use caret::*;
pub use fonts::HAS_NOTO_SANS;
pub use layout::*;
pub use math::*;
pub use style::*;
pub use typesetter::*;
