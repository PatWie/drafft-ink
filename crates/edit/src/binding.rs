//! Arrow bindings: acquiring, keeping and releasing the attachment of arrow ends to shapes.
//!
//! Contract used by other modules (keep this signature): after any change to the geometry of elements
//! (move, resize, rotate, flip, delete, restore), the caller passes the changed ids to
//! [`update_bound_arrows`] inside the same transaction, and every arrow bound to one of them is re-solved
//! once. Arrows named in `changed` are re-solved too.
//!
//! The design (`docs/model.md`, research 02f): the binding is stored **on the arrow only**
//! (`start_binding` / `end_binding`: target, inside or orbit, fixed point). The reverse index is derived
//! (`DerivedIndex::bound_arrows`) and never stored. An end's position is derived from its binding by the
//! solver in `solve`; the stored point is a cache of it.
//!
//! * [`update_bound_arrows`]: re-solve after shapes changed (one pass per transaction, sorted by id).
//! * [`find_hovered`], [`hover_candidate`], [`plan_end_drag`], [`drag_end`]: acquisition while an end is dragged,
//!   with the [`BindingCandidate`] to highlight.
//! * [`unbind_end`], [`unbind_detached`], [`free_ends_bound_to`], [`remap_bindings`], [`heal_bindings`],
//!   [`mirror_fixed_points`], [`reanchor_to_outline`], [`can_move_with`]: the rules for transforms, deletes,
//!   copies and loads.
//!
//! The algorithms are ported from Excalidraw's `binding.ts` (MIT) in our own code.

mod acquire;
mod lifecycle;
mod preview;
mod solve;
mod target;

use std::collections::{BTreeSet, HashSet};

use drafftink_geom::kurbo::Rect;
use drafftink_profile::{Counter, count};
use drafftink_scene::{DerivedIndex, ElementId, PathType, SceneRead, SceneTx, StoreError};

pub use acquire::{
    BindOpts, BindingCandidate, DragPlan, EndChange, Hovered, binding_reach, drag_end, drag_end_among, find_hovered,
    find_hovered_among, hover_candidate, hover_candidate_for, plan_end_drag, plan_end_drag_among, within_binding_reach,
};
pub use lifecycle::{
    can_move_with, free_ends_bound_to, free_ends_bound_to_from, free_ends_bound_to_with, free_unreachable_ends,
    heal_bindings, mirror_fixed_points, reanchor_to_outline, remap_bindings, unbind_detached, unbind_end,
};
pub use preview::BoundArrows;
pub use solve::bound_end_position;
pub use target::{Resolved, bind_target, is_bindable, resolve};

use crate::elbow::{self, ElbowError, RouteOpts};
use crate::linear::{Turn, set_world_points, world_points};

/// Which end of a line or arrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum End {
    /// The first point.
    Start,
    /// The last point.
    End,
}

impl End {
    /// The other end.
    pub fn other(self) -> End {
        match self {
            End::Start => End::End,
            End::End => End::Start,
        }
    }
}

/// Errors from binding rules.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum BindingError {
    /// The element does not exist.
    #[error("element {0} does not exist")]
    Missing(ElementId),
    /// The element is not a line or an arrow.
    #[error("element {0} is not a line or an arrow, so it has no ends to bind")]
    NotLinear(ElementId),
    /// A coordinate is not finite.
    #[error("a binding needs finite coordinates")]
    NotFinite,
    /// The line or arrow has fewer than two points.
    #[error("element {0} has fewer than 2 points, so it has no two ends")]
    TooFewPoints(ElementId),
    /// The elbow router refused for this arrow.
    #[error("arrow {arrow} could not be routed: {source}")]
    Elbow {
        /// The arrow.
        arrow: ElementId,
        /// Why.
        #[source]
        source: ElbowError,
    },
    /// The store refused the change.
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Re-solve every arrow bound to any of `changed` (and arrows in `changed` themselves).
/// Returns the ids of arrows whose geometry changed, sorted.
///
/// The arrows are found with the reader's committed reverse index when it offers one (a store, or a transaction
/// that has not written yet), so the cost follows the number of bound arrows. Otherwise the call scans the
/// transaction's elements once. A caller that writes before it calls, and that has many elements, takes the
/// candidates first with [`arrows_bound_to`] and calls [`update_bound_arrows_from`].
pub fn update_bound_arrows<T: SceneTx>(tx: &mut T, changed: &[ElementId]) -> Result<Vec<ElementId>, BindingError> {
    update_bound_arrows_near(tx, changed, &[])
}

/// Like [`update_bound_arrows`], and the elbow arrows near the shapes in `changed` are routed again as well,
/// because such a shape may now be in their way, or may no longer be (see [`elbow::arrows_near`]). `left` are the
/// boxes where changed shapes used to be (a shape that moved away or was deleted), which the caller reads
/// from before the change. Only arrows whose box touches one of these places are looked at, found with the
/// scene's box index.
pub fn update_bound_arrows_near<T: SceneTx>(
    tx: &mut T,
    changed: &[ElementId],
    left: &[Rect],
) -> Result<Vec<ElementId>, BindingError> {
    if changed.is_empty() && left.is_empty() {
        return Ok(Vec::new());
    }
    let places: Vec<Rect> = changed
        .iter()
        .filter_map(|id| tx.get(id))
        .filter(|e| !e.deleted && elbow::is_obstacle(e))
        .map(elbow::obstacle_box)
        .chain(left.iter().copied())
        .collect();
    let (candidates, near) = if tx.derived_index().is_some() {
        (arrows_bound_to(&*tx, changed), elbow::arrows_near(&*tx, &places))
    } else {
        // One pass finds both.
        count(Counter::ElementsScanned, u64::try_from(tx.len_all()).unwrap_or(u64::MAX));
        let set: HashSet<&ElementId> = changed.iter().collect();
        let (mut bound, mut near) = (BTreeSet::new(), Vec::new());
        for e in tx.iter() {
            if e.binding_targets().into_iter().flatten().any(|t| set.contains(t)) {
                bound.insert(e.id.clone());
            }
            if elbow::is_near(e, &places) {
                near.push(e.id.clone());
            }
        }
        (bound, near)
    };
    solve_changed(tx, changed, candidates, near)
}

/// The lines and arrows with an end bound to any of `ids`, as the reader sees them now. With a derived index this
/// is a lookup per id; without one it is a pass over the scene.
pub fn arrows_bound_to<R: SceneRead>(scene: &R, ids: &[ElementId]) -> BTreeSet<ElementId> {
    if let Some(index) = scene.derived_index() {
        return ids.iter().flat_map(|id| index.bound_arrows(id)).cloned().collect();
    }
    count(Counter::ElementsScanned, u64::try_from(scene.len_all()).unwrap_or(u64::MAX));
    let set: HashSet<&ElementId> = ids.iter().collect();
    scene
        .iter()
        .filter(|e| e.binding_targets().into_iter().flatten().any(|t| set.contains(t)))
        .map(|e| e.id.clone())
        .collect()
}

/// Like [`update_bound_arrows`], with the arrows bound to `changed` given by the caller (from
/// [`arrows_bound_to`], taken before it wrote). A candidate that is no longer bound to a changed id is left
/// alone. Arrows named in `changed` are re-solved when they are bound or elbow arrows.
pub fn update_bound_arrows_from<T: SceneTx>(
    tx: &mut T,
    changed: &[ElementId],
    candidates: BTreeSet<ElementId>,
) -> Result<Vec<ElementId>, BindingError> {
    if changed.is_empty() {
        return Ok(Vec::new());
    }
    solve_changed(tx, changed, candidates, Vec::new())
}

/// Solves the arrows bound to `changed` (from `candidates`) and the arrows in `changed`, then `near`, which are
/// elbow arrows routed again whether or not their ends moved.
fn solve_changed<T: SceneTx>(
    tx: &mut T,
    changed: &[ElementId],
    candidates: BTreeSet<ElementId>,
    near: Vec<ElementId>,
) -> Result<Vec<ElementId>, BindingError> {
    let set: HashSet<&ElementId> = changed.iter().collect();
    let mut affected: BTreeSet<ElementId> = BTreeSet::new();
    for id in candidates.iter().chain(changed) {
        let Some(e) = tx.get(id) else { continue };
        let Some(d) = e.linear_data() else { continue };
        let [a, b] = e.binding_targets();
        let bound_to_changed = a.is_some_and(|t| set.contains(t)) || b.is_some_and(|t| set.contains(t));
        let own = set.contains(&e.id) && (a.is_some() || b.is_some() || d.path == PathType::Elbow);
        if bound_to_changed || own {
            affected.insert(e.id.clone());
        }
    }
    let forced: BTreeSet<ElementId> = near.into_iter().filter(|id| !affected.contains(id)).collect();
    solve_all(tx, affected, forced)
}

/// Like [`update_bound_arrows`], finding the arrows with the committed reverse index instead of scanning. Arrows
/// that gained a binding earlier in the same transaction are only found if they are in `changed`.
pub fn update_bound_arrows_indexed<T: SceneTx>(
    tx: &mut T,
    changed: &[ElementId],
    derived: &DerivedIndex,
) -> Result<Vec<ElementId>, BindingError> {
    let mut affected: BTreeSet<ElementId> = BTreeSet::new();
    for id in changed {
        affected.extend(derived.bound_arrows(id).cloned());
        if let Some(e) = tx.get(id)
            && e.tag().is_linear()
        {
            affected.insert(id.clone());
        }
    }
    solve_all(tx, affected, BTreeSet::new())
}

/// Solves `affected` (as their bindings say) and `forced` (elbow arrows routed again even if their ends are where
/// they were). The shapes each elbow arrow goes around are read before the first write, while the scene's box
/// index is still exact for the reader.
fn solve_all<T: SceneTx>(
    tx: &mut T,
    affected: BTreeSet<ElementId>,
    forced: BTreeSet<ElementId>,
) -> Result<Vec<ElementId>, BindingError> {
    let finder = elbow::ObstacleFinder::new();
    let plan: Vec<(ElementId, bool, Vec<elbow::Bounds>)> = affected
        .into_iter()
        .map(|id| (id, false))
        .chain(forced.into_iter().map(|id| (id, true)))
        .map(|(id, force)| {
            let around = elbow::obstacles_of(&*tx, &id, &finder);
            (id, force, around)
        })
        .collect();
    let mut changed = Vec::new();
    for (id, force, around) in plan {
        if solve_one(tx, &id, force, &around)? {
            changed.push(id);
        }
    }
    changed.sort();
    Ok(changed)
}

pub(crate) fn from_elbow(arrow: &ElementId, err: elbow::ElbowEditError) -> BindingError {
    match err {
        elbow::ElbowEditError::Route(source) => BindingError::Elbow { arrow: arrow.clone(), source },
        elbow::ElbowEditError::Store(s) => BindingError::Store(s),
        elbow::ElbowEditError::Missing(m) | elbow::ElbowEditError::NotElbow(m) => BindingError::Missing(m),
    }
}

const SAME: f64 = 1e-9;

fn same_points(a: &[drafftink_geom::kurbo::Point], b: &[drafftink_geom::kurbo::Point]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(p, q)| (*p - *q).hypot() <= SAME)
}

fn solve_one<T: SceneTx>(
    tx: &mut T,
    id: &ElementId,
    force: bool,
    around: &[elbow::Bounds],
) -> Result<bool, BindingError> {
    let Some(e) = tx.get(id) else { return Ok(false) };
    if e.deleted {
        return Ok(false);
    }
    let Some(d) = e.linear_data() else { return Ok(false) };
    if d.path == PathType::Elbow {
        return elbow::reroute_among(tx, id, around, RouteOpts::default(), force).map_err(|err| from_elbow(id, err));
    }
    let Some(world) = world_points(e) else { return Ok(false) };
    let Some(next) = solve::simple_points(tx, d, &world, false) else { return Ok(false) };
    if same_points(&world, &next) {
        return Ok(false);
    }
    let mut updated = e.clone();
    if !set_world_points(&mut updated, &next, Turn::Keep) {
        return Ok(false);
    }
    tx.update(id, |slot| {
        slot.transform = updated.transform;
        slot.kind = updated.kind;
    })?;
    Ok(true)
}

/// Is `scene` free of arrows whose bound ends sit away from where their bindings put them? Returns the
/// ids of the stale arrows. For tests and consistency checks.
pub fn stale_arrows<R: SceneRead>(scene: &R) -> Vec<ElementId> {
    let mut out = Vec::new();
    for e in scene.iter() {
        let Some(d) = e.linear_data() else { continue };
        if d.start_binding.is_none() && d.end_binding.is_none() {
            continue;
        }
        let Some(world) = world_points(e).filter(|w| w.len() >= 2) else { continue };
        if d.path == PathType::Elbow {
            let Some((s, en)) = elbow::ends_for(scene, d, &world) else { continue };
            let (first, last) = (world[0], world[world.len() - 1]);
            let bad = (s.bound.is_some() && (s.point - first).hypot() > 1e-6)
                || (en.bound.is_some() && (en.point - last).hypot() > 1e-6);
            if bad {
                out.push(e.id.clone());
            }
        } else if let Some(next) = solve::simple_points(scene, d, &world, false)
            && !next.iter().zip(&world).all(|(a, b)| (*a - *b).hypot() < 1e-6)
        {
            out.push(e.id.clone());
        }
    }
    out
}
