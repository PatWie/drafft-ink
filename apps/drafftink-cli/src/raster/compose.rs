//! The document as a Vello scene: what the app's export draws, without the selection overlay.

use drafftink_engine::ports::{RenderRequest, SceneView};
use drafftink_engine::services::SharedTypesetter;
use drafftink_render::{ExportError, ExportSpec, MAX_SIDE, Scene, compose_export};
use drafftink_scene::MemoryStore;
use peniko::Color;

use super::error::RasterError;

/// A scene ready to be rasterised.
pub(super) struct Composed {
    /// The elements, in picture pixels.
    pub scene: Scene,
    /// The colour under the scene (transparent when the request has no background).
    pub base: Color,
    /// Width and height in pixels.
    pub size: [u16; 2],
}

/// Encodes the region of `doc` that `req` asks for. The scene is the one the app's export makes
/// ([`drafftink_render::compose_export`]); only the selection marks are refused.
pub(super) fn compose(
    typesetter: &SharedTypesetter,
    doc: &SceneView,
    req: &RenderRequest,
) -> Result<Composed, RasterError> {
    if req.overlay && !req.selection.is_empty() {
        return Err(RasterError::Overlay);
    }
    let store = MemoryStore::from_document(doc)?;
    let spec = ExportSpec { region: req.region, size_px: req.size_px, dark: req.dark, background: req.background };
    let exported = typesetter.with(|ts| compose_export(&store, &spec, ts))?;
    let [width, height] = exported.size;
    // `compose_export` keeps each side within `MAX_SIDE`, which a `u16` holds.
    let too_big = || ExportError::Size { width, height, max: MAX_SIDE };
    let size = [u16::try_from(width).map_err(|_| too_big())?, u16::try_from(height).map_err(|_| too_big())?];
    Ok(Composed { scene: exported.scene, base: exported.base, size })
}

#[cfg(test)]
mod tests {
    use drafftink_engine::services::LazyTypesetter;
    use drafftink_scene::{Color as SceneColor, Document, ElementId};

    use super::*;

    fn request() -> RenderRequest {
        RenderRequest {
            region: (0.0, 0.0, 10.0, 10.0),
            size_px: (4, 4),
            background: true,
            overlay: false,
            selection: Vec::new(),
            background_color: SceneColor::WHITE,
            dark: false,
        }
    }

    #[test]
    fn a_small_picture_is_composed_at_the_size_asked_for() {
        let composed = compose(&LazyTypesetter::shared(), &Document::default(), &request()).unwrap();
        assert_eq!(composed.size, [4, 4]);
    }

    #[test]
    fn a_picture_without_background_starts_transparent() {
        let req = RenderRequest { background: false, ..request() };
        let composed = compose(&LazyTypesetter::shared(), &Document::default(), &req).unwrap();
        assert_eq!(composed.base, Color::TRANSPARENT);
    }

    #[test]
    fn a_side_the_renderer_cannot_draw_is_refused_in_plain_words() {
        for size_px in [(0, 5), (5, 0), (MAX_SIDE + 1, 5), (5, MAX_SIDE + 1)] {
            let req = RenderRequest { size_px, ..request() };
            let err = compose(&LazyTypesetter::shared(), &Document::default(), &req).err().unwrap();
            assert!(matches!(err, RasterError::Export(ExportError::Size { .. })), "{err}");
            assert!(err.to_string().contains("pixels"), "{err}");
        }
    }

    #[test]
    fn selection_marks_are_refused_because_only_the_app_draws_them() {
        let req = RenderRequest { overlay: true, selection: vec![ElementId::new("a").unwrap()], ..request() };
        let err = compose(&LazyTypesetter::shared(), &Document::default(), &req).err().unwrap();
        assert!(matches!(err, RasterError::Overlay));
        assert!(err.to_string().contains("Export without"), "{err}");
    }
}
