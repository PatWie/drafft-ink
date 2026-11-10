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

use crate::input::{Arrowhead, FillStyle, PressureProfile, Roundness, SketchInput, SketchParams, SketchShape};
use crate::limits::{MAX_COORD, MAX_HEAD_SIZE, MAX_ROUGHNESS, MAX_STROKE_WIDTH, bounded, coord, length};

/// Bump when the generation algorithm changes, so keys of a persisted cache stop matching.
pub const ALGORITHM_VERSION: u32 = 1;

/// A 128-bit identity of a [`SketchInput`]. Cheap to copy and hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SketchKey(pub u128);

/// Two multiplicative lanes with different constants, finished with splitmix64.
struct KeyBuilder {
    a: u64,
    b: u64,
    n: u64,
}

impl KeyBuilder {
    fn new() -> Self {
        Self { a: 0x243f_6a88_85a3_08d3, b: 0x1319_8a2e_0370_7344, n: 0 }
    }

    fn u64(&mut self, x: u64) {
        self.a = (self.a.rotate_left(5) ^ x).wrapping_mul(0x517c_c1b7_2722_0a95);
        self.b = (self.b.rotate_left(23) ^ x.rotate_left(17)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        self.n += 1;
    }

    fn tag(&mut self, x: u8) {
        self.u64(u64::from(x));
    }

    fn f64(&mut self, x: f64) {
        self.u64(x.to_bits());
    }

    fn point(&mut self, p: kurbo::Point) {
        self.f64(coord(p.x));
        self.f64(coord(p.y));
    }

    fn finish(self) -> SketchKey {
        fn mix(mut z: u64) -> u64 {
            z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        }
        let hi = mix(self.a ^ mix(self.n));
        let lo = mix(self.b ^ mix(self.a));
        SketchKey((u128::from(hi) << 64) | u128::from(lo))
    }
}

fn roundness(k: &mut KeyBuilder, r: Roundness) {
    let (t, v) = match r {
        Roundness::Sharp => (0, 0.0),
        Roundness::Radius(v) => (1, v),
        Roundness::Proportional(v) => (2, v),
        Roundness::Adaptive(v) => (3, v),
    };
    k.tag(t);
    // NaN is treated as 0 by `Roundness::radius`; any other value is clamped there too, but the
    // clamp depends on the size, which is hashed as well, so the raw value is exact enough.
    k.f64(if v.is_nan() { 0.0 } else { v + 0.0 });
}

fn head(k: &mut KeyBuilder, h: Arrowhead) {
    k.tag(h.kind as u8);
    k.tag(u8::from(h.filled));
}

fn points(k: &mut KeyBuilder, pts: &[kurbo::Point]) {
    // Non-finite points are dropped by `sketch`, so they are skipped here too.
    let mut n = 0u64;
    for p in pts.iter().filter(|p| p.x.is_finite() && p.y.is_finite()) {
        k.point(*p);
        n += 1;
    }
    k.u64(n);
}

fn shape(k: &mut KeyBuilder, s: &SketchShape) {
    match s {
        SketchShape::Rect { width, height, roundness: r } => {
            k.tag(1);
            k.f64(length(*width));
            k.f64(length(*height));
            roundness(k, *r);
        }
        SketchShape::Ellipse { width, height } => {
            k.tag(2);
            k.f64(length(*width));
            k.f64(length(*height));
        }
        SketchShape::Diamond { width, height, roundness: r } => {
            k.tag(3);
            k.f64(length(*width));
            k.f64(length(*height));
            roundness(k, *r);
        }
        SketchShape::Polyline { points: pts, curve, closed } => {
            k.tag(4);
            k.tag(*curve as u8);
            k.tag(u8::from(*closed));
            points(k, pts);
        }
        SketchShape::Arrow { points: pts, curve, start, end, head_size } => {
            k.tag(5);
            k.tag(*curve as u8);
            head(k, *start);
            head(k, *end);
            k.f64(bounded(*head_size, 0.0, MAX_HEAD_SIZE));
            points(k, pts);
        }
        SketchShape::Freehand { points: pts, pressures, closed, profile } => {
            k.tag(6);
            k.tag(u8::from(*closed));
            let PressureProfile { size_scale, thinning } = *profile;
            k.f64(bounded(size_scale, 2.3, 1000.0));
            k.f64(bounded(thinning, 0.6, 1.0));
            points(k, pts);
            // Pressure only matters for open strokes, and indices follow the raw point list.
            if !*closed {
                for v in pressures {
                    k.f64(if v.is_finite() { *v + 0.0 } else { 1.0 });
                }
                k.u64(pressures.len() as u64);
            }
        }
    }
}

fn has_fill(s: &SketchShape, p: &SketchParams) -> bool {
    // Only closed shapes take a fill.
    let closed = match s {
        SketchShape::Rect { .. } | SketchShape::Ellipse { .. } | SketchShape::Diamond { .. } => true,
        SketchShape::Polyline { closed, .. } => *closed,
        SketchShape::Freehand { closed, .. } => *closed,
        SketchShape::Arrow { .. } => false,
    };
    closed && p.fill != FillStyle::None
}

fn params(k: &mut KeyBuilder, s: &SketchShape, p: &SketchParams) {
    let roughness = bounded(p.roughness, 0.0, MAX_ROUGHNESS);
    let fill = has_fill(s, p);
    let patterned = fill && p.fill != FillStyle::Solid;
    k.f64(roughness);
    k.f64(bounded(p.stroke_width, 0.0, MAX_STROKE_WIDTH));
    k.tag(p.line as u8);
    k.tag(if fill { p.fill as u8 } else { FillStyle::None as u8 });
    if roughness > 0.0 || patterned {
        k.u64(u64::from(p.seed));
    }
    if roughness > 0.0 {
        k.f64(bounded(p.bowing, 1.0, MAX_ROUGHNESS));
    }
    if patterned {
        k.f64(p.fill_gap.filter(|g| g.is_finite()).map_or(-1.0, |g| g.clamp(0.0, MAX_COORD)));
        k.f64(p.fill_angle.filter(|a| a.is_finite()).map_or(f64::MAX, |a| a + 0.0));
    }
}

/// The cache key of `input`. Cost is linear in the number of points; callers with versioned
/// elements should memoise it per version.
pub fn sketch_key(input: &SketchInput) -> SketchKey {
    let mut k = KeyBuilder::new();
    k.u64(u64::from(ALGORITHM_VERSION));
    shape(&mut k, &input.shape);
    params(&mut k, &input.shape, &input.params);
    k.finish()
}

impl SketchInput {
    /// Shorthand for [`sketch_key`].
    pub fn key(&self) -> SketchKey {
        sketch_key(self)
    }
}
