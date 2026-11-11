//! PNG export and screenshots on the desktop: a headless GPU device of our own, created on first use.
//!
//! The engine's `Renderer` port is synchronous and takes a `Document`. This renderer builds a store from it,
//! encodes the visible elements with the render crate, lets Vello draw them into an offscreen texture, reads
//! the pixels back and encodes a PNG. The window's own device stays untouched.

use std::sync::{Mutex, MutexGuard, PoisonError};

use drafftink_canvas::shape::element_box;
use drafftink_engine::PortError;
use drafftink_engine::ports::{RenderRequest, Renderer, SceneView};
use drafftink_engine::services::SharedTypesetter;
use drafftink_geom::kurbo::{Size, Vec2};
use drafftink_gpu::{Compositor, Gpu, GpuOptions};
use drafftink_render::{
    Overlay, OverlayStyle, RenderOptions, Scene, SceneComposer, SelectionBox, Theme, View, build_paint_index,
    paint_overlay,
};
use drafftink_scene::{MemoryStore, SceneRead};
use vello::peniko::Color;

/// Largest side of an image this renderer produces.
const MAX_SIDE: u32 = 16_384;

struct Device {
    gpu: Gpu,
    compositor: Compositor,
}

/// The renderer port.
pub(crate) struct OffscreenRenderer {
    typesetter: SharedTypesetter,
    device: Mutex<Option<Device>>,
}

impl OffscreenRenderer {
    pub(crate) fn new(typesetter: SharedTypesetter) -> Self {
        Self { typesetter, device: Mutex::new(None) }
    }

    fn device(&self) -> Result<MutexGuard<'_, Option<Device>>, PortError> {
        let mut guard = self.device.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.is_none() {
            let gpu = Gpu::headless_blocking(&GpuOptions::default()).map_err(|e| PortError::Failed(e.to_string()))?;
            let compositor =
                Compositor::new(&gpu, wgpu::TextureFormat::Rgba8Unorm).map_err(|e| PortError::Failed(e.to_string()))?;
            *guard = Some(Device { gpu, compositor });
        }
        Ok(guard)
    }
}

impl Renderer for OffscreenRenderer {
    fn render_png(&self, scene: &SceneView, req: &RenderRequest) -> Result<Vec<u8>, PortError> {
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
        self.typesetter.with(|ts| composer.compose(&store, &index, &view, ts, &mut world));
        let base = if req.background { composer.background(&store) } else { Color::TRANSPARENT };

        let mut full = Scene::new();
        if req.overlay && !req.selection.is_empty() {
            let outline = HandlesOff::none();
            let overlay = Overlay {
                selection: req
                    .selection
                    .iter()
                    .filter_map(|id| store.get(id))
                    .map(|e| SelectionBox { obox: element_box(e), handles: outline.0, dashed: true })
                    .collect(),
                ..Overlay::default()
            };
            let mut layer = Scene::new();
            paint_overlay(&mut layer, &overlay, &view, &OverlayStyle::default());
            drafftink_render::combine(&world, &layer, &mut full);
        } else {
            full = world;
        }

        let mut guard = self.device()?;
        let Some(dev) = guard.as_mut() else { return Err(PortError::Failed("no GPU device".into())) };
        let image = dev
            .compositor
            .render_to_image_blocking(&dev.gpu, [w, h], Some((&full, base)), None)
            .map_err(|e| PortError::Failed(e.to_string()))?;
        encode_png(image.width, image.height, &image.rgba)
    }
}

/// A selection box with no handles.
struct HandlesOff(drafftink_geom::select::HandleSet);

impl HandlesOff {
    fn none() -> Self {
        Self(drafftink_geom::select::HandleSet { corners: false, edges: false, rotate: false })
    }
}

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
}
