//! Hand-drawn ("sketchy") path generation.
//!
//! Layer L1. Input: a shape described in local coordinates ([`SketchInput`]). Output: kurbo paths
//! with stroke and fill instructions ([`SketchGeometry`]). No GPU, no vello, no workspace
//! dependencies; builds for `wasm32-unknown-unknown`.
//!
//! ```
//! use drafftink_sketch::{sketch, SketchInput, SketchParams, SketchShape, Roundness, FillStyle};
//!
//! let input = SketchInput::new(
//!     SketchShape::Rect { width: 200.0, height: 120.0, roundness: Roundness::Sharp },
//!     SketchParams { fill: FillStyle::Hachure, seed: 42, ..SketchParams::default() },
//! );
//! let geometry = sketch(&input);
//! assert_eq!(geometry, sketch(&input)); // deterministic
//! assert_eq!(geometry.strokes.len(), 2); // double stroke at roughness 1
//! ```
//!
//! # What it reproduces
//!
//! The look of the old app (`drafft-ink`, `vello_impl.rs`): a deterministic xorshift noise pass
//! over the clean outline (two strokes with different noise), a 0.3x rougher pass for the fill
//! outline, and roughr hachure-style patterns over the bounding box, clipped by the renderer.
//! `tests/old_app.rs` compares the output with the old code. The one deliberate difference: the old
//! noise was scaled by `1/sqrt(zoom)`; here the noise is fixed in local units, which is the old
//! output at zoom 1. A zoom-independent result is what makes caching across pan and zoom possible.
//!
//! # Caching
//!
//! [`sketch_key`] hashes shape kind, local geometry, parameters and seed. Position and rotation are
//! not part of the input, so moving or rotating an element keeps its key and its cached geometry.
//! A size change or a point edit changes the key, so a resize drag regenerates each frame (see the
//! benches for what that costs). [`SketchCache`] is a bounded two-generation cache around it.
//!
//! # Measured cost
//!
//! `cargo bench -p drafftink-sketch` (release, 64-core Linux box; numbers are medians):
//!
//! | Case | Time |
//! |---|---|
//! | hachure fill, 4000 x 3000 (about 875 lines) | 272 µs |
//! | cross-hatch fill, 4000 x 3000 | 562 µs |
//! | hachure fill, 300 x 200 | 25 µs |
//! | dots fill, 1000 x 1000 | 7.4 ms (the most expensive fill; capped at [`MAX_FILL_DOTS`]) |
//! | rectangle outline only | 0.3 µs |
//! | freehand with pressure, 1k / 10k / 50k points | 28 µs / 287 µs / 5.5 ms |
//! | sketchy freehand without pressure, 1k / 10k / 50k points | 29 µs / 345 µs / 6.2 ms |
//! | arrow, 8 points, triangle head | 1.2 µs |
//! | cache hit (drag frame), hachure rectangle | 41 ns |
//! | cache key of a 10k-point stroke | 50 µs |
//! | resize-drag frame (miss), 300 x 200 hachure | 35 µs |
//!
//! A hit costs the key hash, which is linear in the point count. Callers with versioned elements
//! should compute the key once per version and use [`SketchCache::get_with_key`].
//!
//! # Limits
//!
//! Output size is bounded for any input; see the constants of this crate. NaN and infinite points
//! are dropped, coordinates are clamped to ±[`MAX_COORD`], point lists are decimated above
//! [`MAX_POINTS`], and pattern fills draw at most [`MAX_HACHURE_LINES`] lines per direction.

#![warn(missing_docs)]

mod arrowhead;
mod cache;
mod effect;
mod fill;
mod freehand;
mod input;
mod key;
mod limits;
mod outline;
mod output;
mod rng;
mod sketch;

pub use cache::{CacheStats, SketchCache};
pub use input::{
    Arrowhead, Curve, FillStyle, HeadKind, LineDash, PressureProfile, Roundness, SketchInput, SketchParams,
    SketchShape, roughness,
};
pub use key::{ALGORITHM_VERSION, SketchKey, sketch_key};
pub use limits::{
    MAX_COORD, MAX_FILL_DOTS, MAX_FILL_SEGMENTS, MAX_HACHURE_LINES, MAX_HEAD_SIZE, MAX_POINTS, MAX_ROUGHNESS,
    MAX_STROKE_WIDTH, MIN_FILL_GAP,
};
pub use output::{FillKind, SketchGeometry, StrokeStyle};
pub use sketch::sketch;
