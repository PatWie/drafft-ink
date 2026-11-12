//! Vello on wgpu, for a machine that has a GPU (Cargo feature `png-gpu`).
//!
//! The same renderer as the app, so the pixels are the app's own. The device is opened on the first picture, takes
//! whatever adapter wgpu finds (a real GPU, or a software Vulkan driver such as Mesa's lavapipe), and is kept for
//! the next picture. A machine with none gets `None` from [`GpuRaster::open`] and the caller draws on the CPU.

use std::sync::{Arc, Mutex, PoisonError};

use vello::wgpu;
use vello::{AaConfig, AaSupport, RenderParams, Renderer, RendererOptions};

use super::compose::Composed;

/// Most pixels a picture may have to be drawn on the GPU. Vello 0.11 gives no error when a scene needs more of its
/// working buffers than the device has: the picture comes back black. The intro document is fine at 15 million
/// pixels and a thousand shapes at 22 million, but 49 million came out black on a 48 GB card and on lavapipe alike.
/// The CPU has no such limit, so a larger picture is drawn there.
const MAX_GPU_PIXELS: u64 = 1 << 24;

/// Why the GPU did not make a picture.
#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    /// Vello could not draw the scene.
    #[error("Vello failed: {0}")]
    Render(#[from] vello::Error),
    /// The device did not finish its work.
    #[error("the device failed while waiting: {0}")]
    Poll(#[from] wgpu::PollError),
    /// The device dropped the request to read the pixels back.
    #[error("the device gave no answer to the read-back: {0}")]
    NoAnswer(#[from] std::sync::mpsc::RecvError),
    /// The pixels could not be mapped for reading.
    #[error("the pixels could not be read back: {0}")]
    Map(#[from] wgpu::BufferAsyncError),
    /// The mapped pixels could not be viewed.
    #[error("the pixels could not be viewed: {0}")]
    View(#[from] wgpu::MapRangeError),
    /// The device reported an error that no call returned.
    #[error("the device reported: {0}")]
    Device(wgpu::Error),
}

/// A wgpu device with a Vello renderer on it.
pub(super) struct GpuRaster {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer,
    /// Largest side of a texture the device allows.
    max_side: u32,
    /// The first error the device reported that no call returned (wgpu would panic on it by default).
    uncaptured: Arc<Mutex<Option<wgpu::Error>>>,
}

impl GpuRaster {
    /// Opens a device. `None` when the machine has no adapter, or none Vello can run on.
    pub(super) fn open() -> Option<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            display: None,
            // No GL: Vello needs compute shaders, and a GL adapter can sort first and then fail to open.
            backends: wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY),
            flags: wgpu::InstanceFlags::from_build_config().with_env(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            backend_options: wgpu::BackendOptions::from_env_or_default(),
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::from_env().unwrap_or(wgpu::PowerPreference::HighPerformance),
            ..wgpu::RequestAdapterOptions::default()
        }))
        .ok()?;
        log::debug!("png: drawing on {}", adapter.get_info().name);
        let max_side = adapter.limits().max_texture_dimension_2d;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("drafftink png"),
            // A big picture needs more than the default limits give: 8192 pixels on a side, and 128 MiB for Vello's
            // buffers, which it fills with nothing when they do not fit.
            required_limits: adapter.limits(),
            ..wgpu::DeviceDescriptor::default()
        }))
        .ok()?;
        let uncaptured = Arc::new(Mutex::new(None));
        let slot = Arc::clone(&uncaptured);
        device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
            slot.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert(error);
        }));
        let options = RendererOptions { antialiasing_support: AaSupport::area_only(), ..RendererOptions::default() };
        let renderer = Renderer::new(&device, options).ok()?;
        Some(Self { device, queue, renderer, max_side, uncaptured })
    }

    /// Whether a picture of `size` is drawn here: it fits the device and [`MAX_GPU_PIXELS`].
    pub(super) fn fits(&self, size: [u16; 2]) -> bool {
        let [width, height] = size.map(u32::from);
        width.max(height) <= self.max_side && u64::from(width) * u64::from(height) <= MAX_GPU_PIXELS
    }

    /// The scene as straight-alpha RGBA8, top row first.
    pub(super) fn rasterize(&mut self, composed: &Composed) -> Result<Vec<u8>, GpuError> {
        let [width, height] = composed.size.map(u32::from);
        let (device, queue) = (&self.device, &self.queue);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("png"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let params = RenderParams { base_color: composed.base, width, height, antialiasing_method: AaConfig::Area };
        self.renderer.render_to_texture(device, queue, &composed.scene, &view, &params)?;

        let padded_row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("png readback"),
            size: u64::from(padded_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("png copy") });
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        queue.submit([encoder.finish()]);
        let (sender, receiver) = std::sync::mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            // The receiver is gone only when this function has already failed.
            let _ = sender.send(result);
        });
        device.poll(wgpu::PollType::wait_indefinitely())?;
        receiver.recv()??;
        let mapped = buffer.slice(..).get_mapped_range()?;
        let row_bytes = width as usize * 4;
        let mut rgba = Vec::with_capacity(row_bytes * height as usize);
        for row in mapped.chunks_exact(padded_row as usize) {
            rgba.extend_from_slice(&row[..row_bytes]);
        }
        drop(mapped);
        buffer.unmap();
        if let Some(error) = self.uncaptured.lock().unwrap_or_else(PoisonError::into_inner).take() {
            return Err(GpuError::Device(error));
        }
        Ok(rgba)
    }
}
