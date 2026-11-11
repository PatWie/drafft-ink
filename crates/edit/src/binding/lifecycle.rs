//! Keeping bindings consistent when elements are transformed, deleted, copied or loaded.
//!
//! Bindings live on the arrow only; there is no reverse list to repair. Every function here is total: a missing
//! arrow or target is a value, not a crash, and none of them moves an arrow. After them the caller runs
//! [`update_bound_arrows`](super::update_bound_arrows) once for the whole operation.

use std::collections::{BTreeSet, HashMap, HashSet};

use drafftink_geom::binding::{FIXED_POINT_BOUND, normalize_fixed_point};
use drafftink_geom::kurbo::Vec2;
use drafftink_scene::{BindMode, Binding, DerivedIndex, Element, ElementId, ElementKind, SceneRead, SceneTx};

use super::target::{bind_target, resolve};
use super::{BindingError, End};

fn modify<T: SceneTx, F>(tx: &mut T, id: &ElementId, f: F) -> Result<(), BindingError>
where
    F: FnOnce(&mut drafftink_scene::LinearData),
{
    tx.update(id, |e| {
        if let ElementKind::Line(d) | ElementKind::Arrow(d) = &mut e.kind {
            f(d);
        }
    })?;
    Ok(())
}

/// Applies `f` to every binding of the arrow `id` (a free end stays free; `f` returns `None` to free a bound
/// one) and writes both ends back only when one changed. Returns whether it wrote. An element that is missing or
/// not linear is `Ok(false)`.
fn rewrite_bindings<T: SceneTx>(
    tx: &mut T,
    id: &ElementId,
    f: impl Fn(&T, &Binding) -> Option<Binding>,
) -> Result<bool, BindingError> {
    let Some(d) = tx.get(id).and_then(Element::linear_data) else { return Ok(false) };
    let rewrite = |b: &Option<Binding>| b.as_ref().and_then(|b| f(&*tx, b));
    let (start, end) = (rewrite(&d.start_binding), rewrite(&d.end_binding));
    if start == d.start_binding && end == d.end_binding {
        return Ok(false);
    }
    modify(tx, id, |d| {
        d.start_binding = start;
        d.end_binding = end;
    })?;
    Ok(true)
}

/// Frees one end of an arrow. The end stays where it is. Returns whether the end was bound. Total: an element
/// that is not an arrow, or an end that is not bound, is `Ok(false)`; only a missing arrow is an error.
pub fn unbind_end<T: SceneTx>(tx: &mut T, arrow: &ElementId, end: End) -> Result<bool, BindingError> {
    let e = tx.get(arrow).ok_or_else(|| BindingError::Missing(arrow.clone()))?;
    let bound = e.linear_data().is_some_and(|d| match end {
        End::Start => d.start_binding.is_some(),
        End::End => d.end_binding.is_some(),
    });
    if !bound {
        return Ok(false);
    }
    modify(tx, arrow, |d| match end {
        End::Start => d.start_binding = None,
        End::End => d.end_binding = None,
    })?;
    Ok(true)
}

/// Arrows (and lines) in the scene bound to any of `targets`, sorted by id. With `derived` the committed reverse
/// index is used instead of scanning every element.
fn bound_to<R: SceneRead>(
    scene: &R,
    targets: &HashSet<&ElementId>,
    derived: Option<&DerivedIndex>,
) -> BTreeSet<ElementId> {
    let mut out = BTreeSet::new();
    match derived {
        Some(index) => {
            for t in targets {
                out.extend(index.bound_arrows(t).cloned());
            }
        }
        None => {
            for e in scene.iter() {
                let [a, b] = e.binding_targets();
                if a.is_some_and(|t| targets.contains(t)) || b.is_some_and(|t| targets.contains(t)) {
                    out.insert(e.id.clone());
                }
            }
        }
    }
    out
}

/// Frees the ends of arrows bound to any of `deleted`, in place, and returns the arrows changed. Call it in the
/// transaction that deletes the shapes. (An arrow left bound to a deleted shape is also fine: the solver ignores
/// the binding, and it works again if the shape comes back.)
pub fn free_ends_bound_to<T: SceneTx>(tx: &mut T, deleted: &[ElementId]) -> Result<Vec<ElementId>, BindingError> {
    free_ends_bound_to_with(tx, deleted, None)
}

/// [`free_ends_bound_to`] for a caller that took the arrows bound to `deleted` before it wrote
/// ([`arrows_bound_to`](super::arrows_bound_to)): no lookup is made here, so the cost follows `candidates`.
pub fn free_ends_bound_to_from<T: SceneTx>(
    tx: &mut T,
    deleted: &[ElementId],
    candidates: &BTreeSet<ElementId>,
) -> Result<Vec<ElementId>, BindingError> {
    let set: HashSet<&ElementId> = deleted.iter().collect();
    let mut changed = Vec::new();
    for id in candidates {
        if rewrite_bindings(tx, id, |_, b| (!set.contains(&b.target)).then(|| b.clone()))? {
            changed.push(id.clone());
        }
    }
    Ok(changed)
}

/// [`free_ends_bound_to`] using the committed reverse index. Arrows bound earlier in the same transaction are
/// not found.
pub fn free_ends_bound_to_with<T: SceneTx>(
    tx: &mut T,
    deleted: &[ElementId],
    derived: Option<&DerivedIndex>,
) -> Result<Vec<ElementId>, BindingError> {
    let set: HashSet<&ElementId> = deleted.iter().collect();
    let mut changed = Vec::new();
    for id in bound_to(tx, &set, derived) {
        if rewrite_bindings(tx, &id, |_, b| (!set.contains(&b.target)).then(|| b.clone()))? {
            changed.push(id);
        }
    }
    Ok(changed)
}

/// The rule for arrows that are moved, resized, rotated or flipped together with other elements: an end stays
/// bound only if its target is in `set` too (so the pair moves as one). Every other end is freed, in place.
/// This covers dragging and nudging an arrow alone, rotating an arrow, and resizing a selection that contains
/// arrows but not their shapes. Returns the arrows changed.
pub fn unbind_detached<T: SceneTx>(tx: &mut T, set: &[ElementId]) -> Result<Vec<ElementId>, BindingError> {
    let members: HashSet<&ElementId> = set.iter().collect();
    let mut changed = Vec::new();
    for id in set {
        if rewrite_bindings(tx, id, |_, b| members.contains(&b.target).then(|| b.clone()))? {
            changed.push(id.clone());
        }
    }
    Ok(changed)
}

/// An elbow arrow that is bound cannot be dragged on its own (it would have to give up its route). True when
/// `arrow` may be moved with `set`: it is not an elbow arrow, has no binding, or every bound end's target is
/// in `set`.
pub fn can_move_with<R: SceneRead>(scene: &R, arrow: &ElementId, set: &[ElementId]) -> bool {
    let Some(e) = scene.get(arrow) else { return true };
    let Some(d) = e.linear_data() else { return true };
    if d.path != drafftink_scene::PathType::Elbow {
        return true;
    }
    [&d.start_binding, &d.end_binding].into_iter().flatten().all(|b| set.contains(&b.target))
}

/// Remaps the binding targets of copied arrows through `map` (original id to copy id). A target that was not
/// copied becomes unbound, so a copy is bound only to copies. `mode` and `fixed_point` are kept. Returns the
/// arrows changed.
pub fn remap_bindings<T: SceneTx>(
    tx: &mut T,
    arrows: &[ElementId],
    map: &HashMap<ElementId, ElementId>,
) -> Result<Vec<ElementId>, BindingError> {
    let mut changed = Vec::new();
    for id in arrows {
        let remap = |_: &T, b: &Binding| map.get(&b.target).map(|t| Binding { target: t.clone(), ..b.clone() });
        if rewrite_bindings(tx, id, remap)? {
            changed.push(id.clone());
        }
    }
    Ok(changed)
}

/// Repairs the bindings of `ids` (or of every line and arrow with `None`): a binding to a missing element, to
/// something an arrow cannot attach to, or to the arrow itself is removed; a fixed point that is not finite is
/// reset to the centre and one beyond the limit is clamped. A binding to a *deleted* shape is kept, because
/// the shape may come back. Idempotent. Run it on anything that enters from outside: a file, the clipboard, a
/// remote change. Returns the arrows changed.
pub fn heal_bindings<T: SceneTx>(tx: &mut T, ids: Option<&[ElementId]>) -> Result<Vec<ElementId>, BindingError> {
    let all: Vec<ElementId>;
    let list: &[ElementId] = match ids {
        Some(l) => l,
        None => {
            all = tx.iter().filter(|e| e.tag().is_linear()).map(|e| e.id.clone()).collect();
            &all
        }
    };
    let mut changed = Vec::new();
    for id in list {
        let fix = |tx: &T, b: &Binding| {
            if b.target == *id {
                return None;
            }
            bind_target(tx.get(&b.target)?)?;
            let clamp = |v: f64| if v.is_finite() { v.clamp(-FIXED_POINT_BOUND, FIXED_POINT_BOUND) } else { 0.5001 };
            Some(Binding { fixed_point: [clamp(b.fixed_point[0]), clamp(b.fixed_point[1])], ..b.clone() })
        };
        if rewrite_bindings(tx, id, fix)? {
            changed.push(id.clone());
        }
    }
    Ok(changed)
}

/// Mirrors the fixed points of arrows in `set` whose targets are also in `set`, after the set was flipped.
/// A flipped shape swaps the sides of its own box, so a ratio `r` becomes `1 - r` on the flipped axis. (Excalidraw
/// does this for elbow arrows only, which leaves simple arrows attached to the wrong side after the next move.)
/// Call [`update_bound_arrows`](super::update_bound_arrows) afterwards.
pub fn mirror_fixed_points<T: SceneTx>(
    tx: &mut T,
    set: &[ElementId],
    flip_x: bool,
    flip_y: bool,
) -> Result<Vec<ElementId>, BindingError> {
    let members: HashSet<&ElementId> = set.iter().collect();
    let mut changed = Vec::new();
    for id in set {
        let mirror = |_: &T, b: &Binding| {
            if !members.contains(&b.target) {
                return Some(b.clone());
            }
            let fx = if flip_x { 1.0 - b.fixed_point[0] } else { b.fixed_point[0] };
            let fy = if flip_y { 1.0 - b.fixed_point[1] } else { b.fixed_point[1] };
            let v = normalize_fixed_point(Vec2::new(fx, fy));
            Some(Binding { fixed_point: [v.x, v.y], ..b.clone() })
        };
        if rewrite_bindings(tx, id, mirror)? {
            changed.push(id.clone());
        }
    }
    Ok(changed)
}

/// After a shape changed its outline in place (a rectangle became an ellipse), moves any orbit focus that fell
/// off the new outline back onto it along the ray from the centre. Inside bindings are kept. Run
/// [`update_bound_arrows`](super::update_bound_arrows) afterwards. Returns the arrows changed.
pub fn reanchor_to_outline<T: SceneTx>(
    tx: &mut T,
    changed_shapes: &[ElementId],
) -> Result<Vec<ElementId>, BindingError> {
    let set: HashSet<&ElementId> = changed_shapes.iter().collect();
    let mut changed = Vec::new();
    for id in bound_to(tx, &set, None) {
        let reanchor = |tx: &T, b: &Binding| {
            if !set.contains(&b.target) || b.mode != BindMode::Orbit {
                return Some(b.clone());
            }
            Some(reanchored(tx, b).unwrap_or_else(|| b.clone()))
        };
        if rewrite_bindings(tx, &id, reanchor)? {
            changed.push(id);
        }
    }
    Ok(changed)
}

/// The orbit binding `b` with its focus moved back onto the outline of its target along the ray from the centre.
/// `None` when the focus is still within reach of the outline or the geometry gives no better point.
fn reanchored<R: SceneRead>(scene: &R, b: &Binding) -> Option<Binding> {
    let r = resolve(scene, &b.target)?;
    let focus = r.target.global_fixed_point(Vec2::new(b.fixed_point[0], b.fixed_point[1]));
    if r.target.within_reach(focus, r.gap) {
        return None;
    }
    let center = r.target.center();
    let reach = r.target.frame.size.width.max(r.target.frame.size.height) * 2.0;
    let dir = focus - center;
    let len = dir.hypot();
    if len <= 0.0 {
        return None;
    }
    let far = center + dir / len * reach;
    let nearest = r
        .target
        .intersect_segment(center, far, r.gap)
        .into_iter()
        .min_by(|a, b2| (*a - focus).hypot2().total_cmp(&(*b2 - focus).hypot2()))?;
    let v = r.target.fixed_point_from_global(nearest, r.gap);
    Some(Binding { fixed_point: [v.x, v.y], ..b.clone() })
}
