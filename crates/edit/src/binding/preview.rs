//! Where the arrows bound to a shape go when the shape changes size, without writing anything.
//!
//! The text editor grows a shape while a label is typed, and shows the shape at its new size before the edit ends.
//! The arrows bound to the shape must follow it on screen as well, or they jump when the edit is committed. This
//! module solves them against a small copy of the scene, with the same solver the commit uses, so the picture
//! during the edit and the document after it agree.

use drafftink_scene::{Element, ElementId, MemoryStore, SceneRead, SceneStore, SceneTx};

use super::{BindingError, arrows_bound_to, update_bound_arrows};

/// The arrows bound to one shape, with the other shapes they are bound to, copied from the scene.
#[derive(Debug, Clone, Default)]
pub struct BoundArrows {
    shape: Option<ElementId>,
    elements: Vec<Element>,
}

impl BoundArrows {
    /// The arrows bound to `shape` in `scene`, and what their other ends are bound to.
    pub fn capture<R: SceneRead>(scene: &R, shape: &ElementId) -> Self {
        let arrows: Vec<&Element> =
            arrows_bound_to(scene, std::slice::from_ref(shape)).iter().filter_map(|id| scene.get(id)).collect();
        let mut others: Vec<&ElementId> =
            arrows.iter().flat_map(|a| a.binding_targets()).flatten().filter(|t| *t != shape).collect();
        others.sort();
        others.dedup();
        let elements = others.into_iter().filter_map(|id| scene.get(id)).chain(arrows).cloned().collect();
        Self { shape: Some(shape.clone()), elements }
    }

    /// Whether no arrow is bound to the shape.
    pub fn is_empty(&self) -> bool {
        !self.elements.iter().any(|e| e.tag().is_linear())
    }

    /// The ids of the arrows.
    pub fn arrow_ids(&self) -> impl Iterator<Item = &ElementId> {
        self.elements.iter().filter(|e| e.tag().is_linear()).map(|e| &e.id)
    }

    /// The arrows as they are when the shape is `grown` (the same element, with another size or place). Only the
    /// arrows that move are returned.
    pub fn follow(&self, grown: &Element) -> Result<Vec<Element>, BindingError> {
        if self.shape.as_ref() != Some(&grown.id) || self.is_empty() {
            return Ok(Vec::new());
        }
        let mut copy = MemoryStore::new();
        let mut tx = copy.begin();
        tx.insert(grown.clone())?;
        for e in &self.elements {
            tx.insert(e.clone())?;
        }
        let moved = update_bound_arrows(&mut tx, std::slice::from_ref(&grown.id))?;
        Ok(moved.iter().filter_map(|id| tx.get(id)).cloned().collect())
    }
}
