//! Shared structure helpers: relations of a scene and the "unit" a click, a z-order step or an alignment works on.
//!
//! A *unit* is what the user treats as one thing: a plain element, or the outermost group around it. Inside an
//! editing group the unit is the next level down: the outermost group strictly inside the editing group, or the
//! element itself. Bound text (a label) is never a unit of its own: it travels with its container.
//!
//! A transaction offers no derived index (the store's `DerivedIndex` only reflects committed state), so
//! [`Relations`] is built with one pass over the live elements. Build it once per command and pass it around.
//!
//! A group with fewer than two live members counts as no group (`docs/model.md` section 8). Every function here
//! uses the *effective* group stack that skips such stale ids.

use std::collections::{BTreeSet, HashMap};

use drafftink_profile::{Counter, count};
use drafftink_scene::{DerivedIndex, Element, ElementId, GroupId, Kind, SceneRead};

use crate::target::TargetError;

/// The reverse relations of a scene, built in one pass over the live elements.
#[derive(Debug, Clone, Default)]
pub struct Relations {
    members: HashMap<GroupId, Vec<ElementId>>,
    children: HashMap<ElementId, Vec<ElementId>>,
    labels: HashMap<ElementId, Vec<ElementId>>,
}

impl Relations {
    /// Builds the relations of the live elements. Every list is in z-order, back to front.
    pub fn build<R: SceneRead>(scene: &R) -> Self {
        count(Counter::ElementsScanned, u64::try_from(scene.len_all()).unwrap_or(u64::MAX));
        let mut rel = Relations::default();
        for e in scene.iter() {
            for g in &e.group_ids {
                rel.members.entry(g.clone()).or_default().push(e.id.clone());
            }
            if let Some(f) = &e.frame_id {
                rel.children.entry(f.clone()).or_default().push(e.id.clone());
            }
            if let Some(c) = e.container_id() {
                rel.labels.entry(c.clone()).or_default().push(e.id.clone());
            }
        }
        rel
    }

    /// Live members of a group, in z-order.
    pub fn members(&self, group: &GroupId) -> &[ElementId] {
        self.members.get(group).map_or(&[], Vec::as_slice)
    }

    /// A group needs two live members.
    pub fn is_group(&self, group: &GroupId) -> bool {
        self.members(group).len() >= 2
    }

    /// Live children of a frame, in z-order.
    pub fn children(&self, frame: &ElementId) -> &[ElementId] {
        self.children.get(frame).map_or(&[], Vec::as_slice)
    }

    /// Live texts that claim `container` as their container, in z-order.
    pub fn labels(&self, container: &ElementId) -> &[ElementId] {
        self.labels.get(container).map_or(&[], Vec::as_slice)
    }

    /// The groups of `e` that count (at least two live members), innermost first.
    pub fn stack(&self, e: &Element) -> Vec<GroupId> {
        e.group_ids.iter().filter(|g| self.is_group(g)).cloned().collect()
    }

    /// Group ids on `e` that no longer have two live members.
    pub fn stale_groups(&self, e: &Element) -> Vec<GroupId> {
        e.group_ids.iter().filter(|g| !self.is_group(g)).cloned().collect()
    }
}

/// The reverse lookups [`with_companions`] needs, so that a caller can answer them from [`Relations`] (one pass
/// over the scene) or from the committed [`DerivedIndex`] (no pass).
pub trait ReverseLookup {
    /// Live texts that claim `container` as their container.
    fn labels_of(&self, container: &ElementId) -> Vec<ElementId>;
    /// Live children of `frame`.
    fn children_of(&self, frame: &ElementId) -> Vec<ElementId>;
    /// Live members of a group, in no particular order.
    fn members_of(&self, group: &GroupId) -> Vec<ElementId>;
    /// Does the group have two live members?
    fn is_group(&self, group: &GroupId) -> bool;
}

impl ReverseLookup for Relations {
    fn labels_of(&self, container: &ElementId) -> Vec<ElementId> {
        self.labels(container).to_vec()
    }
    fn children_of(&self, frame: &ElementId) -> Vec<ElementId> {
        self.children(frame).to_vec()
    }
    fn members_of(&self, group: &GroupId) -> Vec<ElementId> {
        self.members(group).to_vec()
    }
    fn is_group(&self, group: &GroupId) -> bool {
        Relations::is_group(self, group)
    }
}

impl ReverseLookup for DerivedIndex {
    fn labels_of(&self, container: &ElementId) -> Vec<ElementId> {
        DerivedIndex::labels_of(self, container).cloned().collect()
    }
    fn children_of(&self, frame: &ElementId) -> Vec<ElementId> {
        self.frame_children(frame).cloned().collect()
    }
    fn members_of(&self, group: &GroupId) -> Vec<ElementId> {
        self.group_members(group).cloned().collect()
    }
    fn is_group(&self, group: &GroupId) -> bool {
        DerivedIndex::is_group(self, group)
    }
}

/// Reverse lookups for a few elements of `scene`: the committed index when the reader offers one (a store, or a
/// transaction that has not written yet), else one pass over the scene. The lookups cost O(answer) in the first
/// case, so a gesture frame on a large scene does not walk the scene.
pub enum Reverse<'a> {
    /// Answered by the derived index.
    Index(&'a DerivedIndex),
    /// Answered by relations built with a pass.
    Scan(Relations),
}

impl<'a> Reverse<'a> {
    /// The cheapest source `scene` offers.
    pub fn of<R: SceneRead>(scene: &'a R) -> Self {
        match scene.derived_index() {
            Some(index) => Reverse::Index(index),
            None => Reverse::Scan(Relations::build(scene)),
        }
    }
}

impl ReverseLookup for Reverse<'_> {
    fn labels_of(&self, container: &ElementId) -> Vec<ElementId> {
        match self {
            Reverse::Index(i) => ReverseLookup::labels_of(*i, container),
            Reverse::Scan(r) => r.labels_of(container),
        }
    }
    fn children_of(&self, frame: &ElementId) -> Vec<ElementId> {
        match self {
            Reverse::Index(i) => i.children_of(frame),
            Reverse::Scan(r) => r.children_of(frame),
        }
    }
    fn members_of(&self, group: &GroupId) -> Vec<ElementId> {
        match self {
            Reverse::Index(i) => ReverseLookup::members_of(*i, group),
            Reverse::Scan(r) => r.members_of(group),
        }
    }
    fn is_group(&self, group: &GroupId) -> bool {
        match self {
            Reverse::Index(i) => ReverseLookup::is_group(*i, group),
            Reverse::Scan(r) => ReverseLookup::is_group(r, group),
        }
    }
}

/// What a click or a z-order step treats as one thing.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum UnitKey {
    /// A plain element.
    Element(ElementId),
    /// A group, with all its members.
    Group(GroupId),
}

/// One unit: its key and its members in z-order (members only: no bound text, no frame children).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    /// Element or group.
    pub key: UnitKey,
    /// The elements of the unit, in z-order.
    pub elements: Vec<ElementId>,
}

/// The unit that `e` belongs to when `editing` is the editing group.
pub fn unit_key<L: ReverseLookup + ?Sized>(rel: &L, e: &Element, editing: Option<&GroupId>) -> UnitKey {
    let stack: Vec<&GroupId> = e.group_ids.iter().filter(|g| rel.is_group(g)).collect();
    if let Some(g) = editing
        && let Some(i) = stack.iter().position(|s| *s == g)
    {
        return match i.checked_sub(1).and_then(|j| stack.get(j)) {
            Some(inner) => UnitKey::Group((*inner).clone()),
            None => UnitKey::Element(e.id.clone()),
        };
    }
    match stack.last() {
        Some(g) => UnitKey::Group((*g).clone()),
        None => UnitKey::Element(e.id.clone()),
    }
}

/// The live element with this id.
pub fn live<'a, R: SceneRead>(scene: &'a R, id: &ElementId) -> Result<&'a Element, TargetError> {
    match scene.get(id) {
        Some(e) if !e.deleted => Ok(e),
        _ => Err(TargetError::NotFound(id.clone())),
    }
}

/// `id`, or its container when `id` is the bound text of a live container.
pub fn primary_id<R: SceneRead>(scene: &R, id: &ElementId) -> ElementId {
    if let Some(e) = scene.get(id)
        && let Some(c) = e.container_id()
        && scene.get(c).is_some_and(|c| !c.deleted)
    {
        return c.clone();
    }
    id.clone()
}

/// The ids with every bound text replaced by its container, checked to be live, without duplicates, in z-order.
pub fn primaries<R: SceneRead>(scene: &R, ids: &[ElementId]) -> Result<Vec<ElementId>, TargetError> {
    let mut set = BTreeSet::new();
    for id in ids {
        live(scene, id)?;
        set.insert(primary_id(scene, id));
    }
    let mut v: Vec<ElementId> = set.into_iter().collect();
    sort_z(scene, &mut v);
    Ok(v)
}

/// Sorts ids by z-order, back to front. Unknown ids sort first.
pub fn sort_z<R: SceneRead>(scene: &R, ids: &mut [ElementId]) {
    ids.sort_by(|a, b| {
        let ka = scene.get(a).map(|e| &e.index);
        let kb = scene.get(b).map(|e| &e.index);
        ka.cmp(&kb).then_with(|| a.cmp(b))
    });
}

/// The units of `ids` (bound text counts as its container), topmost unit last. A group unit has all its live
/// members, also those not in `ids`.
pub fn units<R: SceneRead, L: ReverseLookup + ?Sized>(
    scene: &R,
    rel: &L,
    ids: &[ElementId],
    editing: Option<&GroupId>,
) -> Result<Vec<Unit>, TargetError> {
    let mut seen: BTreeSet<UnitKey> = BTreeSet::new();
    let mut out: Vec<Unit> = Vec::new();
    for id in primaries(scene, ids)? {
        let e = live(scene, &id)?;
        let key = unit_key(rel, e, editing);
        if !seen.insert(key.clone()) {
            continue;
        }
        let elements = match &key {
            UnitKey::Group(g) => rel.members_of(g),
            UnitKey::Element(x) => vec![x.clone()],
        };
        // A group may contain bound text (it carries its container's groups). Keep the primaries only.
        let elements = elements.into_iter().map(|m| primary_id(scene, &m)).collect::<BTreeSet<_>>();
        let mut elements: Vec<ElementId> = elements.into_iter().collect();
        sort_z(scene, &mut elements);
        out.push(Unit { key, elements });
    }
    out.sort_by(|a, b| {
        let top = |u: &Unit| u.elements.last().and_then(|i| scene.get(i)).map(|e| e.index.clone());
        top(a).cmp(&top(b))
    });
    Ok(out)
}

/// `ids` plus the bound text of each, and (with `frame_children`) the children of each frame with their bound
/// text. No duplicates, in z-order. Deleted elements are skipped.
pub fn with_companions<R: SceneRead, L: ReverseLookup + ?Sized>(
    scene: &R,
    rel: &L,
    ids: &[ElementId],
    frame_children: bool,
) -> Vec<ElementId> {
    let mut set: BTreeSet<ElementId> = BTreeSet::new();
    let add = |set: &mut BTreeSet<ElementId>, id: &ElementId| {
        if scene.get(id).is_some_and(|e| !e.deleted) {
            set.insert(id.clone());
            set.extend(rel.labels_of(id));
        }
    };
    for id in ids {
        add(&mut set, id);
        if frame_children && scene.get(id).is_some_and(|e| e.tag() == Kind::Frame) {
            for c in rel.children_of(id) {
                add(&mut set, &c);
            }
        }
    }
    let mut v: Vec<ElementId> = set.into_iter().collect();
    sort_z(scene, &mut v);
    v
}

/// Fails with the locked elements among `ids`.
pub fn ensure_unlocked<R: SceneRead>(scene: &R, ids: &[ElementId]) -> Result<(), TargetError> {
    let locked: Vec<ElementId> = ids.iter().filter(|i| scene.get(i).is_some_and(|e| e.locked)).cloned().collect();
    if locked.is_empty() { Ok(()) } else { Err(TargetError::Locked(locked)) }
}

/// True when the element is bound text of a live container.
pub fn is_label<R: SceneRead>(scene: &R, e: &Element) -> bool {
    e.container_id().is_some_and(|c| scene.get(c).is_some_and(|c| !c.deleted))
}
