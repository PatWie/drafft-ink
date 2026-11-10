//! Duplicate, delete, restore, lock and unlock.
//!
//! * **Duplicate** copies the elements (bound text with its container, the children of a copied frame) with new
//!   ids and seeds, offset by a vector. Group ids are regenerated consistently: all copies of a group's members
//!   share one new id, and the copy stays in the editing group and the groups around it. Bindings between copied
//!   elements point at the copies; a binding to an element that was not copied is dropped. Frame membership goes to
//!   a copied frame, otherwise it is cleared. Copies lose their `name` (names address one element), keep their
//!   group names, and are inserted as one block directly above the topmost original.
//! * **Delete** is soft (a tombstone, so undo and sync work). A container takes its bound text with it. A frame
//!   takes its children or releases them. Arrows bound to a deleted element keep their position and are freed at that end
//!   ([`crate::binding::free_ends_bound_to`]); restoring the element does not bind them again (undo does). A group with fewer than two live members counts as no group.
//! * **Lock** locks the whole outermost group (and bound text), so a group stays one unit.
//! * **Restore** brings tombstones back together with their bound text.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use drafftink_geom::kurbo::Vec2;
use drafftink_scene::{
    Element, ElementError, ElementId, ElementKind, GroupId, IdSource, Kind, SceneTx, StoreError, ZIndex,
    validate_element,
};

use crate::binding::{self, BindingError};
use crate::group::name_key;
use crate::group::units::{Relations, ensure_unlocked, impl_from_target, primaries, sort_z, units, with_companions};
use crate::zorder::{self, REBALANCE_LEN, ZOrderError};

/// Why a lifecycle operation was refused.
#[derive(Debug, thiserror::Error)]
pub enum LifecycleError {
    /// No element has this id.
    #[error("element {0} does not exist")]
    NotFound(ElementId),
    /// Locked elements cannot be duplicated or deleted.
    #[error("{} element(s) are locked ({}). Unlock them first", .0.len(), crate::group::units::ids_text(.0))]
    Locked(Vec<ElementId>),
    /// No element was given.
    #[error("no elements were given. Select some, or pass `ids`")]
    NoTarget,
    /// A number is unusable.
    #[error("the offset must be a finite number")]
    NotFinite,
    /// The id source kept returning ids that are in use.
    #[error("could not make a fresh id; the id source keeps returning used ids")]
    IdsExhausted,
    /// A copy would break a rule of the element.
    #[error("the copy of {id} would be invalid: {error}")]
    Invalid {
        /// The original.
        id: ElementId,
        /// The broken rule.
        error: ElementError,
    },
    /// The store refused a write.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Bound arrows could not be updated.
    #[error(transparent)]
    Binding(#[from] BindingError),
    /// Z-order could not be updated.
    #[error(transparent)]
    ZOrder(#[from] ZOrderError),
}
impl_from_target!(LifecycleError);

/// What happens to the children of a deleted frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChildPolicy {
    /// Delete them too (Excalidraw's behaviour).
    #[default]
    Delete,
    /// Keep them and clear their frame.
    Release,
}

/// Outcome of [`delete`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeleteReport {
    /// Everything that became a tombstone: the given elements, their bound text and deleted frame children.
    pub deleted: Vec<ElementId>,
    /// Frame children that were kept and released.
    pub released: Vec<ElementId>,
    /// Arrows whose end was bound to a deleted element and is now free (it stays where it was).
    pub freed: Vec<ElementId>,
    /// Arrows the binding module re-solved.
    pub arrows: Vec<ElementId>,
}

/// Deletes elements softly. Ids that are already deleted are ignored; an id that never existed is an error.
/// Giving a bound text alone deletes just that text.
pub fn delete<T: SceneTx>(
    tx: &mut T,
    ids: &[ElementId],
    children: ChildPolicy,
) -> Result<DeleteReport, LifecycleError> {
    if ids.is_empty() {
        return Err(LifecycleError::NoTarget);
    }
    let rel = Relations::build(&*tx);
    let mut given: Vec<ElementId> = Vec::new();
    for id in ids {
        match tx.get(id) {
            None => return Err(LifecycleError::NotFound(id.clone())),
            Some(e) if !e.deleted => given.push(id.clone()),
            Some(_) => {}
        }
    }
    let with_children = children == ChildPolicy::Delete;
    let all = with_companions(&*tx, &rel, &given, with_children);
    ensure_unlocked(&*tx, &all)?;
    let mut released: Vec<ElementId> = Vec::new();
    if !with_children {
        for g in &given {
            if tx.get(g).is_some_and(|e| e.tag() == Kind::Frame) {
                let kids = with_companions(&*tx, &rel, rel.children(g), false);
                ensure_unlocked(&*tx, &kids)?;
                released.extend(kids);
            }
        }
    }
    for id in &all {
        tx.update(id, |e| e.deleted = true)?;
    }
    for id in &released {
        if tx.get(id).is_some_and(|e| !e.deleted) {
            tx.update(id, |e| e.frame_id = None)?;
        }
    }
    sort_z(&*tx, &mut released);
    // Arrows bound to a deleted element keep their position and become free at that end.
    let freed = binding::free_ends_bound_to(tx, &all)?;
    let arrows = if all.is_empty() { Vec::new() } else { binding::update_bound_arrows(tx, &all)? };
    Ok(DeleteReport { deleted: all, released, freed, arrows })
}

/// Outcome of [`restore`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RestoreReport {
    /// Elements that came back, bound text included.
    pub restored: Vec<ElementId>,
    /// Bound text that stayed deleted because its container is deleted.
    pub skipped: Vec<ElementId>,
    /// Restored arrows whose end was bound to an element that is still deleted; that end is free now.
    pub freed: Vec<ElementId>,
    /// Arrows the binding module re-solved.
    pub arrows: Vec<ElementId>,
}

/// Brings deleted elements back, with their deleted bound text. Live ids are ignored; an unknown id is an error.
/// Frame children deleted together with a frame are not restored by this call: pass them too.
///
/// The structure changed while the elements were gone, so they are reconciled: a restored element joins the groups
/// around its groups as the live members have them now and moves next to them, a frame that is still deleted is
/// dropped from it, arrow ends bound to something still deleted are freed, bound text sits above its container,
/// and bound text whose container is still deleted stays
/// deleted (it is listed in `skipped`).
pub fn restore<T: SceneTx>(tx: &mut T, ids: &[ElementId]) -> Result<RestoreReport, LifecycleError> {
    let mut want: BTreeSet<ElementId> = BTreeSet::new();
    for id in ids {
        match tx.get(id) {
            None => return Err(LifecycleError::NotFound(id.clone())),
            Some(e) if e.deleted => {
                want.insert(id.clone());
            }
            Some(_) => {}
        }
    }
    let labels: Vec<ElementId> = tx
        .iter_all()
        .filter(|e| e.deleted && e.container_id().is_some_and(|c| want.contains(c)))
        .map(|e| e.id.clone())
        .collect();
    want.extend(labels);
    let rel = Relations::build(&*tx);
    let skipped: Vec<ElementId> = want
        .iter()
        .filter(|id| {
            tx.get(id)
                .and_then(Element::container_id)
                .is_some_and(|c| !want.contains(c) && tx.get(c).is_none_or(|c| c.deleted))
        })
        .cloned()
        .collect();
    for s in &skipped {
        want.remove(s);
    }
    let mut restored: Vec<ElementId> = want.into_iter().collect();
    sort_z(&*tx, &mut restored);
    // New values, computed against the live elements as they are now. Bound text takes its container's groups and
    // frame; everything else adopts them from the live members of its own groups.
    type Fix = (Vec<GroupId>, Option<ElementId>);
    let mut fixes: BTreeMap<ElementId, Fix> = BTreeMap::new();
    for id in &restored {
        let Some(e) = tx.get(id).filter(|e| e.container_id().is_none()) else { continue };
        let mut stack: Vec<GroupId> = e.group_ids.iter().cloned().collect();
        let mut adopt_frame: Option<Option<ElementId>> = None;
        // The innermost of its groups that still has live members decides the groups around it, and the frame
        // (the members of a group share their frame).
        for (i, g) in e.group_ids.iter().enumerate() {
            if let Some(m) = rel.members(g).iter().filter_map(|m| tx.get(m)).next()
                && let Some(at) = m.group_ids.iter().position(|x| x == g)
            {
                stack = e
                    .group_ids
                    .iter()
                    .take(i)
                    .filter(|x| !rel.members(x).is_empty())
                    .cloned()
                    .chain(m.group_ids.iter().skip(at).cloned())
                    .collect();
                adopt_frame = Some(m.frame_id.clone());
                break;
            }
        }
        let own = adopt_frame.unwrap_or_else(|| e.frame_id.clone());
        let frame = own.filter(|f| restored.contains(f) || tx.get(f).is_some_and(|f| !f.deleted));
        fixes.insert(id.clone(), (stack, frame));
    }
    for id in &restored {
        let Some(c) = tx.get(id).and_then(Element::container_id) else { continue };
        let fix = fixes
            .get(c)
            .cloned()
            .or_else(|| tx.get(c).map(|c| (c.group_ids.iter().cloned().collect(), c.frame_id.clone())));
        if let Some(fix) = fix {
            fixes.insert(id.clone(), fix);
        }
    }
    for (id, (stack, frame)) in fixes {
        tx.update(&id, move |e| {
            e.deleted = false;
            e.group_ids = stack.into_iter().collect();
            e.frame_id = frame;
        })?;
    }
    for id in &restored {
        if tx.get(id).is_some_and(|e| e.deleted) {
            tx.update(id, |e| e.deleted = false)?;
        }
    }
    place_restored(tx, &rel, &restored)?;
    // A restored arrow bound to something that is still deleted cannot stay attached to it.
    let gone: Vec<ElementId> = restored
        .iter()
        .filter_map(|i| tx.get(i))
        .flat_map(|e| e.binding_targets().into_iter().flatten().cloned().collect::<Vec<_>>())
        .filter(|t| tx.get(t).is_none_or(|t| t.deleted))
        .collect();
    let freed = if gone.is_empty() { Vec::new() } else { binding::free_ends_bound_to(tx, &gone)? };
    let arrows = if restored.is_empty() { Vec::new() } else { binding::update_bound_arrows(tx, &restored)? };
    Ok(RestoreReport { restored, skipped, freed, arrows })
}

/// Puts restored elements next to the structure they belong to: above their container, beside their group, below
/// their frame (in that order of preference).
fn place_restored<T: SceneTx>(tx: &mut T, rel: &Relations, restored: &[ElementId]) -> Result<(), LifecycleError> {
    let set: BTreeSet<&ElementId> = restored.iter().collect();
    // Containers first, last one first, so that restored elements keep their relative order when they land on the
    // same anchor. Bound text goes last, directly above its container.
    for id in restored.iter().rev() {
        let Some(e) = tx.get(id).cloned() else { continue };
        if e.container_id().is_some() {
            continue;
        }
        let outer =
            e.group_ids.iter().find(|g| rel.members(g).iter().any(|m| tx.get(m).is_some_and(|x| !set.contains(&x.id))));
        if let Some(g) = outer {
            let anchor = rel
                .members(g)
                .iter()
                .filter_map(|m| tx.get(m))
                .filter(|m| !set.contains(&m.id))
                .max_by(|a, b| a.index.cmp(&b.index).then_with(|| a.id.cmp(&b.id)))
                .map(|m| m.id.clone());
            if let Some(a) = anchor {
                zorder::move_block(tx, std::slice::from_ref(id), &zorder::Place::Above(a))?;
                continue;
            }
        }
        if let Some(f) = e.frame_id.as_ref().filter(|f| tx.get(f).is_some_and(|f| !f.deleted)) {
            zorder::move_block(tx, std::slice::from_ref(id), &zorder::Place::Below(f.clone()))?;
            continue;
        }
    }
    for id in restored.iter().rev() {
        if let Some(c) = tx.get(id).and_then(Element::container_id).cloned()
            && tx.get(&c).is_some_and(|c| !c.deleted)
        {
            zorder::move_block(tx, std::slice::from_ref(id), &zorder::Place::Above(c))?;
        }
    }
    Ok(())
}

/// Outcome of [`lock`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockReport {
    /// The state the elements now have.
    pub locked: bool,
    /// Elements whose state changed (whole groups, bound text included).
    pub changed: Vec<ElementId>,
}

/// Locks or unlocks the units of `ids` (a whole outermost group, with bound text). `locked: None` toggles:
/// unlock when everything is locked, else lock.
pub fn lock<T: SceneTx>(tx: &mut T, ids: &[ElementId], locked: Option<bool>) -> Result<LockReport, LifecycleError> {
    if ids.is_empty() {
        return Err(LifecycleError::NoTarget);
    }
    let rel = Relations::build(&*tx);
    let us = units(&*tx, &rel, ids, None)?;
    let members: Vec<ElementId> = us.iter().flat_map(|u| u.elements.iter().cloned()).collect();
    let all = with_companions(&*tx, &rel, &members, false);
    let target = locked.unwrap_or_else(|| !all.iter().all(|i| tx.get(i).is_some_and(|e| e.locked)));
    let mut changed = Vec::new();
    for id in &all {
        if tx.get(id).is_some_and(|e| e.locked != target) {
            tx.update(id, |e| e.locked = target)?;
            changed.push(id.clone());
        }
    }
    Ok(LockReport { locked: target, changed })
}

/// Options of [`duplicate`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DuplicateOpts {
    /// Offset of the copies.
    pub offset: Vec2,
    /// The editing group: copies stay in it and in the groups around it.
    pub editing: Option<GroupId>,
}

/// Outcome of [`duplicate`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DuplicateReport {
    /// Original and copy, for every copied element (bound text and frame children included), in z-order.
    pub copies: Vec<(ElementId, ElementId)>,
    /// The copies of the elements that were given (the new selection), in z-order.
    pub selection: Vec<ElementId>,
    /// Old and new id of every regenerated group.
    pub groups: Vec<(GroupId, GroupId)>,
    /// Arrows the binding module re-solved.
    pub arrows: Vec<ElementId>,
}

fn fresh<I: IdSource>(
    src: &mut I,
    taken: &dyn Fn(&ElementId) -> bool,
    used: &BTreeSet<ElementId>,
) -> Result<ElementId, LifecycleError> {
    for _ in 0..64 {
        let id = src.element_id();
        if !taken(&id) && !used.contains(&id) {
            return Ok(id);
        }
    }
    Err(LifecycleError::IdsExhausted)
}

/// Copies the elements. See the module docs for the rules. The copies are inserted as one block directly above
/// the topmost original.
pub fn duplicate<T: SceneTx, I: IdSource>(
    tx: &mut T,
    ids: &[ElementId],
    opts: &DuplicateOpts,
    id_source: &mut I,
) -> Result<DuplicateReport, LifecycleError> {
    if ids.is_empty() {
        return Err(LifecycleError::NoTarget);
    }
    if !opts.offset.x.is_finite() || !opts.offset.y.is_finite() {
        return Err(LifecycleError::NotFinite);
    }
    let rel = Relations::build(&*tx);
    let given = primaries(&*tx, ids)?;
    let all = with_companions(&*tx, &rel, &given, true);
    ensure_unlocked(&*tx, &all)?;
    let originals: Vec<Element> = all.iter().filter_map(|i| tx.get(i).cloned()).collect();

    // New element ids.
    let mut id_map: BTreeMap<ElementId, ElementId> = BTreeMap::new();
    let mut used: BTreeSet<ElementId> = BTreeSet::new();
    for o in &originals {
        let n = fresh(id_source, &|i| tx.contains(i), &used)?;
        used.insert(n.clone());
        id_map.insert(o.id.clone(), n);
    }

    // Group ids: keep the editing group and the groups around it, regenerate the rest, drop groups the copies
    // do not fill.
    let keep: BTreeSet<GroupId> = match &opts.editing {
        Some(g) => originals
            .iter()
            .find_map(|o| {
                o.group_ids.iter().position(|x| x == g).map(|i| o.group_ids.iter().skip(i).cloned().collect())
            })
            .unwrap_or_default(),
        None => BTreeSet::new(),
    };
    let mut counts: HashMap<&GroupId, usize> = HashMap::new();
    for o in &originals {
        for g in &o.group_ids {
            *counts.entry(g).or_default() += 1;
        }
    }
    let existing: BTreeSet<&GroupId> = tx.iter_all().flat_map(|e| e.group_ids.iter()).collect();
    let mut group_map: BTreeMap<GroupId, Option<GroupId>> = BTreeMap::new();
    let mut fresh_groups: BTreeSet<GroupId> = BTreeSet::new();
    let mut order: Vec<&GroupId> = counts.keys().copied().collect();
    order.sort();
    for g in order {
        if keep.contains(g) {
            group_map.insert(g.clone(), Some(g.clone()));
        } else if counts.get(g).copied().unwrap_or(0) >= 2 && rel.is_group(g) {
            let mut n = id_source.group_id();
            for _ in 0..64 {
                if !existing.contains(&n) && !fresh_groups.contains(&n) {
                    break;
                }
                n = id_source.group_id();
            }
            if existing.contains(&n) || fresh_groups.contains(&n) {
                return Err(LifecycleError::IdsExhausted);
            }
            fresh_groups.insert(n.clone());
            group_map.insert(g.clone(), Some(n));
        } else {
            group_map.insert(g.clone(), None);
        }
    }

    // Build and check the copies before writing anything.
    let mut copies: Vec<Element> = Vec::with_capacity(originals.len());
    for o in &originals {
        let Some(new_id) = id_map.get(&o.id) else { continue };
        let mut c = o.clone();
        c.id = new_id.clone();
        c.version = 0;
        c.deleted = false;
        c.name = None;
        c.style.seed = id_source.seed();
        c.transform.x += opts.offset.x;
        c.transform.y += opts.offset.y;
        c.group_ids = o.group_ids.iter().filter_map(|g| group_map.get(g).cloned().flatten()).collect();
        c.frame_id = o.frame_id.as_ref().and_then(|f| id_map.get(f).cloned());
        if let ElementKind::Text(t) = &mut c.kind {
            t.container_id = t.container_id.as_ref().and_then(|x| id_map.get(x).cloned());
        }
        if let Some(custom) = &mut c.custom {
            for (old, new) in &group_map {
                let key = name_key(old);
                if let Some(v) = custom.remove(&key)
                    && let Some(new) = new
                    && c.group_ids.contains(new)
                {
                    custom.insert(name_key(new), v);
                }
            }
            if custom.is_empty() {
                c.custom = None;
            }
        }
        validate_element(&c).map_err(|error| LifecycleError::Invalid { id: o.id.clone(), error })?;
        copies.push(c);
    }

    // Keys: a block directly above the topmost original, but never inside a group the copies do not join, nor
    // inside the children of a frame they do not join: the block then goes above that group or frame, so groups
    // and frame blocks stay contiguous.
    let Some(last) = originals.last() else { return Ok(DuplicateReport::default()) };
    let anchor = zorder::lift_above_enclosures(&*tx, &rel, last, &|f| id_map.contains_key(f), &|g| {
        group_map.get(g).cloned().flatten().as_ref() == Some(g)
    });
    let top = &anchor;
    let top_key = top.index.clone();
    let insert_copies = |tx: &mut T, copies: &mut Vec<Element>| -> Result<(), LifecycleError> {
        let (_, above) = tx.z_neighbors(&top.id);
        let keys = ZIndex::n_between(Some(&top_key), above.as_ref(), copies.len());
        let keys = match keys {
            Ok(k) => k,
            Err(_) => {
                zorder::rebalance(tx)?;
                let top_key = tx.get(&top.id).map(|e| e.index.clone()).unwrap_or_else(|| top_key.clone());
                let (_, above) = tx.z_neighbors(&top.id);
                ZIndex::n_between(Some(&top_key), above.as_ref(), copies.len()).map_err(ZOrderError::from)?
            }
        };
        for (c, k) in copies.iter_mut().zip(keys) {
            c.index = k;
        }
        Ok(())
    };
    insert_copies(tx, &mut copies)?;
    let long = copies.iter().any(|c| c.index.as_str().len() > REBALANCE_LEN);
    let new_ids: Vec<ElementId> = copies.iter().map(|c| c.id.clone()).collect();
    for c in copies {
        tx.insert(c)?;
    }
    if long {
        zorder::rebalance(tx)?;
    }
    // Bindings between copies point at copies; a binding to an element that was not copied is dropped.
    let id_map_std: std::collections::HashMap<ElementId, ElementId> =
        id_map.iter().map(|(a, b)| (a.clone(), b.clone())).collect();
    binding::remap_bindings(tx, &new_ids, &id_map_std)?;
    let arrows = binding::update_bound_arrows(tx, &new_ids)?;
    let pairs: Vec<(ElementId, ElementId)> =
        originals.iter().filter_map(|o| id_map.get(&o.id).map(|n| (o.id.clone(), n.clone()))).collect();
    let selection: Vec<ElementId> = given.iter().filter_map(|g| id_map.get(g).cloned()).collect();
    let mut selection = selection;
    sort_z(&*tx, &mut selection);
    let groups = group_map.into_iter().filter_map(|(o, n)| n.filter(|n| *n != o).map(|n| (o, n))).collect();
    Ok(DuplicateReport { copies: pairs, selection, groups, arrows })
}
