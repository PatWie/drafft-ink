//! The z order of a whole scene with its paint bounds, kept between frames.
//!
//! A frame that shows most of a large scene needs every element in z order. Building that list is one pass over
//! the store and the index (12 ms at 100,000 elements), so [`ZOrder`] keeps it and follows the edits instead:
//! the composer is told which elements a commit touched ([`ZOrder::note`]) and, at the next frame, a touched
//! element that is still where the list has it only gets its bounds read again. An element that is new, gone,
//! or in another place in the order cannot be patched, and the list is built again.

use drafftink_geom::{SpatialIndex, aabb};
use drafftink_scene::{Element, ElementId, SceneRead, ZIndex};
use kurbo::Rect;

/// A patch that touches more than one slot in this many is not worth it: the list is built again.
const PATCH_FRACTION: usize = 8;

/// One element of the order.
struct Slot {
    id: ElementId,
    z: ZIndex,
    bounds: Rect,
}

/// Every indexed element in z order with its paint bounds, valid for one version of the store.
pub(crate) struct ZOrder {
    /// `SceneRead::version_stamp` of the store the list describes, and the index generation it was read at.
    stamp: (u64, u64),
    slots: Vec<Slot>,
    /// The store version the touched ids in `touched` bring the list to.
    noted: u64,
    /// Elements committed since the list was last exact.
    touched: Vec<ElementId>,
    /// Too many touched elements to patch.
    overflowed: bool,
}

/// How the list was brought up to date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refresh {
    /// Already exact for this version.
    Current,
    /// Touched elements were patched in place.
    Patched,
    /// Built from the store.
    Built,
}

impl ZOrder {
    /// Reads the whole store: one pass.
    fn build<S: SceneRead>(store: &S, index: &SpatialIndex<ElementId>, version: u64) -> Self {
        let slots = store
            .iter()
            .filter_map(|el| Some(Slot { id: el.id.clone(), z: el.index.clone(), bounds: index.get(&el.id)? }))
            .collect();
        Self { stamp: (version, index.generation()), slots, noted: version, touched: Vec::new(), overflowed: false }
    }

    /// Records that the store is at `version` after commits that touched `ids`. Call it with every commit's ids, in
    /// order, as the index is told (`update_paint_index`); a commit that is left out leaves a stale slot.
    pub(crate) fn note<'a>(&mut self, version: u64, ids: impl IntoIterator<Item = &'a ElementId>) {
        if self.overflowed {
            return;
        }
        self.touched.extend(ids.into_iter().cloned());
        self.noted = version;
        if self.touched.len() * PATCH_FRACTION > self.slots.len().max(PATCH_FRACTION * 8) {
            self.overflowed = true;
            self.touched = Vec::new();
        }
    }

    /// Brings `slot` up to date with `store` at `version`: nothing to do, a patch of the touched elements, or a new
    /// list from the store.
    pub(crate) fn refresh<S: SceneRead>(
        slot: &mut Option<Self>,
        store: &S,
        index: &SpatialIndex<ElementId>,
        version: u64,
    ) -> Refresh {
        if let Some(o) = slot {
            if o.stamp == (version, index.generation()) {
                return Refresh::Current;
            }
            if o.patch(store, index, version) {
                return Refresh::Patched;
            }
        }
        *slot = Some(Self::build(store, index, version));
        Refresh::Built
    }

    /// Reads the touched elements again. False when one of them cannot be patched, or when the touched ids do not
    /// bring the list to `version`.
    fn patch<S: SceneRead>(&mut self, store: &S, index: &SpatialIndex<ElementId>, version: u64) -> bool {
        if self.overflowed || self.noted != version {
            return false;
        }
        for id in std::mem::take(&mut self.touched) {
            let Some(el) = store.get(&id).filter(|e| !e.deleted) else { return false };
            let Some(bounds) = index.get(&id) else { return false };
            match self.find(el) {
                Some(i) => self.slots[i].bounds = bounds,
                None => return false,
            }
        }
        self.stamp = (version, index.generation());
        true
    }

    /// Where `el` is in the list, if it is there with the z key it has now.
    fn find(&self, el: &Element) -> Option<usize> {
        self.slots.binary_search_by(|s| (&s.z, &s.id).cmp(&(&el.index, &el.id))).ok()
    }

    /// The slots whose bounds touch `world`, back to front.
    pub(crate) fn in_view<'a>(&'a self, world: &'a Rect) -> impl Iterator<Item = (&'a ElementId, Rect)> + 'a {
        self.slots.iter().filter(|s| aabb::intersects(world, &s.bounds)).map(|s| (&s.id, s.bounds))
    }
}
