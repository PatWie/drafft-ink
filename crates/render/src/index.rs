//! Keeping a spatial index of paint bounds in step with a store.
//!
//! [`crate::SceneComposer`] culls with a [`SpatialIndex`] whose entries are [`paint_bounds`] of the live elements:
//! the box of everything an element can paint, not its hit-test box. The canvas layer owns the index and calls
//! these two functions; the render crate never keeps one of its own, so there is only one copy of every bound.

use drafftink_geom::SpatialIndex;
use drafftink_scene::{ChangeKind, ChangeSet, ElementId, Fields, SceneRead};

use crate::spec::paint_bounds;

/// The index type the composer takes.
pub type PaintIndex = SpatialIndex<ElementId>;

/// Builds the index of every live element in `store`.
pub fn build_paint_index<S: SceneRead>(store: &S) -> PaintIndex {
    SpatialIndex::bulk_load(store.iter().map(|e| (e.id.clone(), paint_bounds(e))))
}

/// Brings `index` up to date with one commit. Only the touched elements are looked at.
pub fn update_paint_index<S: SceneRead>(index: &mut PaintIndex, store: &S, changes: &ChangeSet) {
    let bounds_fields =
        Fields::POSITION | Fields::SIZE | Fields::ANGLE | Fields::KIND | Fields::STYLE | Fields::DELETED;
    for c in &changes.elements {
        if c.kind == ChangeKind::Removed {
            index.remove(&c.id);
            continue;
        }
        if c.kind == ChangeKind::Updated && !c.fields.intersects(bounds_fields) && index.contains(&c.id) {
            continue;
        }
        match store.get(&c.id) {
            Some(e) if !e.deleted => {
                if index.insert(e.id.clone(), paint_bounds(e)).is_err() {
                    // Non-finite numbers: such an element has no place on the canvas.
                    index.remove(&e.id);
                }
            }
            _ => {
                index.remove(&c.id);
            }
        }
    }
}
