//! Finding what an arrow end binds to while it is dragged, and the highlight to show.
//!
//! The rules are Excalidraw's live ("simple") strategy (MIT):
//! * the target is the front-most bindable shape the point is inside or within [`max_binding_distance`] of;
//!   an opaque shape hides what is behind it, locked shapes hide but are never chosen, a smaller shape that
//!   mostly lies inside the nearest one wins when the point is inside it;
//! * a point inside the shape binds `Inside` (the end stays exactly there); a point in the margin binds
//!   `Orbit` (the end rides the outline plus gap, aimed at a focus point);
//! * Alt forces `Inside`; Ctrl turns binding off for the drag;
//! * both ends on one shape bind `Inside` (a self loop);
//! * elbow ends always bind `Orbit`, at a point snapped to the outline.

use drafftink_geom::aabb;
use drafftink_geom::binding::{BASE_BINDING_GAP, max_binding_distance};
use drafftink_geom::kurbo::{Point, Vec2};
use drafftink_geom::{HitOpts, Outline};
use drafftink_profile::{Counter, count};
use drafftink_scene::{BindMode, Binding, Element, ElementId, ElementKind, PathType, SceneRead, SceneTx};

use super::solve::{self, side};
use super::target::{Resolved, resolve_element};
use super::{BindingError, End};
use crate::elbow::{self, RouteOpts};
use crate::linear::{self, world_points};

/// Options of a binding drag.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BindOpts {
    /// Zoom (pixels per world unit). The reach shrinks in world units as you zoom out.
    pub zoom: f64,
    /// Alt: bind inside no matter where in the margin the point is.
    pub force_inside: bool,
    /// Binding is off for this drag (the user toggled it, or holds Ctrl).
    pub disabled: bool,
    /// Snap an orbit focus to a side midpoint when the point is near one.
    pub midpoint_snap: bool,
    /// Shift: the end is angle-locked, so midpoint snapping is off.
    pub angle_locked: bool,
}

impl Default for BindOpts {
    fn default() -> Self {
        Self { zoom: 1.0, force_inside: false, disabled: false, midpoint_snap: true, angle_locked: false }
    }
}

/// A shape an arrow end would bind to, for the UI to highlight.
#[derive(Debug, Clone, PartialEq)]
pub struct BindingCandidate {
    /// The shape to highlight: draw its own outline thicker.
    pub target: ElementId,
    /// Inside or orbit.
    pub mode: BindMode,
    /// The binding that would be stored.
    pub fixed_point: [f64; 2],
    /// The world point the arrow aims at (for inside, where the end sits).
    pub focus: Point,
    /// A side midpoint the focus snapped to; draw a dot there.
    pub snap_midpoint: Option<Point>,
    /// Signed distance from the point to the shape's outline: negative inside.
    pub signed_distance: f64,
}

impl BindingCandidate {
    /// The binding to store.
    pub fn binding(&self) -> Binding {
        Binding { target: self.target.clone(), mode: self.mode, fixed_point: self.fixed_point }
    }
}

/// What a drag does to one end.
#[derive(Debug, Clone, PartialEq)]
pub enum EndChange {
    /// Leave the end as it is.
    Keep,
    /// Free the end.
    Unbind,
    /// Bind the end.
    Bind(BindingCandidate),
}

/// What dragging an end does to both ends.
#[derive(Debug, Clone, PartialEq)]
pub struct DragPlan {
    /// The dragged end.
    pub dragged: EndChange,
    /// The other end (it only changes for a self loop).
    pub other: EndChange,
}

/// The shape found under a point.
#[derive(Debug, Clone, PartialEq)]
pub struct Hovered {
    /// The shape.
    pub id: ElementId,
    /// Signed distance from the point to its outline: negative inside.
    pub signed_distance: f64,
}

fn opaque(e: &Element) -> bool {
    match &e.kind {
        ElementKind::Image(_) => true,
        ElementKind::Rectangle | ElementKind::Diamond | ElementKind::Ellipse | ElementKind::Frame(_) => {
            e.style.fill_color.is_some_and(|c| c.a > 0)
        }
        _ => false,
    }
}

struct Cand<'a> {
    e: &'a Element,
    r: Resolved,
    d: f64,
}

/// The shape a dragged end at `p` binds to, or `None`. `skip` ids (the arrow itself, for instance) are ignored.
///
/// This looks at every element of the scene. With a large scene, a caller that has a spatial index asks
/// [`find_hovered_among`] with the elements near the point.
pub fn find_hovered<R: SceneRead>(scene: &R, p: Point, zoom: f64, skip: &[ElementId]) -> Option<Hovered> {
    count(Counter::ElementsScanned, u64::try_from(scene.len_all()).unwrap_or(u64::MAX));
    hovered_in(scene, scene.iter(), p, zoom, skip)
}

/// Like [`find_hovered`], looking only at the elements named in `near` (in any order; unknown and deleted ids are
/// ignored). `near` must hold every bindable element whose box is within the binding reach of `p`
/// ([`binding_reach`] around the point); the spatial index gives that with one query.
pub fn find_hovered_among<R: SceneRead>(
    scene: &R,
    near: &[ElementId],
    p: Point,
    zoom: f64,
    skip: &[ElementId],
) -> Option<Hovered> {
    let mut elements: Vec<&Element> = near.iter().filter_map(|id| scene.get(id)).filter(|e| !e.deleted).collect();
    elements.sort_by(|a, b| a.index.cmp(&b.index).then_with(|| a.id.cmp(&b.id)));
    hovered_in(scene, elements.into_iter(), p, zoom, skip)
}

/// How far from a shape's box a point can still bind to it, in world units at `zoom`. A spatial query for the
/// candidates of [`find_hovered_among`] uses this as its tolerance.
pub fn binding_reach(zoom: f64) -> f64 {
    max_binding_distance(zoom) + 1.0
}

/// `elements` back to front.
fn hovered_in<'a, R: SceneRead>(
    scene: &R,
    elements: impl Iterator<Item = &'a Element>,
    p: Point,
    zoom: f64,
    skip: &[ElementId],
) -> Option<Hovered> {
    if !p.x.is_finite() || !p.y.is_finite() {
        return None;
    }
    let max = max_binding_distance(zoom);
    let near: Vec<&Element> = elements
        .filter(|e| !skip.contains(&e.id))
        .filter(|e| match resolve_element(e) {
            Some(r) => aabb::contains_point(&aabb::inflate(r.target.frame.aabb(), max + 1.0), p),
            None => false,
        })
        .collect();
    let mut occluding_frame: Option<&ElementId> = None;
    let mut cands: Vec<Cand<'_>> = Vec::new();
    for e in near.into_iter().rev() {
        if let Some(of) = occluding_frame
            && e.frame_id.as_ref() != Some(of)
        {
            continue;
        }
        let Some(r) = resolve_element(e) else { continue };
        let is_frame = matches!(e.kind, ElementKind::Frame(_));
        let inside = r.target.contains(p);
        if is_frame && opaque(e) && inside {
            occluding_frame = Some(&e.id);
        }
        // A shape clipped by its frame cannot be bound outside the frame.
        if let Some(fid) = &e.frame_id
            && let Some(fr) = scene.get(fid).and_then(resolve_element)
            && !fr.target.contains(p)
        {
            continue;
        }
        // A frame can only be bound from outside.
        if is_frame && inside {
            continue;
        }
        let d = r.target.signed_distance(p);
        if d <= max {
            if !e.locked {
                cands.push(Cand { e, r, d });
            }
            if d <= 0.0 && opaque(e) {
                break;
            }
        }
    }
    if cands.is_empty() {
        return None;
    }
    cands.sort_by(|a, b| a.d.abs().total_cmp(&b.d.abs()));
    let first = &cands[0];
    if first.d <= 0.0 {
        // A smaller shape lying mostly inside the nearest one wins when the point is inside it.
        let fb = first.r.target.frame.aabb();
        let pick = cands.iter().skip(1).find(|c| {
            if c.d > 0.0 || c.r.area <= 0.0 {
                return false;
            }
            let cb = c.r.target.frame.aabb();
            let overlap = cb.intersect(fb);
            let overlap_area = overlap.width().max(0.0) * overlap.height().max(0.0);
            overlap_area / c.r.area > 0.25 && c.r.area / first.r.area.max(f64::MIN_POSITIVE) < 0.75
        });
        if let Some(c) = pick {
            return Some(Hovered { id: c.e.id.clone(), signed_distance: c.d });
        }
    }
    Some(Hovered { id: first.e.id.clone(), signed_distance: first.d })
}

fn candidate_for(
    r: &Resolved,
    mode: BindMode,
    focus: Point,
    snap_midpoint: Option<Point>,
    signed_distance: f64,
) -> BindingCandidate {
    let v = r.target.fixed_point_from_global(focus, r.gap);
    BindingCandidate { target: r.id.clone(), mode, fixed_point: [v.x, v.y], focus, snap_midpoint, signed_distance }
}

/// The highlight for a point with no arrow yet (the arrow tool hovering) for an arrow of this `path` type. Orbit is
/// shown for a point outside the shape; nothing is shown for a point inside (a click there binds `Inside` only
/// once the arrow is dragged), except for an elbow arrow, which snaps to the outline.
pub fn hover_candidate<R: SceneRead>(scene: &R, p: Point, path: PathType, opts: &BindOpts) -> Option<BindingCandidate> {
    if opts.disabled {
        return None;
    }
    let hit = find_hovered(scene, p, opts.zoom, &[])?;
    hover_candidate_for(scene, &hit, p, path, opts)
}

/// [`hover_candidate`] for a shape the caller found already (with [`find_hovered`] or [`find_hovered_among`]).
pub fn hover_candidate_for<R: SceneRead>(
    scene: &R,
    hit: &Hovered,
    p: Point,
    path: PathType,
    opts: &BindOpts,
) -> Option<BindingCandidate> {
    if opts.disabled {
        return None;
    }
    let elbow = path == PathType::Elbow;
    let r = resolve_element(scene.get(&hit.id)?)?;
    if r.target.contains(p) && !elbow {
        return None;
    }
    let (focus, snap) = if elbow {
        let q = elbow::snap_to_outline(&r.target, r.gap, p, opts.midpoint_snap, opts.zoom);
        (q, None)
    } else {
        orbit_focus(&r, p, None, None, opts)
    };
    Some(candidate_for(&r, BindMode::Orbit, focus, snap, hit.signed_distance))
}

/// The orbit focus for a point outside the shape: a side midpoint if one is near, else the point projected onto
/// the shape's diagonals as seen from `reference`, else the point itself.
fn orbit_focus(
    r: &Resolved,
    p: Point,
    reference: Option<Point>,
    arrow_size: Option<(f64, f64)>,
    opts: &BindOpts,
) -> (Point, Option<Point>) {
    if opts.midpoint_snap && !opts.angle_locked {
        let reach = max_binding_distance(opts.zoom) + (r.gap - BASE_BINDING_GAP).max(0.0);
        if let Some(m) = r.target.snap_side_midpoint(p, reach) {
            return (m, Some(m));
        }
    }
    let tiny = arrow_size.is_some_and(|(w, h)| w < 3.0 && h < 3.0);
    if let (Some(a), false) = (reference, tiny)
        && let Some(q) = r.target.project_onto_diagonals(p, a)
    {
        return (q, None);
    }
    // The ray misses the diagonals (a short or oblique arrow): aim at a point just inside the nearest edge, so the
    // end still rides the outline instead of staying where the pointer is.
    if let Some(n) = r.target.shape(0.0).nearest(p) {
        return (n.point - n.normal * 1.0, None);
    }
    (p, None)
}

/// Where a lookup of the shape under a point looks.
#[derive(Clone, Copy)]
enum Lookup<'a> {
    /// Every element of the scene.
    Scene,
    /// Only these elements, which must include every bindable one within reach of the point.
    Near(&'a [ElementId]),
}

impl Lookup<'_> {
    fn find<R: SceneRead>(self, scene: &R, p: Point, zoom: f64, skip: &[ElementId]) -> Option<Hovered> {
        match self {
            Lookup::Scene => find_hovered(scene, p, zoom, skip),
            Lookup::Near(near) => find_hovered_among(scene, near, p, zoom, skip),
        }
    }
}

/// What dragging `end` of `arrow` to `at` does to its bindings.
pub fn plan_end_drag<R: SceneRead>(scene: &R, arrow: &Element, end: End, at: Point, opts: &BindOpts) -> DragPlan {
    plan_end_drag_in(scene, Lookup::Scene, arrow, end, at, opts)
}

/// Like [`plan_end_drag`], looking for the shape under `at` among `near` only (see [`find_hovered_among`]).
pub fn plan_end_drag_among<R: SceneRead>(
    scene: &R,
    near: &[ElementId],
    arrow: &Element,
    end: End,
    at: Point,
    opts: &BindOpts,
) -> DragPlan {
    plan_end_drag_in(scene, Lookup::Near(near), arrow, end, at, opts)
}

fn plan_end_drag_in<R: SceneRead>(
    scene: &R,
    lookup: Lookup<'_>,
    arrow: &Element,
    end: End,
    at: Point,
    opts: &BindOpts,
) -> DragPlan {
    let keep = DragPlan { dragged: EndChange::Keep, other: EndChange::Keep };
    let Some(d) = arrow.linear_data() else { return keep };
    let (this_b, other_b) = match end {
        End::Start => (&d.start_binding, &d.end_binding),
        End::End => (&d.end_binding, &d.start_binding),
    };
    let free = DragPlan {
        dragged: if this_b.is_some() { EndChange::Unbind } else { EndChange::Keep },
        other: EndChange::Keep,
    };
    if opts.disabled {
        return free;
    }
    let Some(hit) = lookup.find(scene, at, opts.zoom, std::slice::from_ref(&arrow.id)) else { return free };
    let Some(r) = scene.get(&hit.id).and_then(resolve_element) else { return free };

    if d.path == PathType::Elbow {
        let q = elbow::snap_to_outline(&r.target, r.gap, at, opts.midpoint_snap, opts.zoom);
        let cand = candidate_for(&r, BindMode::Orbit, q, None, hit.signed_distance);
        return DragPlan { dragged: EndChange::Bind(cand), other: EndChange::Keep };
    }

    let inside = r.target.contains(at);
    // Both ends on one shape: a self loop, inside at both ends.
    if let Some(ob) = other_b
        && ob.target == r.id
    {
        let dragged = EndChange::Bind(candidate_for(&r, BindMode::Inside, at, None, hit.signed_distance));
        let other_focus = r.target.global_fixed_point(Vec2::new(ob.fixed_point[0], ob.fixed_point[1]));
        let other = EndChange::Bind(BindingCandidate {
            target: r.id.clone(),
            mode: BindMode::Inside,
            fixed_point: ob.fixed_point,
            focus: other_focus,
            snap_midpoint: None,
            signed_distance: r.target.signed_distance(other_focus),
        });
        return DragPlan { dragged, other };
    }
    if opts.force_inside || inside {
        let cand = candidate_for(&r, BindMode::Inside, at, None, hit.signed_distance);
        return DragPlan { dragged: EndChange::Bind(cand), other: EndChange::Keep };
    }
    // Orbit: aim at a focus point.
    let world = world_points(arrow).unwrap_or_default();
    let n = world.len();
    let mut reference = match end {
        End::Start => world.get(1).copied(),
        End::End => n.checked_sub(2).and_then(|i| world.get(i)).copied(),
    };
    if n == 2
        && let Some(os) = side(scene, other_b)
    {
        reference = Some(os.focus());
    }
    let (focus, snap) = orbit_focus(&r, at, reference, Some((arrow.transform.w, arrow.transform.h)), opts);
    let cand = candidate_for(&r, BindMode::Orbit, focus, snap, hit.signed_distance);
    DragPlan { dragged: EndChange::Bind(cand), other: EndChange::Keep }
}

fn apply_change(slot: &mut Option<Binding>, change: &EndChange) {
    match change {
        EndChange::Keep => {}
        EndChange::Unbind => *slot = None,
        EndChange::Bind(c) => *slot = Some(c.binding()),
    }
}

/// Drags one end of an arrow to `at`, binding it to a shape under the point or freeing it, and re-solves the
/// arrow. Call it on every pointer move while the end is dragged and at its start (to bind the start of a new
/// arrow). Returns the candidate to highlight, if the end is now bound.
pub fn drag_end<T: SceneTx>(
    tx: &mut T,
    arrow: &ElementId,
    end: End,
    at: Point,
    opts: &BindOpts,
) -> Result<Option<BindingCandidate>, BindingError> {
    drag_end_in(tx, Lookup::Scene, arrow, end, at, opts)
}

/// Like [`drag_end`], looking for the shape under `at` among `near` only (see [`find_hovered_among`]).
pub fn drag_end_among<T: SceneTx>(
    tx: &mut T,
    near: &[ElementId],
    arrow: &ElementId,
    end: End,
    at: Point,
    opts: &BindOpts,
) -> Result<Option<BindingCandidate>, BindingError> {
    drag_end_in(tx, Lookup::Near(near), arrow, end, at, opts)
}

fn drag_end_in<T: SceneTx>(
    tx: &mut T,
    lookup: Lookup<'_>,
    arrow: &ElementId,
    end: End,
    at: Point,
    opts: &BindOpts,
) -> Result<Option<BindingCandidate>, BindingError> {
    if !at.x.is_finite() || !at.y.is_finite() {
        return Err(BindingError::NotFinite);
    }
    let el = tx.get(arrow).ok_or_else(|| BindingError::Missing(arrow.clone()))?.clone();
    let mut world = world_points(&el).ok_or_else(|| BindingError::NotLinear(arrow.clone()))?;
    let n = world.len();
    if n < 2 {
        return Err(BindingError::TooFewPoints(arrow.clone()));
    }
    let plan = plan_end_drag_in(tx, lookup, &el, end, at, opts);

    let mut e = el;
    let elbow = e.linear_data().is_some_and(|d| d.path == PathType::Elbow);
    let idx = if end == End::Start { 0 } else { n - 1 };
    if let Some(slot) = world.get_mut(idx) {
        *slot = match (&plan.dragged, elbow) {
            (EndChange::Bind(c), true) => c.focus,
            _ => at,
        };
    }
    if let ElementKind::Line(d) | ElementKind::Arrow(d) = &mut e.kind {
        let (this_slot, other_slot) = match end {
            End::Start => (&mut d.start_binding, &mut d.end_binding),
            End::End => (&mut d.end_binding, &mut d.start_binding),
        };
        apply_change(this_slot, &plan.dragged);
        apply_change(other_slot, &plan.other);
    }
    let candidate = match &plan.dragged {
        EndChange::Bind(c) => Some(c.clone()),
        _ => None,
    };
    if elbow {
        let (first, last) = (world[0], world[n - 1]);
        tx.update(arrow, |slot| slot.kind = e.kind)?;
        elbow::reroute_with(tx, arrow, Some((first, last)), RouteOpts { zoom: opts.zoom })
            .map_err(|err| super::from_elbow(arrow, err))?;
        return Ok(candidate);
    }
    linear::set_world_points(&mut e, &world, linear::Turn::Keep);
    // Place the bound ends from the new bindings.
    if let Some(d) = e.linear_data() {
        let w = world_points(&e).unwrap_or_default();
        if let Some(pts) = solve::simple_points(tx, d, &w, true) {
            linear::set_world_points(&mut e, &pts, linear::Turn::Keep);
        }
    }
    tx.update(arrow, |slot| {
        slot.transform = e.transform;
        slot.kind = e.kind;
    })?;
    Ok(candidate)
}

/// Is the point inside or near the shape? A pure convenience for hit tests that need the same notion of "close"
/// as binding does.
pub fn within_binding_reach<R: SceneRead>(scene: &R, id: &ElementId, p: Point, zoom: f64) -> bool {
    scene
        .get(id)
        .and_then(resolve_element)
        .is_some_and(|r| r.target.shape(0.0).hit_test(p, HitOpts::filled(max_binding_distance(zoom), 0.0)))
}
