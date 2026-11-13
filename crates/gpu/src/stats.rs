//! What the renderer knows about itself, for the debug panel: static facts about the adapter, and live numbers that
//! are measured only while the panel is open.
//!
//! None of these types mention wgpu or WebGL, so the app (which must build for both back ends) can show them.
//! A number the machine cannot give is a variant that says so ([`GpuTime::Unsupported`]) or an `Option`, never a
//! zero.

use drafftink_profile::{FrameCost, FrameReading, Stage, now_ns};

use crate::app::AppTimes;
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

/// A stopwatch that reads the clock only when it was asked to measure: a shell that nobody watches times nothing.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Stopwatch(Option<u64>);

impl Stopwatch {
    /// Starts now when `on`, and is a stopped watch (no clock read, ever) otherwise.
    pub(crate) fn start(on: bool) -> Self {
        Self(on.then(now_ns))
    }

    /// Nanoseconds since the start, or 0 for a watch that was never started.
    pub(crate) fn elapsed_ns(self) -> u64 {
        self.0.map_or(0, |start| now_ns().saturating_sub(start))
    }
}

/// Where one frame's time went, as the shell ran it: the parts the shell times itself. A part the shell does not run
/// (a shell without a window presents nothing) is `None`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FrameMarks {
    /// The egui pass with the app's frame in it, panel and scene build included.
    pub ui_ns: u64,
    /// Handing the scene to the GPU renderer.
    pub render_ns: Option<u64>,
    /// Tessellating egui, compositing and presenting.
    pub present_ns: Option<u64>,
    /// What the shell itself spent on the panel, outside the pass: reading its own meter and the GPU numbers.
    pub panel_ns: u64,
}

impl FrameMarks {
    /// The cost of a frame that took `gross_ns` in all, with the debug panel's share named and the stages timed.
    ///
    /// The panel is taken out of the stage it ran in (its drawing out of the UI pass, its tessellation out of the
    /// present), so the stages still add up to the frame without the panel. A shell that does not tessellate in its
    /// frame (`present_ns` is `None`) has no tessellation of the panel to take out.
    pub(crate) fn cost(&self, gross_ns: u64, app: AppTimes) -> FrameCost {
        let tessellate_ns = if self.present_ns.is_some() { app.panel_tessellate_ns } else { 0 };
        let mut cost = FrameCost::new(gross_ns)
            .with_panel(self.panel_ns + app.panel_ns + tessellate_ns)
            .with_stage(Stage::Ui, self.ui_ns.saturating_sub(app.panel_ns + app.scene_ns))
            .with_stage(Stage::Scene, app.scene_ns);
        if let Some(render_ns) = self.render_ns {
            cost = cost.with_stage(Stage::Render, render_ns);
        }
        if let Some(present_ns) = self.present_ns {
            cost = cost.with_stage(Stage::Present, present_ns.saturating_sub(tessellate_ns));
        }
        cost
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    fn app(panel: u64, tessellate: u64, scene: u64) -> AppTimes {
        AppTimes { panel_ns: panel * MS, panel_tessellate_ns: tessellate * MS, scene_ns: scene * MS }
    }

    fn stage(cost: &FrameCost, stage: Stage) -> Option<u64> {
        cost.stages.get(stage).map(|ns| ns / MS)
    }

    #[test]
    fn the_panel_comes_out_of_the_stage_it_ran_in() {
        let marks = FrameMarks { ui_ns: 10 * MS, render_ns: Some(2 * MS), present_ns: Some(5 * MS), panel_ns: MS };
        let cost = marks.cost(20 * MS, app(3, 1, 4));
        assert_eq!(cost.panel_ns, 5 * MS, "1 outside the pass, 3 inside it, 1 of the tessellation");
        assert_eq!(stage(&cost, Stage::Ui), Some(3), "10 less the panel's 3 and the scene's 4");
        assert_eq!(stage(&cost, Stage::Scene), Some(4));
        assert_eq!(stage(&cost, Stage::Render), Some(2));
        assert_eq!(stage(&cost, Stage::Present), Some(4), "5 less the panel's tessellation");
        assert_eq!(cost.counted_ns(), 15 * MS);
    }

    #[test]
    fn a_shell_that_presents_nothing_has_no_present_stage_and_no_tessellation_to_take_out() {
        let marks = FrameMarks { ui_ns: 10 * MS, ..FrameMarks::default() };
        let cost = marks.cost(10 * MS, app(3, 1, 4));
        assert_eq!(cost.panel_ns, 3 * MS);
        assert_eq!(stage(&cost, Stage::Present), None, "not timed is not 0");
        assert_eq!(stage(&cost, Stage::Render), None);
        assert_eq!(cost.counted_ns(), 7 * MS);
    }

    #[test]
    fn times_that_do_not_fit_leave_zero_and_not_a_wrapped_number() {
        let marks = FrameMarks { ui_ns: MS, render_ns: None, present_ns: Some(MS), panel_ns: 0 };
        let cost = marks.cost(2 * MS, app(9, 9, 9));
        assert_eq!(stage(&cost, Stage::Ui), Some(0));
        assert_eq!(stage(&cost, Stage::Present), Some(0));
        assert_eq!(cost.counted_ns(), 0);
    }
}
