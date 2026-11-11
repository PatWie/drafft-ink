//! Z-order: bring forward, send backward, to front and to back, with fractional keys.
//!
//! The rules (`docs/commands.md` section 2.9):
//! * The unit that moves is an element, or the outermost group around it; inside an editing group the unit is the
//!   next level down. A group stays contiguous because the whole group moves.
//! * Forward and backward step over the neighbouring unit and jump over a whole group, and over a frame together
//!   with its children (a frame's children sit contiguously just below it).
//! * Inside an editing group the move stays inside that group's z-range. Children of a frame move among their
//!   siblings and stay below the frame.
//! * Bound text travels with its container.
//! * Only the elements that move get new keys ([`ZIndex::n_between`]). Tombstones keep their place. When a key
//!   cannot be made (or would grow past [`REBALANCE_LEN`]) all keys are rewritten once and the report says so.
//!
//! The same machinery places a block of elements at a spot (`move_block`); `group`, `frame` and `lifecycle` use it.

use std::collections::BTreeSet;

use drafftink_scene::{Element, ElementId, GroupId, IndexError, Kind, SceneRead, SceneTx, StoreError, ZIndex};

use crate::group::units::{Relations, UnitKey, ensure_unlocked, unit_key, units, with_companions};
use crate::target::TargetError;

/// Keys longer than this make the whole order get rewritten with short keys.
pub const REBALANCE_LEN: usize = 48;

/// Why a z-order change was refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ZOrderError {
    /// An element does not exist, is deleted, or is locked.
    #[error(transparent)]
    Target(#[from] TargetError),
    /// The anchor of a placement is missing or is one of the moved elements.
    #[error("cannot place relative to {0}: it does not exist or is moving itself")]
    BadAnchor(ElementId),
    /// The store refused a write.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// A key could not be made even after rebalancing.
    #[error("z-order keys are exhausted: {0}")]
    Index(#[from] IndexError),
}

/// A z-order step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZOp {
    /// To the top of its scope.
    Front,
    /// To the bottom of its scope.
    Back,
    /// Over the next unit above.
    Forward,
    /// Under the next unit below.
    Backward,
}

/// What a z-order change did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ZReport {
    /// Elements that got a new key, in z-order.
    pub moved: Vec<ElementId>,
    /// True when all keys were rewritten (rebalance).
    pub rebalanced: bool,
}

impl ZReport {
    /// True when the order changed.
    pub fn changed(&self) -> bool {
        !self.moved.is_empty()
    }
}

/// All element ids including tombstones, back to front.
pub fn order<R: SceneRead>(scene: &R) -> Vec<ElementId> {
    scene.iter_all().map(|e| e.id.clone()).collect()
}

/// Rewrites every key evenly, keeping the order. Use it when keys have grown long.
pub fn rebalance<T: SceneTx>(tx: &mut T) -> Result<ZReport, ZOrderError> {
    let ids = order(&*tx);
    rewrite_all(tx, &ids)
}

fn rewrite_all<T: SceneTx>(tx: &mut T, ids: &[ElementId]) -> Result<ZReport, ZOrderError> {
    let keys = ZIndex::rebalance(ids.len())?;
    let mut moved = Vec::new();
    for (id, key) in ids.iter().zip(keys) {
        if tx.get(id).is_some_and(|e| e.index != key) {
            tx.update(id, |e| e.index = key)?;
            moved.push(id.clone());
        }
    }
    Ok(ZReport { moved, rebalanced: true })
}

fn key_of<R: SceneRead>(scene: &R, id: &ElementId) -> Option<ZIndex> {
    scene.get(id).map(|e| e.index.clone())
}

/// Gives `new_order` (a permutation of all ids) to the scene by re-keying only elements of `movable`. The
/// elements outside `movable` must keep their relative order.
pub(crate) fn apply_order<T: SceneTx>(
    tx: &mut T,
    new_order: &[ElementId],
    movable: &BTreeSet<ElementId>,
) -> Result<ZReport, ZOrderError> {
    if new_order == order(&*tx).as_slice() {
        return Ok(ZReport::default());
    }
    // Fixed neighbours must already be increasing, else a full rewrite is the only consistent answer.
    let fixed_keys: Vec<ZIndex> =
        new_order.iter().filter(|i| !movable.contains(*i)).filter_map(|i| key_of(&*tx, i)).collect();
    if fixed_keys.windows(2).any(|w| w[0] >= w[1]) {
        return rewrite_all(tx, new_order);
    }
    let mut assign: Vec<(ElementId, ZIndex)> = Vec::new();
    let mut prev: Option<ZIndex> = None;
    let mut i = 0;
    while i < new_order.len() {
        let id = &new_order[i];
        if !movable.contains(id) {
            prev = key_of(&*tx, id);
            i += 1;
            continue;
        }
        let mut j = i;
        while j < new_order.len() && movable.contains(&new_order[j]) {
            j += 1;
        }
        let next = new_order.get(j).and_then(|n| key_of(&*tx, n));
        let run = &new_order[i..j];
        let old: Vec<Option<ZIndex>> = run.iter().map(|r| key_of(&*tx, r)).collect();
        let keep = longest_increasing_within(&old, prev.as_ref(), next.as_ref());
        // Elements outside the kept ones get fresh keys between their kept neighbours.
        let mut lower = prev.clone();
        let mut k = 0;
        while k < run.len() {
            if keep[k] {
                lower = old[k].clone();
                k += 1;
                continue;
            }
            let mut m = k;
            while m < run.len() && !keep[m] {
                m += 1;
            }
            let upper = if m < run.len() { old[m].clone() } else { next.clone() };
            match ZIndex::n_between(lower.as_ref(), upper.as_ref(), m - k) {
                Ok(keys) => {
                    for (r, key) in run[k..m].iter().zip(keys) {
                        assign.push((r.clone(), key));
                    }
                }
                Err(_) => return rewrite_all(tx, new_order),
            }
            k = m;
        }
        prev = match assign.iter().rev().find(|(id, _)| run.last() == Some(id)) {
            Some((_, key)) => Some(key.clone()),
            None => run.last().and_then(|l| key_of(&*tx, l)),
        };
        i = j;
    }
    if assign.iter().any(|(_, k)| k.as_str().len() > REBALANCE_LEN) {
        return rewrite_all(tx, new_order);
    }
    let mut moved = Vec::new();
    for (id, key) in assign {
        tx.update(&id, |e| e.index = key)?;
        moved.push(id);
    }
    Ok(ZReport { moved, rebalanced: false })
}

/// Which of `keys` can keep their value: the longest strictly increasing subsequence of the keys that lie strictly
/// between `lower` and `upper`.
fn longest_increasing_within(keys: &[Option<ZIndex>], lower: Option<&ZIndex>, upper: Option<&ZIndex>) -> Vec<bool> {
    let ok = |k: &ZIndex| lower.is_none_or(|l| l < k) && upper.is_none_or(|u| k < u);
    let mut tails: Vec<usize> = Vec::new();
    let mut pred: Vec<Option<usize>> = vec![None; keys.len()];
    for (i, key) in keys.iter().enumerate() {
        let Some(key) = key.as_ref().filter(|k| ok(k)) else { continue };
        let pos = tails.partition_point(|&t| keys[t].as_ref().is_some_and(|tk| tk < key));
        pred[i] = pos.checked_sub(1).and_then(|p| tails.get(p).copied());
        if pos == tails.len() {
            tails.push(i);
        } else {
            tails[pos] = i;
        }
    }
    let mut keep = vec![false; keys.len()];
    let mut cur = tails.last().copied();
    while let Some(i) = cur {
        keep[i] = true;
        cur = pred[i];
    }
    keep
}

/// Where [`move_block`] puts the block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Place {
    /// Directly below this element.
    Below(ElementId),
    /// Directly above this element.
    Above(ElementId),
    /// At the slot of the topmost moving element: the others join it and whatever sat between ends up below.
    AtTopmost,
}

/// Moves `moving` (any order; their z-order is kept) to `place` as one contiguous block. Elements outside the
/// block keep their relative order.
pub(crate) fn move_block<T: SceneTx>(tx: &mut T, moving: &[ElementId], place: &Place) -> Result<ZReport, ZOrderError> {
    let set: BTreeSet<ElementId> = moving.iter().cloned().collect();
    let all = order(&*tx);
    let block: Vec<ElementId> = all.iter().filter(|i| set.contains(*i)).cloned().collect();
    let mut rest: Vec<ElementId> = all.iter().filter(|i| !set.contains(*i)).cloned().collect();
    let at = match place {
        Place::Below(a) | Place::Above(a) => {
            let Some(p) = rest.iter().position(|r| r == a) else {
                return Err(ZOrderError::BadAnchor(a.clone()));
            };
            if matches!(place, Place::Below(_)) { p } else { p + 1 }
        }
        Place::AtTopmost => match all.iter().rposition(|i| set.contains(i)) {
            Some(top) => all[..top].iter().filter(|i| !set.contains(*i)).count(),
            None => return Ok(ZReport::default()),
        },
    };
    let tail = rest.split_off(at);
    rest.extend(block);
    rest.extend(tail);
    apply_order(tx, &rest, &set)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Scope {
    Root,
    Group(GroupId),
    Frame(ElementId),
}

struct Block {
    key: UnitKey,
    elems: Vec<ElementId>,
    selected: bool,
    live: bool,
}

/// The element whose groups and frame decide where `e` belongs: its container for bound text.
fn effective<'a, R: SceneRead>(scene: &'a R, e: &'a Element) -> &'a Element {
    match e.container_id().and_then(|c| scene.get(c)) {
        Some(c) if !c.deleted => c,
        _ => e,
    }
}

fn live_frame<'a, R: SceneRead>(scene: &'a R, id: &ElementId) -> Option<&'a Element> {
    scene.get(id).filter(|f| !f.deleted && f.tag() == Kind::Frame)
}

fn scope_of<R: SceneRead>(scene: &R, rel: &Relations, e: &Element, editing: Option<&GroupId>) -> Scope {
    if let Some(g) = editing
        && rel.is_group(g)
        && e.group_ids.contains(g)
    {
        return Scope::Group(g.clone());
    }
    if let Some(f) = &e.frame_id
        && live_frame(scene, f).is_some()
    {
        return Scope::Frame(f.clone());
    }
    Scope::Root
}

/// The block key of `e` in `scope`, or `None` when `e` is outside the scope.
fn block_key<R: SceneRead>(scene: &R, rel: &Relations, e: &Element, scope: &Scope) -> Option<UnitKey> {
    let eff = effective(scene, e);
    match scope {
        Scope::Group(g) => {
            // The children of a frame belong to the block of their frame.
            let anchor = eff
                .frame_id
                .as_ref()
                .and_then(|f| live_frame(scene, f))
                .filter(|f| f.group_ids.contains(g))
                .unwrap_or(eff);
            (rel.is_group(g) && anchor.group_ids.contains(g)).then(|| unit_key(rel, anchor, Some(g)))
        }
        Scope::Frame(f) => (eff.frame_id.as_ref() == Some(f)).then(|| unit_key(rel, eff, None)),
        Scope::Root => {
            let anchor = eff.frame_id.as_ref().and_then(|f| live_frame(scene, f)).unwrap_or(eff);
            Some(unit_key(rel, anchor, None))
        }
    }
}

fn step(blocks: &mut Vec<Block>, op: ZOp) {
    match op {
        ZOp::Front | ZOp::Back => {
            let (mut sel, mut rest): (Vec<Block>, Vec<Block>) =
                std::mem::take(blocks).into_iter().partition(|b| b.selected);
            if op == ZOp::Front {
                rest.append(&mut sel);
                *blocks = rest;
            } else {
                sel.append(&mut rest);
                *blocks = sel;
            }
        }
        ZOp::Forward => {
            for i in (0..blocks.len()).rev() {
                if !blocks.get(i).is_some_and(|b| b.selected) {
                    continue;
                }
                if let Some(k) = (i + 1..blocks.len()).find(|&j| !blocks[j].selected && blocks[j].live) {
                    let b = blocks.remove(i);
                    blocks.insert(k, b);
                }
            }
        }
        ZOp::Backward => {
            for i in 0..blocks.len() {
                if !blocks.get(i).is_some_and(|b| b.selected) {
                    continue;
                }
                if let Some(j) = (0..i).rev().find(|&j| !blocks[j].selected && blocks[j].live) {
                    let b = blocks.remove(i);
                    blocks.insert(j, b);
                }
            }
        }
    }
}

/// Moves the units of `ids` in the z-order. See the module docs for the rules. `editing` is the editing group.
///
/// Returns an empty report when nothing moves (already at the end of the order).
pub fn z_order<T: SceneTx>(
    tx: &mut T,
    ids: &[ElementId],
    op: ZOp,
    editing: Option<&GroupId>,
) -> Result<ZReport, ZOrderError> {
    let rel = Relations::build(&*tx);
    let us = units(&*tx, &rel, ids, editing)?;
    let all_ids: Vec<ElementId> = us.iter().flat_map(|u| u.elements.iter().cloned()).collect();
    let touched = with_companions(&*tx, &rel, &all_ids, true);
    ensure_unlocked(&*tx, &touched)?;

    // Units grouped by scope, in a stable order.
    let mut scopes: Vec<(Scope, BTreeSet<UnitKey>)> = Vec::new();
    for u in &us {
        let Some(first) = u.elements.first().and_then(|i| tx.get(i)) else { continue };
        let scope = scope_of(&*tx, &rel, first, editing);
        match scopes.iter_mut().find(|(s, _)| *s == scope) {
            Some((_, keys)) => {
                keys.insert(u.key.clone());
            }
            None => scopes.push((scope, BTreeSet::from([u.key.clone()]))),
        }
    }

    let original = order(&*tx);
    let mut list = original.clone();
    let mut movable: BTreeSet<ElementId> = BTreeSet::new();
    for (scope, keys) in &scopes {
        let mut blocks: Vec<Block> = Vec::new();
        let mut slots: Vec<usize> = Vec::new();
        for (pos, id) in list.iter().enumerate() {
            let Some(e) = tx.get(id) else { continue };
            let Some(key) = block_key(&*tx, &rel, e, scope) else { continue };
            slots.push(pos);
            match blocks.last_mut() {
                Some(b) if b.key == key => {
                    b.elems.push(id.clone());
                    b.live |= !e.deleted;
                }
                _ => {
                    blocks.push(Block { selected: keys.contains(&key), key, elems: vec![id.clone()], live: !e.deleted })
                }
            }
        }
        // A block that is selected by key everywhere: also catch blocks split into several runs.
        for b in &mut blocks {
            b.selected = keys.contains(&b.key);
        }
        for b in blocks.iter().filter(|b| b.selected) {
            movable.extend(b.elems.iter().cloned());
        }
        step(&mut blocks, op);
        let q: Vec<ElementId> = blocks.into_iter().flat_map(|b| b.elems).collect();
        for (slot, id) in slots.into_iter().zip(q) {
            if let Some(s) = list.get_mut(slot) {
                *s = id;
            }
        }
    }
    if list == original {
        return Ok(ZReport::default());
    }
    apply_order(tx, &list, &movable)
}

/// Puts elements that just left a frame (`(element, frame)` pairs) where they do not disturb the frame's block of
/// children: just below the lowest remaining child (or the frame), or, when the frame is inside a group, just
/// above that group. The relative order to everything outside the frame's block is kept.
pub(crate) fn settle_leavers<T: SceneTx>(tx: &mut T, left: &[(ElementId, ElementId)]) -> Result<(), ZOrderError> {
    let mut frames: Vec<&ElementId> = left.iter().map(|(_, f)| f).collect();
    frames.sort();
    frames.dedup();
    for f in frames {
        let rel = Relations::build(&*tx);
        let Some(frame_el) = tx.get(f).filter(|e| !e.deleted) else { continue };
        let moving: Vec<ElementId> = left.iter().filter(|(_, g)| g == f).map(|(i, _)| i.clone()).collect();
        let moving = with_companions(&*tx, &rel, &moving, false);
        let place = match rel.stack(frame_el).last() {
            Some(outer) => rel
                .members(outer)
                .iter()
                .filter_map(|m| tx.get(m))
                .max_by(|a, b| a.index.cmp(&b.index).then_with(|| a.id.cmp(&b.id)))
                .map(|e| Place::Above(e.id.clone())),
            None => Some(Place::Below(rel.children(f).first().cloned().unwrap_or_else(|| f.clone()))),
        };
        if let Some(place) = place {
            let anchor = match &place {
                Place::Below(a) | Place::Above(a) => a,
                Place::AtTopmost => continue,
            };
            if !moving.contains(anchor) {
                move_block(tx, &moving, &place)?;
            }
        }
    }
    Ok(())
}

/// The element above which a new block can be inserted next to `start` without splitting a group or the block of
/// children of a frame: `start` itself, or the frame it is a child of, or the topmost member of one of its
/// groups, repeated until nothing encloses it. `joins_frame` and `joins_group` say which frames and groups the new
/// block will belong to (it may then sit inside them).
pub(crate) fn lift_above_enclosures<R: SceneRead>(
    scene: &R,
    rel: &Relations,
    start: &Element,
    joins_frame: &dyn Fn(&ElementId) -> bool,
    joins_group: &dyn Fn(&GroupId) -> bool,
) -> Element {
    let mut anchor = start.clone();
    for _ in 0..(drafftink_scene::limits::MAX_GROUP_DEPTH + 2) {
        let mut lifted: Option<Element> = None;
        if let Some(f) = anchor.frame_id.as_ref().filter(|f| !joins_frame(f))
            && let Some(fe) = live_frame(scene, f).filter(|f| f.index > anchor.index)
        {
            lifted = Some(fe.clone());
        }
        if lifted.is_none() {
            for g in rel.stack(&anchor) {
                if joins_group(&g) {
                    break;
                }
                let topmost = rel
                    .members(&g)
                    .iter()
                    .filter_map(|m| scene.get(m))
                    .max_by(|a, b| a.index.cmp(&b.index).then_with(|| a.id.cmp(&b.id)));
                if let Some(t) = topmost.filter(|t| t.index > anchor.index) {
                    lifted = Some(t.clone());
                    break;
                }
            }
        }
        match lifted {
            Some(l) => anchor = l,
            None => break,
        }
    }
    anchor
}
