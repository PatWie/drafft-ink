//! The selection model: which elements and groups a click, a marquee or "select all" selects.
//!
//! The selection state is passed in and returned; nothing here stores it (the session does).
//!
//! * `ids` holds the selected *leaf elements*: all members of every selected group. Bound text is never in `ids`:
//!   it follows its container (commands add it with [`super::units::with_companions`]).
//! * `groups` holds the groups selected as a unit (derived from `ids`, kept for the UI).
//! * `editing_group` is the group the user entered. While it is set, a click selects the next level down inside it.
//!
//! The rules follow Excalidraw's `selectGroupsForSelectedElements`, with `docs/commands.md` section 2.5:
//! * A click selects the outermost group around the element. Inside the editing group G it selects the outermost
//!   group strictly inside G, or the element itself.
//! * A deep click (Ctrl) selects the element alone and enters its innermost group.
//! * A selected frame excludes its children. Locked elements are never selected (unless asked for).
//! * A group counts as selected only when all its live members are selected, so a partial (deep) selection never
//!   claims the group.

use std::collections::BTreeSet;

use drafftink_geom::kurbo::Rect;
use drafftink_geom::lasso::{Lasso, lasso_selects};
use drafftink_geom::select::{MarqueeMode, marquee_selects};
use drafftink_scene::{Element, ElementId, GroupId, Kind, SceneRead};

use super::units::{Relations, UnitKey, impl_from_target, is_label, live, primary_id, sort_z, unit_key, units};
use crate::transform::geometry;

/// The selection state of a tab.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    /// Selected leaf elements (no bound text).
    pub ids: BTreeSet<ElementId>,
    /// Groups selected as a unit: all their live members are in `ids`.
    pub groups: BTreeSet<GroupId>,
    /// The group the user entered.
    pub editing_group: Option<GroupId>,
}

impl Selection {
    /// True when nothing is selected.
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

/// How a click combines with the current selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClickMode {
    /// Replace the selection.
    #[default]
    Replace,
    /// Add the unit.
    Add,
    /// Remove the unit.
    Remove,
    /// Remove the unit when fully selected, else add it (Shift-click).
    Toggle,
}

/// Options of [`click`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClickOpts {
    /// How to combine.
    pub mode: ClickMode,
    /// Select the element itself, not its group (Ctrl-click).
    pub deep: bool,
    /// Allow locked elements.
    pub include_locked: bool,
}

/// What was under the pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClickTarget {
    /// An element (the topmost hit; bound text counts as its container).
    Element(ElementId),
    /// Empty canvas. `inside_editing_group` tells whether the point lies inside the editing group's bounds.
    Empty {
        /// The point is inside the bounds of the editing group.
        inside_editing_group: bool,
    },
}

/// Why a selection change was refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SelectionError {
    /// No live element has this id.
    #[error("element {0} does not exist or is deleted")]
    NotFound(ElementId),
    /// The element is locked.
    #[error("{} element(s) are locked ({}). Unlock them first", .0.len(), super::units::ids_text(.0))]
    Locked(Vec<ElementId>),
    /// No group to enter.
    #[error("no group is selected. Select a group first (click it), or pass its id")]
    NoGroup,
    /// The element is not in the group.
    #[error("element {0} is not a member of that group")]
    NotInGroup(ElementId),
    /// Nothing to leave.
    #[error("you are not inside a group, so there is nothing to leave")]
    NotEditing,
    /// Select-all found nothing.
    #[error("there are no elements to select")]
    NothingToSelect,
}
impl_from_target!(SelectionError);

/// Builds a normalized selection from leaf ids: bound text becomes its container, deleted ids are dropped, the
/// children of a selected frame are dropped, and `groups` is derived. Locked elements are kept.
pub fn select_ids<R: SceneRead>(scene: &R, ids: &[ElementId], editing: Option<&GroupId>) -> Selection {
    let rel = Relations::build(scene);
    normalize(scene, &rel, ids.iter().cloned().collect(), editing.cloned())
}

fn normalize<R: SceneRead>(
    scene: &R,
    rel: &Relations,
    raw: BTreeSet<ElementId>,
    editing: Option<GroupId>,
) -> Selection {
    let mut ids: BTreeSet<ElementId> = raw
        .iter()
        .filter(|i| live(scene, i).is_ok())
        .map(|i| primary_id(scene, i))
        .filter(|i| live(scene, i).is_ok())
        .collect();
    // A selected frame excludes its children.
    let drop: Vec<ElementId> =
        ids.iter()
            .filter(|i| {
                scene.get(i).and_then(|e| e.frame_id.as_ref()).is_some_and(|f| {
                    ids.contains(f) && scene.get(f).is_some_and(|f| f.tag() == Kind::Frame && !f.deleted)
                })
            })
            .cloned()
            .collect();
    for d in drop {
        ids.remove(&d);
    }
    let editing = editing.filter(|g| rel.is_group(g));
    let groups = derive_groups(scene, rel, &ids, editing.as_ref());
    Selection { ids, groups, editing_group: editing }
}

/// The groups selected as a unit by `ids` at the level of `editing`.
fn derive_groups<R: SceneRead>(
    scene: &R,
    rel: &Relations,
    ids: &BTreeSet<ElementId>,
    editing: Option<&GroupId>,
) -> BTreeSet<GroupId> {
    let mut out = BTreeSet::new();
    for id in ids {
        let Some(e) = scene.get(id) else { continue };
        if let UnitKey::Group(g) = unit_key(rel, e, editing)
            && !out.contains(&g)
            && rel.members(&g).iter().all(|m| ids.contains(&primary_id(scene, m)))
        {
            out.insert(g);
        }
    }
    out
}

/// Drops deleted and missing ids, locked elements, and an editing group that is no longer a group. Use it after
/// a delete or a lock (inside an editing group the selection falls back to the remaining members).
pub fn prune<R: SceneRead>(scene: &R, sel: &Selection) -> Selection {
    let rel = Relations::build(scene);
    let ids: BTreeSet<ElementId> =
        sel.ids.iter().filter(|i| scene.get(i).is_some_and(|e| !e.locked)).cloned().collect();
    normalize(scene, &rel, ids, sel.editing_group.clone())
}

/// The selected ids in z-order, back to front.
pub fn in_z_order<R: SceneRead>(scene: &R, sel: &Selection) -> Vec<ElementId> {
    let mut v: Vec<ElementId> = sel.ids.iter().cloned().collect();
    sort_z(scene, &mut v);
    v
}

fn innermost_shared(scene: &impl SceneRead, rel: &Relations, ids: &BTreeSet<ElementId>) -> Option<GroupId> {
    let mut common: Option<GroupId> = None;
    for id in ids {
        let g = scene.get(id).and_then(|e| rel.stack(e).into_iter().next())?;
        match &common {
            None => common = Some(g),
            Some(c) if *c == g => {}
            Some(_) => return None,
        }
    }
    common
}

/// A click (or tap) at some target.
pub fn click<R: SceneRead>(
    scene: &R,
    sel: &Selection,
    target: &ClickTarget,
    opts: ClickOpts,
) -> Result<Selection, SelectionError> {
    let rel = Relations::build(scene);
    let hit = match target {
        ClickTarget::Empty { inside_editing_group } => {
            return Ok(match opts.mode {
                ClickMode::Replace => Selection {
                    editing_group: sel.editing_group.clone().filter(|_| *inside_editing_group),
                    ..Selection::default()
                },
                _ => sel.clone(),
            });
        }
        ClickTarget::Element(id) => id,
    };
    live(scene, hit)?;
    let primary = primary_id(scene, hit);
    let e = live(scene, &primary)?;
    if e.locked && !opts.include_locked {
        return Err(SelectionError::Locked(vec![primary]));
    }
    let editing = sel.editing_group.as_ref();
    let mut unit: BTreeSet<ElementId> = if opts.deep {
        BTreeSet::from([primary.clone()])
    } else {
        units(scene, &rel, std::slice::from_ref(&primary), editing)?.into_iter().flat_map(|u| u.elements).collect()
    };
    if !opts.include_locked {
        unit.retain(|i| scene.get(i).is_some_and(|e| !e.locked));
    }
    let hit_inside_editing = editing.is_some_and(|g| rel.is_group(g) && e.group_ids.contains(g));
    let (ids, editing_out) = match opts.mode {
        ClickMode::Replace => {
            let ed = if opts.deep {
                innermost_shared(scene, &rel, &unit)
            } else {
                editing.cloned().filter(|_| hit_inside_editing)
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
            let ed = if opts.deep && !remove { innermost_shared(scene, &rel, &ids) } else { sel.editing_group.clone() };
            (ids, ed)
        }
    };
    Ok(normalize(scene, &rel, ids, editing_out))
}

/// Enters a group: clicks now select one level down inside it. `group` defaults to the single selected group.
/// With `select`, that element's unit inside the group becomes the selection (the double-click target);
/// otherwise the current ids are kept.
pub fn enter_group<R: SceneRead>(
    scene: &R,
    sel: &Selection,
    group: Option<&GroupId>,
    select: Option<&ElementId>,
) -> Result<Selection, SelectionError> {
    let rel = Relations::build(scene);
    let g = match group {
        Some(g) => g.clone(),
        None if sel.groups.len() == 1 => sel.groups.iter().next().cloned().ok_or(SelectionError::NoGroup)?,
        None => return Err(SelectionError::NoGroup),
    };
    if !rel.is_group(&g) {
        return Err(SelectionError::NoGroup);
    }
    let ids = match select {
        Some(id) => {
            let p = primary_id(scene, id);
            let e = live(scene, &p)?;
            if !e.group_ids.contains(&g) {
                return Err(SelectionError::NotInGroup(p));
            }
            units(scene, &rel, &[p], Some(&g))?.into_iter().flat_map(|u| u.elements).collect()
        }
        None => sel.ids.clone(),
    };
    Ok(normalize(scene, &rel, ids, Some(g)))
}

/// Leaves the editing group and selects that group again. The editing group becomes the group around it, if any.
pub fn exit_group<R: SceneRead>(scene: &R, sel: &Selection) -> Result<Selection, SelectionError> {
    let rel = Relations::build(scene);
    let Some(g) = sel.editing_group.clone().filter(|g| rel.is_group(g)) else {
        return Err(SelectionError::NotEditing);
    };
    let members: BTreeSet<ElementId> = rel.members(&g).iter().map(|m| primary_id(scene, m)).collect();
    let parent = members.iter().next().and_then(|m| scene.get(m)).and_then(|e| {
        let stack = rel.stack(e);
        let i = stack.iter().position(|s| *s == g)?;
        stack.get(i + 1).cloned()
    });
    Ok(normalize(scene, &rel, members, parent))
}

/// What "select all" is limited to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Within {
    /// The children of a frame.
    Frame(ElementId),
    /// The members of a group.
    Group(GroupId),
}

/// Selects all unlocked elements: inside an editing group its members, with `within` a frame's children or a
/// group's members, else the whole scene (as groups).
pub fn select_all<R: SceneRead>(
    scene: &R,
    sel: &Selection,
    within: Option<&Within>,
    include_locked: bool,
) -> Result<Selection, SelectionError> {
    let rel = Relations::build(scene);
    let (candidates, editing): (Vec<ElementId>, Option<GroupId>) = match within {
        Some(Within::Frame(f)) => {
            let fr = live(scene, f)?;
            if fr.tag() != Kind::Frame {
                return Err(SelectionError::NotInGroup(f.clone()));
            }
            (rel.children(f).to_vec(), None)
        }
        Some(Within::Group(g)) => {
            if !rel.is_group(g) {
                return Err(SelectionError::NoGroup);
            }
            (rel.members(g).to_vec(), None)
        }
        None => match sel.editing_group.as_ref().filter(|g| rel.is_group(g)) {
            Some(g) => (rel.members(g).to_vec(), Some(g.clone())),
            None => (scene.iter().map(|e| e.id.clone()).collect(), None),
        },
    };
    let ids: BTreeSet<ElementId> = candidates
        .into_iter()
        .filter(|i| scene.get(i).is_some_and(|e| (include_locked || !e.locked) && !is_label(scene, e)))
        .collect();
    let out = normalize(scene, &rel, ids, editing);
    if out.ids.is_empty() { Err(SelectionError::NothingToSelect) } else { Ok(out) }
}

/// How a marquee combines with the current selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RegionMode {
    /// Replace the selection and leave the editing group.
    #[default]
    Replace,
    /// Add to the selection (Shift).
    Add,
}

/// Options of [`select_region`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegionOpts {
    /// Replace or add.
    pub mode: RegionMode,
    /// Touch (any overlap, the old behaviour) or contain (fully inside).
    pub contain: MarqueeMode,
    /// Allow locked elements.
    pub include_locked: bool,
}

impl Default for RegionOpts {
    fn default() -> Self {
        Self { mode: RegionMode::Replace, contain: MarqueeMode::Touch, include_locked: false }
    }
}

/// Marquee selection. Any member inside the rectangle selects its whole unit (the outermost group; with `Add`
/// inside the editing group, the next level down). Locked elements and bound text are ignored, and the children
/// of a selected frame are not selected together with it.
pub fn select_region<R: SceneRead>(scene: &R, sel: &Selection, rect: Rect, opts: RegionOpts) -> Selection {
    select_where(scene, sel, opts, |e| in_region(e, &rect, opts.contain))
}

/// Lasso selection: the marquee rules with a free-hand outline instead of a rectangle (`opts.contain` is the
/// lasso's `Touch` or `Contain`, see [`drafftink_geom::lasso`]). Any member selected selects its whole unit, locked
/// elements and bound text are ignored, and the children of a selected frame are not selected together with it.
pub fn select_lasso<R: SceneRead>(scene: &R, sel: &Selection, lasso: &Lasso, opts: RegionOpts) -> Selection {
    select_where(scene, sel, opts, |e| in_lasso(e, lasso, opts.contain))
}

/// The selection after a region gesture whose test is `hit`, combined with `sel` as `opts.mode` says.
fn select_where<R: SceneRead>(
    scene: &R,
    sel: &Selection,
    opts: RegionOpts,
    hit: impl Fn(&Element) -> bool,
) -> Selection {
    let rel = Relations::build(scene);
    let editing = match opts.mode {
        RegionMode::Replace => None,
        RegionMode::Add => sel.editing_group.clone(),
    };
    let hits: Vec<ElementId> = scene
        .iter()
        .filter(|e| (opts.include_locked || !e.locked) && !is_label(scene, e))
        .filter(|e| hit(e))
        .map(|e| e.id.clone())
        .collect();
    let mut ids: BTreeSet<ElementId> = match opts.mode {
        RegionMode::Replace => BTreeSet::new(),
        RegionMode::Add => sel.ids.clone(),
    };
    if let Ok(us) = units(scene, &rel, &hits, editing.as_ref()) {
        for u in us {
            ids.extend(
                u.elements.into_iter().filter(|i| opts.include_locked || scene.get(i).is_some_and(|e| !e.locked)),
            );
        }
    }
    normalize(scene, &rel, ids, editing)
}

fn in_region(e: &Element, rect: &Rect, mode: MarqueeMode) -> bool {
    match geometry::primitive(e) {
        Some(p) => marquee_selects(&p, rect, mode, geometry::is_filled(e)),
        None => false,
    }
}

fn in_lasso(e: &Element, lasso: &Lasso, mode: MarqueeMode) -> bool {
    match geometry::primitive(e) {
        Some(p) => lasso_selects(&p, lasso, mode, geometry::is_filled(e)),
        None => false,
    }
}
