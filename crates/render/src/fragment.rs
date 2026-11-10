//! A fragment: one element encoded as a Vello scene in the element's local frame.

use drafftink_scene::FileId;
use kurbo::Rect;
use vello::Scene;

/// An element encoded once and appended to a frame many times.
///
/// The scene is in the element's local frame (origin at the top-left of its unrotated box). The composer appends
/// it with `view * element_to_world`, so moving, rotating, panning and zooming never re-encode it.
pub struct Fragment {
    /// The encoded drawing.
    pub scene: Scene,
    /// What the drawing covers in the local frame, stroke included. `Rect::ZERO` for an empty drawing.
    pub bounds: Rect,
    /// Heap size estimate in bytes, for the cache budget.
    pub bytes: usize,
    /// Set while the element shows a placeholder because its picture is not decoded or not in the store yet.
    pub awaiting_image: Option<FileId>,
}

impl std::fmt::Debug for Fragment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fragment")
            .field("bounds", &self.bounds)
            .field("bytes", &self.bytes)
            .field("awaiting_image", &self.awaiting_image)
            .finish_non_exhaustive()
    }
}

impl Fragment {
    pub(crate) fn new(scene: Scene, bounds: Option<Rect>, awaiting_image: Option<FileId>) -> Self {
        let bytes = scene_bytes(&scene);
        Self { scene, bounds: bounds.unwrap_or(Rect::ZERO), bytes, awaiting_image }
    }

    /// True when nothing was drawn. A glyph run has draw data but no path data (its outlines are resolved when
    /// the scene is rendered), so both streams are checked.
    pub fn is_empty(&self) -> bool {
        let e = self.scene.encoding();
        e.path_tags.is_empty() && e.draw_tags.is_empty()
    }
}

/// Estimated heap size of an encoded scene.
pub fn scene_bytes(scene: &Scene) -> usize {
    use std::mem::size_of_val;
    let e = scene.encoding();
    let r = &e.resources;
    e.path_tags.len()
        + size_of_val(e.path_data.as_slice())
        + size_of_val(e.draw_tags.as_slice())
        + size_of_val(e.draw_data.as_slice())
        + size_of_val(e.transforms.as_slice())
        + size_of_val(e.styles.as_slice())
        + size_of_val(r.patches.as_slice())
        + size_of_val(r.color_stops.as_slice())
        + size_of_val(r.glyphs.as_slice())
        + size_of_val(r.glyph_runs.as_slice())
        + size_of_val(r.normalized_coords.as_slice())
        + 256
}
