//! The document as a Vello scene: what the app's export draws, without the selection overlay.

use drafftink_engine::ports::{RenderRequest, SceneView};
use drafftink_engine::services::SharedTypesetter;
use drafftink_geom::kurbo::{Size, Vec2};
use drafftink_render::{RenderOptions, Scene, SceneComposer, Theme, View, build_paint_index};
use drafftink_scene::MemoryStore;
use peniko::Color;

use super::error::RasterError;

/// Longest side of a picture, as in the app's own export.
pub(super) const MAX_SIDE: u32 = 16_384;

/// A scene ready to be rasterised.
pub(super) struct Composed {
    /// The elements, in picture pixels.
    pub scene: Scene,
    /// The colour under the scene (transparent when the request has no background).
    pub base: Color,
    /// Width and height in pixels.
    pub size: [u16; 2],
}

/// Encodes the region of `doc` that `req` asks for.
pub(super) fn compose(
    typesetter: &SharedTypesetter,
    doc: &SceneView,
    req: &RenderRequest,
) -> Result<Composed, RasterError> {
    if req.overlay && !req.selection.is_empty() {
        return Err(RasterError::Overlay);
    }
    let (w, h) = req.size_px;
    let too_big = || RasterError::Size { width: w, height: h, max: MAX_SIDE };
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        return Err(too_big());
    }
    let size = [u16::try_from(w).map_err(|_| too_big())?, u16::try_from(h).map_err(|_| too_big())?];
    let store = MemoryStore::from_document(doc).map_err(|e| RasterError::Document(e.to_string()))?;
    let (x, y, region_width, _) = req.region;
    let zoom = f64::from(w) / region_width.max(f64::EPSILON);
    let view = View::new(zoom, Vec2::new(-x * zoom, -y * zoom), Size::new(f64::from(w), f64::from(h)));

    let mut composer = SceneComposer::new();
    composer.set_options(RenderOptions::for_export(if req.dark { Theme::Dark } else { Theme::Light }));
    let index = build_paint_index(&store);
    let mut scene = Scene::new();
    typesetter.with(|ts| composer.compose(&store, &index, &view, ts, &mut scene));
    let base = if req.background { composer.background(&store) } else { Color::TRANSPARENT };
    Ok(Composed { scene, base, size })
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
            assert!(matches!(err, RasterError::Size { .. }), "{err}");
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
