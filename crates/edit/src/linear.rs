//! Lines and arrows: editing their points and ends, arrowhead and path-type changes, and the queries the
//! point editor needs (handles, midpoints, curves).
//!
//! Points are stored in the element's local frame (see `LinearData`). The functions here take and return
//! **world** positions, so callers never deal with the box, the rotation or the refit that keeps the
//! stored points at the origin. Every function that changes points ends with the arrow's own bound ends
//! re-solved, so a bound arrow is never left stale.
//!
//! The rules follow Excalidraw's linear element editor (MIT): moving one end of a bound arrow frees that
//! end, deleting an end point frees the end it belonged to, elbow arrows only have two movable points,
//! and a curved line passes through its points as a Catmull-Rom spline.

use drafftink_scene::{ElementId, ElementKind, Head, LinearData, PathType, SceneRead, SceneTx, StoreError, Transform};
use kurbo::{CubicBez, ParamCurve, ParamCurveArclen, Point};

use crate::binding::{End, update_bound_arrows};
use crate::elbow::{self, ElbowEditError, RouteOpts};

/// Radius of a point handle on screen, in pixels.
pub const POINT_HANDLE_SIZE: f64 = 10.0;
/// A midpoint handle is hidden when its segment is shorter than this on screen (pixels).
pub const MIDPOINT_MIN_SCREEN_LENGTH: f64 = POINT_HANDLE_SIZE * 4.0;
/// A midpoint handle of an elbow segment is hidden below this length on screen (pixels).
pub const ELBOW_MIDPOINT_MIN_SCREEN_LENGTH: f64 = POINT_HANDLE_SIZE / 2.0;
/// A point handle is hit within this many pixels of its centre.
pub const POINT_HIT_RADIUS: f64 = POINT_HANDLE_SIZE + 1.0;

/// Why a line or arrow edit was refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LinearError {
    /// The element does not exist.
    #[error("element {0} does not exist")]
    Missing(ElementId),
    /// The element is not a line or an arrow.
    #[error("element {0} is not a line or an arrow; only lines and arrows have points")]
    NotLinear(ElementId),
    /// A point index is outside the element.
    #[error("point {index} does not exist; this element has {len} points (0 to {})", .len.saturating_sub(1))]
    NoSuchPoint {
        /// The index asked for.
        index: usize,
        /// How many points the element has.
        len: usize,
    },
    /// The edit would leave too few points.
    #[error("a {kind} needs at least {min} points and this edit would leave {have}; delete the element instead")]
    TooFewPoints {
        /// Line or arrow.
        kind: &'static str,
        /// The fewest points allowed.
        min: usize,
        /// How many would be left.
        have: usize,
    },
    /// A coordinate is not finite.
    #[error("a point needs finite coordinates")]
    NotFinite,
    /// An elbow arrow has only its two ends as points.
    #[error("an elbow arrow has only two movable points, its start and its end; use a segment handle to reshape it")]
    ElbowInteriorPoint,
    /// A closed line cannot be an elbow.
    #[error("a closed line cannot be an elbow; open it first")]
    ClosedElbow,
    /// The elbow router refused.
    #[error(transparent)]
    Elbow(#[from] ElbowEditError),
    /// The store refused the change.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Binding failed.
    #[error(transparent)]
    Binding(#[from] crate::binding::BindingError),
}

// ---------------------------------------------------------------------------------------------
// Frames
// ---------------------------------------------------------------------------------------------

/// The world position of every point of a line or arrow. `None` for other kinds.
pub fn world_points(e: &drafftink_scene::Element) -> Option<Vec<Point>> {
    let pts = e.points().filter(|_| e.tag().is_linear())?;
    Some(pts.iter().map(|p| e.transform.local_to_world(*p)).collect())
}

/// The inverse of `Transform::local_to_world`.
pub fn to_local(t: &Transform, p: Point) -> Point {
    let c = t.center();
    let (s, co) = (-t.angle).sin_cos();
    let v = p - c;
    Point::new(v.x * co - v.y * s + t.w / 2.0, v.x * s + v.y * co + t.h / 2.0)
}

/// Replaces the points of a line or arrow by `world` and refits the box. With `upright` the element's
/// rotation is reset to 0 first (elbow arrows cannot be rotated). Returns false, changing nothing, for a
/// wrong kind, no points or a non-finite coordinate.
pub fn set_world_points(e: &mut drafftink_scene::Element, world: &[Point], upright: bool) -> bool {
    if world.is_empty() || world.iter().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
        return false;
    }
    if !matches!(e.kind, ElementKind::Line(_) | ElementKind::Arrow(_)) {
        return false;
    }
    let before = e.transform;
    if upright {
        e.transform = Transform::new(0.0, 0.0, 0.0, 0.0);
    }
    let t = e.transform;
    let local: Vec<Point> = world.iter().map(|p| to_local(&t, *p)).collect();
    let (ElementKind::Line(d) | ElementKind::Arrow(d)) = &mut e.kind else { return false };
    let old_points = std::mem::replace(&mut d.points, local);
    if e.refit_points() {
        true
    } else {
        if let ElementKind::Line(d) | ElementKind::Arrow(d) = &mut e.kind {
            d.points = old_points;
        }
        e.transform = before;
        false
    }
}

/// Size of an arrowhead in world units. Elbow routing uses it to leave room for the head.
pub fn head_size(h: Head) -> f64 {
    match h {
        Head::None => 0.0,
        Head::Arrow => 25.0,
        Head::Diamond | Head::DiamondOutline => 12.0,
        Head::CardinalityMany | Head::CardinalityOneOrMany | Head::CardinalityZeroOrMany => 15.0,
        Head::CardinalityOne | Head::CardinalityExactlyOne | Head::CardinalityZeroOrOne => 20.0,
        Head::Bar | Head::Circle | Head::CircleOutline | Head::Triangle | Head::TriangleOutline => 15.0,
    }
}

pub(crate) fn linear_of<R: SceneRead>(
    scene: &R,
    id: &ElementId,
) -> Result<(drafftink_scene::Element, Vec<Point>), LinearError> {
    let e = scene.get(id).ok_or_else(|| LinearError::Missing(id.clone()))?;
    let world = world_points(e).ok_or_else(|| LinearError::NotLinear(id.clone()))?;
    if world.len() < 2 {
        return Err(LinearError::TooFewPoints { kind: e.tag().name(), min: 2, have: world.len() });
    }
    Ok((e.clone(), world))
}

fn commit<T: SceneTx>(tx: &mut T, id: &ElementId, e: drafftink_scene::Element) -> Result<(), LinearError> {
    tx.update(id, |slot| {
        slot.transform = e.transform;
        slot.kind = e.kind;
    })?;
    Ok(())
}

fn data_mut(e: &mut drafftink_scene::Element) -> Option<&mut LinearData> {
    match &mut e.kind {
        ElementKind::Line(d) | ElementKind::Arrow(d) => Some(d),
        _ => None,
    }
}

fn min_points(e: &drafftink_scene::Element) -> usize {
    match e.linear_data() {
        Some(d) if d.closed => 3,
        _ => 2,
    }
}

fn is_elbow(e: &drafftink_scene::Element) -> bool {
    e.linear_data().is_some_and(|d| d.path == PathType::Elbow)
}

fn finish<T: SceneTx>(tx: &mut T, id: &ElementId) -> Result<(), LinearError> {
    update_bound_arrows(tx, std::slice::from_ref(id))?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Editing points
// ---------------------------------------------------------------------------------------------

/// Moves points to world positions. Moving an end of a bound arrow frees that end (use
/// `binding::drag_end` to drag an end onto a shape). On an elbow arrow only the two ends can move, and the
/// route is made again.
pub fn move_points<T: SceneTx>(tx: &mut T, id: &ElementId, moves: &[(usize, Point)]) -> Result<(), LinearError> {
    let (mut e, mut world) = linear_of(tx, id)?;
    let len = world.len();
    for (i, p) in moves {
        if *i >= len {
            return Err(LinearError::NoSuchPoint { index: *i, len });
        }
        if !p.x.is_finite() || !p.y.is_finite() {
            return Err(LinearError::NotFinite);
        }
    }
    let elbow = is_elbow(&e);
    if elbow && moves.iter().any(|(i, _)| *i != 0 && *i + 1 != len) {
        return Err(LinearError::ElbowInteriorPoint);
    }
    for (i, p) in moves {
        if let Some(w) = world.get_mut(*i) {
            *w = *p;
        }
        if let Some(d) = data_mut(&mut e) {
            if *i == 0 {
                d.start_binding = None;
            }
            if *i + 1 == len {
                d.end_binding = None;
            }
        }
    }
    if elbow {
        commit(tx, id, e)?;
        elbow::reroute_with(tx, id, Some((world[0], world[len - 1])), RouteOpts::default())?;
    } else {
        set_world_points(&mut e, &world, false);
        commit(tx, id, e)?;
    }
    finish(tx, id)
}

/// Inserts a point at `index` (0 to the number of points). Inserting before the first or after the last
/// point makes it the new end and frees the binding of the end it replaces. Elbow arrows refuse.
pub fn add_point<T: SceneTx>(tx: &mut T, id: &ElementId, index: usize, at: Point) -> Result<(), LinearError> {
    let (mut e, mut world) = linear_of(tx, id)?;
    let len = world.len();
    if index > len {
        return Err(LinearError::NoSuchPoint { index, len });
    }
    if !at.x.is_finite() || !at.y.is_finite() {
        return Err(LinearError::NotFinite);
    }
    if is_elbow(&e) {
        return Err(LinearError::ElbowInteriorPoint);
    }
    world.insert(index, at);
    if let Some(d) = data_mut(&mut e) {
        if index == 0 {
            d.start_binding = None;
        }
        if index == len {
            d.end_binding = None;
        }
    }
    set_world_points(&mut e, &world, false);
    commit(tx, id, e)?;
    finish(tx, id)
}

/// Inserts a point in segment `segment` (1 to len-1, the index of the segment's end point) at `at` and
/// returns the new point's index. This is what dragging a midpoint handle does.
pub fn insert_midpoint<T: SceneTx>(
    tx: &mut T,
    id: &ElementId,
    segment: usize,
    at: Point,
) -> Result<usize, LinearError> {
    let (_, world) = linear_of(tx, id)?;
    if segment < 1 || segment >= world.len() {
        return Err(LinearError::NoSuchPoint { index: segment, len: world.len() });
    }
    add_point(tx, id, segment, at)?;
    Ok(segment)
}

/// What `delete_points` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deleted {
    /// How many points were removed.
    pub removed: usize,
}

/// Deletes the points at `indices` (duplicates are ignored). Deleting the first or last point frees the
/// binding of that end. The element keeps at least 2 points (3 for a closed line); an edit that would
/// leave fewer is refused. Elbow arrows refuse: their route is made, not edited point by point.
pub fn delete_points<T: SceneTx>(tx: &mut T, id: &ElementId, indices: &[usize]) -> Result<Deleted, LinearError> {
    let (mut e, world) = linear_of(tx, id)?;
    let len = world.len();
    if let Some(bad) = indices.iter().find(|i| **i >= len) {
        return Err(LinearError::NoSuchPoint { index: *bad, len });
    }
    if is_elbow(&e) {
        return Err(LinearError::ElbowInteriorPoint);
    }
    let mut drop: Vec<usize> = indices.to_vec();
    drop.sort_unstable();
    drop.dedup();
    let have = len - drop.len();
    let min = min_points(&e);
    if have < min {
        let kind = e.tag().name();
        return Err(LinearError::TooFewPoints { kind, min, have });
    }
    if drop.is_empty() {
        return Ok(Deleted { removed: 0 });
    }
    let kept: Vec<Point> =
        world.iter().enumerate().filter(|(i, _)| drop.binary_search(i).is_err()).map(|(_, p)| *p).collect();
    if let Some(d) = data_mut(&mut e) {
        if drop.first() == Some(&0) {
            d.start_binding = None;
        }
        if drop.last() == Some(&(len - 1)) {
            d.end_binding = None;
        }
    }
    set_world_points(&mut e, &kept, false);
    commit(tx, id, e)?;
    finish(tx, id)?;
    Ok(Deleted { removed: drop.len() })
}

// ---------------------------------------------------------------------------------------------
// Heads and path type
// ---------------------------------------------------------------------------------------------

/// Sets the marker at one end. An elbow route is made again, as the head needs room.
pub fn set_head<T: SceneTx>(tx: &mut T, id: &ElementId, end: End, head: Head) -> Result<(), LinearError> {
    let (mut e, _) = linear_of(tx, id)?;
    let Some(d) = data_mut(&mut e) else { return Err(LinearError::NotLinear(id.clone())) };
    match end {
        End::Start => d.start_head = head,
        End::End => d.end_head = head,
    }
    let elbow = d.path == PathType::Elbow;
    commit(tx, id, e)?;
    if elbow {
        elbow::reroute_forced(tx, id, RouteOpts::default())?;
    }
    finish(tx, id)
}

/// Swaps the markers of the two ends.
pub fn swap_heads<T: SceneTx>(tx: &mut T, id: &ElementId) -> Result<(), LinearError> {
    let (mut e, _) = linear_of(tx, id)?;
    let Some(d) = data_mut(&mut e) else { return Err(LinearError::NotLinear(id.clone())) };
    std::mem::swap(&mut d.start_head, &mut d.end_head);
    let elbow = d.path == PathType::Elbow;
    commit(tx, id, e)?;
    if elbow {
        elbow::reroute_forced(tx, id, RouteOpts::default())?;
    }
    finish(tx, id)
}

/// Changes how the points are joined: straight (`Sharp`), curved (`Round`) or elbow.
///
/// * Straight and curved keep the points and the bindings.
/// * To elbow keeps only the two ends, binds them the elbow way (snapped to the outline) and routes.
/// * From elbow keeps the route's points, drops the pins and re-anchors the bindings inside their shapes so
///   the ends keep following them.
pub fn set_path<T: SceneTx>(tx: &mut T, id: &ElementId, path: PathType) -> Result<(), LinearError> {
    let (mut e, world) = linear_of(tx, id)?;
    let Some(d) = data_mut(&mut e) else { return Err(LinearError::NotLinear(id.clone())) };
    let from = d.path;
    if from == path {
        return Ok(());
    }
    if path == PathType::Elbow && d.closed {
        return Err(LinearError::ClosedElbow);
    }
    d.path = path;
    if path == PathType::Elbow {
        let ends = (world[0], world[world.len() - 1]);
        d.fixed_segments.clear();
        d.start_is_special = false;
        d.end_is_special = false;
        commit(tx, id, e)?;
        elbow::bind_ends_for_elbow(tx, id)?;
        elbow::reroute_with(tx, id, Some(ends), RouteOpts::default())?;
    } else if from == PathType::Elbow {
        d.fixed_segments.clear();
        d.start_is_special = false;
        d.end_is_special = false;
        commit(tx, id, e)?;
        elbow::reanchor_for_simple(tx, id)?;
    } else {
        commit(tx, id, e)?;
    }
    finish(tx, id)
}

// ---------------------------------------------------------------------------------------------
// Queries for the point editor
// ---------------------------------------------------------------------------------------------

/// One cubic per segment of a curved line: the Catmull-Rom spline through `points` with tension 0.5.
/// Two points give a straight cubic.
pub fn curve_segments(points: &[Point]) -> Vec<CubicBez> {
    let n = points.len();
    let mut out = Vec::with_capacity(n.saturating_sub(1));
    for i in 0..n.saturating_sub(1) {
        let p0 = points[i.saturating_sub(1)];
        let (p1, p2) = (points[i], points[i + 1]);
        let p3 = points[(i + 2).min(n - 1)];
        let c1 = p1 + (p2 - p0) / 6.0;
        let c2 = p2 - (p3 - p1) / 6.0;
        out.push(CubicBez::new(p1, c1, c2, p2));
    }
    out
}

/// A segment midpoint handle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MidHandle {
    /// The segment: it joins point `segment - 1` and point `segment`.
    pub segment: usize,
    /// Where the handle is, in world space. Half the arc length along a curved segment.
    pub at: Point,
}

/// The midpoint handles of a line or arrow, in world space. A segment shorter than the minimum on screen
/// (`zoom` pixels per world unit) has none. Curved segments put the handle at half their length. An elbow
/// arrow has a handle per segment.
pub fn midpoint_handles(e: &drafftink_scene::Element, zoom: f64) -> Vec<MidHandle> {
    let Some(world) = world_points(e) else { return Vec::new() };
    let Some(d) = e.linear_data() else { return Vec::new() };
    let zoom = if zoom.is_finite() && zoom > 0.0 { zoom } else { 1.0 };
    let min = if d.path == PathType::Elbow { ELBOW_MIDPOINT_MIN_SCREEN_LENGTH } else { MIDPOINT_MIN_SCREEN_LENGTH };
    let curves = (d.path == PathType::Round && world.len() > 2).then(|| curve_segments(&world));
    let mut out = Vec::new();
    for i in 1..world.len() {
        let (a, b) = (world[i - 1], world[i]);
        let (len, at) = match curves.as_ref().and_then(|c| c.get(i - 1)) {
            Some(c) => {
                let total = c.arclen(1e-3);
                (total, c.eval(c.inv_arclen(total / 2.0, 1e-3)))
            }
            None => ((b - a).hypot(), a.midpoint(b)),
        };
        if len * zoom >= min {
            out.push(MidHandle { segment: i, at });
        }
    }
    out
}

/// The index of the point under `p`, last point first (later points are drawn on top). `zoom` is pixels
/// per world unit.
pub fn point_at(e: &drafftink_scene::Element, p: Point, zoom: f64) -> Option<usize> {
    let world = world_points(e)?;
    let zoom = if zoom.is_finite() && zoom > 0.0 { zoom } else { 1.0 };
    world.iter().enumerate().rev().find(|(_, q)| (**q - p).hypot() * zoom < POINT_HIT_RADIUS).map(|(i, _)| i)
}

/// The shortest distance from `p` to the line or arrow (to its curve when curved, to its stored route when
/// elbow), or `None` for other kinds. The arrowheads are not part of it.
pub fn distance_to(e: &drafftink_scene::Element, p: Point) -> Option<f64> {
    use drafftink_geom::{Outline, Polyline};
    let world = world_points(e)?;
    let d = e.linear_data()?;
    if d.path == PathType::Round && world.len() > 2 {
        let mut path = kurbo::BezPath::new();
        path.move_to(*world.first()?);
        for c in curve_segments(&world) {
            path.curve_to(c.p1, c.p2, c.p3);
        }
        let shape = drafftink_geom::PathShape::new(path);
        return Some(shape.outline_distance(p));
    }
    Some(Polyline::new(world, d.closed).outline_distance(p))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn two_points_make_a_straight_cubic() {
        let c = curve_segments(&[Point::new(0.0, 0.0), Point::new(90.0, 0.0)]);
        assert_eq!(c.len(), 1);
        assert!((c[0].eval(0.5) - Point::new(45.0, 0.0)).hypot() < 1e-9);
    }

    #[test]
    fn catmull_rom_control_points() {
        let c = curve_segments(&[Point::new(0.0, 0.0), Point::new(100.0, 0.0), Point::new(100.0, 100.0)]);
        assert_eq!(c.len(), 2);
        assert!((c[0].p1 - Point::new(100.0 / 6.0, 0.0)).hypot() < 1e-9);
        assert!((c[0].p2 - Point::new(100.0 - 100.0 / 6.0, -100.0 / 6.0)).hypot() < 1e-9);
        assert!((c[1].p1 - Point::new(100.0 + 100.0 / 6.0, 100.0 / 6.0)).hypot() < 1e-9);
        assert!((c[1].p2 - Point::new(100.0, 100.0 - 100.0 / 6.0)).hypot() < 1e-9);
    }

    #[test]
    fn local_and_world_round_trip() {
        let mut t = Transform::new(10.0, 20.0, 30.0, 40.0);
        t.angle = 0.7;
        let p = Point::new(5.0, 9.0);
        let back = to_local(&t, t.local_to_world(p));
        assert!((back - p).hypot() < 1e-9);
    }
}
