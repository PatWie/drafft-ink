//! `vello_gpu` on a WebGL2 canvas: the part that draws a Vello scene, shared by the window and the exporter.

use std::collections::{HashMap, HashSet};

use vello::Scene as VelloScene;
use vello::low_level::vello_encoding::Patch;
use vello::peniko::{Color, ImageData};
use vello_gpu::{ImageId, ImageSource, RenderSize, Resources, Scene, WebGlRenderer, WebGlTextureBindings};
use web_sys::{HtmlCanvasElement, WebGl2RenderingContext};

use super::canvas::GpuCanvas;
use super::replay::{ReplayStats, replay};
use crate::backend::Detail;
use crate::error::GpuError;

fn webgl(e: impl std::fmt::Display) -> GpuError {
    GpuError::WebGl(e.to_string())
}

/// A WebGL2 renderer for one canvas.
pub(crate) struct GlRenderer {
    renderer: WebGlRenderer,
    resources: Resources,
    scene: Scene,
    /// Atlas images by the id of their pixel blob. `None`: the picture could not be uploaded, so it is left out.
    images: HashMap<u64, Option<ImageId>>,
    size: [u16; 2],
}

impl GlRenderer {
    /// Compile the shaders for `canvas`. Fails when the browser has no WebGL2 or only a software one.
    pub(crate) fn new(canvas: &HtmlCanvasElement) -> Result<Self, GpuError> {
        let (renderer, resources) = WebGlRenderer::new(canvas).map_err(webgl)?;
        Ok(Self { renderer, resources, scene: Scene::new(1, 1), images: HashMap::new(), size: [1, 1] })
    }

    /// The context, for the code that blits and paints around the scene.
    pub(crate) fn gl(&self) -> &WebGl2RenderingContext {
        self.renderer.gl_context()
    }

    /// Draw `scene` over `base` into the canvas' framebuffer, which must be `size` pixels.
    pub(crate) fn draw(
        &mut self,
        scene: &VelloScene,
        base: Color,
        size: [u32; 2],
        detail: Detail,
    ) -> Result<ReplayStats, GpuError> {
        let (w, h) = match (u16::try_from(size[0]), u16::try_from(size[1])) {
            (Ok(w), Ok(h)) if w > 0 && h > 0 => (w, h),
            _ => return Err(GpuError::Render(format!("a {}x{} canvas is out of range", size[0], size[1]))),
        };
        self.sync_images(scene);
        if self.size == [w, h] {
            self.scene.reset();
        } else {
            self.scene.reset_and_resize(w, h);
            self.size = [w, h];
        }
        let stats = {
            let images = &self.images;
            let mut canvas = GpuCanvas { scene: &mut self.scene, resources: &mut self.resources, images };
            replay(scene.encoding(), &mut canvas, detail)
        };
        self.renderer
            .render(
                &self.scene,
                &mut self.resources,
                &RenderSize { width: w, height: h },
                &WebGlTextureBindings::new(),
                base,
            )
            .map_err(webgl)?;
        Ok(stats)
    }

    /// Uploads the pictures `scene` uses that the atlas does not have, and frees those it no longer uses.
    fn sync_images(&mut self, scene: &VelloScene) {
        let used: HashSet<u64> = pictures(scene).map(|image| image.data.id()).collect();
        let stale: Vec<u64> = self.images.keys().filter(|id| !used.contains(id)).copied().collect();
        for id in stale {
            if let Some(Some(image)) = self.images.remove(&id)
                && let Err(e) = self.renderer.destroy_image(&mut self.resources, image)
            {
                log::debug!("picture not freed: {e}");
            }
        }
        for image in pictures(scene) {
            if !self.images.contains_key(&image.data.id()) {
                let uploaded = self.upload(image);
                self.images.insert(image.data.id(), uploaded);
            }
        }
    }

    fn upload(&mut self, image: &ImageData) -> Option<ImageId> {
        let (w, h) = (u16::try_from(image.width).ok()?, u16::try_from(image.height).ok()?);
        if w == 0
            || h == 0
            || !matches!(image.format, vello::peniko::ImageFormat::Rgba8 | vello::peniko::ImageFormat::Bgra8)
        {
            return None;
        }
        let ImageSource::Pixmap(pixmap) = ImageSource::from_peniko_image_data(image) else { return None };
        match self.renderer.upload_image(&mut self.resources, &pixmap) {
            Ok(id) => Some(id),
            Err(e) => {
                log::warn!("a picture does not fit the WebGL atlas: {e}");
                None
            }
        }
    }
}

/// The pictures a scene draws.
fn pictures(scene: &VelloScene) -> impl Iterator<Item = &ImageData> {
    scene.encoding().resources.patches.iter().filter_map(|p| match p {
        Patch::Image { image, .. } => Some(image),
        _ => None,
    })
}
