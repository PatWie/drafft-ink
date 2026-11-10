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
//! # Limits
//!
//! Output size is bounded for any input; see [`limits`](crate#constants). NaN and infinite points
//! are dropped, coordinates are clamped to ±[`MAX_COORD`], point lists are decimated above
//! [`MAX_POINTS`], and pattern fills draw at most [`MAX_HACHURE_LINES`] lines per direction.

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
