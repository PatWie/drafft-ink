//! PNG export and screenshots on the desktop: a headless GPU device of our own, created on first use.
//!
//! The engine's `Renderer` port is synchronous and takes a `Document`. This renderer builds the scene with
//! [`crate::offscreen::compose`], lets Vello draw it into an offscreen texture, reads the pixels back and encodes
//! a PNG. The window's own device stays untouched.

use std::sync::{Mutex, MutexGuard, PoisonError};

use drafftink_engine::PortError;
use drafftink_engine::ports::{RenderRequest, Renderer, SceneView};
use drafftink_engine::services::SharedTypesetter;
use drafftink_gpu::{Compositor, Gpu, GpuOptions};

use crate::offscreen::{compose, encode_png};

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
        let composed = compose(&self.typesetter, scene, req)?;
        let mut guard = self.device()?;
        let Some(dev) = guard.as_mut() else { return Err(PortError::Failed("no GPU device".into())) };
        let image = dev
            .compositor
            .render_to_image_blocking(&dev.gpu, composed.size, Some((&composed.scene, composed.base)), None)
            .map_err(|e| PortError::Failed(e.to_string()))?;
        encode_png(image.width, image.height, &image.rgba)
    }
}
