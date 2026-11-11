//! Repairs after remote edits were merged (`docs/crdt.md` section 10a).
//!
//! Two peers can each make a valid edit and the merge of both is not valid: one deletes a shape while the other
//! binds an arrow to it, two peers label the same shape, one wraps a shape in a frame that the other deletes, or two
//! group operations nest groups differently. The CRDT converges on such a state; these functions bring it back to
//! one that satisfies the relations of `validate_scene`.
//!
//! Every function here is a **deterministic function of the merged scene and of the change list**:
//!
//! * it reads only the scene (no clock, no peer id, no randomness, no order of arrival), so two peers that hold the
//!   same scene compute the same fix and write the same values;
//! * it is idempotent: run on its own result it writes nothing;
//! * it writes only the elements that must change, so a peer that receives another peer's repair finds its own
//!   rule already satisfied and writes nothing back. That is what stops two peers from answering each other
//!   forever.
//!
//! The rules follow what the commands do when one peer works alone, so a repaired merge looks like a sequential
//! history: deleting a shape frees the arrow ends bound to it (the ends stay where they are), a deleted container
//! releases its label as free text, a deleted frame releases its children.
//!
//! What is *not* repaired, because readers resolve it and the sequential commands leave it too: a group with fewer
//! than two live members (a stale id that the next group command removes; deleting one member and undoing must
//! bring the group back) and duplicate names.
//!
//! The functions find the elements to look at through the committed [`DerivedIndex`](drafftink_scene::DerivedIndex)
//! when the transaction offers one, so the cost follows the change. Call each one on a transaction that has not
//! written yet, and make the next call in a new transaction.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use drafftink_scene::{
    Change, ChangeKind, ElementId, ElementKind, Fields, GroupId, IndexError, Kind, SceneRead, SceneTx, StoreError,
    ZIndex,
};

use crate::binding::{self, BindingError};

/// Why a repair could not be written.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RepairError {
    /// Freeing an arrow end failed.
    #[error(transparent)]
    Binding(#[from] BindingError),
    /// The store refused a write.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// No z-order key fits between the tied keys and the next one. Rebalance the keys.
    #[error(transparent)]
    Index(#[from] IndexError),
}

/// What [`dangling_references`] wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Released {
    /// Arrows that lost an end binding.
    pub arrow_ends: Vec<ElementId>,
    /// Texts that stopped being labels.
    pub labels: Vec<ElementId>,
    /// Elements whose frame was cleared, or whose label took the frame of its container.
    pub frame_children: Vec<ElementId>,
    /// Labels that took the groups of their container.
    pub label_groups: Vec<ElementId>,
}

impl Released {
    /// True when nothing was written.
    pub fn is_empty(&self) -> bool {
        self.arrow_ends.is_empty()
            && self.labels.is_empty()
            && self.frame_children.is_empty()
            && self.label_groups.is_empty()
    }

    /// Every element written, sorted.
    pub fn touched(&self) -> BTreeSet<ElementId> {
        self.arrow_ends
            .iter()
            .chain(&self.labels)
            .chain(&self.frame_children)
            .chain(&self.label_groups)
            .cloned()
            .collect()
    }
}

/// Fields whose change can make the element's own references wrong.
fn reference_fields() -> Fields {
    Fields::KIND | Fields::FRAME | Fields::GROUPS | Fields::DELETED
}

/// Drops references that point nowhere useful: arrow ends bound to a missing, deleted or unattachable element,
/// labels whose container is gone, deleted or cannot hold a label (and all but the smallest id of several labels
/// of one container), and frame children whose frame is gone, deleted or not a frame. A label also takes the frame
/// and the groups of its container, as `text.bind` and `group.create` leave them.
///
/// `changed` is what the merge changed. The elements looked at are the changed ones, and those that refer to an
/// element that was removed or deleted by the merge.
pub fn dangling_references<T: SceneTx>(tx: &mut T, changed: &[Change]) -> Result<Released, RepairError> {
    // Elements nothing may point to any more: removed, deleted, or a text that just became a label (arrows cannot
    // attach to those).
    // The frame and groups of a container are also looked at: its label follows them.
    let gone: Vec<ElementId> = changed
        .iter()
        .filter(|c| {
            c.kind == ChangeKind::Removed
                || c.fields.intersects(Fields::DELETED | Fields::FRAME | Fields::GROUPS)
                || (c.fields.intersects(Fields::KIND) && tx.get(&c.id).is_some_and(|e| e.container_id().is_some()))
        })
        .map(|c| c.id.clone())
        .collect();
    let mut candidates: BTreeSet<ElementId> = changed
        .iter()
        .filter(|c| {
            c.kind == ChangeKind::Added || (c.kind == ChangeKind::Updated && c.fields.intersects(reference_fields()))
        })
        .map(|c| c.id.clone())
        .collect();
    candidates.extend(referrers(&*tx, &gone));

    let plan = Plan::make(&*tx, &candidates);
    let mut done = Released { arrow_ends: binding::free_unreachable_ends(tx, &plan.arrows)?, ..Released::default() };
    for id in &plan.labels {
        tx.update(id, |e| {
            if let ElementKind::Text(t) = &mut e.kind {
                t.container_id = None;
                t.label_position = None;
                t.auto_resize = false;
            }
        })?;
        done.labels.push(id.clone());
    }
    for (id, frame) in plan.frames {
        tx.update(&id, |e| e.frame_id = frame)?;
        done.frame_children.push(id);
    }
    for (id, groups) in plan.groups {
        tx.update(&id, |e| e.group_ids = groups.into())?;
        done.label_groups.push(id);
    }
    Ok(done)
}

/// The live elements that refer to any of `ids`: arrows bound to them, labels of them, children of them.
fn referrers<R: SceneRead>(r: &R, ids: &[ElementId]) -> BTreeSet<ElementId> {
    if ids.is_empty() {
        return BTreeSet::new();
    }
    if let Some(ix) = r.derived_index() {
        return ids
            .iter()
            .flat_map(|id| ix.bound_arrows(id).chain(ix.labels_of(id)).chain(ix.frame_children(id)))
            .cloned()
            .collect();
    }
    let wanted: BTreeSet<&ElementId> = ids.iter().collect();
    r.iter()
        .filter(|e| {
            e.binding_targets().into_iter().flatten().any(|t| wanted.contains(t))
                || e.container_id().is_some_and(|c| wanted.contains(c))
                || e.frame_id.as_ref().is_some_and(|f| wanted.contains(f))
        })
        .map(|e| e.id.clone())
        .collect()
}

/// What to write, decided before the first write so the committed index stays valid for all the reads.
#[derive(Default)]
struct Plan {
    arrows: Vec<ElementId>,
    labels: Vec<ElementId>,
    /// Elements whose `frame_id` becomes the given value.
    frames: Vec<(ElementId, Option<ElementId>)>,
    /// Labels whose `group_ids` become the given stack.
    groups: Vec<(ElementId, Vec<GroupId>)>,
}

impl Plan {
    fn make<R: SceneRead>(r: &R, candidates: &BTreeSet<ElementId>) -> Self {
        let mut plan = Plan::default();
        let mut rivals: BTreeMap<ElementId, BTreeSet<ElementId>> = BTreeMap::new();
        let scanned = labels_by_container(r);
        for id in candidates {
            let Some(e) = r.get(id).filter(|e| !e.deleted) else { continue };
            if e.tag().is_linear() {
                plan.arrows.push(id.clone());
            }
            // A label belongs to the frame of its container; any other element to its own frame, if that is a frame.
            let label_of = e.container_id().and_then(|c| holder(r, c, &e.id));
            let want = match label_of {
                Some(c) => c.frame_id.clone().filter(|f| frame_ok(r, f, c)),
                None => e.frame_id.clone().filter(|f| frame_ok(r, f, e)),
            };
            if e.frame_id != want {
                plan.frames.push((id.clone(), want));
            }
            if let Some(c) = label_of.filter(|c| c.group_ids != e.group_ids) {
                plan.groups.push((id.clone(), c.group_ids.to_vec()));
            }
            if let Some(c) = e.container_id() {
                if label_of.is_none() {
                    plan.labels.push(id.clone());
                } else {
                    rivals.entry(c.clone()).or_insert_with(|| claimants(r, scanned.as_ref(), c));
                }
            }
        }
        // Several live texts claim one container: the smallest id keeps it, as readers already assume.
        for claims in rivals.into_values() {
            plan.labels.extend(claims.into_iter().skip(1));
        }
        plan.labels.sort();
        plan.labels.dedup();
        plan
    }
}

/// The container `id` names, if a text can be its label: it exists, is alive, is not the text itself and is a kind
/// that holds labels.
fn holder<'a, R: SceneRead>(
    r: &'a R,
    container: &ElementId,
    label: &ElementId,
) -> Option<&'a drafftink_scene::Element> {
    r.get(container).filter(|c| !c.deleted && c.tag().is_label_container() && c.id != *label)
}

/// True when `frame` is a live frame that `child` can belong to: not the child itself, and the child is not a frame
/// (frames do not nest).
fn frame_ok<R: SceneRead>(r: &R, frame: &ElementId, child: &drafftink_scene::Element) -> bool {
    *frame != child.id
        && child.tag() != Kind::Frame
        && r.get(frame).is_some_and(|f| !f.deleted && f.tag() == Kind::Frame)
}

/// Every live text that claims `container`, sorted by id.
fn claimants<R: SceneRead>(
    r: &R,
    scanned: Option<&HashMap<ElementId, BTreeSet<ElementId>>>,
    container: &ElementId,
) -> BTreeSet<ElementId> {
    if let Some(ix) = r.derived_index() {
        return ix.labels_of(container).cloned().collect();
    }
    scanned.and_then(|m| m.get(container)).cloned().unwrap_or_default()
}

/// Without a committed index: the claims of every container, from one pass over the scene.
fn labels_by_container<R: SceneRead>(r: &R) -> Option<HashMap<ElementId, BTreeSet<ElementId>>> {
    if r.derived_index().is_some() {
        return None;
    }
    let mut map: HashMap<ElementId, BTreeSet<ElementId>> = HashMap::new();
    for e in r.iter() {
        if let Some(c) = e.container_id() {
            map.entry(c.clone()).or_default().insert(e.id.clone());
        }
    }
    Some(map)
}

/// Makes group nesting consistent again: when the live members of a group disagree about the groups around it
/// (the outer part of their `group_ids`), the group is dissolved, and the check runs again on what that changed.
/// The smallest group id is dissolved first, so every peer dissolves the same groups in the same order. Returns the
/// groups that were dissolved.
///
/// `changed` is what the merge changed: only groups of elements that were added, deleted or restored, or whose
/// `group_ids` changed, are examined, together with the groups that share a member with them.
pub fn inconsistent_groups<T: SceneTx>(tx: &mut T, changed: &[Change]) -> Result<Vec<GroupId>, RepairError> {
    let mut stacks = affected_stacks(&*tx, changed);
    let dissolved = dissolve_until_consistent(&mut stacks);
    for (id, stack) in stacks {
        if tx.get(&id).is_some_and(|e| e.group_ids.as_slice() != stack.as_slice()) {
            tx.update(&id, |e| e.group_ids = stack.into())?;
        }
    }
    Ok(dissolved)
}

/// Gives elements that share a z-order key distinct keys. Two peers that insert at the same spot, or move to the
/// same spot, make equal keys; readers order them by id, but a command that needs a key between them cannot make
/// one. Of each set of equal keys the smallest id keeps it and the others get keys between it and the next larger
/// key, in id order, so nothing moves on screen. Returns the elements that got a new key.
///
/// `changed` is what the merge changed: only elements that were added or whose key changed are examined, and only
/// when a neighbour in z-order has the same key. Finding the elements that share a key and the next larger key is a
/// pass over the scene, which a merge needs only when a tie exists.
pub fn tied_order_keys<T: SceneTx>(tx: &mut T, changed: &[Change]) -> Result<Vec<ElementId>, RepairError> {
    let tied: BTreeSet<ZIndex> = changed
        .iter()
        .filter(|c| {
            c.kind == ChangeKind::Added || (c.kind == ChangeKind::Updated && c.fields.intersects(Fields::INDEX))
        })
        .filter_map(|c| tx.get(&c.id))
        .filter(|e| {
            let (below, above) = tx.z_neighbors(&e.id);
            below.as_ref() == Some(&e.index) || above.as_ref() == Some(&e.index)
        })
        .map(|e| e.index.clone())
        .collect();
    let mut rekeyed = Vec::new();
    for key in tied {
        let sharing: Vec<ElementId> = tx.iter_all().filter(|e| e.index == key).map(|e| e.id.clone()).collect();
        let upper = tx.iter_all().map(|e| &e.index).find(|i| **i > key).cloned();
        let fresh = ZIndex::n_between(Some(&key), upper.as_ref(), sharing.len().saturating_sub(1))?;
        for (id, index) in sharing.into_iter().skip(1).zip(fresh) {
            tx.update(&id, |e| e.index = index)?;
            rekeyed.push(id);
        }
    }
    Ok(rekeyed)
}

/// Puts a frame child that sits above its frame back below it, with a key just under the frame, in the order the
/// children already had. Concurrent reorders can leave a child above its frame; `frame.add` keeps children directly
/// below the frame, and the clip and the draw order rely on it. Returns the elements that got a new key.
///
/// `changed` is what the merge changed: elements that were added or whose key or frame changed, and the children of
/// frames whose key changed.
pub fn children_below_frame<T: SceneTx>(tx: &mut T, changed: &[Change]) -> Result<Vec<ElementId>, RepairError> {
    let mut misplaced: BTreeMap<ElementId, Vec<(ZIndex, ElementId)>> = BTreeMap::new();
    for id in order_candidates(&*tx, changed) {
        let Some(e) = tx.get(&id).filter(|e| !e.deleted) else { continue };
        let frame = e.frame_id.as_ref().and_then(|f| tx.get(f)).filter(|f| !f.deleted && f.tag() == Kind::Frame);
        if let Some(f) = frame.filter(|f| e.index > f.index) {
            misplaced.entry(f.id.clone()).or_default().push((e.index.clone(), e.id.clone()));
        }
    }
    let mut rekeyed = Vec::new();
    for (frame, mut moved) in misplaced {
        moved.sort();
        let (below, _) = tx.z_neighbors(&frame);
        let Some(top) = tx.get(&frame).map(|f| f.index.clone()) else { continue };
        let lower = below.filter(|b| *b < top);
        let fresh = ZIndex::n_between(lower.as_ref(), Some(&top), moved.len())?;
        for ((_, id), index) in moved.into_iter().zip(fresh) {
            tx.update(&id, |e| e.index = index)?;
            rekeyed.push(id);
        }
    }
    Ok(rekeyed)
}

/// Puts a label that sits below its container back above it, with a key just over the container. `text.bind` keeps
/// the label directly above its container, and a label below it is hidden by it. Returns the labels that got a new
/// key. Run it after [`children_below_frame`], so the container is where it stays.
///
/// `changed` is what the merge changed, as for [`children_below_frame`] with the labels of containers whose key
/// changed.
pub fn labels_above_container<T: SceneTx>(tx: &mut T, changed: &[Change]) -> Result<Vec<ElementId>, RepairError> {
    let mut misplaced: BTreeMap<ElementId, Vec<(ZIndex, ElementId)>> = BTreeMap::new();
    for id in order_candidates(&*tx, changed) {
        let Some(e) = tx.get(&id).filter(|e| !e.deleted) else { continue };
        let container = e.container_id().and_then(|c| tx.get(c)).filter(|c| !c.deleted && c.tag().is_label_container());
        if let Some(c) = container.filter(|c| e.index < c.index) {
            misplaced.entry(c.id.clone()).or_default().push((e.index.clone(), e.id.clone()));
        }
    }
    let mut rekeyed = Vec::new();
    for (container, mut moved) in misplaced {
        moved.sort();
        let (_, above) = tx.z_neighbors(&container);
        let Some(bottom) = tx.get(&container).map(|c| c.index.clone()) else { continue };
        let upper = above.filter(|a| *a > bottom);
        let fresh = ZIndex::n_between(Some(&bottom), upper.as_ref(), moved.len())?;
        for ((_, id), index) in moved.into_iter().zip(fresh) {
            tx.update(&id, |e| e.index = index)?;
            rekeyed.push(id);
        }
    }
    Ok(rekeyed)
}

/// The elements whose place in z-order relative to a frame or a container the merge can have broken: the added
/// ones, those whose key, frame or kind data changed, and the children and labels of elements whose key changed.
fn order_candidates<R: SceneRead>(r: &R, changed: &[Change]) -> BTreeSet<ElementId> {
    let reordered: Vec<ElementId> = changed
        .iter()
        .filter(|c| c.kind == ChangeKind::Updated && c.fields.intersects(Fields::INDEX))
        .map(|c| c.id.clone())
        .collect();
    let mut candidates: BTreeSet<ElementId> = changed
        .iter()
        .filter(|c| {
            c.kind == ChangeKind::Added
                || (c.kind == ChangeKind::Updated && c.fields.intersects(Fields::INDEX | Fields::FRAME | Fields::KIND))
        })
        .map(|c| c.id.clone())
        .collect();
    candidates.extend(referrers(r, &reordered));
    candidates
}

/// The `group_ids` of the live members of every group that the change can have made inconsistent, and of every
/// group those members belong to.
fn affected_stacks<R: SceneRead>(r: &R, changed: &[Change]) -> BTreeMap<ElementId, Vec<GroupId>> {
    let relevant = Fields::GROUPS | Fields::DELETED;
    let mut queue: Vec<GroupId> = changed
        .iter()
        .filter(|c| c.kind == ChangeKind::Added || (c.kind == ChangeKind::Updated && c.fields.intersects(relevant)))
        .filter_map(|c| r.get(&c.id))
        .filter(|e| !e.deleted)
        .flat_map(|e| e.group_ids.iter().cloned())
        .collect();
    let by_group = if r.derived_index().is_some() { None } else { Some(members_by_group(r)) };
    let mut seen: BTreeSet<GroupId> = BTreeSet::new();
    let mut stacks: BTreeMap<ElementId, Vec<GroupId>> = BTreeMap::new();
    while let Some(g) = queue.pop() {
        if !seen.insert(g.clone()) {
            continue;
        }
        let members: Vec<ElementId> = match (r.derived_index(), &by_group) {
            (Some(ix), _) => ix.group_members(&g).cloned().collect(),
            (None, Some(map)) => map.get(&g).map(|s| s.iter().cloned().collect()).unwrap_or_default(),
            (None, None) => Vec::new(),
        };
        for m in members {
            if stacks.contains_key(&m) {
                continue;
            }
            if let Some(e) = r.get(&m).filter(|e| !e.deleted) {
                queue.extend(e.group_ids.iter().cloned());
                stacks.insert(m, e.group_ids.to_vec());
            }
        }
    }
    stacks
}

/// Live members of every group, from one pass over the scene (for a reader without an index).
fn members_by_group<R: SceneRead>(r: &R) -> HashMap<GroupId, BTreeSet<ElementId>> {
    let mut map: HashMap<GroupId, BTreeSet<ElementId>> = HashMap::new();
    for e in r.iter() {
        for g in &e.group_ids {
            map.entry(g.clone()).or_default().insert(e.id.clone());
        }
    }
    map
}

/// Removes the smallest inconsistent group from every stack, and repeats until none is left. Each round removes one
/// group, so it ends.
fn dissolve_until_consistent(stacks: &mut BTreeMap<ElementId, Vec<GroupId>>) -> Vec<GroupId> {
    let mut dissolved = Vec::new();
    while let Some(g) = first_inconsistent(stacks) {
        for stack in stacks.values_mut() {
            stack.retain(|x| *x != g);
        }
        dissolved.push(g);
    }
    dissolved
}

/// The smallest group whose members have different groups around it.
fn first_inconsistent(stacks: &BTreeMap<ElementId, Vec<GroupId>>) -> Option<GroupId> {
    let mut outer_of: BTreeMap<&GroupId, &[GroupId]> = BTreeMap::new();
    let mut bad: BTreeSet<&GroupId> = BTreeSet::new();
    for stack in stacks.values() {
        for (i, g) in stack.iter().enumerate() {
            let outer = stack.get(i + 1..).unwrap_or(&[]);
            match outer_of.get(g) {
                None => {
                    outer_of.insert(g, outer);
                }
                Some(first) if *first != outer => {
                    bad.insert(g);
                }
                Some(_) => {}
            }
        }
    }
    bad.first().map(|g| (*g).clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gid(s: &str) -> GroupId {
        GroupId::new(s).unwrap()
    }

    fn eid(s: &str) -> ElementId {
        ElementId::new(s).unwrap()
    }

    fn stacks(rows: &[(&str, &[&str])]) -> BTreeMap<ElementId, Vec<GroupId>> {
        rows.iter().map(|(id, gs)| (eid(id), gs.iter().map(|g| gid(g)).collect())).collect()
    }

    #[test]
    fn consistent_nesting_is_left_alone() {
        let mut s = stacks(&[("a", &["g1", "g2"]), ("b", &["g1", "g2"]), ("c", &["g2"])]);
        assert!(dissolve_until_consistent(&mut s).is_empty());
        assert_eq!(s, stacks(&[("a", &["g1", "g2"]), ("b", &["g1", "g2"]), ("c", &["g2"])]));
    }

    #[test]
    fn members_that_disagree_about_the_outer_group_dissolve_the_inner_one() {
        let mut s = stacks(&[("a", &["g1", "g2"]), ("b", &["g1"]), ("c", &["g2"])]);
        assert_eq!(dissolve_until_consistent(&mut s), [gid("g1")]);
        assert_eq!(s, stacks(&[("a", &["g2"]), ("b", &[]), ("c", &["g2"])]));
    }

    #[test]
    fn opposite_nesting_ends_with_every_stack_consistent() {
        let mut s = stacks(&[("a", &["g1", "g2"]), ("b", &["g2", "g1"]), ("c", &["g1"]), ("d", &["g2"])]);
        let dissolved = dissolve_until_consistent(&mut s);
        assert_eq!(dissolved.first(), Some(&gid("g1")), "the smallest id goes first");
        assert!(first_inconsistent(&s).is_none());
    }

    #[test]
    fn the_result_does_not_depend_on_the_order_the_members_are_listed() {
        let rows: [(&str, &[&str]); 4] = [("a", &["g1", "g3"]), ("b", &["g1"]), ("c", &["g3", "g1"]), ("d", &["g3"])];
        let mut forward = stacks(&rows);
        let mut reversed: BTreeMap<_, _> =
            rows.iter().rev().map(|(id, gs)| (eid(id), gs.iter().map(|g| gid(g)).collect())).collect();
        assert_eq!(dissolve_until_consistent(&mut forward), dissolve_until_consistent(&mut reversed));
        assert_eq!(forward, reversed);
    }
}
