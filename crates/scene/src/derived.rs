//! Indexes derived from the elements: never stored in the document, never synced, rebuilt or updated by the store.
//!
//! * bound arrows by target (reverse of `start_binding` / `end_binding`),
//! * members by group (reverse of `group_ids`),
//! * children by frame (reverse of `frame_id`),
//! * label text by container (reverse of `container_id`),
//! * the frames themselves (a small set, so "which frame is under this point" need not scan the scene),
//! * the boxes of the live elements in an R-tree ([`DerivedIndex::ids_in_rect`]: "what lies in this
//!   rectangle", which the elbow router asks for the obstacles between two arrow ends).
//!
//! Only live elements are indexed. The index remembers what it recorded for each element, so it updates from a
//! [`ChangeSet`] and the store's current state without needing the old element values.

use std::collections::{BTreeSet, HashMap};

use drafftink_geom::SpatialIndex;
use drafftink_geom::kurbo::Rect;
use smallvec::SmallVec;

use crate::element::{Element, Kind};
use crate::id::{ElementId, GroupId};
use crate::store::{ChangeSet, SceneRead};

/// What the index recorded for one element.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Recorded {
    targets: [Option<ElementId>; 2],
    groups: SmallVec<[GroupId; 2]>,
    frame: Option<ElementId>,
    container: Option<ElementId>,
    is_frame: bool,
}

impl Recorded {
    fn of(e: &Element) -> Option<Recorded> {
        if e.deleted {
            return None;
        }
        let [a, b] = e.binding_targets();
        let r = Recorded {
            targets: [a.cloned(), b.cloned()],
            groups: e.group_ids.clone(),
            frame: e.frame_id.clone(),
            container: e.container_id().cloned(),
            is_frame: e.tag() == Kind::Frame,
        };
        let empty = r.targets.iter().all(Option::is_none)
            && r.groups.is_empty()
            && r.frame.is_none()
            && r.container.is_none()
            && !r.is_frame;
        (!empty).then_some(r)
    }
}

/// The boxes of the live elements (the axis-aligned box of the rotated box) in an R-tree. Two of them are
/// equal when they hold the same boxes, whatever the shape of their trees.
#[derive(Debug, Clone, Default)]
struct Boxes(SpatialIndex<ElementId>);

impl PartialEq for Boxes {
    fn eq(&self, other: &Self) -> bool {
        self.0.len() == other.0.len() && self.0.iter().all(|(id, r)| other.0.get(id) == Some(r))
    }
}

impl Eq for Boxes {}

/// A change set that touches more elements than this, and more than half of the boxes, loads them in bulk.
const BULK_BOXES_MIN: usize = 2048;

/// The derived indexes of a scene.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DerivedIndex {
    recorded: HashMap<ElementId, Recorded>,
    bound_arrows: HashMap<ElementId, BTreeSet<ElementId>>,
    group_members: HashMap<GroupId, BTreeSet<ElementId>>,
    frame_children: HashMap<ElementId, BTreeSet<ElementId>>,
    labels: HashMap<ElementId, BTreeSet<ElementId>>,
    frames: BTreeSet<ElementId>,
    boxes: Boxes,
}

fn add<K: std::hash::Hash + Eq + Clone>(map: &mut HashMap<K, BTreeSet<ElementId>>, key: &K, id: &ElementId) {
    map.entry(key.clone()).or_default().insert(id.clone());
}

fn del<K: std::hash::Hash + Eq>(map: &mut HashMap<K, BTreeSet<ElementId>>, key: &K, id: &ElementId) {
    if let Some(set) = map.get_mut(key) {
        set.remove(id);
        if set.is_empty() {
            map.remove(key);
        }
    }
}

impl DerivedIndex {
    /// An empty index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuilds the index from scratch.
    pub fn rebuild<R: SceneRead>(&mut self, scene: &R) {
        *self = Self::default();
        for e in scene.iter_all() {
            self.record(e);
        }
        self.load_boxes(scene);
    }

    /// Replaces the boxes with a bulk-loaded index of the live elements of `scene`.
    fn load_boxes<R: SceneRead>(&mut self, scene: &R) {
        self.boxes = Boxes(SpatialIndex::bulk_load(scene.iter().map(|e| (e.id.clone(), e.transform.obox().aabb()))));
    }

    /// Brings the box of one element up to date. An element that is gone, or whose box is not finite, has none.
    fn track_box(&mut self, id: &ElementId, live: Option<&Element>) {
        let Some(e) = live else {
            self.boxes.0.remove(id);
            return;
        };
        let now = e.transform.obox().aabb();
        if self.boxes.0.get(id) != Some(now) && self.boxes.0.insert(id.clone(), now).is_err() {
            self.boxes.0.remove(id);
        }
    }

    /// Brings the index up to date after `changes`. `scene` must be in the state the change set produced.
    pub fn apply<R: SceneRead>(&mut self, changes: &ChangeSet, scene: &R) {
        let n = changes.elements.len();
        let bulk = n > BULK_BOXES_MIN && n * 2 > self.boxes.0.len();
        for c in &changes.elements {
            self.forget(&c.id);
            let e = scene.get(&c.id);
            if let Some(e) = e {
                self.record(e);
            }
            if !bulk {
                self.track_box(&c.id, e.filter(|e| !e.deleted));
            }
        }
        if bulk {
            self.load_boxes(scene);
        }
    }

    fn forget(&mut self, id: &ElementId) {
        let Some(old) = self.recorded.remove(id) else { return };
        for t in old.targets.iter().flatten() {
            del(&mut self.bound_arrows, t, id);
        }
        for g in &old.groups {
            del(&mut self.group_members, g, id);
        }
        if let Some(f) = &old.frame {
            del(&mut self.frame_children, f, id);
        }
        if let Some(c) = &old.container {
            del(&mut self.labels, c, id);
        }
        if old.is_frame {
            self.frames.remove(id);
        }
    }

    fn record(&mut self, e: &Element) {
        let Some(r) = Recorded::of(e) else { return };
        for t in r.targets.iter().flatten() {
            add(&mut self.bound_arrows, t, &e.id);
        }
        for g in &r.groups {
            add(&mut self.group_members, g, &e.id);
        }
        if let Some(f) = &r.frame {
            add(&mut self.frame_children, f, &e.id);
        }
        if let Some(c) = &r.container {
            add(&mut self.labels, c, &e.id);
        }
        if r.is_frame {
            self.frames.insert(e.id.clone());
        }
        self.recorded.insert(e.id.clone(), r);
    }

    /// Live lines and arrows whose start or end is bound to `target`, sorted by id.
    pub fn bound_arrows(&self, target: &ElementId) -> impl Iterator<Item = &ElementId> {
        self.bound_arrows.get(target).into_iter().flatten()
    }

    /// Live members of a group, sorted by id. Sort by z-order with the store when order matters.
    pub fn group_members(&self, group: &GroupId) -> impl Iterator<Item = &ElementId> {
        self.group_members.get(group).into_iter().flatten()
    }

    /// Number of live members of a group.
    pub fn group_len(&self, group: &GroupId) -> usize {
        self.group_members.get(group).map_or(0, BTreeSet::len)
    }

    /// A group with fewer than two live members counts as no group.
    pub fn is_group(&self, group: &GroupId) -> bool {
        self.group_len(group) >= 2
    }

    /// Live children of a frame, sorted by id.
    pub fn frame_children(&self, frame: &ElementId) -> impl Iterator<Item = &ElementId> {
        self.frame_children.get(frame).into_iter().flatten()
    }

    /// The live label text of a container. If several claim it (a merge conflict), the one with the smallest id.
    pub fn label_of(&self, container: &ElementId) -> Option<&ElementId> {
        self.labels.get(container).and_then(|s| s.iter().next())
    }

    /// All live texts that claim `container` as their container.
    pub fn labels_of(&self, container: &ElementId) -> impl Iterator<Item = &ElementId> {
        self.labels.get(container).into_iter().flatten()
    }

    /// The live frames, sorted by id. Sort by z-order with the store when order matters.
    pub fn frames(&self) -> impl Iterator<Item = &ElementId> {
        self.frames.iter()
    }

    /// Number of groups with at least one live member.
    pub fn group_count(&self) -> usize {
        self.group_members.len()
    }

    /// Live elements whose box (rotation included) overlaps or touches `rect`, in no particular order. Read the
    /// elements from the scene.
    pub fn ids_in_rect(&self, rect: &Rect) -> impl Iterator<Item = &ElementId> {
        self.boxes.0.query_rect(rect).map(|(id, _)| id)
    }
}
