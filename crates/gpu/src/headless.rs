//! A shell without a window: frames of a [`ShellApp`] run by hand and read back as an image.
//!
//! The winit shell does three things per frame: it runs egui and the app, runs Vello on the scene the app
//! made, and composites the scene and egui into the surface. [`Headless`] does the same with an offscreen target,
//! so the picture is the app as a person sees it (the UI over the canvas), not the UI alone. It serves
//! `cargo xtask media`, which renders the README screenshots, and tests that need pixels of the whole window.
//!
//! No event loop runs here: the app's [`Waker`] is [detached](Waker::detached), the clock of egui is moved by hand
//! ([`Headless::frame`] advances it), and nothing is presented.

use drafftink_profile::{FrameMeter, now_ns};
use egui::{Event, Pos2, RawInput, Rect, ViewportId, ViewportInfo};
use vello::Scene;
use vello::peniko::Color;

use crate::RedrawReason;
use crate::app::{EventCx, FrameCx, FrameOutput, Outbox, SetupCx, ShellApp, ViewInfo, Waker};
use crate::backend::BackendKind;
use crate::compositor::Compositor;
use crate::error::GpuError;
use crate::events::{Handled, PixelSample, Repaint, ShellEvent};
use crate::frame::{EguiFrame, Image};
use crate::gpu::{Gpu, GpuOptions};
use crate::readback::pixel_area;
use crate::stats::{BackendStats, GpuMemory, RendererInfo, ShellStats};

/// Seconds the clock of egui advances per frame. Long enough that fades and tooltips settle within a few frames.
const FRAME_SECONDS: f64 = 0.1;

/// The window a [`Headless`] shell pretends to have.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeadlessOptions {
    /// Size in egui points (logical pixels).
    pub size_points: [f32; 2],
    /// Physical pixels per point. The image is `size_points * scale` pixels.
    pub scale: f32,
    /// The device is used with a finger: the app starts in its touch layout.
    pub touch_first: bool,
}

impl HeadlessOptions {
    /// The image size in physical pixels.
    pub fn size_px(&self) -> [u32; 2] {
        // Rounded and clamped to at least one pixel: both conversions are exact for any real window.
        self.size_points.map(|p| ((p * self.scale).round().max(1.0)) as u32)
    }
}

/// An app, a GPU device without a window, and the state of the frames run so far.
pub struct Headless<A: ShellApp> {
    app: A,
    gpu: Gpu,
    compositor: Compositor,
    ctx: egui::Context,
    options: HeadlessOptions,
    scene: Scene,
    scene_base: Color,
    scene_drawn: bool,
    time: f64,
    resized: bool,
    reason: RedrawReason,
    /// Texture uploads and frees of the frames not painted yet; egui sends each of them once.
    textures: egui::TexturesDelta,
    /// The shapes of the last frame, painted by [`Self::screenshot`].
    last: Option<egui::FullOutput>,
    waker: Waker,
    renderer: RendererInfo,
    /// The app asked for statistics (its debug panel is open) and the frames are being timed.
    stats_on: bool,
    meter: FrameMeter,
}

impl<A: ShellApp> Drop for Headless<A> {
    /// egui panics when it drops texture uploads nobody applied, which is what a shell dropped after frames without
    /// a [`Headless::screenshot`] holds. Nothing will be painted any more, so they are discarded.
    fn drop(&mut self) {
        self.textures.clear();
        if let Some(last) = self.last.as_mut() {
            last.textures_delta.clear();
        }
    }
}

impl<A: ShellApp> Headless<A> {
    /// Opens a device (`VK_ICD_FILENAMES` or `WGPU_BACKEND` choose software rendering when there is no GPU) and
    /// starts `app` in a window of `options.size_points`.
    pub fn new(mut app: A, options: HeadlessOptions) -> Result<Self, GpuError> {
        let gpu = Gpu::headless_blocking(&GpuOptions::default())?;
        // The format the winit shell picks: 8 bits per channel, not sRGB (`PresentSurface::configure`).
        let compositor = Compositor::new(&gpu, wgpu::TextureFormat::Rgba8Unorm)?;
        let ctx = egui::Context::default();
        let waker = Waker::detached();
        let renderer = gpu.info().renderer_info();
        let view = ViewInfo { size_px: options.size_px(), scale: options.scale };
        app.setup(&mut SetupCx {
            ctx: &ctx,
            backend: BackendKind::Vello,
            waker: waker.clone(),
            view,
            touch_first: options.touch_first,
        });
        Ok(Self {
            app,
            gpu,
            compositor,
            ctx,
            options,
            scene: Scene::new(),
            scene_base: Color::WHITE,
            scene_drawn: false,
            time: 0.0,
            resized: true,
            reason: RedrawReason::Startup,
            textures: egui::TexturesDelta::default(),
            last: None,
            waker,
            renderer,
            stats_on: false,
            meter: FrameMeter::new(),
        })
    }

    /// The app, to run commands on its session between frames.
    pub fn app_mut(&mut self) -> &mut A {
        &mut self.app
    }

    /// The GPU the frames are drawn on.
    pub fn gpu(&self) -> &Gpu {
        &self.gpu
    }

    /// Gives `event` to the app as the shell does, and answers the pixel reads it asks for with the colour of the canvas
    /// as the last frame drew it (the answer is delivered at once, as a native shell does within the next turn of its
    /// loop). Returns what the app answered to `event` and the earliest repaint of that and of the answers.
    pub fn event(&mut self, event: ShellEvent) -> (Repaint, Handled) {
        let mut queue = std::collections::VecDeque::from([event]);
        let (mut repaint, mut handled) = (Repaint::Idle, None);
        while let Some(event) = queue.pop_front() {
            let mut out = Outbox::default();
            let (again, taken) = {
                let mut cx = EventCx {
                    ctx: &self.ctx,
                    backend: BackendKind::Vello,
                    waker: &self.waker,
                    view: ViewInfo { size_px: self.options.size_px(), scale: self.options.scale },
                    modifiers: egui::Modifiers::NONE,
                    text_input_on: false,
                    out: &mut out,
                };
                self.app.event(&mut cx, event)
            };
            repaint = repaint.min(again);
            handled.get_or_insert(taken);
            for (at, purpose) in out.samples {
                let rgba = self.read_pixel(at);
                queue.push_back(ShellEvent::Pixel(PixelSample { at, purpose, rgba }));
            }
        }
        (repaint, handled.unwrap_or_default())
    }

    /// The colour of the canvas at `at` (points) as the last frame drew it, without the interface over it.
    fn read_pixel(&mut self, at: Pos2) -> Option<[u8; 4]> {
        let area = pixel_area(at, self.options.scale, self.options.size_px())?;
        if self.scene_drawn {
            self.compositor.render_scene(&self.gpu, &self.scene, self.scene_base, self.options.size_px()).ok()?;
        }
        let read = self.compositor.start_scene_read(&self.gpu, area).ok()?;
        pollster::block_on(read.image()).ok()?.pixel(0, 0)
    }

    /// Runs one frame with `events` as the input of that frame.
    pub fn frame(&mut self, events: Vec<Event>) -> FrameOutput {
        let [w, h] = self.options.size_points;
        let viewport = ViewportInfo { native_pixels_per_point: Some(self.options.scale), ..ViewportInfo::default() };
        let mut raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(w, h))),
            time: Some(self.time),
            events,
            focused: true,
            ..RawInput::default()
        };
        raw.viewports.insert(ViewportId::ROOT, viewport);
        self.time += FRAME_SECONDS;

        let mut out = Outbox::default();
        let mut scene_dirty = false;
        let mut frame_out = FrameOutput::default();
        let view = ViewInfo { size_px: self.options.size_px(), scale: self.options.scale };
        let started = now_ns();
        let stats = self.stats_on.then(|| ShellStats { frames: self.meter.reading(started), gpu: self.gpu_stats() });
        let mut output = self.ctx.run_ui(raw, |ui| {
            let mut cx = FrameCx {
                backend: BackendKind::Vello,
                waker: &self.waker,
                view,
                renderer: &self.renderer,
                stats: stats.as_ref(),
                reason: self.reason,
                resized: self.resized,
                scene: &mut self.scene,
                scene_dirty: &mut scene_dirty,
                scene_base: &mut self.scene_base,
                out: &mut out,
            };
            frame_out = self.app.frame(ui, &mut cx);
        });
        if frame_out.stats {
            self.meter.record(started, now_ns().saturating_sub(started));
        } else {
            self.meter.clear();
        }
        self.stats_on = frame_out.stats;
        self.scene_drawn |= scene_dirty;
        self.resized = false;
        self.reason = RedrawReason::Timer;
        self.textures.append(std::mem::take(&mut output.textures_delta));
        self.last = Some(output);
        frame_out
    }

    /// The GPU numbers of a shell without a window: the allocator totals. The composite pass is not timed here: the
    /// picture is painted only by [`Self::screenshot`].
    fn gpu_stats(&self) -> BackendStats {
        let memory = self
            .gpu
            .device
            .generate_allocator_report()
            .map(|report| GpuMemory { allocated: report.total_allocated_bytes, reserved: report.total_reserved_bytes });
        BackendStats { memory, ..BackendStats::default() }
    }

    /// Runs `frames` frames with no input, so animations finish and the app finishes work it started (fonts,
    /// images, layout).
    pub fn settle(&mut self, frames: u32) {
        for _ in 0..frames {
            self.frame(Vec::new());
        }
    }

    /// Paints the last frame, the canvas under the UI, and reads the pixels back. Call [`Self::frame`] first.
    pub fn screenshot(&mut self) -> Result<Image, GpuError> {
        let output = self.last.take().ok_or_else(|| GpuError::Render("no frame has run".into()))?;
        // Every upload and free since the last picture, in order (`TexturesDelta::append`).
        let mut frame = EguiFrame {
            primitives: self.ctx.tessellate(output.shapes, output.pixels_per_point),
            textures_delta: std::mem::take(&mut self.textures),
            pixels_per_point: output.pixels_per_point,
        };
        let scene = self.scene_drawn.then_some((&self.scene, self.scene_base));
        self.compositor.render_to_image_blocking(&self.gpu, self.options.size_px(), scene, Some(&mut frame))
    }
}
