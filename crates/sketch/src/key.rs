//! Cache keys.
//!
//! A [`SketchKey`] identifies everything that changes the output of [`sketch`](crate::sketch): the
//! kind of shape, its local geometry, the sketch parameters and the seed. It does **not** include
//! position or rotation, because the input is in local coordinates and the caller moves and rotates
//! the cached geometry with a transform. Dragging or rotating a shape therefore never changes the
//! key. Resizing does (the size is part of the geometry), as does editing points, so a resize drag
//! regenerates every frame.
//!
//! Parameters that cannot change the output are left out: fill parameters when there is no fill,
//! and seed and bowing when there is neither roughness nor a patterned fill. Values are hashed
//! after the same clamping that [`sketch`](crate::sketch) applies, with `-0.0` equal to `0.0` and
//! all NaNs equal, so equal keys always mean equal output.

use std::hash::Hash;

use drafftink_geom::hash::{Bits, Hash128};

use crate::input::{FillStyle, PressureProfile, Roundness, SketchInput, SketchParams, SketchShape};
use crate::limits::{MAX_COORD, MAX_HEAD_SIZE, MAX_ROUGHNESS, MAX_STROKE_WIDTH, bounded, coord, length};

/// Bump when the generation algorithm changes, so keys of a persisted cache stop matching.
pub const ALGORITHM_VERSION: u32 = 2;

/// A 128-bit identity of a [`SketchInput`]. Cheap to copy and hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SketchKey(pub u128);

// Every function below destructures its input without `..`, so a new field is a compile error until it is hashed.

fn roundness(h: &mut Hash128, r: Roundness) {
    let (tag, v) = match r {
        Roundness::Sharp => (0u8, 0.0),
        Roundness::Radius(v) => (1, v),
        Roundness::Proportional(v) => (2, v),
        Roundness::Adaptive(v) => (3, v),
    };
    // NaN is treated as 0 by `Roundness::radius`; any other value is clamped there too, but the
    // clamp depends on the size, which is hashed as well, so the raw value is exact enough.
    (tag, Bits(if v.is_nan() { 0.0 } else { v })).hash(h);
}

fn points(h: &mut Hash128, pts: &[kurbo::Point]) {
    // Non-finite points are dropped by `sketch`, so they are skipped here too.
    let mut n = 0u64;
    for p in pts.iter().filter(|p| p.x.is_finite() && p.y.is_finite()) {
        (Bits(coord(p.x)), Bits(coord(p.y))).hash(h);
        n += 1;
    }
    n.hash(h);
}

fn shape(h: &mut Hash128, s: &SketchShape) {
    match s {
        SketchShape::Rect { width, height, roundness: r } => {
            1u8.hash(h);
            (Bits(length(*width)), Bits(length(*height))).hash(h);
            roundness(h, *r);
        }
        SketchShape::Ellipse { width, height } => {
            2u8.hash(h);
            (Bits(length(*width)), Bits(length(*height))).hash(h);
        }
        SketchShape::Diamond { width, height, roundness: r } => {
            3u8.hash(h);
            (Bits(length(*width)), Bits(length(*height))).hash(h);
            roundness(h, *r);
        }
        SketchShape::Polyline { points: pts, curve, closed } => {
            4u8.hash(h);
            (curve, closed).hash(h);
            points(h, pts);
        }
        SketchShape::Arrow { points: pts, curve, start, end, head_size } => {
            5u8.hash(h);
            (curve, start, end, Bits(bounded(*head_size, 0.0, MAX_HEAD_SIZE))).hash(h);
            points(h, pts);
        }
        SketchShape::Freehand { points: pts, pressures, closed, profile } => {
            6u8.hash(h);
            closed.hash(h);
            let PressureProfile { size_scale, thinning } = *profile;
            (Bits(bounded(size_scale, 2.3, 1000.0)), Bits(bounded(thinning, 0.6, 1.0))).hash(h);
            points(h, pts);
            // Pressure only matters for open strokes, and indices follow the raw point list.
            if !*closed {
                for v in pressures {
                    Bits(if v.is_finite() { *v } else { 1.0 }).hash(h);
                }
                pressures.len().hash(h);
            }
        }
    }
}

fn has_fill(s: &SketchShape, p: &SketchParams) -> bool {
    // Only closed shapes take a fill.
    let closed = match s {
        SketchShape::Rect { .. } | SketchShape::Ellipse { .. } | SketchShape::Diamond { .. } => true,
        SketchShape::Polyline { closed, .. } | SketchShape::Freehand { closed, .. } => *closed,
        SketchShape::Arrow { .. } => false,
    };
    closed && p.fill != FillStyle::None
}

fn params(h: &mut Hash128, s: &SketchShape, p: &SketchParams) {
    let SketchParams { roughness, bowing, seed, stroke_width, line, fill, fill_gap, fill_angle } = p;
    let roughness = bounded(*roughness, 0.0, MAX_ROUGHNESS);
    let filled = has_fill(s, p);
    let patterned = filled && *fill != FillStyle::Solid;
    let fill = if filled { *fill } else { FillStyle::None };
    (Bits(roughness), Bits(bounded(*stroke_width, 0.0, MAX_STROKE_WIDTH)), line, fill).hash(h);
    // The seed and bowing only change the output when something is randomised.
    if roughness > 0.0 || patterned {
        seed.hash(h);
    }
    if roughness > 0.0 {
        Bits(bounded(*bowing, 1.0, MAX_ROUGHNESS)).hash(h);
    }
    if patterned {
        let gap = fill_gap.filter(|g| g.is_finite()).map_or(-1.0, |g| g.clamp(0.0, MAX_COORD));
        let angle = fill_angle.filter(|a| a.is_finite()).unwrap_or(f64::MAX);
        (Bits(gap), Bits(angle)).hash(h);
    }
}

/// The cache key of `input`. Cost is linear in the number of points; callers with versioned
/// elements should memoise it per version.
pub fn sketch_key(input: &SketchInput) -> SketchKey {
    let SketchInput { shape: s, params: p } = input;
    let mut h = Hash128::new();
    ALGORITHM_VERSION.hash(&mut h);
    shape(&mut h, s);
    params(&mut h, s, p);
    SketchKey(h.finish128())
}

impl SketchInput {
    /// Shorthand for [`sketch_key`].
    pub fn key(&self) -> SketchKey {
        sketch_key(self)
    }
}
