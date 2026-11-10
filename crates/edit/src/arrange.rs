//! Align, distribute and flip. A group, a frame with its children and a container with its text are one unit.
//!
//! Align and distribute move whole units by a translation, so no member of a group is displaced relative to the
//! others (the old app had no distribute and mirrored groups wrongly). They follow the transform rule of
//! [`crate::transform`]: bound arrows follow, and an arrow aligned without its target is unbound.
//!
//! Edge names follow the axis (`CenterX` makes the x-centres equal). The old app's "center vertical" and
//! "center horizontal" icons were inverted against the axis.

use drafftink_geom::aabb;
use drafftink_geom::align::{align_to, distribute as geom_distribute};
use drafftink_geom::kurbo::{Rect, Vec2};
use drafftink_geom::transform::{flip_x_about, flip_y_about};
use drafftink_scene::{ElementId, GroupId, SceneRead, SceneTx};

pub use drafftink_geom::align::{AlignMode, Axis};

use crate::group::units::{
    Relations, Unit, ensure_unlocked, impl_from_target, live, primary_id, units, with_companions,
};
use crate::transform::{TransformError, TransformReport, affine_all, geometry, translate_each};

/// Why an arrangement was refused.
#[derive(Debug, thiserror::Error)]
pub enum ArrangeError {
    /// No live element has this id.
    #[error("element {0} does not exist or is deleted")]
    NotFound(ElementId),
    /// Locked elements cannot be arranged.
    #[error("{} element(s) are locked ({}). Unlock them first", .0.len(), crate::group::units::ids_text(.0))]
    Locked(Vec<ElementId>),
    /// Not enough units.
    #[error("{what} needs at least {need} elements (a group counts as one), and {have} were given. Select more")]
    TooFew {
        /// Align or distribute.
        what: &'static str,
        /// The minimum.
        need: usize,
        /// Units given.
        have: usize,
    },
    /// The gap is not a usable number.
    #[error("the gap must be a finite number")]
    BadGap,
    /// The move itself failed.
    #[error(transparent)]
    Transform(#[from] TransformError),
}
impl_from_target!(ArrangeError);

/// The units that have bounds, with their bounds, in the same order.
fn with_rects<R: SceneRead>(scene: &R, us: Vec<Unit>) -> (Vec<Unit>, Vec<Rect>) {
    us.into_iter().filter_map(|u| unit_rect(scene, &u).map(|r| (u, r))).unzip()
}

fn unit_rect<R: SceneRead>(scene: &R, u: &Unit) -> Option<Rect> {
    geometry::union_bounds(u.elements.iter().filter_map(|i| scene.get(i)))
}

/// Everything that moves with a unit: its members, their bound text, and the children of its frames.
fn unit_closure<R: SceneRead>(scene: &R, rel: &Relations, u: &Unit) -> Vec<ElementId> {
    with_companions(scene, rel, &u.elements, true)
}

/// Aligns the units of `ids` to the edge or centre of their common bounds, or of the unit of `to`.
pub fn align<T: SceneTx>(
    tx: &mut T,
    ids: &[ElementId],
    mode: AlignMode,
    to: Option<&ElementId>,
    editing: Option<&GroupId>,
) -> Result<TransformReport, ArrangeError> {
    let rel = Relations::build(&*tx);
    let mut us = units(&*tx, &rel, ids, editing)?;
    let reference = match to {
        Some(t) => {
            live(&*tx, t)?;
            let tu = units(&*tx, &rel, std::slice::from_ref(&primary_id(&*tx, t)), editing)?;
            let r = tu.first().and_then(|u| unit_rect(&*tx, u));
            us.retain(|u| tu.first().is_none_or(|x| x.key != u.key));
            r
        }
        None => None,
    };
    let need = if to.is_some() { 1 } else { 2 };
    if us.len() < need {
        return Err(ArrangeError::TooFew { what: "align", need, have: us.len() });
    }
    let (us, rects) = with_rects(&*tx, us);
    let reference = reference.or_else(|| aabb::union_all(rects.iter().copied()));
    let Some(reference) = reference else {
        return Err(TransformError::NotFinite.into());
    };
    let deltas = align_to(&rects, mode, reference);
    run(tx, &rel, &us, &deltas)
}

/// How units are spaced by [`distribute`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DistributeOpts {
    /// Exact gap between neighbours, starting from the first unit. `None` equalises the gaps between the two
    /// outer units, which stay where they are.
    pub gap: Option<f64>,
}

/// Spaces the units of `ids` evenly along `axis`. Needs at least three units.
pub fn distribute<T: SceneTx>(
    tx: &mut T,
    ids: &[ElementId],
    axis: Axis,
    opts: DistributeOpts,
    editing: Option<&GroupId>,
) -> Result<TransformReport, ArrangeError> {
    if opts.gap.is_some_and(|g| !g.is_finite()) {
        return Err(ArrangeError::BadGap);
    }
    let rel = Relations::build(&*tx);
    let us = units(&*tx, &rel, ids, editing)?;
    if us.len() < 3 {
        return Err(ArrangeError::TooFew { what: "distribute", need: 3, have: us.len() });
    }
    let (us, rects) = with_rects(&*tx, us);
    let deltas = match opts.gap {
        None => geom_distribute(&rects, axis),
        Some(gap) => exact_gap(&rects, axis, gap),
    };
    run(tx, &rel, &us, &deltas)
}

fn exact_gap(rects: &[Rect], axis: Axis, gap: f64) -> Vec<Vec2> {
    let span = |r: &Rect| match axis {
        Axis::X => (r.x0.min(r.x1), r.x0.max(r.x1)),
        Axis::Y => (r.y0.min(r.y1), r.y0.max(r.y1)),
    };
    let mut order: Vec<usize> = (0..rects.len()).collect();
    order.sort_by(|a, b| {
        let (sa, sb) = (span(&rects[*a]), span(&rects[*b]));
        (sa.0 + sa.1).total_cmp(&(sb.0 + sb.1)).then(a.cmp(b))
    });
    let mut out = vec![Vec2::ZERO; rects.len()];
    let mut pos = order.first().map_or(0.0, |i| span(&rects[*i]).0);
    for i in order {
        let (lo, hi) = span(&rects[i]);
        let d = pos - lo;
        out[i] = match axis {
            Axis::X => Vec2::new(d, 0.0),
            Axis::Y => Vec2::new(0.0, d),
        };
        pos += (hi - lo) + gap;
    }
    out
}

fn run<T: SceneTx>(tx: &mut T, rel: &Relations, us: &[Unit], deltas: &[Vec2]) -> Result<TransformReport, ArrangeError> {
    let moves: Vec<(Vec<ElementId>, Vec2)> =
        us.iter().zip(deltas).map(|(u, d)| (unit_closure(&*tx, rel, u), *d)).collect();
    let all: Vec<ElementId> = moves.iter().flat_map(|(ids, _)| ids.iter().cloned()).collect();
    ensure_unlocked(&*tx, &all)?;
    Ok(translate_each(tx, &moves)?)
}

/// The mirror axis of [`flip`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlipAxis {
    /// Left to right (about a vertical line).
    Horizontal,
    /// Top to bottom (about a horizontal line).
    Vertical,
}

/// Mirrors the elements about the centre of their common bounds. The selection flips as one block, so a group
/// keeps its shape. Linear points and binding fixed points are mirrored, images toggle their flip flags, and text
/// moves without being mirrored.
pub fn flip<T: SceneTx>(tx: &mut T, ids: &[ElementId], axis: FlipAxis) -> Result<TransformReport, TransformError> {
    affine_all(tx, ids, |b| {
        let c = aabb::center(&b);
        match axis {
            FlipAxis::Horizontal => flip_x_about(c.x),
            FlipAxis::Vertical => flip_y_about(c.y),
        }
    })
}
