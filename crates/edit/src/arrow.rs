//! Rules that the `arrow` commands need and no other module of this crate offers.
//!
//! * [`bind_end`]: bind one end of a line or arrow to a named target (`arrow.bind`), with the default
//!   fixed point facing the other end, and re-solve the arrow.
//! * [`reroute`]: clear the pinned elbow segments of arrows, re-solve their bindings and route them again
//!   (`arrow.reroute`).
//! * [`snap_dragged_point`], [`snap_candidates`]: grid, angle and smart-guide snapping of a point that is
//!   being dragged (`arrow.move_point`), as pure functions over `drafftink-geom`.
//!
//! The module sits beside `binding`, `elbow` and `linear`, which own the solver, the router and the point
//! editing. Nothing here holds a second copy of their rules.

use drafftink_geom::guides::{SmartGuide, detect_smart_guides_for_point, snap_ray_to_smart_guides};
use drafftink_geom::snap::{
    ENDPOINT_SNAP_RADIUS, GRID_SIZE, MAX_SNAP_CANDIDATES, SMART_GUIDE_THRESHOLD, snap_angle, snap_ray_to_grid_lines,
    snap_to_grid,
};
use drafftink_geom::{OrientedBox, aabb};
use drafftink_scene::{
    BindMode, Binding, Element, ElementId, ElementKind, Kind, PathType, SceneRead, SceneTx, StoreError,
};
use kurbo::{Point, Rect, Vec2};

use crate::binding::{BindingError, End, resolve, update_bound_arrows};
use crate::elbow::{self, ElbowEditError, RouteOpts};
use crate::linear::world_points;

/// Why an arrow operation was refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ArrowError {
    /// The element does not exist (or is deleted).
    #[error("element {0} does not exist")]
    Missing(ElementId),
    /// The element is not a line or an arrow.
    #[error("element {0} is not a line or an arrow, so it has no ends to bind")]
    NotLinear(ElementId),
    /// The target is not something an arrow can attach to.
    #[error("{kind} elements cannot be arrow targets")]
    NotBindable {
        /// The element.
        id: ElementId,
        /// Its kind, lower case.
        kind: Kind,
        /// True for text that labels another element.
        label: bool,
    },
    /// A fixed point ratio is not a finite number.
    #[error("a fixed point needs finite ratios")]
    NotFinite,
    /// The binding module refused.
    #[error(transparent)]
    Binding(#[from] BindingError),
    /// The elbow router refused.
    #[error(transparent)]
    Elbow(#[from] ElbowEditError),
    /// The store refused the change.
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// What to bind an end to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BindRequest<'a> {
    /// The target element.
    pub target: &'a ElementId,
    /// Inside or orbit. Elbow arrows always bind orbit.
    pub mode: BindMode,
    /// Where on the target, as ratios of its unrotated box. `None` picks the outline point facing the arrow.
    pub fixed_point: Option<[f64; 2]>,
    /// Displayed zoom (1 is 100%), which sets the binding reach for elbow routing.
    pub zoom: f64,
    /// Snap an elbow end to a side midpoint when it is near one.
    pub midpoint_snap: bool,
}

/// The outermost crossing of the ray from the target's centre towards `toward` with its outline, or the
/// centre when the ray hits nothing.
fn facing_point(r: &crate::binding::Resolved, toward: Point) -> Point {
    let c = r.target.center();
    let dir = toward - c;
    let len = dir.hypot();
    if len <= 0.0 || !len.is_finite() {
        return c;
    }
    let size = r.target.frame.size;
    let far = c + dir / len * (size.width.max(size.height) * 2.0 + len);
    let mut hits = r.target.intersect_segment(c, far, 0.0);
    hits.sort_by(|a, b| (*a - far).hypot2().total_cmp(&(*b - far).hypot2()));
    hits.first().copied().unwrap_or(c)
}

/// Binds `end` of the line or arrow `arrow` to `req.target` and re-solves the arrow (an elbow arrow is
/// routed once). Returns the binding that was stored.
///
/// The default fixed point is the target's outline point facing the arrow's neighbouring point (the other
/// end of a two-point arrow). An elbow end snaps to the outline the elbow way and is bound in orbit mode.
pub fn bind_end<T: SceneTx>(
    tx: &mut T,
    arrow: &ElementId,
    end: End,
    req: &BindRequest<'_>,
) -> Result<Binding, ArrowError> {
    if let Some(v) = req.fixed_point
        && !(v[0].is_finite() && v[1].is_finite())
    {
        return Err(ArrowError::NotFinite);
    }
    let a = tx.get(arrow).filter(|e| !e.deleted).ok_or_else(|| ArrowError::Missing(arrow.clone()))?.clone();
    let d = a.linear_data().ok_or_else(|| ArrowError::NotLinear(arrow.clone()))?;
    let world = world_points(&a).ok_or_else(|| ArrowError::NotLinear(arrow.clone()))?;
    let t = tx.get(req.target).filter(|e| !e.deleted).ok_or_else(|| ArrowError::Missing(req.target.clone()))?;
    let not_bindable = |t: &Element| ArrowError::NotBindable {
        id: t.id.clone(),
        kind: t.tag(),
        label: matches!(&t.kind, ElementKind::Text(x) if x.container_id.is_some()),
    };
    if req.target == arrow {
        return Err(not_bindable(t));
    }
    let Some(r) = resolve(tx, req.target) else { return Err(not_bindable(t)) };

    let n = world.len();
    let reference = match end {
        End::Start => world.get(1),
        End::End => n.checked_sub(2).and_then(|i| world.get(i)),
    }
    .copied()
    .ok_or_else(|| ArrowError::NotLinear(arrow.clone()))?;

    let elbow = d.path == PathType::Elbow;
    let (mode, fixed_point) = if elbow {
        let fp = match req.fixed_point {
            Some(v) => v,
            None => {
                let snapped = elbow::snap_to_outline(&r.target, r.gap, reference, req.midpoint_snap, req.zoom);
                elbow::fixed_point_for(&r.target, r.gap, snapped)
            }
        };
        (BindMode::Orbit, fp)
    } else {
        let fp = match req.fixed_point {
            Some(v) => v,
            None => {
                let v = r.target.fixed_point_from_global(facing_point(&r, reference), r.gap);
                [v.x, v.y]
            }
        };
        (req.mode, fp)
    };
    let fp = drafftink_geom::binding::normalize_fixed_point(Vec2::new(fixed_point[0], fixed_point[1]));
    let binding = Binding { target: req.target.clone(), mode, fixed_point: [fp.x, fp.y] };

    let stored = binding.clone();
    tx.update(arrow, |slot| {
        if let ElementKind::Line(d) | ElementKind::Arrow(d) = &mut slot.kind {
            match end {
                End::Start => d.start_binding = Some(stored),
                End::End => d.end_binding = Some(stored),
            }
        }
    })?;
    if elbow {
        elbow::reroute_with(tx, arrow, None, RouteOpts { zoom: req.zoom })?;
    } else {
        update_bound_arrows(tx, std::slice::from_ref(arrow))?;
    }
    Ok(binding)
}

/// Re-routes lines and arrows from their bindings: elbow arrows lose their pinned segments and are routed
/// afresh, and every bound end is re-solved from the targets' current positions. Ids that are not lines or
/// arrows are skipped. Returns the ids whose geometry changed, sorted.
pub fn reroute<T: SceneTx>(tx: &mut T, ids: &[ElementId], zoom: f64) -> Result<Vec<ElementId>, ArrowError> {
    let linear: Vec<ElementId> =
        ids.iter().filter(|id| tx.get(id).is_some_and(|e| !e.deleted && e.tag().is_linear())).cloned().collect();
    let before: Vec<(ElementId, Element)> =
        linear.iter().filter_map(|id| tx.get(id).map(|e| (id.clone(), e.clone()))).collect();
    for id in &linear {
        let is_elbow = tx.get(id).and_then(Element::linear_data).is_some_and(|d| d.path == PathType::Elbow);
        if !is_elbow {
            continue;
        }
        tx.update(id, |e| {
            if let ElementKind::Line(d) | ElementKind::Arrow(d) = &mut e.kind {
                d.fixed_segments.clear();
                d.start_is_special = false;
                d.end_is_special = false;
            }
        })?;
        elbow::reroute_forced(tx, id, RouteOpts { zoom })?;
    }
    update_bound_arrows(tx, &linear)?;
    let mut changed: Vec<ElementId> = before
        .into_iter()
        .filter(|(id, old)| tx.get(id).is_some_and(|now| now.kind != old.kind || now.transform != old.transform))
        .map(|(id, _)| id)
        .collect();
    changed.sort();
    Ok(changed)
}

// ---------------------------------------------------------------------------------------------
// Snapping a dragged point
// ---------------------------------------------------------------------------------------------

/// Which snaps are on for a drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SnapFlags {
    /// Snap to the 20-unit grid.
    pub grid: bool,
    /// Snap to the edges and centres of other elements.
    pub smart: bool,
    /// Lock the direction from the neighbouring point to a multiple of the angle step.
    pub angle: bool,
}

/// The result of [`snap_dragged_point`].
#[derive(Debug, Clone, PartialEq)]
pub struct PointSnap {
    /// Where the point ends up.
    pub point: Point,
    /// Guides to draw, from the smart-guide pass.
    pub guides: Vec<SmartGuide>,
    /// The locked angle in degrees (`[0, 360)`) when the angle snap applied.
    pub angle: Option<f64>,
}

/// Snaps a dragged point.
///
/// * Angle snap (with a `reference`, the neighbouring point): the direction is rounded to `angle_step`
///   degrees and the length is kept; then smart guides move the point along that ray, or else the grid does.
/// * Otherwise smart guides snap each axis to the nearest edge or centre of `others` (within 8 units), and
///   the grid snaps the axes the guides left alone.
///
/// A point that is not finite is returned unchanged.
pub fn snap_dragged_point(
    to: Point,
    reference: Option<Point>,
    others: &[Rect],
    flags: SnapFlags,
    angle_step: f64,
) -> PointSnap {
    let mut out = PointSnap { point: to, guides: Vec::new(), angle: None };
    if !drafftink_geom::point_is_finite(to) {
        return out;
    }
    if flags.angle
        && let Some(origin) = reference.filter(|o| drafftink_geom::point_is_finite(*o))
    {
        let (dx, dy) = (to.x - origin.x, to.y - origin.y);
        let dist = dx.hypot(dy);
        if dist.is_finite() && dist >= 0.001 {
            let degrees = snap_angle(dy.atan2(dx).to_degrees(), angle_step);
            let (s, c) = degrees.to_radians().sin_cos();
            let mut p = Point::new(origin.x + dist * c, origin.y + dist * s);
            out.angle = Some(degrees);
            if flags.smart {
                let g = snap_ray_to_smart_guides(origin, degrees, p, others, SMART_GUIDE_THRESHOLD);
                if g.snapped_x || g.snapped_y {
                    p = g.point;
                    out.guides = g.guides;
                }
            } else if flags.grid {
                let g = snap_ray_to_grid_lines(origin, degrees, p, GRID_SIZE);
                if g.is_snapped() {
                    p = g.point;
                }
            }
            out.point = p;
            return out;
        }
    }
    let mut p = to;
    let (mut sx, mut sy) = (false, false);
    if flags.smart {
        let g = detect_smart_guides_for_point(p, others, SMART_GUIDE_THRESHOLD);
        p = g.point;
        (sx, sy) = (g.snapped_x, g.snapped_y);
        out.guides = g.guides;
    }
    if flags.grid {
        let g = snap_to_grid(p, GRID_SIZE);
        if !sx {
            p.x = g.point.x;
        }
        if !sy {
            p.y = g.point.y;
        }
    }
    out.point = p;
    out
}

/// The boxes a dragged point may align with: the bounds of every live element near `around` except the
/// ones in `skip`, in scene order, at most [`MAX_SNAP_CANDIDATES`]. Looks at the whole scene; a caller with a
/// spatial index uses [`snap_candidates_among`].
pub fn snap_candidates<R: SceneRead>(scene: &R, skip: &[ElementId], around: Point) -> Vec<Rect> {
    drafftink_profile::count(drafftink_profile::Counter::ElementsScanned, scene.len_all() as u64);
    boxes_near(scene.iter(), skip, around)
}

/// Like [`snap_candidates`], looking only at the elements named in `near` (any order). `near` must hold every
/// element whose box is within [`snap_reach`] of `around`.
pub fn snap_candidates_among<R: SceneRead>(
    scene: &R,
    near: &[ElementId],
    skip: &[ElementId],
    around: Point,
) -> Vec<Rect> {
    let mut elements: Vec<&Element> = near.iter().filter_map(|id| scene.get(id)).filter(|e| !e.deleted).collect();
    elements.sort_by(|a, b| a.index.cmp(&b.index).then_with(|| a.id.cmp(&b.id)));
    boxes_near(elements.into_iter(), skip, around)
}

/// How far from the dragged point a spatial query must look so that [`snap_candidates_among`] sees every box the
/// snap zone touches (the zone is a square, a query reaches in a circle).
pub fn snap_reach() -> f64 {
    ENDPOINT_SNAP_RADIUS * std::f64::consts::SQRT_2
}

fn boxes_near<'a>(elements: impl Iterator<Item = &'a Element>, skip: &[ElementId], around: Point) -> Vec<Rect> {
    if !drafftink_geom::point_is_finite(around) {
        return Vec::new();
    }
    let zone = Rect::new(
        around.x - ENDPOINT_SNAP_RADIUS,
        around.y - ENDPOINT_SNAP_RADIUS,
        around.x + ENDPOINT_SNAP_RADIUS,
        around.y + ENDPOINT_SNAP_RADIUS,
    );
    let mut out = Vec::new();
    for e in elements {
        if out.len() >= MAX_SNAP_CANDIDATES {
            break;
        }
        if skip.contains(&e.id) {
            continue;
        }
        let t = &e.transform;
        let b = OrientedBox::from_xywh(t.x, t.y, t.w, t.h, t.angle).aabb();
        if aabb::is_finite(&b) && aabb::intersects(&b, &zone) {
            out.push(b);
        }
    }
    out
}
