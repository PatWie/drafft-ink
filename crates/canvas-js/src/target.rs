//! The GPU half of a frame: a Vello scene to pixels.
//!
//! [`Renderer`] owns the device and the compositor of `drafftink-gpu`. In a browser it draws to a canvas surface;
//! natively (tests, tools) it has no surface and renders to an image.

use drafftink_gpu::{Compositor, EguiFrame, Gpu, GpuError, Image, LostInfo, PresentSurface};
use drafftink_render::Scene;

/// A device, a compositor and, in the browser, the canvas surface.
pub struct Renderer {
    gpu: Gpu,
    compositor: Compositor,
    present: Option<PresentSurface>,
}

impl Renderer {
    /// A renderer for a canvas of `size` device pixels. Needs WebGPU.
    #[cfg(target_arch = "wasm32")]
    pub async fn for_canvas(canvas: web_sys::HtmlCanvasElement, size: [u32; 2]) -> Result<Self, GpuError> {
        use drafftink_gpu::{GpuOptions, create_instance};
        if !drafftink_gpu::web::webgpu_available() {
            return Err(GpuError::NoAdapter(
                "this browser has no WebGPU (navigator.gpu is missing); use a current Chrome, Edge, Safari or Firefox"
                    .to_owned(),
            ));
        }
        let instance = create_instance(None).await;
        let surface = PresentSurface::create(&instance, wgpu::SurfaceTarget::Canvas(canvas))?;
        let gpu = Gpu::request(instance, Some(&surface), &GpuOptions::default()).await?;
        let present = PresentSurface::configure(&gpu, surface, size)?;
        let compositor = Compositor::new(&gpu, present.format())?;
        Ok(Self { gpu, compositor, present: Some(present) })
    }

    /// A renderer without a window: it renders to images. Blocks while the device starts.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn headless() -> Result<Self, GpuError> {
        let gpu = Gpu::headless_blocking(&drafftink_gpu::GpuOptions::default())?;
        let compositor = Compositor::new(&gpu, wgpu::TextureFormat::Rgba8Unorm)?;
        Ok(Self { gpu, compositor, present: None })
    }

    /// Facts about the adapter, for bug reports.
    pub fn adapter(&self) -> String {
        self.gpu.info().summary()
    }

    /// `Some` once the device was lost.
    pub fn lost(&self) -> Option<LostInfo> {
        self.gpu.lost()
    }

    /// Largest image side the device draws.
    pub fn max_side(&self) -> u32 {
        self.gpu.info().max_texture_dimension_2d
    }

    /// Draws `scene` to the canvas surface at `size` device pixels and presents it. `Ok(false)` when the browser
    /// had no frame to give (the surface was outdated and has been reconfigured); ask again.
    pub fn draw(&mut self, scene: &Scene, base: peniko::Color, size: [u32; 2]) -> Result<bool, GpuError> {
        let Some(present) = self.present.as_mut() else {
            return Err(GpuError::Surface("this renderer has no canvas; use render_image".to_owned()));
        };
        present.resize(&self.gpu, size);
        self.compositor.render_scene(&self.gpu, scene, base, size)?;
        let texture = match present.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                present.reconfigure(&self.gpu);
                t
            }
            other => {
                log::debug!("no frame this time: {other:?}");
                present.reconfigure(&self.gpu);
                return Ok(false);
            }
        };
        let view = texture.texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.compositor.composite(&self.gpu, &mut EguiFrame::empty(), &view, size)?;
        self.gpu.queue.present(texture);
        Ok(true)
    }

    /// Draws `scene` into an RGBA image. The scene texture is resized to `size`, so a stage that shows a canvas
    /// must draw again afterwards.
    pub async fn render_image(
        &mut self,
        scene: &Scene,
        base: peniko::Color,
        size: [u32; 2],
    ) -> Result<Image, GpuError> {
        self.compositor.render_to_image(&self.gpu, size, Some((scene, base)), None).await
    }

    /// Blocking form of [`Renderer::render_image`] for native tests.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn render_image_blocking(
        &mut self,
        scene: &Scene,
        base: peniko::Color,
        size: [u32; 2],
    ) -> Result<Image, GpuError> {
        pollster::block_on(self.render_image(scene, base, size))
    }
}
