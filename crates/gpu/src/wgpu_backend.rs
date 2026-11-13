//! The wgpu back end: Vello's compute renderer into a texture, then that texture and egui into the surface.

use std::sync::Arc;

use std::time::Duration;

use drafftink_profile::{Phase, in_phase};
use vello::Scene;
use vello::peniko::Color;
use web_time::Instant;
use winit::event_loop::OwnedDisplayHandle;
use winit::window::Window;

use crate::backend::{Backend, BackendKind, Detail, Presented};
use crate::compositor::Compositor;
use crate::error::{GpuError, LostInfo};
use crate::frame::EguiFrame;
use crate::gpu::{Gpu, GpuOptions, PresentSurface, create_instance};
use crate::stats::{BackendStats, GpuMemory, RendererInfo};

/// How long a memory report is reused. Building one lists every allocation, so it is not done every frame.
const MEMORY_REFRESH: Duration = Duration::from_millis(500);

/// The allocator totals and when they were read.
struct MemoryReading {
    at: Instant,
    memory: Option<GpuMemory>,
}

/// The device, the window surface and the compositor.
pub(crate) struct WgpuBackend {
    gpu: Gpu,
    surface: PresentSurface,
    comp: Compositor,
    /// `Some` while the debug panel is open.
    memory: Option<MemoryReading>,
    stats_on: bool,
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
        Ok(Self { gpu, surface, comp, memory: None, stats_on: false })
    }
}

impl Backend for WgpuBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Vello
    }

    fn describe(&self) -> String {
        self.gpu.info().summary()
    }

    fn info(&self) -> RendererInfo {
        self.gpu.info().renderer_info()
    }

    fn set_stats(&mut self, on: bool) {
        self.stats_on = on;
        self.comp.set_timing(on);
        if !on {
            self.memory = None;
        }
    }

    fn stats(&mut self) -> BackendStats {
        let gpu_time = self.comp.gpu_time(&self.gpu);
        let stale = self.memory.as_ref().is_none_or(|m| m.at.elapsed() >= MEMORY_REFRESH);
        if stale {
            let memory = self.gpu.device.generate_allocator_report().map(|report| GpuMemory {
                allocated: report.total_allocated_bytes,
                reserved: report.total_reserved_bytes,
            });
            self.memory = Some(MemoryReading { at: Instant::now(), memory });
        }
        BackendStats { gpu_time, memory: self.memory.as_ref().and_then(|m| m.memory) }
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
