//! Frames: clipping containers whose children sit contiguously just below them in the z-order.
//!
//! * A child has `frame_id` set; frames do not nest. Children are drawn clipped to the frame when
//!   [`FrameClip`] says so ([`clips`]), and export with it.
//! * **Z-order.** The children of a frame form a block directly below the frame (the model validator warns about a
//!   child above its frame). Adding an element puts it at the top of that block, in its old relative order.
//! * **Moving a frame moves its children** and deleting it deletes them (or releases them): see
//!   [`crate::transform`] and [`crate::lifecycle`].
//! * **Groups and bound text.** An element in a group joins or leaves a frame with its whole outermost group, and
//!   bound text follows its container. A group that contains a frame cannot join a frame.
//! * **Membership by drag.** [`update_membership_after_move`] applies the pointer rule on the committed run of a
//!   move; [`recompute_after_resize`] does the same after a frame was resized.

use std::collections::BTreeSet;

use drafftink_geom::aabb;
use drafftink_geom::kurbo::{Point, Rect};
use drafftink_scene::{
    Element, ElementId, ElementKind, FrameData, IdSource, IndexError, Kind, SceneRead, SceneTx, StoreError, Style,
    ZIndex, limits,
};

use drafftink_profile::{Counter, count};

use crate::binding::BindingError;
use crate::group::units::{
    Relations, Reverse, Unit, UnitKey, ensure_unlocked, impl_from_target, live, primaries, units, with_companions,
};
use crate::lifecycle::{self, ChildPolicy, LifecycleError};
use crate::transform::geometry;
use crate::zorder::{self, Place, ZOrderError, ZReport, move_block, settle_leavers};

/// Why a frame operation was refused.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// No live element has this id.
    #[error("element {0} does not exist or is deleted")]
    NotFound(ElementId),
    /// Locked elements cannot change frames.
    #[error("{} element(s) are locked ({}). Unlock them first", .0.len(), crate::group::units::ids_text(.0))]
    Locked(Vec<ElementId>),
    /// The target is not a frame.
    #[error("{id} is {}, and this works on frames", drafftink_scene::words::Article::new(.kind))]
    NotAFrame {
        /// The element.
        id: ElementId,
        /// Its kind.
        kind: &'static str,
    },
    /// Frames cannot be nested.
    #[error("frames cannot be nested ({0} is a frame). Remove the frame from the selection, or unwrap it first")]
    NestedFrame(ElementId),
    /// A number is unusable.
    #[error("{0}")]
    BadArgs(&'static str),
    /// The store refused a write.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Z-order could not be updated.
    #[error(transparent)]
    ZOrder(#[from] ZOrderError),
    /// A key could not be made.
    #[error(transparent)]
    Index(#[from] IndexError),
    /// Bound arrows could not be updated.
    #[error(transparent)]
    Binding(#[from] BindingError),
    /// Deleting the frame failed.
    #[error(transparent)]
    Lifecycle(#[from] LifecycleError),
}
impl_from_target!(FrameError);

/// A frame that clips: what the renderer needs.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameClip {
    /// The frame.
    pub frame: ElementId,
    /// The clip rectangle (frames are never rotated).
    pub rect: Rect,
    /// Live children, back to front.
    pub children: Vec<ElementId>,
}

/// Every live frame with `clip` set, back to front, with its live children.
pub fn clips<R: SceneRead>(scene: &R) -> Vec<FrameClip> {
    let rel = Relations::build(scene);
    scene
        .iter()
        .filter(|e| matches!(&e.kind, ElementKind::Frame(d) if d.clip))
        .map(|f| FrameClip { frame: f.id.clone(), rect: f.rect(), children: rel.children(&f.id).to_vec() })
        .collect()
}

fn frame_of<'a, R: SceneRead>(scene: &'a R, id: &ElementId) -> Result<&'a Element, FrameError> {
    let e = live(scene, id)?;
    if e.tag() == Kind::Frame { Ok(e) } else { Err(FrameError::NotAFrame { id: id.clone(), kind: e.tag().name() }) }
}

/// The live children of a frame, back to front.
pub fn children_of<R: SceneRead>(scene: &R, frame: &ElementId) -> Result<Vec<ElementId>, FrameError> {
    frame_of(scene, frame)?;
    Ok(Relations::build(scene).children(frame).to_vec())
}

/// The topmost live, unlocked frame under `point`, ignoring `exclude`.
///
/// With a derived index (a store, or a transaction that has not written yet) only the frames are looked at;
/// otherwise the call scans the scene.
pub fn frame_at<R: SceneRead>(scene: &R, point: Point, exclude: &BTreeSet<ElementId>) -> Option<ElementId> {
    let under = |e: &Element| {
        e.tag() == Kind::Frame && !e.locked && !exclude.contains(&e.id) && aabb::contains_point(&e.rect(), point)
    };
    if let Some(index) = scene.derived_index() {
        return index
            .frames()
            .filter_map(|id| scene.get(id))
            .filter(|e| under(e))
            .max_by(|a, b| a.index.cmp(&b.index).then_with(|| a.id.cmp(&b.id)))
            .map(|e| e.id.clone());
    }
    count(Counter::ElementsScanned, u64::try_from(scene.len_all()).unwrap_or(u64::MAX));
    scene.iter().filter(|e| under(e)).last().map(|e| e.id.clone())
}

/// Why an element was not added to a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// Frames cannot join frames.
    IsFrame,
    /// A group that contains a frame cannot join a frame.
    GroupContainsFrame,
    /// An arrow joins only when an end is bound to, or lies inside, a member.
    ArrowNotAttached,
}

/// An element that was not added, with the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// The element as given.
    pub id: ElementId,
    /// Why.
    pub reason: SkipReason,
}

/// Outcome of adding to a frame.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AddReport {
    /// Elements that joined (bound text not listed), in z-order.
    pub added: Vec<ElementId>,
    /// Elements that were left out.
    pub skipped: Vec<Skipped>,
    /// The z-order change.
    pub z: ZReport,
}

fn attach<T: SceneTx>(tx: &mut T, frame: &ElementId, all: &[ElementId]) -> Result<ZReport, FrameError> {
    for id in all {
        if tx.get(id).is_some_and(|e| e.frame_id.as_ref() != Some(frame)) {
            let f = frame.clone();
            tx.update(id, |e| e.frame_id = Some(f))?;
        }
    }
    Ok(move_block(tx, all, &Place::Below(frame.clone()))?)
}

/// Clears the frame of `all`. The block of children stays contiguous: the elements that left move just below the
/// lowest remaining child (just above the group, when the frame is in a group).
fn detach<T: SceneTx>(tx: &mut T, all: &[ElementId]) -> Result<(), FrameError> {
    let leaving: Vec<(ElementId, ElementId)> =
        all.iter().filter_map(|id| tx.get(id).and_then(|e| e.frame_id.clone()).map(|f| (id.clone(), f))).collect();
    for id in all {
        if tx.get(id).is_some_and(|e| e.frame_id.is_some()) {
            tx.update(id, |e| e.frame_id = None)?;
        }
    }
    Ok(settle_leavers(tx, &leaving)?)
}

fn unit_ids(u: &Unit) -> impl Iterator<Item = &ElementId> {
    u.elements.iter()
}

/// Makes the units of `ids` children of `frame`. Frames and groups containing a frame are skipped and listed; an
/// arrow joins only if an end is bound to or lies inside a member; bound text joins with its container. The new
/// children are moved to just below the frame.
pub fn add_to_frame<T: SceneTx>(tx: &mut T, frame: &ElementId, ids: &[ElementId]) -> Result<AddReport, FrameError> {
    let f = frame_of(&*tx, frame)?;
    if f.locked {
        return Err(FrameError::Locked(vec![frame.clone()]));
    }
    let rel = Relations::build(&*tx);
    let prim = primaries(&*tx, ids)?;
    let us = units(&*tx, &rel, &prim, None)?;
    let mut skipped = Vec::new();
    let mut plain: Vec<&Unit> = Vec::new();
    let mut arrows: Vec<&Unit> = Vec::new();
    for u in &us {
        let given = |u: &Unit| -> Vec<ElementId> { prim.iter().filter(|p| u.elements.contains(p)).cloned().collect() };
        let has_frame = u.elements.iter().any(|i| tx.get(i).is_some_and(|e| e.tag() == Kind::Frame));
        if has_frame || unit_ids(u).any(|i| i == frame) {
            let reason =
                if matches!(u.key, UnitKey::Element(_)) { SkipReason::IsFrame } else { SkipReason::GroupContainsFrame };
            skipped.extend(given(u).into_iter().map(|id| Skipped { id, reason }));
        } else if matches!(&u.key, UnitKey::Element(x) if tx.get(x).is_some_and(|e| e.tag().is_linear())) {
            arrows.push(u);
        } else {
            plain.push(u);
        }
    }
    let mut members: BTreeSet<ElementId> = plain.iter().flat_map(|u| u.elements.iter().cloned()).collect();
    let mut anchors: BTreeSet<ElementId> = members.clone();
    anchors.extend(rel.children(frame).iter().cloned());
    let attached = |tx: &T, arrow: &ElementId| -> bool {
        let Some(a) = tx.get(arrow) else { return false };
        let ends = geometry::world_points(a).map(|p| [p.first().copied(), p.last().copied()]).unwrap_or([None, None]);
        let bound = a.binding_targets();
        (0..2).any(|i| {
            bound.get(i).copied().flatten().is_some_and(|t| anchors.contains(t))
                || ends.get(i).copied().flatten().is_some_and(|p| {
                    anchors
                        .iter()
                        .filter_map(|m| tx.get(m))
                        .any(|m| aabb::contains_point(&geometry::world_bounds(m), p))
                })
        })
    };
    for u in arrows {
        match u.elements.first() {
            Some(a) if attached(&*tx, a) => {
                members.insert(a.clone());
            }
            _ => skipped.extend(
                prim.iter()
                    .filter(|p| u.elements.contains(p))
                    .cloned()
                    .map(|id| Skipped { id, reason: SkipReason::ArrowNotAttached }),
            ),
        }
    }
    let mut added: Vec<ElementId> = members.into_iter().collect();
    crate::group::units::sort_z(&*tx, &mut added);
    let all = with_companions(&*tx, &rel, &added, false);
    ensure_unlocked(&*tx, &all)?;
    let z = attach(tx, frame, &all)?;
    Ok(AddReport { added, skipped, z })
}

/// Takes the units of `ids` out of their frames and returns them. Their place in the z-order stays below the frame:
/// they move just below the lowest remaining child, so the children of the frame stay one contiguous block (the
/// command catalog says "z-order unchanged"; this is the only change, and only among the frame's own block).
pub fn remove_from_frame<T: SceneTx>(tx: &mut T, ids: &[ElementId]) -> Result<Vec<ElementId>, FrameError> {
    let rel = Relations::build(&*tx);
    let prim = primaries(&*tx, ids)?;
    let us = units(&*tx, &rel, &prim, None)?;
    let members: Vec<ElementId> = us
        .iter()
        .flat_map(|u| u.elements.iter().cloned())
        .filter(|i| tx.get(i).is_some_and(|e| e.frame_id.is_some()))
        .collect();
    let all = with_companions(&*tx, &rel, &members, false);
    ensure_unlocked(&*tx, &all)?;
    detach(tx, &all)?;
    Ok(members)
}

/// Takes all children out of `frame`. Returns them.
pub fn remove_all_children<T: SceneTx>(tx: &mut T, frame: &ElementId) -> Result<Vec<ElementId>, FrameError> {
    frame_of(&*tx, frame)?;
    let rel = Relations::build(&*tx);
    let all = rel.children(frame).to_vec();
    ensure_unlocked(&*tx, &all)?;
    detach(tx, &all)?;
    let mut members: Vec<ElementId> =
        all.into_iter().filter(|i| tx.get(i).is_some_and(|e| !crate::group::units::is_label(&*tx, e))).collect();
    crate::group::units::sort_z(&*tx, &mut members);
    Ok(members)
}

fn inside_units<R: SceneRead>(scene: &R, rel: &Relations, rect: &Rect, candidates: &[ElementId]) -> Vec<ElementId> {
    let Ok(us) = units(scene, rel, candidates, None) else { return Vec::new() };
    let mut out = Vec::new();
    for u in us {
        let ok = u.elements.iter().all(|i| {
            scene.get(i).is_some_and(|e| {
                !e.locked && e.tag() != Kind::Frame && aabb::contains_rect(rect, &geometry::world_bounds(e))
            })
        });
        if ok {
            out.extend(u.elements);
        }
    }
    out
}

fn free_candidates<R: SceneRead>(scene: &R, frame: &ElementId) -> Vec<ElementId> {
    scene
        .iter()
        .filter(|e| {
            e.id != *frame
                && e.frame_id.is_none()
                && !e.locked
                && e.tag() != Kind::Frame
                && !crate::group::units::is_label(scene, e)
        })
        .map(|e| e.id.clone())
        .collect()
}

/// Adds every unlocked element that lies completely inside the frame (a group only if all its members do; no
/// frames), for a frame that was just drawn around them.
pub fn capture_contained<T: SceneTx>(tx: &mut T, frame: &ElementId) -> Result<AddReport, FrameError> {
    let f = frame_of(&*tx, frame)?;
    if f.locked {
        return Err(FrameError::Locked(vec![frame.clone()]));
    }
    let rect = f.rect();
    let rel = Relations::build(&*tx);
    let candidates = free_candidates(&*tx, frame);
    let mut added = inside_units(&*tx, &rel, &rect, &candidates);
    crate::group::units::sort_z(&*tx, &mut added);
    if added.is_empty() {
        return Ok(AddReport::default());
    }
    let all = with_companions(&*tx, &rel, &added, false);
    let z = attach(tx, frame, &all)?;
    Ok(AddReport { added, skipped: Vec::new(), z })
}

/// Options of [`wrap`].
#[derive(Debug, Clone, PartialEq)]
pub struct WrapOpts {
    /// Label of the new frame.
    pub name: Option<String>,
    /// Space between the elements and the frame edge. Forced to 0 when an image is included.
    pub padding: f64,
}

impl Default for WrapOpts {
    fn default() -> Self {
        Self { name: None, padding: 16.0 }
    }
}

/// Outcome of [`wrap`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrapReport {
    /// The new frame.
    pub frame: ElementId,
    /// What was added to it.
    pub add: AddReport,
}

/// Creates a frame around the common bounds of the units of `ids` plus padding, and adds them with the rules of
/// [`add_to_frame`]. The frame sits directly above them.
pub fn wrap<T: SceneTx, I: IdSource>(
    tx: &mut T,
    ids: &[ElementId],
    id_source: &mut I,
    opts: &WrapOpts,
) -> Result<WrapReport, FrameError> {
    if !opts.padding.is_finite() || opts.padding < 0.0 {
        return Err(FrameError::BadArgs("padding must be a number of 0 or more"));
    }
    let rel = Relations::build(&*tx);
    let prim = primaries(&*tx, ids)?;
    let us = units(&*tx, &rel, &prim, None)?;
    let members: Vec<ElementId> = us.iter().flat_map(|u| u.elements.iter().cloned()).collect();
    if let Some(fr) = members.iter().find(|i| tx.get(i).is_some_and(|e| e.tag() == Kind::Frame)) {
        return Err(FrameError::NestedFrame(fr.clone()));
    }
    let all = with_companions(&*tx, &rel, &members, false);
    ensure_unlocked(&*tx, &all)?;
    let elems: Vec<&Element> = members.iter().filter_map(|i| tx.get(i)).collect();
    let bounds = geometry::union_bounds(elems.iter().copied()).ok_or(FrameError::BadArgs("nothing to wrap"))?;
    let pad = if elems.iter().any(|e| e.tag() == Kind::Image) { 0.0 } else { opts.padding };
    let top = members
        .iter()
        .filter_map(|i| tx.get(i))
        .max_by(|a, b| a.index.cmp(&b.index).then_with(|| a.id.cmp(&b.id)))
        .cloned()
        .ok_or(FrameError::BadArgs("nothing to wrap"))?;
    // Directly above the members, but not inside the block of children of the frame they leave, nor inside a group.
    let top = zorder::lift_above_enclosures(&*tx, &rel, &top, &|_| false, &|_| false);
    let (_, above) = tx.z_neighbors(&top.id);
    let top_key = Some(top.index.clone());
    let index = ZIndex::between(top_key.as_ref(), above.as_ref())?;
    let mut id = id_source.element_id();
    for _ in 0..16 {
        if !tx.contains(&id) {
            break;
        }
        id = id_source.element_id();
    }
    let mut frame =
        Element::new(id.clone(), ElementKind::Frame(FrameData { name: opts.name.clone(), clip: true }), index)
            .with_bounds(bounds.x0 - pad, bounds.y0 - pad, bounds.width() + 2.0 * pad, bounds.height() + 2.0 * pad);
    frame.style = Style { seed: id_source.seed(), ..Style::default() };
    tx.insert(frame)?;
    let add = add_to_frame(tx, &id, ids)?;
    Ok(WrapReport { frame: id, add })
}

/// Releases the children of a frame and deletes the frame. Returns the children.
pub fn unwrap<T: SceneTx>(tx: &mut T, frame: &ElementId) -> Result<Vec<ElementId>, FrameError> {
    let f = frame_of(&*tx, frame)?;
    if f.locked {
        return Err(FrameError::Locked(vec![frame.clone()]));
    }
    let children = remove_all_children(tx, frame)?;
    lifecycle::delete(tx, std::slice::from_ref(frame), ChildPolicy::Release)?;
    Ok(children)
}

/// Sets the label of a frame. `None` restores the default "Frame".
pub fn rename<T: SceneTx>(tx: &mut T, frame: &ElementId, name: Option<&str>) -> Result<(), FrameError> {
    let f = frame_of(&*tx, frame)?;
    if f.locked {
        return Err(FrameError::Locked(vec![frame.clone()]));
    }
    if let Some(n) = name
        && (n.is_empty() || n.chars().count() > limits::MAX_NAME_CHARS || n.chars().any(char::is_control))
    {
        return Err(FrameError::BadArgs("a frame name must be 1 to 256 characters without control characters"));
    }
    let name = name.map(str::to_owned);
    tx.update(frame, |e| {
        if let ElementKind::Frame(d) = &mut e.kind {
            d.name = name;
        }
    })?;
    Ok(())
}

/// Frame changes caused by a drag or a resize.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MembershipReport {
    /// Elements that joined a frame, with the frame.
    pub joined: Vec<(ElementId, ElementId)>,
    /// Elements that left their frame.
    pub left: Vec<ElementId>,
}

/// Applies the drop rule after elements were moved: a unit dropped over a frame joins it, a unit dragged out of
/// its frame leaves it. The drop point is `pointer`, or the centre of the unit. Frames among `moved` never join a
/// frame, and children that moved together with their frame keep it. Run it on the committed move only.
pub fn update_membership_after_move<T: SceneTx>(
    tx: &mut T,
    moved: &[ElementId],
    pointer: Option<Point>,
) -> Result<MembershipReport, FrameError> {
    // Read first, write after: the lookups borrow the transaction, and a transaction that has not written yet
    // answers them from the committed index instead of a pass over the scene.
    let (leaves, joins, report) = {
        let scene = &*tx;
        let rel = Reverse::of(scene);
        let prim = primaries(scene, moved)?;
        let moved_frames: BTreeSet<ElementId> =
            prim.iter().filter(|i| scene.get(i).is_some_and(|e| e.tag() == Kind::Frame)).cloned().collect();
        let candidates: Vec<ElementId> = prim
            .iter()
            .filter(|i| {
                scene.get(i).is_some_and(|e| {
                    e.tag() != Kind::Frame && !e.locked && e.frame_id.as_ref().is_none_or(|f| !moved_frames.contains(f))
                })
            })
            .cloned()
            .collect();
        let us = units(scene, &rel, &candidates, None)?;
        let mut report = MembershipReport::default();
        let mut leaves: Vec<Vec<ElementId>> = Vec::new();
        let mut joins: Vec<(ElementId, Vec<ElementId>)> = Vec::new();
        for u in &us {
            let Some(rect) = geometry::union_bounds(u.elements.iter().filter_map(|i| scene.get(i))) else { continue };
            let at = pointer.unwrap_or_else(|| aabb::center(&rect));
            let target = frame_at(scene, at, &moved_frames);
            let current: BTreeSet<Option<ElementId>> =
                u.elements.iter().filter_map(|i| scene.get(i)).map(|e| e.frame_id.clone()).collect();
            if current.len() == 1 && current.contains(&target) {
                continue;
            }
            let all = with_companions(scene, &rel, &u.elements, false);
            if ensure_unlocked(scene, &all).is_err() {
                continue;
            }
            match target {
                Some(f) => {
                    for id in &u.elements {
                        report.joined.push((id.clone(), f.clone()));
                    }
                    match joins.iter_mut().find(|(g, _)| *g == f) {
                        Some((_, v)) => v.extend(all),
                        None => joins.push((f, all)),
                    }
                }
                None => {
                    leaves.push(all);
                    report.left.extend(u.elements.iter().cloned());
                }
            }
        }
        (leaves, joins, report)
    };
    for all in &leaves {
        detach(tx, all)?;
    }
    for (f, all) in joins {
        attach(tx, &f, &all)?;
    }
    Ok(report)
}

/// After a frame was resized: children that no longer touch the frame leave it, and free elements that now lie
/// completely inside join it. Run it on the committed resize only.
pub fn recompute_after_resize<T: SceneTx>(tx: &mut T, frame: &ElementId) -> Result<MembershipReport, FrameError> {
    let f = frame_of(&*tx, frame)?;
    let rect = f.rect();
    let rel = Relations::build(&*tx);
    let mut report = MembershipReport::default();
    let gone: Vec<ElementId> = rel
        .children(frame)
        .iter()
        .filter(|i| {
            tx.get(i).is_some_and(|e| {
                !e.locked
                    && !crate::group::units::is_label(&*tx, e)
                    && !aabb::intersects(&geometry::world_bounds(e), &rect)
            })
        })
        .cloned()
        .collect();
    let gone_all = with_companions(&*tx, &rel, &gone, false);
    detach(tx, &gone_all)?;
    report.left = gone;
    let rel = Relations::build(&*tx);
    let candidates = free_candidates(&*tx, frame);
    let mut joined = inside_units(&*tx, &rel, &rect, &candidates);
    crate::group::units::sort_z(&*tx, &mut joined);
    if !joined.is_empty() {
        let all = with_companions(&*tx, &rel, &joined, false);
        attach(tx, frame, &all)?;
        report.joined = joined.into_iter().map(|j| (j, frame.clone())).collect();
    }
    Ok(report)
}
