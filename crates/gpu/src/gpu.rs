//! The wgpu instance, adapter, device and queue, and the window surface.

use std::sync::{Arc, Mutex, PoisonError};

use thiserror::Error;

/// Why the GPU could not be set up.
#[derive(Debug, Error)]
pub enum GpuError {
    #[error("no GPU adapter is available ({0}). Drafft.ink needs Vulkan, Metal, DirectX 12 or WebGPU")]
    NoAdapter(String),
    #[error("the GPU refused to start ({0})")]
    Device(String),
    #[error("could not create a drawing surface for the window ({0})")]
    Surface(String),
    #[error("the vello renderer could not be created ({0})")]
    Vello(String),
    #[error("the surface supports no usable pixel format")]
    NoFormat,
    #[error("rendering failed ({0})")]
    Render(String),
    #[error("reading back the image failed ({0})")]
    Readback(String),
}

/// What kind of adapter this is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdapterKind {
    Discrete,
    Integrated,
    Virtual,
    /// Software rasteriser: llvmpipe, lavapipe, SwiftShader, WARP.
    Cpu,
    Other,
}

/// Facts about the adapter, for logs, bug reports and tests that must tell a real GPU from a CPU one.
#[derive(Clone, Debug)]
pub struct GpuInfo {
    pub name: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub backend: String,
    pub driver: String,
    pub driver_info: String,
    pub kind: AdapterKind,
    /// Largest texture side the device allows.
    pub max_texture_dimension_2d: u32,
}

impl GpuInfo {
    pub(crate) fn from_adapter(adapter: &wgpu::Adapter) -> Self {
        let info = adapter.get_info();
        let kind = match info.device_type {
            wgpu::DeviceType::DiscreteGpu => AdapterKind::Discrete,
            wgpu::DeviceType::IntegratedGpu => AdapterKind::Integrated,
            wgpu::DeviceType::VirtualGpu => AdapterKind::Virtual,
            wgpu::DeviceType::Cpu => AdapterKind::Cpu,
            wgpu::DeviceType::Other => AdapterKind::Other,
        };
        Self {
            name: info.name,
            vendor_id: info.vendor,
            device_id: info.device,
            backend: format!("{:?}", info.backend),
            driver: info.driver,
            driver_info: info.driver_info,
            kind,
            max_texture_dimension_2d: adapter.limits().max_texture_dimension_2d,
        }
    }

    /// True for llvmpipe, lavapipe, SwiftShader, WARP and any adapter that reports itself as a CPU.
    pub fn is_software(&self) -> bool {
        if self.kind == AdapterKind::Cpu {
            return true;
        }
        let name = self.name.to_ascii_lowercase();
        ["llvmpipe", "lavapipe", "swiftshader", "software", "warp", "basic render"].iter().any(|s| name.contains(s))
    }

    /// The vendor from the PCI vendor id, when it is a well-known one.
    pub fn vendor(&self) -> &'static str {
        match self.vendor_id {
            0x10de => "NVIDIA",
            0x1002 | 0x1022 => "AMD",
            0x8086 | 0x8087 => "Intel",
            0x106b => "Apple",
            0x5143 => "Qualcomm",
            0x13b5 => "ARM",
            0x1010 => "Imagination",
            0x1414 => "Microsoft",
            _ => "unknown",
        }
    }

    /// One line for logs: `NVIDIA L40S (Vulkan, Discrete)`.
    pub fn summary(&self) -> String {
        format!("{} ({}, {:?}, vendor {})", self.name, self.backend, self.kind, self.vendor())
    }
}

/// How to pick the adapter.
#[derive(Clone, Copy, Debug)]
pub struct GpuOptions {
    pub power_preference: wgpu::PowerPreference,
    /// Ask for a software adapter (tests on machines without a GPU). Not used by the app.
    pub force_fallback_adapter: bool,
}

impl Default for GpuOptions {
    fn default() -> Self {
        Self {
            power_preference: wgpu::PowerPreference::from_env().unwrap_or(wgpu::PowerPreference::HighPerformance),
            force_fallback_adapter: false,
        }
    }
}

/// Why the device is gone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LostInfo {
    pub destroyed: bool,
    pub message: String,
}

/// Create a wgpu instance for this target. WebGPU only in the browser (the `webgl` feature is off,
/// Vello needs compute shaders); Vulkan, Metal and DX12 elsewhere, or what `WGPU_BACKEND` says.
///
/// `display` is the window system connection from winit (`event_loop.owned_display_handle()`); `None` for headless use.
pub async fn create_instance(display: Option<Box<dyn wgpu::wgt::WgpuHasDisplayHandle>>) -> wgpu::Instance {
    let backends = if cfg!(target_arch = "wasm32") {
        wgpu::Backends::BROWSER_WEBGPU
    } else {
        // No GL: Vello needs compute shaders and storage textures, which GLES 3.x does not have, and a
        // GL adapter can sort first and then fail at device creation.
        wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY)
    };
    wgpu::util::new_instance_with_webgpu_detection(wgpu::InstanceDescriptor {
        backends,
        flags: wgpu::InstanceFlags::from_build_config().with_env(),
        backend_options: wgpu::BackendOptions::from_env_or_default(),
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        display,
    })
    .await
}

/// The device, queue and adapter facts.
pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    info: GpuInfo,
    lost: Arc<Mutex<Option<LostInfo>>>,
}

impl Gpu {
    /// Pick an adapter (compatible with `surface` when given) and open a device.
    pub async fn request(
        instance: wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
        options: &GpuOptions,
    ) -> Result<Self, GpuError> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: options.power_preference,
                compatible_surface: surface,
                force_fallback_adapter: options.force_fallback_adapter,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|e| GpuError::NoAdapter(e.to_string()))?;
        let info = GpuInfo::from_adapter(&adapter);
        log::info!("adapter: {}", info.summary());
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("drafftink device"),
                // Vello's target texture can be as large as the screen in physical pixels.
                required_limits: wgpu::Limits {
                    max_texture_dimension_2d: adapter.limits().max_texture_dimension_2d,
                    ..wgpu::Limits::default()
                },
                ..Default::default()
            })
            .await
            .map_err(|e| GpuError::Device(e.to_string()))?;
        let lost = Arc::new(Mutex::new(None));
        // wgpu panics on uncaptured errors by default. A browser or driver error must not take the
        // app down: log it, and treat an out-of-memory error as a lost device.
        let oom_slot = lost.clone();
        device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
            log::error!("GPU error: {error}");
            if matches!(error, wgpu::Error::OutOfMemory { .. }) {
                let mut guard = oom_slot.lock().unwrap_or_else(PoisonError::into_inner);
                guard.get_or_insert(LostInfo { destroyed: false, message: error.to_string() });
            }
        }));
        let slot = lost.clone();
        device.set_device_lost_callback(move |reason, message| {
            log::error!("GPU device lost ({reason:?}): {message}");
            let mut guard = slot.lock().unwrap_or_else(PoisonError::into_inner);
            *guard = Some(LostInfo { destroyed: matches!(reason, wgpu::DeviceLostReason::Destroyed), message });
        });
        Ok(Self { instance, adapter, device, queue, info, lost })
    }

    /// A device with no window, for offscreen rendering, export and tests. Blocks (native only).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn headless_blocking(options: &GpuOptions) -> Result<Self, GpuError> {
        pollster::block_on(async {
            let instance = create_instance(None).await;
            Self::request(instance, None, options).await
        })
    }

    pub fn info(&self) -> &GpuInfo {
        &self.info
    }

    /// `Some` once the device was lost (driver reset, GPU removed, `destroy()`).
    pub fn lost(&self) -> Option<LostInfo> {
        self.lost.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Destroy the device, as a driver reset would, and mark it lost at once. For tests of the
    /// recovery path; the app never calls it.
    pub fn destroy_for_test(&self) {
        self.device.destroy();
        let mut guard = self.lost.lock().unwrap_or_else(PoisonError::into_inner);
        guard.get_or_insert(LostInfo { destroyed: true, message: "destroyed by test".into() });
    }

    /// Open a new device on the same instance, on a fresh adapter request (a WebGPU adapter can
    /// give out one device only). Call after [`Self::lost`]; every resource created from the old
    /// device is dead.
    pub async fn recover(&self, surface: Option<&wgpu::Surface<'_>>, options: &GpuOptions) -> Result<Self, GpuError> {
        Self::request(self.instance.clone(), surface, options).await
    }
}

impl std::fmt::Debug for Gpu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gpu").field("info", &self.info).field("lost", &self.lost()).finish()
    }
}

/// `DRAFFTINK_PRESENT_MODE` (`fifo`, `mailbox`, `immediate`, `auto_vsync`, `auto_no_vsync`) for measurements: some
/// display servers (Xvfb) make `Fifo` present expensive in a way a real display does not. Not for users.
#[cfg(not(target_arch = "wasm32"))]
fn present_mode_from_env() -> Option<wgpu::PresentMode> {
    let value = std::env::var("DRAFFTINK_PRESENT_MODE").ok()?;
    Some(match value.to_ascii_lowercase().as_str() {
        "fifo" => wgpu::PresentMode::Fifo,
        "mailbox" => wgpu::PresentMode::Mailbox,
        "immediate" => wgpu::PresentMode::Immediate,
        "auto_vsync" => wgpu::PresentMode::AutoVsync,
        "auto_no_vsync" => wgpu::PresentMode::AutoNoVsync,
        _ => return None,
    })
}

#[cfg(target_arch = "wasm32")]
fn present_mode_from_env() -> Option<wgpu::PresentMode> {
    None
}

/// The present mode for a surface that supports `supported`. `requested` (the measurement override) wins when the
/// surface supports it. Otherwise `Fifo`, never the first mode the driver lists: that order is the driver's own,
/// so it could be `Immediate` (tearing, frames as fast as we ask) or `Mailbox` (an extra image and GPU work for
/// frames nobody sees). The app paces its own frames (`RepaintState::set_min_interval`), and `Fifo` is the mode
/// every Vulkan, Metal and DX12 surface has (the browser's canvas supports no other, `docs/performance.md` 7.11).
fn choose_present_mode(supported: &[wgpu::PresentMode], requested: Option<wgpu::PresentMode>) -> wgpu::PresentMode {
    use wgpu::PresentMode::Fifo;
    requested.filter(|m| supported.contains(m)).unwrap_or_else(|| match supported {
        [first, ..] if !supported.contains(&Fifo) => *first,
        _ => Fifo,
    })
}

/// A window surface and its configuration.
pub struct PresentSurface {
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
}

impl PresentSurface {
    /// Create the surface for a window. Do this before [`Gpu::request`] so the adapter can be chosen for it.
    pub fn create(
        instance: &wgpu::Instance,
        target: impl Into<wgpu::SurfaceTarget<'static>>,
    ) -> Result<wgpu::Surface<'static>, GpuError> {
        instance.create_surface(target).map_err(|e| GpuError::Surface(e.to_string()))
    }

    /// Configure `surface` for `gpu` at `size` physical pixels.
    pub fn configure(gpu: &Gpu, surface: wgpu::Surface<'static>, size: [u32; 2]) -> Result<Self, GpuError> {
        let caps = surface.get_capabilities(&gpu.adapter);
        // A non-sRGB 8-bit format: egui blends in gamma space and Vello writes sRGB-encoded values.
        let format = egui_wgpu::preferred_framebuffer_format(&caps.formats).map_err(|_| GpuError::NoFormat)?;
        let mut config = surface
            .get_default_config(&gpu.adapter, size[0].max(1), size[1].max(1))
            .ok_or_else(|| GpuError::Surface("the adapter cannot present to this surface".into()))?;
        config.format = format;
        config.view_formats = vec![format];
        config.present_mode = choose_present_mode(&caps.present_modes, present_mode_from_env());
        log::info!("present mode: {:?} (supported: {:?})", config.present_mode, caps.present_modes);
        surface.configure(&gpu.device, &config);
        Ok(Self { surface, config })
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    pub fn size(&self) -> [u32; 2] {
        [self.config.width, self.config.height]
    }

    /// Reconfigure for a new size. Returns whether anything changed. A zero side is ignored.
    pub fn resize(&mut self, gpu: &Gpu, size: [u32; 2]) -> bool {
        if size[0] == 0 || size[1] == 0 || size == self.size() {
            return false;
        }
        self.config.width = size[0];
        self.config.height = size[1];
        self.surface.configure(&gpu.device, &self.config);
        true
    }

    /// Reconfigure at the current size (after `Outdated`, `Lost` or `Suboptimal`).
    pub fn reconfigure(&self, gpu: &Gpu) {
        self.surface.configure(&gpu.device, &self.config);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str, vendor_id: u32, kind: AdapterKind) -> GpuInfo {
        GpuInfo {
            name: name.into(),
            vendor_id,
            device_id: 0,
            backend: "Vulkan".into(),
            driver: String::new(),
            driver_info: String::new(),
            kind,
            max_texture_dimension_2d: 8192,
        }
    }

    #[test]
    fn software_adapters_are_recognised_by_kind_and_by_name() {
        assert!(info("llvmpipe (LLVM 20.1.2, 256 bits)", 0x10005, AdapterKind::Cpu).is_software());
        assert!(info("llvmpipe", 0, AdapterKind::Other).is_software());
        assert!(info("SwiftShader Device (Subzero)", 0x1ae0, AdapterKind::Other).is_software());
        assert!(info("Microsoft Basic Render Driver", 0x1414, AdapterKind::Other).is_software());
        assert!(!info("NVIDIA L40S", 0x10de, AdapterKind::Discrete).is_software());
        assert!(!info("Apple M2", 0x106b, AdapterKind::Integrated).is_software());
    }

    #[test]
    fn present_mode_is_fifo_whatever_the_driver_lists_first() {
        use wgpu::PresentMode::{Fifo, Immediate, Mailbox};
        assert_eq!(choose_present_mode(&[Immediate, Mailbox, Fifo], None), Fifo);
        assert_eq!(choose_present_mode(&[Mailbox, Fifo], None), Fifo);
        assert_eq!(choose_present_mode(&[Fifo, Immediate], None), Fifo);
    }

    #[test]
    fn present_mode_override_applies_only_when_supported() {
        use wgpu::PresentMode::{Fifo, Immediate, Mailbox};
        assert_eq!(choose_present_mode(&[Fifo, Immediate], Some(Immediate)), Immediate);
        assert_eq!(choose_present_mode(&[Fifo, Immediate], Some(Mailbox)), Fifo);
        // A surface that lists no Fifo (the spec says none does) is configured with what it has.
        assert_eq!(choose_present_mode(&[Immediate], None), Immediate);
    }

    #[test]
    fn vendor_from_pci_id() {
        assert_eq!(info("x", 0x10de, AdapterKind::Discrete).vendor(), "NVIDIA");
        assert_eq!(info("x", 0x1002, AdapterKind::Discrete).vendor(), "AMD");
        assert_eq!(info("x", 0x8086, AdapterKind::Integrated).vendor(), "Intel");
        assert_eq!(info("x", 0x1234, AdapterKind::Other).vendor(), "unknown");
        assert!(info("NVIDIA L40S", 0x10de, AdapterKind::Discrete).summary().contains("NVIDIA"));
    }
}
