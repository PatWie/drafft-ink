//! Elbow arrows: orthogonal routes around the shapes an arrow is bound to.
//!
//! The route is **stored geometry**: `points` are the corners, `fixed_segments` are the pins the user
//! dragged, and `start_is_special` / `end_is_special` mark a hidden stub. Hit tests, handles and the renderer
//! all read the stored points. (The old app only drew a fixed Z at render time.)
//!
//! * [`route`], [`ElbowEnd`], [`ElbowTarget`]: the pure router (A* on a grid of obstacle-box edges).
//! * [`move_segment`], [`release_segment`], [`update_ends`], [`renormalize`]: pin handling on a pure path.
//! * [`reroute`], [`move_segment_of`], [`release_segment_of`]: the same on an element in a transaction.
//! * [`snap_to_outline`], [`fixed_point_for`]: where an elbow end attaches to a shape.
//! * [`rounded_path`]: the drawn shape, with 16-unit rounded corners.
//!
//! The router is a port of the algorithm in Excalidraw's `elbowArrow.ts` (MIT), written for our model.

mod route;
mod segments;

use drafftink_geom::binding::{BASE_BINDING_GAP, BindTarget, Heading, max_binding_distance};
use drafftink_geom::kurbo::{BezPath, Point, Vec2};
use drafftink_scene::{
    BindMode, Binding, Element, ElementId, ElementKind, FixedSegment, LinearData, PathType, SceneRead, SceneTx,
    StoreError,
};

pub use route::{
    BASE_PADDING, Bounds, CORNER_RADIUS, DEDUP_THRESHOLD, ElbowEnd, ElbowError, ElbowTarget, RouteOpts, heading_of_end,
    is_orthogonal, route,
};
pub use segments::{ElbowPath, Pin, move_segment, release_segment, renormalize, update_ends};

use crate::binding::resolve;
use crate::linear::{self, world_points};

/// Why an elbow edit was refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ElbowEditError {
    /// The element does not exist.
    #[error("element {0} does not exist")]
    Missing(ElementId),
    /// The element is not an elbow line or arrow.
    #[error("element {0} is not an elbow arrow; change its path to elbow first")]
    NotElbow(ElementId),
    /// The router refused.
    #[error(transparent)]
    Route(#[from] ElbowError),
    /// The store refused the change.
    #[error(transparent)]
    Store(#[from] StoreError),
}

// ---------------------------------------------------------------------------------------------
// Reading and writing the stored route
// ---------------------------------------------------------------------------------------------

/// The two ends of an arrow for the router: bound ends sit at their binding's fixed point, free ends where
/// the arrow's own points are.
pub(crate) fn ends_for<R: SceneRead>(scene: &R, d: &LinearData, world: &[Point]) -> Option<(ElbowEnd, ElbowEnd)> {
    let (first, last) = (*world.first()?, *world.last()?);
    let one = |binding: &Option<Binding>, fallback: Point, head| -> ElbowEnd {
        if let Some(b) = binding
            && let Some(r) = resolve(scene, &b.target)
        {
            let p = r.target.global_fixed_point(Vec2::new(b.fixed_point[0], b.fixed_point[1]));
            return ElbowEnd::bound(p, ElbowTarget { id: r.id, target: r.target, gap: r.gap }, head);
        }
        ElbowEnd::free(fallback, head)
    };
    Some((one(&d.start_binding, first, d.start_head), one(&d.end_binding, last, d.end_head)))
}

/// The stored route of an element, in world coordinates.
pub(crate) fn path_of(e: &Element, d: &LinearData, world: &[Point]) -> ElbowPath {
    let t = &e.transform;
    let pins = d
        .fixed_segments
        .iter()
        .map(|s| Pin { index: s.index as usize, start: t.local_to_world(s.start), end: t.local_to_world(s.end) })
        .collect();
    ElbowPath { points: world.to_vec(), pins, start_special: d.start_is_special, end_special: d.end_is_special }
}

/// Writes a route into an element: points, pins and stub flags. Elbow arrows are never rotated.
pub(crate) fn write_path(e: &mut Element, path: &ElbowPath) -> bool {
    if !linear::set_world_points(e, &path.points, linear::Turn::Upright) {
        return false;
    }
    let (x, y) = (e.transform.x, e.transform.y);
    let n = path.points.len();
    if let ElementKind::Line(d) | ElementKind::Arrow(d) = &mut e.kind {
        d.fixed_segments = path
            .pins
            .iter()
            .filter(|p| p.index >= 1 && p.index < n)
            .filter_map(|p| {
                Some(FixedSegment {
                    index: u32::try_from(p.index).ok()?,
                    start: Point::new(p.start.x - x, p.start.y - y),
                    end: Point::new(p.end.x - x, p.end.y - y),
                })
            })
            .collect();
        d.start_is_special = path.start_special;
        d.end_is_special = path.end_special;
    }
    true
}

const SAME: f64 = 1e-9;

/// The route an elbow element should have now, or `None` if it is already right. `ends` overrides where
/// free ends are.
pub(crate) fn solve_path<R: SceneRead>(
    scene: &R,
    e: &Element,
    ends: Option<(Point, Point)>,
    opts: RouteOpts,
    force: bool,
) -> Result<Option<ElbowPath>, ElbowError> {
    let Some(d) = e.linear_data() else { return Ok(None) };
    let Some(world) = world_points(e) else { return Ok(None) };
    if world.len() < 2 {
        return Err(ElbowError::TooFewPoints(world.len()));
    }
    let n = world.len();
    let mut wanted = world.clone();
    if let Some((a, b)) = ends {
        wanted[0] = a;
        wanted[n - 1] = b;
    }
    let Some((s, en)) = ends_for(scene, d, &wanted) else { return Ok(None) };
    let path = path_of(e, d, &world);
    let (first, last) = (path.points[0], path.points[n - 1]);
    let moved = (s.point - first).hypot() > SAME || (en.point - last).hypot() > SAME;
    if path.is_valid() && !moved && !force && e.transform.angle == 0.0 {
        return Ok(None);
    }
    if !path.is_valid() {
        return Ok(Some(ElbowPath::free(route(&s, &en, opts)?)));
    }
    update_ends(&path, &s, &en, opts).map(Some)
}

/// A copy of `e` with its elbow route solved against `scene`, for a preview of an arrow that is not in the
/// document yet (bound ends sit at their bindings, free ends where the points are). Nothing is written. An
/// element that is not an elbow line or arrow comes back unchanged.
pub fn routed_copy<R: SceneRead>(scene: &R, e: &Element, opts: RouteOpts) -> Result<Element, ElbowEditError> {
    let mut out = e.clone();
    if e.linear_data().is_none_or(|d| d.path != PathType::Elbow) {
        return Ok(out);
    }
    if let Some(path) = solve_path(scene, e, None, opts, true)?
        && !write_path(&mut out, &path)
    {
        return Err(ElbowEditError::Route(ElbowError::NotFinite));
    }
    Ok(out)
}

fn commit<T: SceneTx>(tx: &mut T, id: &ElementId, e: Element) -> Result<(), StoreError> {
    tx.update(id, |slot| {
        slot.transform = e.transform;
        slot.kind = e.kind;
    })
}

fn elbow_element<R: SceneRead>(scene: &R, id: &ElementId) -> Result<Element, ElbowEditError> {
    let e = scene.get(id).ok_or_else(|| ElbowEditError::Missing(id.clone()))?;
    match e.linear_data() {
        Some(d) if d.path == PathType::Elbow => Ok(e.clone()),
        _ => Err(ElbowEditError::NotElbow(id.clone())),
    }
}

/// Routes the elbow arrow again from its bindings. Free ends stay where they are. Returns whether anything
/// changed.
pub fn reroute<T: SceneTx>(tx: &mut T, id: &ElementId, opts: RouteOpts) -> Result<bool, ElbowEditError> {
    reroute_with(tx, id, None, opts)
}

/// Routes the elbow arrow again even if its ends did not move, as after a change of arrowhead (a head needs
/// room). Pins are kept.
pub fn reroute_forced<T: SceneTx>(tx: &mut T, id: &ElementId, opts: RouteOpts) -> Result<bool, ElbowEditError> {
    reroute_inner(tx, id, None, opts, true)
}

/// Like [`reroute`], with new positions for the two ends (free ends take them; bound ends keep their binding's
/// point).
pub fn reroute_with<T: SceneTx>(
    tx: &mut T,
    id: &ElementId,
    ends: Option<(Point, Point)>,
    opts: RouteOpts,
) -> Result<bool, ElbowEditError> {
    reroute_inner(tx, id, ends, opts, false)
}

fn reroute_inner<T: SceneTx>(
    tx: &mut T,
    id: &ElementId,
    ends: Option<(Point, Point)>,
    opts: RouteOpts,
    force: bool,
) -> Result<bool, ElbowEditError> {
    let mut e = elbow_element(tx, id)?;
    let Some(path) = solve_path(tx, &e, ends, opts, force)? else { return Ok(false) };
    if !write_path(&mut e, &path) {
        return Err(ElbowEditError::Route(ElbowError::NotFinite));
    }
    commit(tx, id, e)?;
    Ok(true)
}

fn with_ends<T: SceneTx, F>(tx: &mut T, id: &ElementId, f: F) -> Result<(), ElbowEditError>
where
    F: FnOnce(&ElbowPath, &ElbowEnd, &ElbowEnd) -> Result<ElbowPath, ElbowError>,
{
    let mut e = elbow_element(tx, id)?;
    let d = e.linear_data().ok_or_else(|| ElbowEditError::NotElbow(id.clone()))?;
    let world = world_points(&e).ok_or_else(|| ElbowEditError::NotElbow(id.clone()))?;
    let (s, en) = ends_for(tx, d, &world).ok_or(ElbowError::TooFewPoints(world.len()))?;
    let mut path = path_of(&e, d, &world);
    if !path.is_valid() {
        path = ElbowPath::free(route(&s, &en, RouteOpts::default())?);
    }
    let next = f(&path, &s, &en)?;
    if !write_path(&mut e, &next) {
        return Err(ElbowEditError::Route(ElbowError::NotFinite));
    }
    commit(tx, id, e)?;
    Ok(())
}

/// Drags segment `index` (the segment from point `index - 1` to point `index`) so it passes through `pointer`.
/// The segment is pinned there and keeps its place when the arrow's ends move. Call it on every pointer move.
pub fn move_segment_of<T: SceneTx>(
    tx: &mut T,
    id: &ElementId,
    index: usize,
    pointer: Point,
    opts: RouteOpts,
) -> Result<(), ElbowEditError> {
    with_ends(tx, id, |path, s, e| move_segment(path, s, e, index, pointer, opts))
}

/// Unpins segment `index` and routes that part of the arrow again (a double click on the handle).
pub fn release_segment_of<T: SceneTx>(
    tx: &mut T,
    id: &ElementId,
    index: usize,
    opts: RouteOpts,
) -> Result<(), ElbowEditError> {
    with_ends(tx, id, |path, s, e| release_segment(path, s, e, index, opts))
}

// ---------------------------------------------------------------------------------------------
// Attaching an elbow end to a shape
// ---------------------------------------------------------------------------------------------

fn clamp_safe(v: f64, lo: f64, hi: f64) -> f64 {
    v.max(lo).min(hi.max(lo))
}

/// A side midpoint (or, for a diamond, an edge midpoint) near `p`, which an elbow end snaps to. The flag says
/// the snap is on one of the shape's own axes.
pub fn snap_midpoint(target: &BindTarget, gap: f64, p: Point, zoom: f64) -> Option<(Point, bool)> {
    let frame = &target.frame;
    let max_dist = max_binding_distance(zoom) + (gap - BASE_BINDING_GAP).max(0.0);
    let (w, h) = (frame.size.width, frame.size.height);
    let h_thr = clamp_safe(0.05 * w, 5.0, max_dist);
    let v_thr = clamp_safe(0.05 * h, 5.0, max_dist);
    let nr = frame.to_local(p);
    if nr.to_vec2().hypot() < gap {
        return None;
    }
    let [top, right, bottom, left] = target.side_midpoints();
    if nr.x <= 0.0 && nr.y > -v_thr && nr.y < v_thr {
        return Some((left, true));
    }
    if nr.y <= 0.0 && nr.x > -h_thr && nr.x < h_thr {
        return Some((top, true));
    }
    if nr.x >= 0.0 && nr.y > -v_thr && nr.y < v_thr {
        return Some((right, true));
    }
    if nr.y >= 0.0 && nr.x > -h_thr && nr.x < h_thr {
        return Some((bottom, true));
    }
    if target.kind == drafftink_geom::binding::BindKind::Diamond {
        let thr = h_thr.max(v_thr);
        let (hw, hh) = (w / 2.0, h / 2.0);
        for (ex, ey, dx, dy) in [
            (-hw / 2.0, -hh / 2.0, -1.0, -1.0),
            (hw / 2.0, -hh / 2.0, 1.0, -1.0),
            (-hw / 2.0, hh / 2.0, -1.0, 1.0),
            (hw / 2.0, hh / 2.0, 1.0, 1.0),
        ] {
            let zone = Point::new(ex + dx * gap, ey + dy * gap);
            if (zone - nr).hypot() < thr {
                return Some((frame.to_world(Point::new(ex, ey)), false));
            }
        }
    }
    None
}

/// Where an elbow end dropped at `p` attaches: on the shape's outline plus its gap, on the side the point is
/// nearest to, snapped to a side midpoint when `midpoint_snap` is on and `p` is near one.
pub fn snap_to_outline(target: &BindTarget, gap: f64, p: Point, midpoint_snap: bool, zoom: f64) -> Point {
    if !target.is_valid() || !p.x.is_finite() || !p.y.is_finite() {
        return p;
    }
    let edge = target.avoid_rectangular_corner(p, gap);
    let snap = midpoint_snap.then(|| snap_midpoint(target, gap, edge, zoom)).flatten();
    let resolved = snap.map_or(p, |(q, _)| q);
    let center = target.center();
    let aabb = target.frame.aabb();
    let horizontal = match snap {
        Some((q, true)) => Heading::from_vector(q - center).is_horizontal(),
        _ => target.heading_for_point(p, aabb).is_horizontal(),
    };
    let reach = target.frame.size.width.max(target.frame.size.height) * 2.0;
    let cast = |from: Point, offset: f64| -> Option<Point> {
        let dir = resolved - from;
        let len = dir.hypot();
        if len <= 0.0 {
            return None;
        }
        let far = from + dir / len * reach;
        let mut hits = target.intersect_segment(from, far, offset);
        hits.sort_by(|a, b| (*a - resolved).hypot2().total_cmp(&(*b - resolved).hypot2()));
        hits.into_iter().next()
    };
    let other =
        Point::new(if horizontal { center.x } else { resolved.x }, if horizontal { resolved.y } else { center.y });
    let mut hit = cast(other, gap);
    if hit.is_none() {
        let another =
            Point::new(if horizontal { resolved.x } else { center.x }, if horizontal { center.y } else { resolved.y });
        hit = cast(another, BASE_BINDING_GAP);
    }
    match hit {
        Some(h) if (edge - h).hypot2() >= 1e-4 => h,
        _ => edge,
    }
}

/// The fixed point ratio for an elbow end at `snapped` on `target`.
pub fn fixed_point_for(target: &BindTarget, gap: f64, snapped: Point) -> [f64; 2] {
    let v = target.fixed_point_from_global(snapped, gap);
    [v.x, v.y]
}

/// Makes the bindings of an arrow elbow bindings: orbit mode, the point snapped to the outline.
pub(crate) fn bind_ends_for_elbow<T: SceneTx>(tx: &mut T, id: &ElementId) -> Result<(), StoreError> {
    let Some(e) = tx.get(id) else { return Ok(()) };
    let (Some(d), Some(world)) = (e.linear_data(), world_points(e)) else { return Ok(()) };
    let (Some(first), Some(last)) = (world.first().copied(), world.last().copied()) else { return Ok(()) };
    let rebind = |b: &Option<Binding>, at: Point| -> Option<Binding> {
        let b = b.as_ref()?;
        let Some(r) = resolve(tx, &b.target) else { return Some(b.clone()) };
        let snapped = snap_to_outline(&r.target, r.gap, at, true, 1.0);
        Some(Binding {
            target: b.target.clone(),
            mode: BindMode::Orbit,
            fixed_point: fixed_point_for(&r.target, r.gap, snapped),
        })
    };
    let (sb, eb) = (rebind(&d.start_binding, first), rebind(&d.end_binding, last));
    tx.update(id, |slot| {
        if let ElementKind::Line(d) | ElementKind::Arrow(d) = &mut slot.kind {
            d.start_binding = sb;
            d.end_binding = eb;
        }
    })
}

/// After an arrow stops being an elbow: aims each binding at a point inside its shape, so the ends keep
/// following the shape the way simple arrows do.
pub(crate) fn reanchor_for_simple<T: SceneTx>(tx: &mut T, id: &ElementId) -> Result<(), StoreError> {
    let Some(e) = tx.get(id) else { return Ok(()) };
    let (Some(d), Some(world)) = (e.linear_data(), world_points(e)) else { return Ok(()) };
    let (Some(first), Some(last)) = (world.first().copied(), world.last().copied()) else { return Ok(()) };
    let rebind = |b: &Option<Binding>, at: Point| -> Option<Binding> {
        let b = b.as_ref()?;
        let Some(r) = resolve(tx, &b.target) else { return Some(b.clone()) };
        let inward = r.target.center() - at;
        let len = inward.hypot();
        let focus = if len > 0.0 { at + inward / len * (2.0 * r.gap).min(len) } else { at };
        Some(Binding {
            target: b.target.clone(),
            mode: BindMode::Orbit,
            fixed_point: fixed_point_for(&r.target, r.gap, focus),
        })
    };
    let (sb, eb) = (rebind(&d.start_binding, first), rebind(&d.end_binding, last));
    tx.update(id, |slot| {
        if let ElementKind::Line(d) | ElementKind::Arrow(d) = &mut slot.kind {
            d.start_binding = sb;
            d.end_binding = eb;
        }
    })
}

// ---------------------------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------------------------

fn is_jog(points: &[Point], i: usize) -> bool {
    if i < 2 || i + 2 > points.len() {
        return false;
    }
    let (a, b, c, d) = (points[i - 2], points[i - 1], points[i], points[i + 1]);
    (b - c).hypot() < 1.0 && (a - b).hypot() >= 1.0 && (c - d).hypot() >= 1.0 && (b - a).dot(d - c) > 0.0
}

/// The drawn path of an elbow route: straight legs, each corner rounded by a quadratic curve whose control
/// point is the corner. The radius shrinks so a corner never uses more than half of a leg. Jogs under one
/// unit are drawn straight.
pub fn rounded_path(points: &[Point], radius: f64) -> BezPath {
    let mut out = BezPath::new();
    let pts: Vec<Point> = points
        .iter()
        .enumerate()
        .filter(|(i, _)| !is_jog(points, *i) && !is_jog(points, *i + 1))
        .map(|(_, p)| *p)
        .collect();
    let (Some(first), Some(last)) = (pts.first(), pts.last()) else { return out };
    out.move_to(*first);
    let radius = if radius.is_finite() { radius.max(0.0) } else { 0.0 };
    for i in 1..pts.len().saturating_sub(1) {
        let (prev, p, next) = (pts[i - 1], pts[i], pts[i + 1]);
        let corner = radius.min((p - next).hypot() / 2.0).min((p - prev).hypot() / 2.0);
        let back = (p - prev).hypot();
        let fwd = (next - p).hypot();
        if back <= 0.0 || fwd <= 0.0 {
            out.line_to(p);
            continue;
        }
        let pre = p - (p - prev) / back * corner;
        let post = p + (next - p) / fwd * corner;
        out.line_to(pre);
        out.quad_to(p, post);
    }
    out.line_to(*last);
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn rounded_path_shrinks_the_radius_on_short_legs() {
        let pts = [Point::new(0.0, 0.0), Point::new(20.0, 0.0), Point::new(20.0, 100.0)];
        let p = rounded_path(&pts, 16.0);
        // corner = min(16, 10, 50) = 10: the curve starts 10 before the corner
        let els: Vec<_> = p.elements().to_vec();
        assert!(
            matches!(els[1], drafftink_geom::kurbo::PathEl::LineTo(q) if (q - Point::new(10.0, 0.0)).hypot() < 1e-9),
            "{els:?}"
        );
    }

    #[test]
    fn rounded_path_of_a_straight_line_is_a_line() {
        let p = rounded_path(&[Point::new(0.0, 0.0), Point::new(50.0, 0.0)], 16.0);
        assert_eq!(p.elements().len(), 2);
    }
}
