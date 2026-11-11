//! Selecting many targets at once, selecting a group by id, and the pointer rules of a click on the canvas.
//!
//! [`pick`] is [`click`](super::click) for a list of targets: one pass over the scene instead
//! of one per target. For one target it gives the same result as `click` (a test checks every mode). The
//! differences are the ones a list needs: a locked target is skipped and reported instead of refused, and
//! toggle removes the targets only when all of them are selected.

use std::collections::BTreeSet;

use drafftink_geom::kurbo::{Point, Rect};

use drafftink_scene::{ElementId, GroupId, SceneRead};

use super::{ClickMode, ClickOpts, ClickTarget, Selection, SelectionError, select_ids};
use crate::group::units::{Relations, UnitKey, primaries, unit_key, units};
use crate::query::AtHit;
use crate::transform::common_bounds;

/// The selection after a pick, and the locked elements that were left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    /// The new selection. It equals the old one when nothing could be picked.
    pub selection: Selection,
    /// Locked targets (or locked members of a picked group) that were skipped, sorted. Empty with
    /// `include_locked`.
    pub skipped_locked: Vec<ElementId>,
}

/// Picks `targets` (bound text counts as its container) into `sel`.
///
/// Like a click: without `deep` each target selects its unit (the outermost group, inside an editing group the
/// next level down); with `deep` the element itself, and the editing group becomes the innermost group they all
/// share. Locked elements are skipped unless `include_locked`. When nothing is left to pick the selection is
/// returned unchanged. Fails with `NotFound` for an id that is not a live element.
pub fn pick<R: SceneRead>(
    scene: &R,
    sel: &Selection,
    targets: &[ElementId],
    opts: ClickOpts,
) -> Result<Pick, SelectionError> {
    let rel = Relations::build(scene);
    pick_with(scene, &rel, sel, targets, opts)
}

/// Picks the members of `group`. Fails with `NoGroup` when `group` has fewer than two live members.
pub fn pick_group<R: SceneRead>(
    scene: &R,
    sel: &Selection,
    group: &GroupId,
    opts: ClickOpts,
) -> Result<Pick, SelectionError> {
    let rel = Relations::build(scene);
    if !rel.is_group(group) {
        return Err(SelectionError::NoGroup);
    }
    let members = rel.members(group).to_vec();
    pick_with(scene, &rel, sel, &members, opts)
}

fn pick_with<R: SceneRead>(
    scene: &R,
    rel: &Relations,
    sel: &Selection,
    targets: &[ElementId],
    opts: ClickOpts,
) -> Result<Pick, SelectionError> {
    let is_locked = |i: &ElementId| scene.get(i).is_some_and(|e| e.locked);
    let mut skipped: Vec<ElementId> = Vec::new();
    let mut usable: Vec<ElementId> = Vec::new();
    for t in primaries(scene, targets)? {
        if is_locked(&t) && !opts.include_locked {
            skipped.push(t);
        } else {
            usable.push(t);
        }
    }
    let unchanged = |skipped: Vec<ElementId>| Pick { selection: sel.clone(), skipped_locked: sorted(skipped) };
    if usable.is_empty() {
        return Ok(unchanged(skipped));
    }
    let editing = sel.editing_group.as_ref();
    let mut unit: BTreeSet<ElementId> = if opts.deep {
        usable.iter().cloned().collect()
    } else {
        units(scene, rel, &usable, editing)?.into_iter().flat_map(|u| u.elements).collect()
    };
    if !opts.include_locked {
        skipped.extend(unit.iter().filter(|i| is_locked(i)).cloned());
        unit.retain(|i| !is_locked(i));
    }
    if unit.is_empty() {
        return Ok(unchanged(skipped));
    }
    let (ids, editing_out) = match opts.mode {
        ClickMode::Replace => {
            let ed = if opts.deep {
                innermost_shared(scene, rel, &unit)
            } else {
                editing.cloned().filter(|g| {
                    rel.is_group(g) && usable.iter().all(|i| scene.get(i).is_some_and(|e| e.group_ids.contains(g)))
                })
            };
            (unit, ed)
        }
        ClickMode::Add | ClickMode::Remove | ClickMode::Toggle => {
            let remove = match opts.mode {
                ClickMode::Remove => true,
                ClickMode::Toggle => unit.is_subset(&sel.ids),
                _ => false,
            };
            let mut ids = sel.ids.clone();
            if remove {
                for i in &unit {
                    ids.remove(i);
                }
            } else {
                ids.extend(unit.iter().cloned());
            }
            let ed = if opts.deep && !remove { innermost_shared(scene, rel, &ids) } else { sel.editing_group.clone() };
            (ids, ed)
        }
    };
    let ids: Vec<ElementId> = ids.into_iter().collect();
    Ok(Pick { selection: select_ids(scene, &ids, editing_out.as_ref()), skipped_locked: sorted(skipped) })
}

fn sorted(mut v: Vec<ElementId>) -> Vec<ElementId> {
    v.sort();
    v.dedup();
    v
}

/// The innermost group that all `ids` share as their first group (the group a deep selection enters).
fn innermost_shared<R: SceneRead>(scene: &R, rel: &Relations, ids: &BTreeSet<ElementId>) -> Option<GroupId> {
    let mut common: Option<GroupId> = None;
    for id in ids {
        let g = scene.get(id).and_then(|e| rel.stack(e).next().cloned())?;
        match &common {
            None => common = Some(g),
            Some(c) if *c == g => {}
            Some(_) => return None,
        }
    }
    common
}

fn contains_point(r: &Rect, p: Point) -> bool {
    p.x >= r.x0 && p.x <= r.x1 && p.y >= r.y0 && p.y <= r.y1
}

/// Whether `point` lies inside the bounds of the editing group (false without one).
pub fn inside_editing_group<R: SceneRead>(scene: &R, rel: &Relations, sel: &Selection, point: Point) -> bool {
    let Some(g) = sel.editing_group.as_ref().filter(|g| rel.is_group(g)) else { return false };
    common_bounds(scene, rel.members(g)).is_some_and(|b| contains_point(&b, point))
}

/// What a click at `point` hits. `hits` are the elements under the point, top first
/// ([`elements_at`](crate::query::elements_at); with `all` when `cycle`).
///
/// With `cycle` the units under the point are visited top to bottom, one per call: when the selection is
/// exactly one of them, the one below it (wrapping around) is chosen, else the topmost.
pub fn click_target<R: SceneRead>(
    scene: &R,
    sel: &Selection,
    hits: &[AtHit],
    point: Point,
    deep: bool,
    cycle: bool,
) -> ClickTarget {
    let rel = Relations::build(scene);
    let chosen = if cycle { cycle_pick(scene, &rel, sel, hits, deep) } else { hits.first() };
    match chosen {
        Some(h) => ClickTarget::Element(h.id.clone()),
        None => ClickTarget::Empty { inside_editing_group: inside_editing_group(scene, &rel, sel, point) },
    }
}

fn cycle_pick<'a, R: SceneRead>(
    scene: &R,
    rel: &Relations,
    sel: &Selection,
    hits: &'a [AtHit],
    deep: bool,
) -> Option<&'a AtHit> {
    let editing = sel.editing_group.as_ref();
    let mut seen: Vec<UnitKey> = Vec::new();
    let mut cands: Vec<(&AtHit, BTreeSet<ElementId>)> = Vec::new();
    for h in hits {
        let Some(e) = scene.get(&h.id) else { continue };
        let key = if deep { UnitKey::Element(h.id.clone()) } else { unit_key(rel, e, editing) };
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        let members: BTreeSet<ElementId> = if deep {
            BTreeSet::from([h.id.clone()])
        } else {
            units(scene, rel, std::slice::from_ref(&h.id), editing)
                .ok()
                .into_iter()
                .flatten()
                .flat_map(|u| u.elements)
                .collect()
        };
        cands.push((h, members));
    }
    let next = match cands.iter().position(|(_, m)| *m == sel.ids) {
        Some(i) => i.checked_add(1).filter(|n| *n < cands.len()).unwrap_or(0),
        None => 0,
    };
    cands.get(next).map(|(h, _)| *h)
}

/// True when a click at `point` should keep the current multi-selection: at least two elements are selected,
/// the point lies inside their common bounds, and nothing is under it or the top hit is selected. (A click on
/// the body of a selected group starts a move, it does not narrow the selection.)
pub fn keeps_multi_selection<R: SceneRead>(scene: &R, sel: &Selection, hits: &[AtHit], point: Point) -> bool {
    if sel.ids.len() < 2 {
        return false;
    }
    let ids: Vec<ElementId> = sel.ids.iter().cloned().collect();
    let inside = common_bounds(scene, &ids).is_some_and(|b| contains_point(&b, point));
    inside && hits.first().is_none_or(|h| sel.ids.contains(&h.id))
}
