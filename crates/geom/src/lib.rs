//! drafftink-geom (L0): the geometry foundation for the canvas library and the editor.
//!
//! Everything here is pure: no document model, no I/O, no clock. It builds for
//! `wasm32-unknown-unknown`. [`kurbo`] is re-exported so higher layers use one version.
//!
//! Conventions
//! - World space has y pointing down. Angles are in radians; a positive angle turns a shape
//!   clockwise on screen (the same as `kurbo::Affine::rotate`).
//! - Every function accepts hostile input (NaN, infinities, numbers above 1e150, zero or negative sizes) and
//!   returns a neutral value instead of panicking: `false`, `None`, an empty `Vec`,
//!   `f64::INFINITY` for a distance, or the input unchanged for a snap.
//!
//! Modules
//! - [`aabb`], [`obox`], [`transform`]: axis-aligned boxes, oriented boxes, element transforms.
//! - [`outline`], [`shapes`]: primitive shapes and the [`Outline`] queries (bounds, containment,
//!   distance, closest point, ray and segment intersection, hit tests).
//! - [`binding`]: headings, fixed points and outline helpers used by arrow binding and elbow routing.
//! - [`index`]: [`SpatialIndex`], an R-tree keyed by id.
//! - [`snap`], [`guides`]: grid, angle and smart-guide snapping math.
//! - [`spline`]: the Catmull-Rom curve of a rounded line.
//! - [`align`]: align and distribute as pure functions.
//! - [`select`]: handle positions, box resize and rotate math, marquee tests.
//! - [`lasso`]: the free-hand closed path and its selection test, the counterpart of the marquee.

#![warn(missing_docs)]

pub use kurbo;

pub mod aabb;
pub mod align;
pub mod binding;
pub mod guides;
pub mod index;
pub mod lasso;
pub mod obox;
pub mod outline;
pub mod select;
pub mod shapes;
pub mod snap;
pub mod spline;
pub mod transform;

mod convex;
mod ellipse;

pub use index::{IndexError, SpatialIndex};
pub use obox::OrientedBox;
pub use outline::{Hit, HitOpts, Nearest, Outline, Primitive, Probe};
pub use shapes::{DiamondShape, EllipseShape, PathShape, Polyline, RectShape};
pub use transform::ElementFrame;

/// Distances and lengths below this are treated as zero.
pub const EPS: f64 = 1e-9;

/// The smallest box side the outline code works with. A zero-size shape is treated as this thin,
/// which keeps normals and divisions finite.
pub const MIN_DIM: f64 = 1e-6;

/// The largest magnitude the geometry accepts for a coordinate, a size or a radius. It is far
/// beyond any canvas and small enough that squaring it cannot overflow, so a number above it is
/// treated like a NaN.
pub const MAX_COORD: f64 = 1e150;

/// True when `v` is a usable number: finite and at most [`MAX_COORD`] in magnitude.
#[inline]
pub fn num_is_sane(v: f64) -> bool {
    v.abs() <= MAX_COORD
}

/// True when both coordinates are usable numbers (see [`num_is_sane`]).
#[inline]
pub fn point_is_finite(p: kurbo::Point) -> bool {
    num_is_sane(p.x) && num_is_sane(p.y)
}

/// True when both components are usable numbers (see [`num_is_sane`]).
#[inline]
pub fn vec_is_finite(v: kurbo::Vec2) -> bool {
    num_is_sane(v.x) && num_is_sane(v.y)
}
