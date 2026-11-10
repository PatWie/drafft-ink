//! Groups: a group is an id shared by the `group_ids` of its members, innermost first.
//!
//! * **Ids never change.** Grouping and ungrouping only edit `group_ids`.
//! * **Grouping keeps the order of everything else.** The members become one contiguous block at the slot of the
//!   topmost member; elements that sat between members end up below the block, in their old relative order. (The
//!   old app put the group at the front slot with index arithmetic that dropped elements.)
//! * **Nesting.** A new group is the outermost entry of its members, or, inside an editing group, sits just inside
//!   that group. Ungrouping removes only the selected level.
//! * **Bound text** carries its container's groups, frame and place in the order, so it is grouped, ungrouped and
//!   moved with the container.
//! * **Stale ids.** A group with fewer than two live members counts as no group everywhere. A stale id is removed
//!   from an element the next time `group` or `ungroup` edits it.
//! * **Group names.** A group is not an element, and the scene model has no group table, so the name is stored in
//!   the `custom` map of every live member under the key `drafftink.group.<group id>` (a JSON string). It follows
//!   the members through undo and sync, is copied by duplicate (with the new group id) and removed on ungroup. If
//!   members disagree (concurrent edits), the first member in z-order that has a name wins.
//!
//! Selection rules (which ids and groups a click selects, entering and leaving a group, marquee) are in
//! [`selection`]. The shared unit and relation helpers are in [`units`].

use drafftink_scene::{Element, ElementId, GroupId, Kind, SceneRead, SceneTx, StoreError, limits};

use crate::zorder::{Place, ZOrderError, ZReport, move_block};

pub mod selection;
pub mod units;

use units::{Relations, ensure_unlocked, impl_from_target, primaries, units as units_of, with_companions};

/// Why a group operation was refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum GroupError {
    /// No live element has this id.
    #[error("element {0} does not exist or is deleted")]
    NotFound(ElementId),
    /// Locked elements cannot be grouped or ungrouped.
    #[error("{} element(s) are locked ({}). Unlock them first", .0.len(), units::ids_text(.0))]
    Locked(Vec<ElementId>),
    /// Fewer than two units were given.
    #[error("grouping needs at least 2 elements, and {have} were given. Select more")]
    TooFew {
        /// Units given.
        have: usize,
    },
    /// The elements already are one group at this level.
    #[error("these elements already form one group at this level. Ungroup them first, or select more elements")]
    AlreadyGrouped,
    /// A frame and one of its own children cannot be in one group.
    #[error("frame {frame} and its own children cannot be grouped together. Select either the frame or its children")]
    FrameAndChildren {
        /// The frame.
        frame: ElementId,
    },
    /// No group to work on.
    #[error("no group is selected. Select a group first, or pass its id")]
    NoGroup,
    /// The group id is already used by an element, deleted ones included.
    #[error("the group id {0} is already in use")]
    GroupIdInUse(GroupId),
    /// Grouping would nest groups deeper than the model allows.
    #[error("element {id} would be nested in more than {max} groups")]
    TooDeep {
        /// The element.
        id: ElementId,
        /// The limit.
        max: usize,
    },
    /// The editing group is not a group of this element.
    #[error("element {0} is not inside the editing group")]
    NotInEditingGroup(ElementId),
    /// A group name is 1 to 64 characters without control characters.
    #[error("a group name must be 1 to 64 characters without control characters")]
    BadName,
    /// The store refused a write.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Z-order could not be updated.
    #[error(transparent)]
    ZOrder(#[from] ZOrderError),
}
impl_from_target!(GroupError);

/// Longest group name, in characters.
pub const MAX_GROUP_NAME_CHARS: usize = 64;

/// The `custom` key a group's name is stored under.
pub fn name_key(group: &GroupId) -> String {
    format!("drafftink.group.{group}")
}

/// Outcome of [`group`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupReport {
    /// The new group.
    pub group: GroupId,
    /// Its members (bound text not listed), in z-order.
    pub members: Vec<ElementId>,
    /// Members that were removed from their frames because they came from different frames.
    pub left_frames: Vec<ElementId>,
    /// What the z-order change touched.
    pub z: ZReport,
}

/// Groups the units of `ids` under `new_group`.
///
/// `editing` is the editing group: units are the next level down and the new group sits just inside it.
/// The caller supplies a fresh `new_group` (from an `IdSource`).
pub fn group<T: SceneTx>(
    tx: &mut T,
    ids: &[ElementId],
    editing: Option<&GroupId>,
    new_group: GroupId,
) -> Result<GroupReport, GroupError> {
    let rel = Relations::build(&*tx);
    let prim = primaries(&*tx, ids)?;
    if tx.iter_all().any(|e| e.group_ids.contains(&new_group)) {
        return Err(GroupError::GroupIdInUse(new_group));
    }
    if let Some(g) = editing {
        for id in &prim {
            if tx.get(id).is_some_and(|e| !rel.is_group(g) || !e.group_ids.contains(g)) {
                return Err(GroupError::NotInEditingGroup(id.clone()));
            }
        }
    }
    let us = units_of(&*tx, &rel, &prim, editing)?;
    if us.len() < 2 {
        return match us.first().map(|u| &u.key) {
            Some(units::UnitKey::Group(_)) => Err(GroupError::AlreadyGrouped),
            _ => Err(GroupError::TooFew { have: us.len() }),
        };
    }
    let members: Vec<ElementId> = {
        let mut m: Vec<ElementId> = us.iter().flat_map(|u| u.elements.iter().cloned()).collect();
        units::sort_z(&*tx, &mut m);
        m
    };
    let all = with_companions(&*tx, &rel, &members, false);
    ensure_unlocked(&*tx, &all)?;
    for m in &members {
        let Some(e) = tx.get(m) else { continue };
        if e.tag() == Kind::Frame && rel.children(m).iter().any(|c| members.contains(c)) {
            return Err(GroupError::FrameAndChildren { frame: m.clone() });
        }
        let stack = rel.stack(e).len();
        if stack + 1 > limits::MAX_GROUP_DEPTH {
            return Err(GroupError::TooDeep { id: m.clone(), max: limits::MAX_GROUP_DEPTH });
        }
    }
    // Members from different frames leave their frames first.
    let frames: Vec<Option<ElementId>> = members.iter().filter_map(|m| tx.get(m)).map(|e| e.frame_id.clone()).collect();
    let same_frame = frames.windows(2).all(|w| w[0] == w[1]);
    let mut left_frames = Vec::new();
    let mut left_pairs: Vec<(ElementId, ElementId)> = Vec::new();

    for m in &members {
        let stale = tx.get(m).map(|e| rel.stale_groups(e)).unwrap_or_default();
        let clear_frame = !same_frame && tx.get(m).is_some_and(|e| e.frame_id.is_some());
        if clear_frame {
            left_frames.push(m.clone());
            if let Some(f) = tx.get(m).and_then(|e| e.frame_id.clone()) {
                left_pairs.push((m.clone(), f));
            }
        }
        let new_group = new_group.clone();
        tx.update(m, |e| {
            e.group_ids.retain(|g| !stale.contains(g));
            let at = editing.and_then(|g| e.group_ids.iter().position(|x| x == g));
            match at {
                Some(i) => e.group_ids.insert(i, new_group),
                None => e.group_ids.push(new_group),
            }
            if clear_frame {
                e.frame_id = None;
            }
        })?;
    }
    sync_labels(tx, &rel, &members)?;
    crate::zorder::settle_leavers(tx, &left_pairs)?;
    // A frame among the members keeps its children directly below it.
    let moving = with_companions(&*tx, &rel, &members, true);
    let z = move_block(tx, &moving, &Place::AtTopmost)?;
    Ok(GroupReport { group: new_group, members, left_frames, z })
}

/// Makes each label carry the groups and frame of its container.
fn sync_labels<T: SceneTx>(tx: &mut T, rel: &Relations, containers: &[ElementId]) -> Result<(), StoreError> {
    for c in containers {
        let Some((groups, frame)) = tx.get(c).map(|e| (e.group_ids.clone(), e.frame_id.clone())) else {
            continue;
        };
        for l in rel.labels(c) {
            let (groups, frame) = (groups.clone(), frame.clone());
            tx.update(l, |e| {
                e.group_ids = groups;
                e.frame_id = frame;
            })?;
        }
    }
    Ok(())
}

/// Outcome of [`ungroup`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UngroupReport {
    /// The groups that were dissolved.
    pub removed: Vec<GroupId>,
    /// Their members (bound text not listed), in z-order.
    pub members: Vec<ElementId>,
}

/// Dissolves groups: the groups that `ids` stand for at the level of `editing` (an element of a group stands for
/// its group), or just `group` when given.
/// Only that level is removed from the members; inner and outer groups stay. Z-order is not changed.
pub fn ungroup<T: SceneTx>(
    tx: &mut T,
    ids: &[ElementId],
    group: Option<&GroupId>,
    editing: Option<&GroupId>,
) -> Result<UngroupReport, GroupError> {
    let rel = Relations::build(&*tx);
    let targets: Vec<GroupId> = match group {
        Some(g) if rel.is_group(g) => vec![g.clone()],
        Some(_) => return Err(GroupError::NoGroup),
        None => {
            // The group units among the given elements: an element of a group stands for its group.
            let prim = primaries(&*tx, ids)?;
            let mut found: Vec<GroupId> = Vec::new();
            for u in units_of(&*tx, &rel, &prim, editing)? {
                if let units::UnitKey::Group(g) = u.key {
                    found.push(g);
                }
            }
            found
        }
    };
    if targets.is_empty() {
        return Err(GroupError::NoGroup);
    }
    let mut members: Vec<ElementId> = targets.iter().flat_map(|g| rel.members(g).iter().cloned()).collect();
    members.sort();
    members.dedup();
    ensure_unlocked(&*tx, &members)?;
    for id in &members {
        let keys: Vec<String> = targets.iter().map(name_key).collect();
        let t = targets.clone();
        tx.update(id, move |e| {
            e.group_ids.retain(|g| !t.contains(g));
            remove_custom(e, &keys);
        })?;
    }
    let mut primary_members: Vec<ElementId> = members
        .iter()
        .map(|m| units::primary_id(&*tx, m))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    units::sort_z(&*tx, &mut primary_members);
    Ok(UngroupReport { removed: targets, members: primary_members })
}

fn remove_custom(e: &mut Element, keys: &[String]) {
    if let Some(c) = &mut e.custom {
        for k in keys {
            c.remove(k);
        }
        if c.is_empty() {
            e.custom = None;
        }
    }
}

/// The name of a group, if it has one.
pub fn group_name<R: SceneRead>(scene: &R, group: &GroupId) -> Option<String> {
    let key = name_key(group);
    scene
        .iter()
        .filter(|e| e.group_ids.contains(group))
        .find_map(|e| e.custom.as_ref().and_then(|c| c.get(&key)).and_then(|v| v.as_str()).map(str::to_owned))
}

/// Sets (or with `None` clears) the name of a group on all its members. Returns the elements written.
pub fn set_group_name<T: SceneTx>(
    tx: &mut T,
    group: &GroupId,
    name: Option<&str>,
) -> Result<Vec<ElementId>, GroupError> {
    let rel = Relations::build(&*tx);
    if !rel.is_group(group) {
        return Err(GroupError::NoGroup);
    }
    if let Some(n) = name
        && (n.is_empty() || n.chars().count() > MAX_GROUP_NAME_CHARS || n.chars().any(char::is_control))
    {
        return Err(GroupError::BadName);
    }
    let members = rel.members(group).to_vec();
    ensure_unlocked(&*tx, &members)?;
    let key = name_key(group);
    for m in &members {
        let key = key.clone();
        tx.update(m, |e| match name {
            Some(n) => {
                e.custom.get_or_insert_with(Default::default).insert(key, n.into());
            }
            None => remove_custom(e, &[key]),
        })?;
    }
    Ok(members)
}

/// Groups whose live members are not next to each other in the z-order. Tombstones are ignored, and so are the
/// children of a frame that is a member of the group (they sit between the group's members by design). A
/// healthy scene has none.
pub fn non_contiguous_groups<R: SceneRead>(scene: &R) -> Vec<GroupId> {
    let live: Vec<&Element> = scene.iter().collect();
    let mut groups: std::collections::BTreeSet<&GroupId> = std::collections::BTreeSet::new();
    for e in &live {
        groups.extend(e.group_ids.iter());
    }
    let mut bad = Vec::new();
    for g in groups {
        let frames: std::collections::HashSet<&ElementId> =
            live.iter().filter(|e| e.tag() == Kind::Frame && e.group_ids.contains(g)).map(|e| &e.id).collect();
        let seq: Vec<&&Element> = live
            .iter()
            .filter(|e| e.frame_id.as_ref().is_none_or(|f| !frames.contains(f)) || e.group_ids.contains(g))
            .collect();
        let positions: Vec<usize> =
            seq.iter().enumerate().filter(|(_, e)| e.group_ids.contains(g)).map(|(i, _)| i).collect();
        if let (Some(lo), Some(hi)) = (positions.first(), positions.last())
            && hi - lo + 1 != positions.len()
        {
            bad.push(g.clone());
        }
    }
    bad
}

/// Repairs a group that is not contiguous (after a merge of concurrent edits): its members move together at the
/// slot of the topmost member.
pub fn make_contiguous<T: SceneTx>(tx: &mut T, group: &GroupId) -> Result<ZReport, GroupError> {
    let rel = Relations::build(&*tx);
    if !rel.is_group(group) {
        return Err(GroupError::NoGroup);
    }
    let all = with_companions(&*tx, &rel, rel.members(group), true);
    Ok(move_block(tx, &all, &Place::AtTopmost)?)
}
