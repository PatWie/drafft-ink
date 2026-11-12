//! The picture behind PNG export and screenshots, shared by the desktop and the web renderer ports.
//!
//! [`compose`] turns a document snapshot and a [`RenderRequest`] into a Vello scene (what the render crate encodes
//! for the visible elements, plus the selection outline when asked); the platform draws it into a texture and reads the
//! pixels back, and `drafftink_render::encode_png` writes them as a PNG. Only the GPU part differs between the platforms:
//! blocking on the desktop, asynchronous in the browser.

use drafftink_canvas::shape::element_box;
use drafftink_engine::PortError;
use drafftink_engine::ports::{RenderRequest, SceneView};
use drafftink_engine::services::SharedTypesetter;
use drafftink_render::{ExportSpec, Overlay, OverlayStyle, Scene, SelectionBox, compose_export, paint_overlay};
use drafftink_scene::{MemoryStore, SceneRead};
use vello::peniko::Color;

/// A scene ready to be drawn.
pub(crate) struct Composed {
    /// The Vello scene.
    pub scene: Scene,
    /// The colour under the scene (transparent when the request has no background).
    pub base: Color,
    /// Pixels.
    pub size: [u32; 2],
}

/// Builds the scene of `req` for `scene`. Fails for a size the renderers refuse.
pub(crate) fn compose(
    typesetter: &SharedTypesetter,
    scene: &SceneView,
    req: &RenderRequest,
) -> Result<Composed, PortError> {
    let fail = |e: &dyn std::fmt::Display| PortError::Failed(e.to_string());
    let store = MemoryStore::from_document(scene).map_err(|e| fail(&e))?;
    let spec = ExportSpec { region: req.region, size_px: req.size_px, dark: req.dark, background: req.background };
    let exported = typesetter.with(|ts| compose_export(&store, &spec, ts)).map_err(|e| fail(&e))?;
    let view = &exported.view;
    let scene = if req.overlay && !req.selection.is_empty() {
        let overlay = Overlay {
            selection: req
                .selection
                .iter()
                .filter_map(|id| store.get(id))
                .map(|e| SelectionBox { obox: element_box(e), handles: HANDLES_OFF, dashed: true })
                .collect(),
            ..Overlay::default()
        };
        let mut layer = Scene::new();
        typesetter.with(|ts| paint_overlay(&mut layer, &overlay, view, &OverlayStyle::default(), ts));
        let mut full = Scene::new();
        drafftink_render::combine(&exported.scene, &layer, &mut full);
        full
    } else {
        exported.scene
    };
    Ok(Composed { scene, base: exported.base, size: exported.size })
}

/// A selection box with no handles.
const HANDLES_OFF: drafftink_geom::select::HandleSet =
    drafftink_geom::select::HandleSet { corners: false, edges: false, rotate: false };

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_or_huge_picture_is_refused_before_any_work() {
        let ts = drafftink_engine::services::LazyTypesetter::shared();
        let doc = drafftink_scene::Document::default();
        let mut req = RenderRequest {
            region: (0.0, 0.0, 10.0, 10.0),
            size_px: (0, 5),
            background: true,
            overlay: false,
            selection: Vec::new(),
            background_color: drafftink_scene::Color::WHITE,
            dark: false,
        };
        assert!(compose(&ts, &doc, &req).is_err());
        req.size_px = (drafftink_render::MAX_SIDE + 1, 5);
        assert!(compose(&ts, &doc, &req).is_err());
        req.size_px = (4, 4);
        assert!(compose(&ts, &doc, &req).is_ok());
    }
}
