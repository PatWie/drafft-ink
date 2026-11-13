//! What the renderer knows about itself, for the debug panel: static facts about the adapter, and live numbers that
//! are measured only while the panel is open.
//!
//! None of these types mention wgpu or WebGL, so the app (which must build for both back ends) can show them.
//! A number the machine cannot give is a variant that says so ([`GpuTime::Unsupported`]) or an `Option`, never a
//! zero.

use drafftink_profile::FrameReading;

use crate::backend::BackendKind;

/// The graphics API the adapter is driven through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphicsApi {
    /// Vulkan.
    Vulkan,
    /// Metal.
    Metal,
    /// Direct3D 12.
    Dx12,
    /// OpenGL or OpenGL ES through wgpu.
    OpenGl,
    /// WebGPU in the browser.
    WebGpu,
    /// WebGL2 in the browser (the compatibility renderer).
    WebGl2,
    /// Something wgpu has no name for here.
    Other,
}

impl GraphicsApi {
    /// The name the panel shows.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Vulkan => "Vulkan",
            Self::Metal => "Metal",
            Self::Dx12 => "DirectX 12",
            Self::OpenGl => "OpenGL",
            Self::WebGpu => "WebGPU",
            Self::WebGl2 => "WebGL2",
            Self::Other => "unknown",
        }
    }
}

/// What kind of device the adapter reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceClass {
    /// A discrete graphics card.
    Discrete,
    /// Graphics integrated with the processor.
    Integrated,
    /// A virtual GPU of a virtual machine.
    Virtual,
    /// A software rasteriser that runs on the processor.
    Cpu,
    /// The adapter does not say (the browser hides it).
    Unknown,
}

impl DeviceClass {
    /// The name the panel shows.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Discrete => "discrete GPU",
            Self::Integrated => "integrated GPU",
            Self::Virtual => "virtual GPU",
            Self::Cpu => "CPU (software)",
            Self::Unknown => "not reported",
        }
    }
}

/// Who draws the pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rasteriser {
    /// A graphics chip.
    Gpu,
    /// The processor, through a software driver such as llvmpipe, lavapipe or SwiftShader.
    Cpu,
}

impl Rasteriser {
    /// The name the panel shows.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Gpu => "GPU",
            Self::Cpu => "CPU (software driver)",
        }
    }
}

/// The facts about the renderer and its adapter. They never change while the device lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RendererInfo {
    /// Which renderer draws the canvas.
    pub kind: BackendKind,
    /// The adapter's name as the driver reports it (empty when the browser hides it).
    pub adapter: String,
    /// The API behind it.
    pub api: GraphicsApi,
    /// The kind of device.
    pub device: DeviceClass,
    /// The driver, as the driver names itself (empty when unknown).
    pub driver: String,
    /// Who draws.
    pub rasteriser: Rasteriser,
}

impl RendererInfo {
    /// The renderer's own name.
    pub const fn renderer_label(&self) -> &'static str {
        match self.kind {
            BackendKind::Vello => "Vello (compute shaders)",
            BackendKind::WebGl => "vello_gpu (WebGL2)",
        }
    }
}

/// The time the GPU spent on the composite pass of the last frame that was measured.
///
/// This is the pass that copies the Vello image and paints egui. Vello's own compute passes are not timed:
/// Vello 0.11 submits them itself and has no hook for timestamps outside its profiler feature.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum GpuTime {
    /// The device has no timestamp queries (software drivers, the browser, WebGL).
    #[default]
    Unsupported,
    /// Supported, and the first result has not come back yet.
    Pending,
    /// Milliseconds of GPU time.
    Measured(f32),
}

/// Memory the GPU allocator holds, in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuMemory {
    /// In use by buffers and textures.
    pub allocated: u64,
    /// Reserved from the driver, in use or not.
    pub reserved: u64,
}

/// The numbers the back end measures while the panel is open.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BackendStats {
    /// GPU time of the composite pass.
    pub gpu_time: GpuTime,
    /// Allocator totals. `None` when the back end cannot report them (the browser, WebGL, some drivers).
    pub memory: Option<GpuMemory>,
}

/// What the shell hands the app for one frame while the app asks for statistics.
#[derive(Clone, Debug, PartialEq)]
pub struct ShellStats {
    /// Frames per second and frame time.
    pub frames: FrameReading,
    /// GPU numbers.
    pub gpu: BackendStats,
}
