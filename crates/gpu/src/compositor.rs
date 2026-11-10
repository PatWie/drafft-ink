//! Vello into a texture, then the texture and egui into the target, in one render pass.
//!
//! Per frame: [`Compositor::render_scene`] runs Vello into a texture that is reused until the
//! size changes (Vello submits its own command buffers, this is Vello 0.11's API). Then
//! [`Compositor::composite`] records the blit of that texture and the egui pass into one command
//! buffer and submits once. A frame without a new scene skips Vello and composites the old texture.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};

use drafftink_profile::{Counter, inc};
use egui::epaint::ClippedPrimitive;
use vello::peniko::Color;
use vello::{AaConfig, AaSupport, RenderParams, Renderer, RendererOptions, Scene};

use crate::gpu::{Gpu, GpuError};

/// What egui produced for one frame, ready to paint.
pub struct EguiFrame {
    pub primitives: Vec<ClippedPrimitive>,
    pub textures_delta: egui::TexturesDelta,
    pub pixels_per_point: f32,
}

impl EguiFrame {
    /// Nothing from egui (scene only).
    pub fn empty() -> Self {
        Self { primitives: Vec::new(), textures_delta: egui::TexturesDelta::default(), pixels_per_point: 1.0 }
    }

    /// Tessellate the output of `ctx.run_ui` / `ctx.run`. Platform output stays with the caller.
    pub fn from_output(ctx: &egui::Context, output: egui::FullOutput) -> Self {
        let pixels_per_point = output.pixels_per_point;
        Self {
            primitives: ctx.tessellate(output.shapes, pixels_per_point),
            textures_delta: output.textures_delta,
            pixels_per_point,
        }
    }
}

/// Counters that tests read to prove nothing is reallocated or redone per frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompositorStats {
    /// Scene textures created. Grows only when the size changes.
    pub scene_textures_created: u64,
    /// Offscreen targets created.
    pub offscreen_targets_created: u64,
    /// Vello runs.
    pub vello_runs: u64,
    /// Composite passes (one submit each).
    pub composites: u64,
}

struct SceneTarget {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: [u32; 2],
    bind_group: wgpu::BindGroup,
}

struct Offscreen {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: [u32; 2],
    readback: wgpu::Buffer,
    padded_row: u32,
}

/// An RGBA8 image read back from the GPU. Rows are tightly packed, top row first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    /// The pixel at (x, y), or `None` outside the image.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        let p = self.rgba.get(i..i + 4)?;
        Some([p[0], p[1], p[2], p[3]])
    }
}

/// Composites a Vello scene and egui into a target of one fixed format.
pub struct Compositor {
    vello: Renderer,
    egui: egui_wgpu::Renderer,
    format: wgpu::TextureFormat,
    bind_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    target: Option<SceneTarget>,
    /// A scene has been rendered into `target` at least once.
    has_scene: bool,
    base: Color,
    offscreen: Option<Offscreen>,
    stats: CompositorStats,
}

const SCENE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

impl Compositor {
    /// `format` is the format of the surface (or offscreen target) the frames go to.
    pub fn new(gpu: &Gpu, format: wgpu::TextureFormat) -> Result<Self, GpuError> {
        let vello = Renderer::new(
            &gpu.device,
            RendererOptions {
                // Area AA only: fewer shader permutations, faster start (docs/stack.md).
                antialiasing_support: AaSupport::area_only(),
                ..RendererOptions::default()
            },
        )
        .map_err(|e| GpuError::Vello(e.to_string()))?;
        let egui = egui_wgpu::Renderer::new(&gpu.device, format, egui_wgpu::RendererOptions::default());
        let bind_layout = gpu.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blit layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let shader = gpu.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blit"),
            source: wgpu::ShaderSource::Wgsl(include_str!("blit.wgsl").into()),
        });
        let layout = gpu.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blit pipeline layout"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let pipeline = gpu.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blit"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        Ok(Self {
            vello,
            egui,
            format,
            bind_layout,
            pipeline,
            target: None,
            has_scene: false,
            base: Color::BLACK,
            offscreen: None,
            stats: CompositorStats::default(),
        })
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    pub fn stats(&self) -> CompositorStats {
        self.stats
    }

    /// Size of the Vello texture, if one exists.
    pub fn scene_size(&self) -> Option<[u32; 2]> {
        self.target.as_ref().map(|t| t.size)
    }

    /// The egui renderer, for apps that register their own egui textures.
    pub fn egui_renderer_mut(&mut self) -> &mut egui_wgpu::Renderer {
        &mut self.egui
    }

    /// Make sure the Vello texture has this size. Returns true when it was (re)created, in which
    /// case its content is empty until the next [`Self::render_scene`].
    pub fn ensure_scene_target(&mut self, gpu: &Gpu, size: [u32; 2]) -> bool {
        let size = [size[0].max(1), size[1].max(1)];
        if self.target.as_ref().is_some_and(|t| t.size == size) {
            return false;
        }
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vello scene"),
            size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SCENE_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit bind group"),
            layout: &self.bind_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }],
        });
        self.target = Some(SceneTarget { _texture: texture, view, size, bind_group });
        self.has_scene = false;
        self.stats.scene_textures_created += 1;
        true
    }

    /// Run Vello on `scene` into the scene texture (created or resized if needed). Vello submits
    /// its own command buffers.
    pub fn render_scene(&mut self, gpu: &Gpu, scene: &Scene, base: Color, size: [u32; 2]) -> Result<(), GpuError> {
        self.ensure_scene_target(gpu, size);
        let Some(target) = self.target.as_ref() else {
            return Err(GpuError::Render("no scene texture".into()));
        };
        self.vello
            .render_to_texture(
                &gpu.device,
                &gpu.queue,
                scene,
                &target.view,
                &RenderParams {
                    base_color: base,
                    width: target.size[0],
                    height: target.size[1],
                    antialiasing_method: AaConfig::Area,
                },
            )
            .map_err(|e| GpuError::Render(e.to_string()))?;
        self.has_scene = true;
        self.base = base;
        self.stats.vello_runs += 1;
        inc(Counter::GpuSubmits);
        Ok(())
    }

    /// Whether a scene has been rendered into the current texture.
    pub fn has_scene(&self) -> bool {
        self.has_scene
    }

    /// Record the blit and the egui pass for `target`. Returns the command buffers to submit,
    /// the encoder's own last. The caller submits them in ONE `queue.submit`.
    fn encode(
        &mut self,
        gpu: &Gpu,
        encoder: wgpu::CommandEncoder,
        frame: &EguiFrame,
        target: &wgpu::TextureView,
        size: [u32; 2],
        finish: impl FnOnce(&mut wgpu::CommandEncoder),
    ) -> Vec<wgpu::CommandBuffer> {
        let mut encoder = encoder;
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: size, pixels_per_point: frame.pixels_per_point };
        for (id, deltas) in &frame.textures_delta.set {
            for delta in deltas {
                self.egui.update_texture(&gpu.device, &gpu.queue, *id, delta);
            }
        }
        let mut buffers = self.egui.update_buffers(&gpu.device, &gpu.queue, &mut encoder, &frame.primitives, &screen);
        {
            let base = self.base.to_rgba8();
            let clear = wgpu::Color {
                r: f64::from(base.r) / 255.0,
                g: f64::from(base.g) / 255.0,
                b: f64::from(base.b) / 255.0,
                a: 1.0,
            };
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("composite"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime();
            if let (true, Some(scene)) = (self.has_scene, self.target.as_ref()) {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &scene.bind_group, &[]);
                pass.draw(0..3, 0..1);
            }
            self.egui.render(&mut pass, &frame.primitives, &screen);
        }
        finish(&mut encoder);
        buffers.push(encoder.finish());
        buffers
    }

    /// Free the textures egui is done with and empty the delta (egui debug-asserts on a dropped, non-empty one).
    fn finish_textures(&mut self, frame: &mut EguiFrame) {
        for id in &frame.textures_delta.free {
            self.egui.free_texture(id);
        }
        frame.textures_delta.clear();
    }

    /// Blit the last Vello texture to `target` and draw egui over it, in one submit.
    /// `size` is the size of `target` in physical pixels. The caller presents. The frame's texture
    /// updates are applied and cleared.
    pub fn composite(
        &mut self,
        gpu: &Gpu,
        frame: &mut EguiFrame,
        target: &wgpu::TextureView,
        size: [u32; 2],
    ) -> Result<(), GpuError> {
        let encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("composite") });
        let buffers = self.encode(gpu, encoder, frame, target, size, |_| {});
        gpu.queue.submit(buffers);
        self.finish_textures(frame);
        self.stats.composites += 1;
        inc(Counter::GpuSubmits);
        Ok(())
    }

    fn ensure_offscreen(&mut self, gpu: &Gpu, size: [u32; 2]) {
        let size = [size[0].max(1), size[1].max(1)];
        if self.offscreen.as_ref().is_some_and(|o| o.size == size) {
            return;
        }
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen target"),
            size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let padded_row = (size[0] * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("offscreen readback"),
            size: u64::from(padded_row) * u64::from(size[1]),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        self.offscreen = Some(Offscreen { texture, view, size, readback, padded_row });
        self.stats.offscreen_targets_created += 1;
    }

    /// Render a scene (and optionally egui on top) into an RGBA8 image: PNG export, pixel probes, tests.
    /// `scene`: when `Some`, Vello runs first at `size`; when `None` the last Vello texture is used (it must have the same size).
    /// Works on the web too, which is why it is async (the buffer map cannot block there).
    pub async fn render_to_image(
        &mut self,
        gpu: &Gpu,
        size: [u32; 2],
        scene: Option<(&Scene, Color)>,
        egui: Option<&mut EguiFrame>,
    ) -> Result<Image, GpuError> {
        let size = [size[0].max(1), size[1].max(1)];
        if let Some((scene, base)) = scene {
            self.render_scene(gpu, scene, base, size)?;
        }
        self.ensure_offscreen(gpu, size);
        let mut empty = EguiFrame::empty();
        let frame = egui.unwrap_or(&mut empty);
        let (view, texture_copy, buffer, padded_row) = {
            let Some(o) = self.offscreen.as_ref() else {
                return Err(GpuError::Render("no offscreen target".into()));
            };
            (o.view.clone(), o.texture.clone(), o.readback.clone(), o.padded_row)
        };
        let encoder =
            gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("offscreen composite") });
        let buffers = self.encode(gpu, encoder, &*frame, &view, size, |enc| {
            enc.copy_texture_to_buffer(
                texture_copy.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded_row),
                        rows_per_image: Some(size[1]),
                    },
                },
                wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
            );
        });
        gpu.queue.submit(buffers);
        self.finish_textures(frame);
        self.stats.composites += 1;
        inc(Counter::GpuSubmits);

        let swap_rb = matches!(self.format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
        read_buffer(gpu, &buffer, size, padded_row, swap_rb).await
    }

    /// Blocking form of [`Self::render_to_image`] for native tests and the CLI.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn render_to_image_blocking(
        &mut self,
        gpu: &Gpu,
        size: [u32; 2],
        scene: Option<(&Scene, Color)>,
        egui: Option<&mut EguiFrame>,
    ) -> Result<Image, GpuError> {
        pollster::block_on(self.render_to_image(gpu, size, scene, egui))
    }
}

/// A one-shot future completed from a wgpu callback.
type MapState = Arc<Mutex<(Option<Result<(), wgpu::BufferAsyncError>>, Option<Waker>)>>;

struct MapFuture {
    state: MapState,
}

impl Future for MapFuture {
    type Output = Result<(), wgpu::BufferAsyncError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(result) = guard.0.take() {
            return Poll::Ready(result);
        }
        guard.1 = Some(cx.waker().clone());
        Poll::Pending
    }
}

async fn read_buffer(
    gpu: &Gpu,
    buffer: &wgpu::Buffer,
    size: [u32; 2],
    padded_row: u32,
    swap_rb: bool,
) -> Result<Image, GpuError> {
    let state: MapState = Arc::new(Mutex::new((None, None)));
    let slot = state.clone();
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
        let mut guard = slot.lock().unwrap_or_else(PoisonError::into_inner);
        guard.0 = Some(result);
        if let Some(waker) = guard.1.take() {
            waker.wake();
        }
    });
    // Native: drive the callback now. On the web the browser runs it from the event loop.
    #[cfg(not(target_arch = "wasm32"))]
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| GpuError::Readback(e.to_string()))?;
    #[cfg(target_arch = "wasm32")]
    let _ = gpu;
    MapFuture { state }.await.map_err(|e| GpuError::Readback(e.to_string()))?;

    let mapped = buffer.slice(..).get_mapped_range().map_err(|e| GpuError::Readback(e.to_string()))?;
    let row_bytes = size[0] as usize * 4;
    let mut rgba = Vec::with_capacity(row_bytes * size[1] as usize);
    for row in mapped.chunks(padded_row as usize).take(size[1] as usize) {
        let Some(pixels) = row.get(..row_bytes) else {
            return Err(GpuError::Readback("short row".into()));
        };
        rgba.extend_from_slice(pixels);
    }
    drop(mapped);
    buffer.unmap();
    if swap_rb {
        for px in rgba.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
    }
    Ok(Image { width: size[0], height: size[1], rgba })
}
