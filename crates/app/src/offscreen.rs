//! The picture behind PNG export and screenshots, shared by the desktop and the web renderer ports.
//!
//! [`compose`] turns a document snapshot and a [`RenderRequest`] into a Vello scene (what the render crate encodes
//! for the visible elements, plus the selection outline when asked); the platform draws it into a texture and reads the
//! pixels back, and [`encode_png`] writes them as a PNG. Only the GPU part differs between the platforms:
//! blocking on the desktop, asynchronous in the browser.

use drafftink_canvas::shape::element_box;
use drafftink_engine::PortError;
use drafftink_engine::ports::{RenderRequest, SceneView};
use drafftink_engine::services::SharedTypesetter;
use drafftink_geom::kurbo::{Size, Vec2};
use drafftink_render::{
    Overlay, OverlayStyle, RenderOptions, Scene, SceneComposer, SelectionBox, Theme, View, build_paint_index,
    paint_overlay,
};
use drafftink_scene::{MemoryStore, SceneRead};
use vello::peniko::Color;

/// Largest side of an image the renderers produce.
pub(crate) const MAX_SIDE: u32 = 16_384;

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
pub(crate) fn compose(typesetter: &SharedTypesetter, scene: &SceneView, req: &RenderRequest) -> Result<Composed, PortError> {
    let (w, h) = req.size_px;
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        return Err(PortError::Failed(format!("the picture would be {w} by {h} pixels; the limit is {MAX_SIDE}")));
    }
    let store = MemoryStore::from_document(scene).map_err(|e| PortError::Failed(e.to_string()))?;
    let (rx, ry, rw, _) = req.region;
    let zoom = f64::from(w) / rw.max(f64::EPSILON);
    let view = View::new(zoom, Vec2::new(-rx * zoom, -ry * zoom), Size::new(f64::from(w), f64::from(h)));

    let theme = if req.dark { Theme::Dark } else { Theme::Light };
    let mut composer = SceneComposer::new();
    composer.set_options(RenderOptions::for_export(theme));
    let index = build_paint_index(&store);
    let mut world = Scene::new();
    typesetter.with(|ts| composer.compose(&store, &index, &view, ts, &mut world));
    let base = if req.background { composer.background(&store) } else { Color::TRANSPARENT };

    let mut full = Scene::new();
    if req.overlay && !req.selection.is_empty() {
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
        typesetter.with(|ts| paint_overlay(&mut layer, &overlay, &view, &OverlayStyle::default(), ts));
        drafftink_render::combine(&world, &layer, &mut full);
    } else {
        full = world;
    }
    Ok(Composed { scene: full, base, size: [w, h] })
}

/// A selection box with no handles.
const HANDLES_OFF: drafftink_geom::select::HandleSet =
    drafftink_geom::select::HandleSet { corners: false, edges: false, rotate: false };

/// An 8-bit RGBA PNG.
pub(crate) fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, PortError> {
    let mut out = Vec::new();
    let fail = |e: png::EncodingError| PortError::Failed(format!("could not encode the PNG: {e}"));
    let mut enc = png::Encoder::new(&mut out, width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(fail)?;
    writer.write_image_data(rgba).map_err(fail)?;
    writer.finish().map_err(fail)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_has_the_signature_and_the_size() {
        let png = encode_png(2, 1, &[255, 0, 0, 255, 0, 255, 0, 255]).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(u32::from_be_bytes([png[16], png[17], png[18], png[19]]), 2);
    }

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
        req.size_px = (MAX_SIDE + 1, 5);
        assert!(compose(&ts, &doc, &req).is_err());
        req.size_px = (4, 4);
        assert!(compose(&ts, &doc, &req).is_ok());
    }
}
