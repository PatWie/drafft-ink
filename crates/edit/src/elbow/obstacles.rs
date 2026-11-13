//! The shapes an elbow arrow goes around: which elements count, and how they are found.
//!
//! The router ([`super::route`]) is pure and takes boxes. This module reads them from a scene. The candidates
//! come from the scene's box index ([`DerivedIndex::ids_in_rect`](drafftink_scene::DerivedIndex::ids_in_rect))
//! when the reader has one, so the cost follows the number of shapes near the arrow and not the size of the
//! document. A reader without the index (a transaction that has already written) is scanned once, by the
//! [`ObstacleFinder`] that serves all the arrows of one call.

use std::cell::OnceCell;

use drafftink_geom::SpatialIndex;
use drafftink_geom::kurbo::Rect;
use drafftink_profile::{Counter, count, inc};
use drafftink_scene::{Element, ElementId, ElementKind, SceneRead};

use super::route::{AVOID_PADDING, BASE_PADDING, Bounds, ElbowEnd, bounds_of};

/// How far beyond the two ends the search for obstacles looks, at least, in world units. A detour that goes
/// further out than this around a shape is not planned around the shapes there.
const MIN_REACH: f64 = 4.0 * BASE_PADDING;

/// Does a shape of this element's kind stand in the way of an elbow arrow? Shapes, pictures and free text do.
/// Lines, arrows, drawings and frames do not (an arrow crossing a frame is the common case), and neither does
/// a label, which lies inside the shape it belongs to.
pub(crate) fn is_obstacle(e: &Element) -> bool {
    match &e.kind {
        ElementKind::Rectangle | ElementKind::Diamond | ElementKind::Ellipse | ElementKind::Math(_) => true,
        ElementKind::Image(_) => true,
        ElementKind::Text(t) => t.container_id.is_none(),
        _ => false,
    }
}

/// The box of an element for the router: the axis-aligned box of the rotated box.
pub(crate) fn box_of(e: &Element) -> Rect {
    e.transform.obox().aabb()
}

/// The region an arrow from `s` to `e` may need obstacles from: the box of both ends and their shapes, grown by
/// half its longer side (and by at least [`MIN_REACH`]).
fn region(s: &ElbowEnd, e: &ElbowEnd) -> Rect {
    let mut r = Rect::from_points(s.point, e.point);
    for end in [s, e] {
        if let Some(b) = &end.bound {
            r = r.union(b.target.frame.aabb());
        }
    }
    let reach = (r.width().max(r.height()) / 2.0).max(MIN_REACH);
    r.inflate(reach, reach)
}

/// Finds the shapes near a place for any number of arrows of one scene state. With the scene's box index it
/// asks the index. A reader that has none (it has written since it was opened) is scanned the first time the
/// finder is asked, into a small index of the obstacles of its own, which then answers every later question.
/// The finder is meant for one stretch in which the shapes do not change; an arrow written meanwhile does not
/// matter, arrows are no obstacles.
#[derive(Default)]
pub struct ObstacleFinder {
    scanned: OnceCell<Option<SpatialIndex<ElementId>>>,
}

impl ObstacleFinder {
    /// A finder that has not looked at a scene yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ids of the elements whose box touches `rect`: obstacles at least, and from the box index, other kinds too.
    fn candidates<R: SceneRead>(&self, scene: &R, rect: &Rect) -> Vec<ElementId> {
        inc(Counter::SpatialQueries);
        if let Some(index) = scene.derived_index() {
            return index.ids_in_rect(rect).cloned().collect();
        }
        let scanned = self.scanned.get_or_init(|| {
            count(Counter::ElementsScanned, u64::try_from(scene.len_all()).unwrap_or(u64::MAX));
            Some(SpatialIndex::bulk_load(scene.iter().filter(|e| is_obstacle(e)).map(|e| (e.id.clone(), box_of(e)))))
        });
        scanned.iter().flat_map(|ix| ix.query_rect(rect)).map(|(id, _)| id.clone()).collect()
    }

    /// The boxes an arrow `arrow` from `s` to `e` is routed around. Empty for an arrow with no bound end: a line the
    /// user drew between two free points goes where it was put.
    pub(crate) fn gather<R: SceneRead>(&self, scene: &R, arrow: &ElementId, s: &ElbowEnd, e: &ElbowEnd) -> Vec<Bounds> {
        if s.bound.is_none() && e.bound.is_none() {
            return Vec::new();
        }
        let ends = [s.bound.as_ref().map(|b| &b.id), e.bound.as_ref().map(|b| &b.id)];
        let mut ids = self.candidates(scene, &region(s, e));
        ids.sort();
        ids.into_iter()
            .filter(|id| id != arrow && !ends.contains(&Some(id)))
            .filter_map(|id| scene.get(&id))
            .filter(|el| !el.deleted && is_obstacle(el))
            .map(|el| bounds_of(box_of(el)))
            .collect()
    }
}

/// Ids of the elements of the box index whose box touches `rect`. The reader must have the index.
fn ids_in_rect<R: SceneRead>(scene: &R, rect: &Rect) -> Vec<ElementId> {
    inc(Counter::SpatialQueries);
    scene.derived_index().into_iter().flat_map(|index| index.ids_in_rect(rect)).cloned().collect()
}

fn touches(a: &Rect, b: &Rect) -> bool {
    a.x0 <= b.x1 && a.x1 >= b.x0 && a.y0 <= b.y1 && a.y1 >= b.y0
}

/// How far from a region an arrow's box may be and still count as near it: a route keeps [`AVOID_PADDING`] from
/// the shapes it goes around, and a shape that left a region may have been the reason for a detour that far out.
const NEAR_REACH: f64 = 2.0 * AVOID_PADDING;

/// Is `e` an elbow arrow, bound at an end and with no pinned segment, whose box touches one of `places` grown by
/// [`NEAR_REACH`]?
pub(crate) fn is_near(e: &Element, places: &[Rect]) -> bool {
    !e.deleted && is_routed_by_obstacles(e) && {
        let b = box_of(e);
        places.iter().any(|r| touches(&b, &r.inflate(NEAR_REACH, NEAR_REACH)))
    }
}

/// The arrows [`is_near`] says are near `places`. With the box index it is one query per place; without it, one
/// pass over the scene. Sorted by id.
pub(crate) fn arrows_near<R: SceneRead>(scene: &R, places: &[Rect]) -> Vec<ElementId> {
    if places.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<ElementId> = if scene.derived_index().is_some() {
        places.iter().flat_map(|r| ids_in_rect(scene, &r.inflate(NEAR_REACH, NEAR_REACH))).collect()
    } else {
        count(Counter::ElementsScanned, u64::try_from(scene.len_all()).unwrap_or(u64::MAX));
        scene.iter().filter(|e| is_near(e, places)).map(|e| e.id.clone()).collect()
    };
    out.sort();
    out.dedup();
    out.retain(|id| scene.get(id).is_some_and(|e| is_near(e, places)));
    out
}

/// An elbow arrow with a bound end and no pinned segment: its route is a function of the scene alone, so it is
/// the one the router may change when something moves near it.
fn is_routed_by_obstacles(e: &Element) -> bool {
    e.linear_data().is_some_and(|d| {
        d.path == drafftink_scene::PathType::Elbow
            && d.fixed_segments.is_empty()
            && (d.start_binding.is_some() || d.end_binding.is_some())
    })
}
