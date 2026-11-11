//! Copying elements as one block: the one rule behind [`crate::lifecycle::duplicate`] and
//! [`crate::paste::insert_elements`].
//!
//! * Every copy gets a new id and a new seed. Version, deleted flag and name are reset (a name addresses one
//!   element).
//! * A group that two or more copies share (and that was a real group where the originals are) gets a new id;
//!   groups in [`Params::keep_groups`] stay; every other group is dropped. Group names (kept in `custom`) follow
//!   their group.
//! * Bound text keeps its container when the container was copied. Otherwise it becomes free text. Frame
//!   membership is kept only when the frame was copied too.
//! * The copies are checked before anything is written, go in as one block of z-order keys (rebalancing when the
//!   keys run out), and bindings between copies point at copies.
//!
//! The callers decide which elements are copied, where the block goes and which groups stay.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use drafftink_geom::kurbo::Vec2;
use drafftink_scene::{
    Element, ElementError, ElementId, ElementKind, GroupId, IdSource, IndexError, SceneRead, SceneTx, StoreError,
    ZIndex, validate_element,
};

use crate::binding::{self, BindingError};
use crate::group::name_key;
use crate::ids::{fresh_element_id, fresh_group_id};
use crate::zorder::{self, REBALANCE_LEN, ZOrderError};

/// The start of every `custom` key that holds a group name (see [`name_key`]).
const GROUP_NAME_PREFIX: &str = "drafftink.group.";

/// Why a block of copies could not be made or written.
#[derive(Debug, thiserror::Error)]
pub enum CopyError {
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
    /// Z-order keys could not be made.
    #[error(transparent)]
    ZOrder(#[from] ZOrderError),
}

impl From<IndexError> for CopyError {
    fn from(e: IndexError) -> Self {
        Self::ZOrder(e.into())
    }
}

/// What differs between callers.
pub(crate) struct Params<'a> {
    /// Added to the position of every copy.
    pub offset: Vec2,
    /// Groups the copies stay in (they keep their id).
    pub keep_groups: &'a BTreeSet<GroupId>,
    /// Whether an original group still counts as a group where the originals are. A group the copies share is
    /// only regenerated when it does.
    pub is_group: &'a dyn Fn(&GroupId) -> bool,
}

/// Where a block of copies goes in the z-order.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Placement<'a> {
    /// On top of the scene.
    OnTop,
    /// Directly above this element.
    Above(&'a ElementId),
}

/// The copies of a set of elements, not written yet.
#[derive(Debug)]
pub(crate) struct Copies {
    /// The copies, in the order of the sources.
    pub elements: Vec<Element>,
    /// Original id to copy id.
    pub ids: HashMap<ElementId, ElementId>,
    /// Original group to its copy's group: `None` when the group is dropped, the same id when it is kept.
    pub groups: BTreeMap<GroupId, Option<GroupId>>,
}

impl Copies {
    /// Original and copy of every source, in the order of `sources`.
    pub(crate) fn pairs(&self, sources: &[&Element]) -> Vec<(ElementId, ElementId)> {
        sources.iter().filter_map(|o| self.ids.get(&o.id).map(|n| (o.id.clone(), n.clone()))).collect()
    }

    /// Old and new id of every regenerated group.
    pub(crate) fn regenerated_groups(&self) -> Vec<(GroupId, GroupId)> {
        self.groups.iter().filter_map(|(o, n)| n.as_ref().filter(|n| *n != o).map(|n| (o.clone(), n.clone()))).collect()
    }

    /// The ids of the copies.
    pub(crate) fn new_ids(&self) -> Vec<ElementId> {
        self.elements.iter().map(|c| c.id.clone()).collect()
    }
}

/// Builds and checks the copies of `sources` (in z-order) without writing anything.
pub(crate) fn build<R: SceneRead>(
    scene: &R,
    sources: &[&Element],
    params: &Params<'_>,
    ids: &mut dyn IdSource,
) -> Result<Copies, CopyError> {
    let id_map = new_ids(scene, sources, ids)?;
    let groups = new_groups(scene, sources, params, ids)?;
    let mut elements = Vec::with_capacity(sources.len());
    for o in sources {
        let Some(new_id) = id_map.get(&o.id) else { continue };
        let mut c = (*o).clone();
        c.id = new_id.clone();
        c.version = 0;
        c.deleted = false;
        c.name = None;
        c.style.seed = ids.seed();
        c.transform.x += params.offset.x;
        c.transform.y += params.offset.y;
        c.group_ids = o.group_ids.iter().filter_map(|g| groups.get(g).cloned().flatten()).collect();
        c.frame_id = o.frame_id.as_ref().and_then(|f| id_map.get(f).cloned());
        if let ElementKind::Text(t) = &mut c.kind {
            match t.container_id.as_ref().and_then(|x| id_map.get(x)) {
                Some(n) => t.container_id = Some(n.clone()),
                None => {
                    t.container_id = None;
                    t.label_position = None;
                }
            }
        }
        carry_group_names(&mut c, o, &groups);
        validate_element(&c).map_err(|error| CopyError::Invalid { id: o.id.clone(), error })?;
        elements.push(c);
    }
    Ok(Copies { elements, ids: id_map, groups })
}

fn new_ids<R: SceneRead>(
    scene: &R,
    sources: &[&Element],
    ids: &mut dyn IdSource,
) -> Result<HashMap<ElementId, ElementId>, CopyError> {
    let mut map: HashMap<ElementId, ElementId> = HashMap::with_capacity(sources.len());
    let mut used: BTreeSet<ElementId> = BTreeSet::new();
    for o in sources {
        let n =
            fresh_element_id(ids, |i| scene.contains(i) || used.contains(i)).map_err(|_| CopyError::IdsExhausted)?;
        used.insert(n.clone());
        map.insert(o.id.clone(), n);
    }
    Ok(map)
}

fn new_groups<R: SceneRead>(
    scene: &R,
    sources: &[&Element],
    params: &Params<'_>,
    ids: &mut dyn IdSource,
) -> Result<BTreeMap<GroupId, Option<GroupId>>, CopyError> {
    let mut counts: BTreeMap<&GroupId, usize> = BTreeMap::new();
    for g in sources.iter().flat_map(|o| o.group_ids.iter()) {
        *counts.entry(g).or_default() += 1;
    }
    let existing: BTreeSet<&GroupId> = scene.iter_all().flat_map(|e| e.group_ids.iter()).collect();
    let mut fresh: BTreeSet<GroupId> = BTreeSet::new();
    let mut map = BTreeMap::new();
    for (g, n) in counts {
        let new = if params.keep_groups.contains(g) {
            Some(g.clone())
        } else if n >= 2 && (params.is_group)(g) {
            let id = fresh_group_id(ids, |c| existing.contains(c) || fresh.contains(c))
                .map_err(|_| CopyError::IdsExhausted)?;
            fresh.insert(id.clone());
            Some(id)
        } else {
            None
        };
        map.insert(g.clone(), new);
    }
    Ok(map)
}

/// The names of the groups a copy stays in follow the group; names of other groups are dropped.
fn carry_group_names(copy: &mut Element, original: &Element, groups: &BTreeMap<GroupId, Option<GroupId>>) {
    let Some(custom) = &mut copy.custom else { return };
    let mut kept = BTreeMap::new();
    for (old, new) in original.group_ids.iter().filter_map(|g| groups.get(g).cloned().flatten().map(|n| (g, n))) {
        if let Some(v) = custom.remove(&name_key(old)) {
            kept.insert(name_key(&new), v);
        }
    }
    custom.retain(|k, _| !k.starts_with(GROUP_NAME_PREFIX));
    custom.extend(kept);
    if custom.is_empty() {
        copy.custom = None;
    }
}

/// What [`insert`] did.
#[derive(Debug)]
pub(crate) struct Inserted {
    /// Arrows the binding module re-solved.
    pub arrows: Vec<ElementId>,
}

/// Writes the copies as one block at `at`, then points the bindings between copies at copies and re-solves
/// the arrows bound to them.
pub(crate) fn insert<T: SceneTx>(tx: &mut T, copies: &mut Copies, at: Placement<'_>) -> Result<Inserted, CopyError> {
    assign_keys(tx, &mut copies.elements, at)?;
    let long = copies.elements.iter().any(|c| c.index.as_str().len() > REBALANCE_LEN);
    let new_ids = copies.new_ids();
    for c in std::mem::take(&mut copies.elements) {
        tx.insert(c)?;
    }
    if long {
        zorder::rebalance(tx)?;
    }
    // A binding to an element that was not copied is dropped.
    binding::remap_bindings(tx, &new_ids, &copies.ids)?;
    let arrows = binding::update_bound_arrows(tx, &new_ids)?;
    Ok(Inserted { arrows })
}

/// The keys between which the block goes.
fn bounds<R: SceneRead>(scene: &R, at: Placement<'_>) -> (Option<ZIndex>, Option<ZIndex>) {
    match at {
        Placement::OnTop => (scene.top_z(), None),
        Placement::Above(id) => (scene.get(id).map(|e| e.index.clone()), scene.z_neighbors(id).1),
    }
}

fn assign_keys<T: SceneTx>(tx: &mut T, elements: &mut [Element], at: Placement<'_>) -> Result<(), CopyError> {
    let (lo, hi) = bounds(&*tx, at);
    let keys = match ZIndex::n_between(lo.as_ref(), hi.as_ref(), elements.len()) {
        Ok(k) => k,
        Err(_) => {
            zorder::rebalance(tx)?;
            let (lo, hi) = bounds(&*tx, at);
            ZIndex::n_between(lo.as_ref(), hi.as_ref(), elements.len())?
        }
    };
    for (c, k) in elements.iter_mut().zip(keys) {
        c.index = k;
    }
    Ok(())
}
