//! The wgpu back end: Vello's compute renderer into a texture, then that texture and egui into the surface.

use std::sync::Arc;

use drafftink_profile::{Phase, in_phase};
use vello::Scene;
use vello::peniko::Color;
use winit::event_loop::OwnedDisplayHandle;
use winit::window::Window;

use crate::backend::{Backend, BackendKind, Detail, Presented};
use crate::compositor::Compositor;
use crate::error::{GpuError, LostInfo};
use crate::frame::EguiFrame;
use crate::gpu::{Gpu, GpuOptions, PresentSurface, create_instance};

/// The device, the window surface and the compositor.
pub(crate) struct WgpuBackend {
    gpu: Gpu,
    surface: PresentSurface,
    comp: Compositor,
}

impl WgpuBackend {
    /// Create the instance, the surface, the adapter and the device for `window`.
    pub(crate) async fn create(
        window: Arc<Window>,
        display: OwnedDisplayHandle,
        size: [u32; 2],
    ) -> Result<Self, GpuError> {
        let instance = create_instance(Some(Box::new(display))).await;
        let surface = PresentSurface::create(&instance, window)?;
        let gpu = Gpu::request(instance, Some(&surface), &GpuOptions::default()).await?;
        let surface = PresentSurface::configure(&gpu, surface, size)?;
        let comp = Compositor::new(&gpu, surface.format())?;
        Ok(Self { gpu, surface, comp })
    }
}

impl Backend for WgpuBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Vello
    }

    fn describe(&self) -> String {
        self.gpu.info().summary()
    }

    fn max_texture_side(&self) -> u32 {
        self.gpu.info().max_texture_dimension_2d
    }

    fn lost(&self) -> Option<LostInfo> {
        self.gpu.lost()
    }

    fn destroy_for_test(&self) {
        self.gpu.destroy_for_test();
    }

    fn size(&self) -> [u32; 2] {
        self.surface.size()
    }

    fn resize(&mut self, size: [u32; 2]) -> bool {
        self.surface.resize(&self.gpu, size)
    }

    fn ensure_scene_target(&mut self, size: [u32; 2]) -> bool {
        self.comp.ensure_scene_target(&self.gpu, size)
    }

    fn render_scene(&mut self, scene: &Scene, base: Color, size: [u32; 2], _detail: Detail) -> Result<(), GpuError> {
        self.comp.render_scene(&self.gpu, scene, base, size)
    }

    fn present(&mut self, window: &Window, frame: &mut EguiFrame, size: [u32; 2]) -> Result<Presented, GpuError> {
        let acquire = drafftink_profile::span("acquire_surface");
        let acquired = self.surface.surface.get_current_texture();
        drop(acquire);
        let texture = match acquired {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                self.surface.reconfigure(&self.gpu);
                t
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.reconfigure(&self.gpu);
                return Ok(Presented::Retry);
            }
            other => {
                log::debug!("skipping frame: {other:?}");
                return Ok(Presented::Skipped);
            }
        };
        let target = texture.texture.create_view(&wgpu::TextureViewDescriptor::default());
        in_phase(Phase::GpuSubmit, || self.comp.composite(&self.gpu, frame, &target, size))?;
        window.pre_present_notify();
        drafftink_profile::scope("present", || self.gpu.queue.present(texture));
        Ok(Presented::Shown)
    }
}
