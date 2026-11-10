//! The demo app shared by the native and the web example: a Vello scene of rectangles with pan (drag),
//! zoom (wheel, pinch), a hover highlight that is decided by the app, and an egui window with a button.
//!
//! It exists to show the contract and to be measured: with nothing changing it must draw 0 frames,
//! and moving the pointer over empty canvas must draw 0 frames.

use drafftink_gpu::{
    EventCx, FrameCx, FrameOutput, Handled, PointerPhase, RedrawReason, Repaint, SetupCx, ShellApp, ShellEvent,
};
use vello::kurbo::{Affine, Rect, RoundedRect, Stroke, Vec2};
use vello::peniko::{Color, Fill};

const COLS: usize = 40;
const ROWS: usize = 25;
const CELL: f64 = 90.0;

pub struct Demo {
    /// World to screen: scale then translate, in egui points.
    pan: Vec2,
    zoom: f64,
    hover: Option<usize>,
    drag_from: Option<egui::Pos2>,
    dirty: bool,
    pub clicks: u32,
    /// The app asks for a repaint every N ms when set (test hook for `WaitUntil`).
    pub tick_ms: Option<u64>,
    last_reason: RedrawReason,
}

impl Demo {
    pub fn new() -> Self {
        Self {
            pan: Vec2::new(40.0, 40.0),
            zoom: 1.0,
            hover: None,
            drag_from: None,
            dirty: true,
            clicks: 0,
            tick_ms: None,
            last_reason: RedrawReason::Startup,
        }
    }

    fn cell_rect(i: usize) -> Rect {
        let (c, r) = ((i % COLS) as f64, (i / COLS) as f64);
        Rect::new(c * CELL, r * CELL, c * CELL + CELL - 20.0, r * CELL + CELL - 20.0)
    }

    fn hit(&self, pos: egui::Pos2) -> Option<usize> {
        let world = ((f64::from(pos.x) - self.pan.x) / self.zoom, (f64::from(pos.y) - self.pan.y) / self.zoom);
        let (c, r) = ((world.0 / CELL).floor(), (world.1 / CELL).floor());
        if c < 0.0 || r < 0.0 || c >= COLS as f64 || r >= ROWS as f64 {
            return None;
        }
        let i = r as usize * COLS + c as usize;
        let p = vello::kurbo::Point::new(world.0, world.1);
        Self::cell_rect(i).contains(p).then_some(i)
    }
}

impl ShellApp for Demo {
    fn setup(&mut self, _cx: &mut SetupCx<'_>) {
        self.dirty = true;
    }

    fn event(&mut self, _cx: &mut EventCx<'_>, event: ShellEvent) -> (Repaint, Handled) {
        let repaint = match event {
            ShellEvent::Pointer(s) => match s.phase {
                PointerPhase::Hover => {
                    let hover = self.hit(s.point.pos);
                    if hover == self.hover {
                        Repaint::Idle
                    } else {
                        self.hover = hover;
                        self.dirty = true;
                        Repaint::Now(RedrawReason::HoverChange)
                    }
                }
                PointerPhase::Down => {
                    self.drag_from = Some(s.point.pos);
                    Repaint::Idle
                }
                PointerPhase::Move => match self.drag_from {
                    Some(from) => {
                        // Coalesced points give every intermediate position; the sum of the steps is the same as one step.
                        let d = s.point.pos - from;
                        self.pan += Vec2::new(f64::from(d.x), f64::from(d.y));
                        self.drag_from = Some(s.point.pos);
                        self.dirty = true;
                        Repaint::Now(RedrawReason::Gesture)
                    }
                    None => Repaint::Idle,
                },
                PointerPhase::Up | PointerPhase::Cancel => {
                    self.drag_from = None;
                    Repaint::Idle
                }
                PointerPhase::Leave => {
                    if self.hover.take().is_some() {
                        self.dirty = true;
                        Repaint::Now(RedrawReason::HoverChange)
                    } else {
                        Repaint::Idle
                    }
                }
            },
            ShellEvent::Wake(reason) => Repaint::Now(reason),
            ShellEvent::GpuRecreated => {
                self.dirty = true;
                Repaint::Now(RedrawReason::Surface)
            }
            ShellEvent::Paste(c) => {
                log::info!("paste: text={:?} images={}", c.text, c.images.len());
                Repaint::Idle
            }
            ShellEvent::FilesDropped(files) => {
                for f in &files {
                    log::info!("dropped {} ({}, {} bytes)", f.name, f.mime, f.bytes.len());
                }
                Repaint::Idle
            }
            _ => Repaint::Idle,
        };
        (repaint, Handled::No)
    }

    fn frame(&mut self, ui: &mut egui::Ui, cx: &mut FrameCx<'_>) -> FrameOutput {
        self.last_reason = cx.reason;
        let ctx = ui.ctx().clone();
        let canvas = ctx.content_rect();

        if ctx.input(|i| i.key_pressed(egui::Key::F9)) {
            cx.debug_lose_device();
        }

        // Wheel and pinch zoom around the pointer, but not while the pointer is over the egui window.
        if !ctx.egui_wants_pointer_input() {
            let (scroll, pinch, pos) = ctx.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta(), i.pointer.hover_pos()));
            let factor = f64::from(pinch) * (f64::from(scroll) * 0.002).exp();
            if (factor - 1.0).abs() > 1e-9 {
                let anchor = pos.unwrap_or(canvas.center());
                let a = Vec2::new(f64::from(anchor.x), f64::from(anchor.y));
                let new_zoom = (self.zoom * factor).clamp(0.05, 20.0);
                let k = new_zoom / self.zoom;
                self.pan = a - (a - self.pan) * k;
                self.zoom = new_zoom;
                self.dirty = true;
            }
        }

        egui::Window::new("drafftink-gpu demo").default_pos([16.0, 16.0]).show(&ctx, |ui| {
            if ui.button("Click me").clicked() {
                self.clicks += 1;
            }
            ui.label(format!("clicks: {}", self.clicks));
            ui.label(format!("frames drawn: {}", drafftink_gpu::frames_drawn()));
            ui.label(format!("vello runs: {}", drafftink_gpu::vello_runs()));
            ui.label(format!("last redraw reason: {}", self.last_reason.name()));
            ui.label(format!("zoom {:.2}", self.zoom));
            if ui.button("Lose GPU device").clicked() {
                cx.debug_lose_device();
            }
        });

        if self.dirty {
            self.dirty = false;
            let ppp = f64::from(cx.view.scale);
            let view = Affine::scale(ppp) * Affine::translate(self.pan) * Affine::scale(self.zoom);
            let size = cx.view.size_px;
            let visible =
                view.inverse().transform_rect_bbox(Rect::new(0.0, 0.0, f64::from(size[0]), f64::from(size[1])));
            let hover = self.hover;
            let scene = cx.scene_mut(Color::from_rgb8(0x1e, 0x1e, 0x24));
            for i in 0..COLS * ROWS {
                let r = Self::cell_rect(i);
                if r.x1 < visible.x0 || r.x0 > visible.x1 || r.y1 < visible.y0 || r.y0 > visible.y1 {
                    continue; // culled: Vello resolves everything it is given
                }
                let hue = (i * 37 % 255) as u8;
                let fill = if Some(i) == hover {
                    Color::from_rgb8(255, 220, 90)
                } else {
                    Color::from_rgb8(60 + hue / 2, 110, 255 - hue / 2)
                };
                let shape = RoundedRect::from_rect(r, 10.0);
                scene.fill(Fill::NonZero, view, fill, None, &shape);
                scene.stroke(&Stroke::new(2.0), view, Color::WHITE, None, &shape);
            }
        }

        let repaint = match self.tick_ms {
            Some(ms) => Repaint::After(std::time::Duration::from_millis(ms), RedrawReason::Timer),
            None => Repaint::Idle,
        };
        FrameOutput { repaint, canvas_rect: Some(canvas), ..FrameOutput::default() }
    }
}
