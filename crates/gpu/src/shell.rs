//! The winit shell: window, event loop, repaint discipline, egui and the compositor.
//!
//! Event flow and rules are in `docs/shell.md`. In short: the loop sleeps in `ControlFlow::Wait`
//! (or `WaitUntil` the earliest deadline) and `Window::request_redraw` is called only through
//! [`RepaintState`], so every frame has a recorded reason. Hover moves over the canvas go to the
//! app and draw nothing unless the app answers with a repaint.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use drafftink_profile::{Counter, Phase, RedrawReason, begin_frame, end_frame, in_phase, inc};
use thiserror::Error;
use vello::Scene;
use vello::peniko::Color;
use web_time::Instant;
use winit::application::ApplicationHandler;
#[cfg(not(target_arch = "wasm32"))]
use winit::event::ElementState;
use winit::event::{Ime, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy, OwnedDisplayHandle};
use winit::window::{Window, WindowId};

use crate::app::{EventCx, FrameCx, FrameOutput, Outbox, SetupCx, ShellApp, ViewInfo, Waker};
use crate::backend::{Backend, Presented};
use crate::error::GpuError;
use crate::events::{ClipboardContent, Handled, Repaint, ShellEvent, WheelInput};
#[cfg(not(target_arch = "wasm32"))]
use crate::events::{PointerKind, PointerPhase, PointerPoint, PointerSample, TextInput};
use crate::frame::EguiFrame;
use crate::host::HostEvent;
#[cfg(not(target_arch = "wasm32"))]
use crate::host::now_ms;
use crate::repaint::{HeldHover, RepaintState, coalesce_pointer_moves, egui_owns_pointer};
use crate::settle::Settle;

/// Window setup.
#[derive(Clone, Debug)]
pub struct Options {
    pub title: String,
    /// Native: initial inner size in points.
    pub initial_size: [f32; 2],
    /// Web: id of the `<canvas>` element to draw into. Its size comes from CSS.
    pub canvas_id: String,
}

impl Default for Options {
    fn default() -> Self {
        Self { title: "Drafft.ink".into(), initial_size: [1280.0, 800.0], canvas_id: "canvas".into() }
    }
}

/// Why the shell stopped.
#[derive(Debug, Error)]
pub enum ShellError {
    #[error("could not start the event loop ({0})")]
    EventLoop(String),
    #[error("could not create the window ({0})")]
    Window(String),
    #[error(transparent)]
    Gpu(#[from] GpuError),
    #[error("this browser cannot run Drafft.ink ({0})")]
    Unsupported(String),
}

/// Events the shell sends itself through the event loop.
pub(crate) enum UserEvent {
    /// Async GPU setup finished; the result is in the slot. (wgpu objects are not `Send` on the web.)
    GpuReady,
    /// egui asked for a repaint after this delay (zero: as soon as possible).
    EguiRepaint(Duration),
    /// Another thread called [`Waker::wake`].
    Wake(RedrawReason),
    /// The platform host (web glue, file reader thread) has an event.
    Host(HostEvent),
    /// [`Waker::exit`]: close the window and leave the loop.
    Exit,
}

static FRAMES: AtomicU64 = AtomicU64::new(0);
static VELLO_RUNS: AtomicU64 = AtomicU64::new(0);

/// Frames drawn since start (egui pass + composite + present). The idle checks read this.
pub fn frames_drawn() -> u64 {
    FRAMES.load(Ordering::Relaxed)
}

/// Frames in which Vello ran.
pub fn vello_runs() -> u64 {
    VELLO_RUNS.load(Ordering::Relaxed)
}

type GpuSlot = Rc<RefCell<Option<Result<Box<dyn Backend>, GpuError>>>>;

/// State that exists once the GPU is ready.
struct Running {
    backend: Box<dyn Backend>,
    ctx: egui::Context,
    state: egui_winit::State,
    canvas_rect: Option<egui::Rect>,
    /// The pointer was over egui at the end of the last frame (so leaving it needs a frame).
    over_egui: bool,
    /// A `CursorMoved` that egui has not seen: hover over the canvas. Delivered before the next event that needs a position.
    held_cursor: HeldHover<WindowEvent>,
    /// The caret rectangle the app asked for, and the one currently applied to the platform.
    text_input: Option<egui::Rect>,
    applied_text_input: Option<egui::Rect>,
    last_cursor: Option<egui::CursorIcon>,
    /// Texture updates egui produced for a frame that could not be presented.
    stashed_textures: egui::TexturesDelta,
    last_scale: f32,
}

impl Drop for Running {
    fn drop(&mut self) {
        // egui debug-asserts that a TexturesDelta is empty when dropped.
        self.stashed_textures.clear();
    }
}

impl Running {
    fn view(&self) -> ViewInfo {
        ViewInfo { size_px: self.backend.size(), scale: self.ctx.pixels_per_point() }
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct Platform {
    clipboard: crate::native::Clipboard,
}
#[cfg(target_arch = "wasm32")]
struct Platform {
    host: Option<crate::web::WebHost>,
}

impl Platform {
    #[cfg(not(target_arch = "wasm32"))]
    fn new() -> Self {
        Self { clipboard: crate::native::Clipboard::new() }
    }
    #[cfg(target_arch = "wasm32")]
    fn new() -> Self {
        Self { host: None }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn read_clipboard(&mut self) -> ClipboardContent {
        self.clipboard.read()
    }

    fn write_clipboard(&mut self, content: &ClipboardContent) {
        #[cfg(not(target_arch = "wasm32"))]
        self.clipboard.write(content);
        #[cfg(target_arch = "wasm32")]
        if let Some(host) = &self.host {
            host.write_clipboard(content.clone());
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn open_url(&mut self, url: &str, new_tab: bool) {
        if let Some(host) = &self.host {
            host.open_url(url, new_tab);
        }
    }

    /// The primary pointer is a finger or a stylus. Only the web knows.
    fn coarse_pointer(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            self.host.as_ref().is_some_and(|host| host.coarse_pointer())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = self;
            false
        }
    }

    fn set_text_input(&mut self, rect: Option<egui::Rect>) {
        #[cfg(target_arch = "wasm32")]
        if let Some(host) = &self.host {
            host.set_text_input(rect);
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = rect; // native: egui-winit toggles the IME from `PlatformOutput::ime`
    }
}

/// The text of the last clipboard write, with its private payload, so that a paste of our own
/// copy brings the payload back on platforms whose clipboard cannot carry it.
#[derive(Default)]
struct ClipboardCache {
    last: Option<ClipboardContent>,
}

impl ClipboardCache {
    fn remember(&mut self, content: &ClipboardContent) {
        self.last = Some(content.clone());
    }

    /// Fill in `custom` when the pasted text equals the text we copied last.
    fn restore(&self, mut pasted: ClipboardContent) -> ClipboardContent {
        if pasted.custom.is_none()
            && let (Some(last), Some(text)) = (&self.last, &pasted.text)
            && last.text.as_ref() == Some(text)
        {
            pasted.custom.clone_from(&last.custom);
        }
        pasted
    }
}

struct Shell<A: ShellApp> {
    app: A,
    options: Options,
    proxy: Arc<EventLoopProxy<UserEvent>>,
    waker: Waker,
    window: Option<Arc<Window>>,
    gpu_slot: GpuSlot,
    running: Option<Running>,
    repaint: RepaintState,
    platform: Platform,
    clipboard_cache: ClipboardCache,
    scene: Scene,
    scene_dirty: bool,
    scene_base: Color,
    /// The compatibility renderer's reduced-detail bookkeeping (`settle.rs`).
    settle: Settle,
    /// The app has submitted a scene at least once (so a resized or recreated target is re-rendered).
    scene_submitted: bool,
    recovering: bool,
    modifiers: egui::Modifiers,
    #[cfg(not(target_arch = "wasm32"))]
    buttons_down: u32,
    /// The pointer position in points; `None` when it left the window.
    last_pos: Option<egui::Pos2>,
    ime_enabled: bool,
    error: Option<ShellError>,
    /// Test hook: destroy the device at the next frame (`EventCx::debug_lose_device`).
    lose_device_next: bool,
}

/// Run `app` in a window (native) or on the canvas (web). Native: blocks until the window closes.
/// Web: returns at once; the browser keeps the event loop alive.
pub fn run<A: ShellApp>(app: A, options: Options) -> Result<(), ShellError> {
    #[cfg(target_arch = "wasm32")]
    {
        if !crate::web::webgpu_available() && !cfg!(feature = "webgl") {
            let msg = "navigator.gpu is not available";
            crate::web::show_error_page("This browser cannot run Drafft.ink", msg);
            return Err(ShellError::Unsupported(msg.into()));
        }
    }
    let event_loop =
        EventLoop::<UserEvent>::with_user_event().build().map_err(|e| ShellError::EventLoop(e.to_string()))?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = Arc::new(event_loop.create_proxy());
    let shell = Shell::new(app, options, proxy);
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut shell = shell;
        event_loop.run_app(&mut shell).map_err(|e| ShellError::EventLoop(e.to_string()))?;
        match shell.error.take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        use winit::platform::web::EventLoopExtWebSys;
        event_loop.spawn_app(shell);
        Ok(())
    }
}

fn modifiers_from(state: winit::keyboard::ModifiersState) -> egui::Modifiers {
    let mac = cfg!(target_os = "macos");
    egui::Modifiers {
        alt: state.alt_key(),
        ctrl: state.control_key(),
        shift: state.shift_key(),
        mac_cmd: mac && state.super_key(),
        command: if mac { state.super_key() } else { state.control_key() },
    }
}

impl<A: ShellApp> Shell<A> {
    fn new(app: A, options: Options, proxy: Arc<EventLoopProxy<UserEvent>>) -> Self {
        let waker = Waker { proxy: Some(proxy.clone()) };
        Self {
            app,
            options,
            proxy,
            waker,
            window: None,
            gpu_slot: GpuSlot::default(),
            running: None,
            repaint: RepaintState::default(),
            platform: Platform::new(),
            clipboard_cache: ClipboardCache::default(),
            scene: Scene::new(),
            scene_dirty: false,
            scene_base: Color::BLACK,
            settle: Settle::default(),
            scene_submitted: false,
            recovering: false,
            modifiers: egui::Modifiers::NONE,
            #[cfg(not(target_arch = "wasm32"))]
            buttons_down: 0,
            last_pos: None,
            ime_enabled: false,
            error: None,
            lose_device_next: false,
        }
    }

    fn request_redraw(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    /// Ask for a frame now, with a reason.
    fn want_frame(&mut self, reason: RedrawReason) {
        if self.repaint.request_now(reason) {
            self.request_redraw();
        }
    }

    fn apply_repaint(&mut self, repaint: Repaint) {
        if self.repaint.apply(Instant::now(), repaint) {
            self.request_redraw();
        }
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: ShellError) {
        log::error!("{error}");
        #[cfg(target_arch = "wasm32")]
        crate::web::show_error_page("Drafft.ink could not start", &error.to_string());
        self.error = Some(error);
        event_loop.exit();
    }

    /// Give an event to the app. Applies its repaint answer and its outbox.
    fn dispatch(&mut self, event: ShellEvent) -> (Handled, bool) {
        let Some(run) = self.running.as_ref() else { return (Handled::No, false) };
        let mut out = Outbox::default();
        let (repaint, handled) = {
            let mut cx = EventCx {
                ctx: &run.ctx,
                backend: run.backend.kind(),
                waker: &self.waker,
                view: run.view(),
                modifiers: self.modifiers,
                text_input_on: run.applied_text_input.is_some(),
                out: &mut out,
            };
            self.app.event(&mut cx, event)
        };
        self.apply_repaint(repaint);
        let wrote = out.clipboard.is_some();
        self.flush_outbox(out);
        (handled, wrote)
    }

    fn flush_outbox(&mut self, out: Outbox) {
        if let Some(content) = out.clipboard {
            self.clipboard_cache.remember(&content);
            self.platform.write_clipboard(&content);
        }
        if let (Some(icon), Some(window)) = (out.cursor, &self.window) {
            set_window_cursor(window, icon);
        }
        if let Some((url, new_tab)) = out.open_url {
            #[cfg(target_arch = "wasm32")]
            self.platform.open_url(&url, new_tab);
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(run) = &self.running {
                // egui-winit opens it (webbrowser) when the next frame hands over the platform output.
                run.ctx.open_url(egui::OpenUrl { url, new_tab });
                self.want_frame(RedrawReason::Ui);
            }
        }
        if out.lose_device {
            self.lose_device_next = true;
            self.want_frame(RedrawReason::Other);
        }
        if let (Some(caret), Some(run)) = (out.text_input, self.running.as_mut())
            && run.applied_text_input.is_none()
        {
            // The editor opened in this event. Focus its input now: a key that arrives before the next frame
            // would otherwise go to the canvas and be lost.
            run.text_input = Some(caret);
            run.applied_text_input = Some(caret);
            self.platform.set_text_input(Some(caret));
        }
    }

    fn push_egui_event(&mut self, event: egui::Event) {
        if let Some(run) = self.running.as_mut() {
            run.state.egui_input_mut().events.push(event);
        }
    }

    fn egui_wants_keyboard(&self) -> bool {
        self.running.as_ref().is_some_and(|r| r.ctx.egui_wants_keyboard_input())
    }

    fn handle_paste(&mut self, content: ClipboardContent) {
        let content = self.clipboard_cache.restore(content);
        let text = content.text.clone();
        let (handled, _) = self.dispatch(ShellEvent::Paste(content));
        if handled == Handled::No
            && let Some(text) = text.filter(|t| !t.is_empty())
            && self.egui_wants_keyboard()
        {
            self.push_egui_event(egui::Event::Paste(text));
            self.want_frame(RedrawReason::Ui);
        }
    }

    fn handle_copy(&mut self, cut: bool) {
        let (_, wrote) = self.dispatch(ShellEvent::CopyRequested { cut });
        if !wrote && self.egui_wants_keyboard() {
            self.push_egui_event(if cut { egui::Event::Cut } else { egui::Event::Copy });
            self.want_frame(RedrawReason::Ui);
        }
    }

    fn handle_host_event(&mut self, event: HostEvent) {
        match event {
            HostEvent::Egui(ev) => {
                if matches!(ev, egui::Event::MouseWheel { .. } | egui::Event::Zoom(_))
                    && self.offer_wheel(std::slice::from_ref(&ev))
                {
                    return;
                }
                self.push_egui_event(ev);
                self.want_frame(RedrawReason::Ui);
            }
            HostEvent::App(ShellEvent::Paste(content)) => self.handle_paste(content),
            HostEvent::App(ShellEvent::CopyRequested { cut }) => self.handle_copy(cut),
            HostEvent::App(ShellEvent::Visibility(visible)) => self.set_visible(visible),
            HostEvent::App(ev) => {
                self.dispatch(ev);
            }
        }
    }

    /// Offers wheel, scroll and pinch events to the app when the pointer is over the canvas. True when the app took
    /// them; egui then never sees them (it would smooth the scroll over several frames and repaint for each).
    fn offer_wheel(&mut self, events: &[egui::Event]) -> bool {
        let Some(run) = self.running.as_ref() else { return false };
        let Some(pos) = self.last_pos else { return false };
        if egui_owns_pointer(&run.ctx, run.canvas_rect, Some(pos)) {
            return false;
        }
        let wheel = WheelInput { pos, events: events.to_vec(), modifiers: self.modifiers };
        self.dispatch(ShellEvent::Wheel(wheel)).0 == Handled::Yes
    }

    fn set_visible(&mut self, visible: bool) {
        let redraw_now = self.repaint.set_hidden(!visible);
        self.dispatch(ShellEvent::Visibility(visible));
        if visible {
            // The surface may have been lost while hidden.
            self.want_frame(RedrawReason::Surface);
        }
        if redraw_now {
            self.request_redraw();
        }
    }

    // ----- frame -----

    /// One egui pass, the optional Vello run, one composite submit and one present.
    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let Some(window) = self.window.clone() else { return };
        // `None`: nobody asked, the OS exposed the window. That is a reason too.
        let reason = self.repaint.take().unwrap_or_else(|| {
            drafftink_profile::redraw(RedrawReason::Surface);
            RedrawReason::Surface
        });
        if self.repaint.hidden() {
            self.repaint.keep(reason);
            return;
        }
        if self.running.is_none() {
            // GPU not ready (start or recovery); `on_gpu_ready` asks for the frame.
            self.repaint.keep(reason);
            return;
        }
        if self.lose_device_next {
            self.lose_device_next = false;
            if let Some(run) = &self.running {
                run.backend.destroy_for_test();
            }
        }
        if self.running.as_ref().is_some_and(|r| r.backend.lost().is_some()) {
            self.repaint.keep(RedrawReason::Surface);
            self.start_recovery(&window, event_loop);
            return;
        }
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return; // minimized; a Resized event wakes us
        }
        begin_frame();
        let drawn = self.draw(&window, reason, [size.width, size.height]);
        end_frame();
        if self.repaint.needs_redraw_call() {
            window.request_redraw(); // the surface was not ready: try again
        }
        if drawn {
            let n = FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
            // One line per frame; the idle checks count these.
            log::debug!(target: "drafftink_gpu::frame", "frame {n} reason={} vello_runs={}", reason.name(), vello_runs());
        }
    }

    fn draw(&mut self, window: &Arc<Window>, reason: RedrawReason, size: [u32; 2]) -> bool {
        let Some(run) = self.running.as_mut() else { return false };
        let resized = run.backend.resize(size);
        let scale = run.ctx.pixels_per_point();
        let scale_changed = (scale - run.last_scale).abs() > f32::EPSILON;
        run.last_scale = scale;
        if run.backend.ensure_scene_target(size) && self.scene_submitted {
            // The target was recreated (size change, new device): draw the retained scene again.
            self.scene_dirty = true;
        }

        // ---- egui pass ----
        let raw = drafftink_profile::scope("egui_input", || {
            let mut raw = run.state.take_egui_input(window);
            if let Some(info) = raw.viewports.get_mut(&raw.viewport_id) {
                egui_winit::update_viewport_info(info, &run.ctx, window, false);
            }
            raw
        });
        let mut frame_out = FrameOutput::default();
        let view = ViewInfo { size_px: size, scale };
        let mut out = Outbox::default();
        let mut output = in_phase(Phase::Ui, || {
            let backend = run.backend.kind();
            run.ctx.run_ui(raw, |ui| {
                let mut cx = FrameCx {
                    backend,
                    waker: &self.waker,
                    view,
                    reason,
                    resized: resized || scale_changed,
                    scene: &mut self.scene,
                    scene_dirty: &mut self.scene_dirty,
                    scene_base: &mut self.scene_base,
                    out: &mut out,
                };
                frame_out = self.app.frame(ui, &mut cx);
            })
        });
        if self.scene_dirty {
            self.scene_submitted = true;
        }

        // ---- platform output ----
        let mut platform_output = std::mem::take(&mut output.platform_output);
        let mut copies = Vec::new();
        platform_output.commands.retain_mut(|cmd| match cmd {
            egui::OutputCommand::CopyText(text) => {
                copies.push(ClipboardContent::text(std::mem::take(text)));
                false
            }
            #[cfg(target_arch = "wasm32")]
            egui::OutputCommand::OpenUrl(url) => {
                self.platform.open_url(&url.url, url.new_tab);
                false
            }
            _ => true,
        });
        let want_text_input = frame_out.text_input;
        if let Some(rect) = want_text_input {
            platform_output.ime = Some(egui::output::IMEOutput {
                rect,
                cursor_rect: rect,
                purpose: egui::IMEPurpose::Normal,
                should_interrupt_composition: false,
            });
        }
        let ime_rect = platform_output.ime.as_ref().map(|i| i.rect);
        if frame_out.cursor.is_some() && platform_output.cursor_icon == egui::CursorIcon::Default {
            platform_output.cursor_icon = frame_out.cursor.unwrap_or_default();
        }
        run.last_cursor = Some(platform_output.cursor_icon);
        drafftink_profile::scope("platform_output", || run.state.handle_platform_output(window, platform_output));
        run.canvas_rect = frame_out.canvas_rect;
        run.text_input = want_text_input;
        let ime_changed = run.applied_text_input != ime_rect;
        run.applied_text_input = ime_rect;

        // ---- Vello ----
        let plan = self.settle.plan(Instant::now(), run.backend.kind().is_compat(), reason, self.scene_dirty);
        if plan.redraw_scene {
            self.scene_dirty = true;
        }
        let settle_after = plan.settle_after;
        let ran_vello = self.scene_dirty;
        let render = if self.scene_dirty {
            self.scene_dirty = false;
            in_phase(Phase::GpuSubmit, || run.backend.render_scene(&self.scene, self.scene_base, size, plan.detail))
        } else {
            Ok(())
        };
        if let Err(e) = render {
            log::error!("vello: {e}");
        } else if ran_vello {
            VELLO_RUNS.fetch_add(1, Ordering::Relaxed);
        }

        // ---- composite + present ----
        let mut frame = drafftink_profile::scope("tessellate", || EguiFrame::from_output(&run.ctx, output));
        if !run.stashed_textures.is_empty() {
            let mut merged = std::mem::take(&mut run.stashed_textures);
            merged.append(std::mem::take(&mut frame.textures_delta));
            frame.textures_delta = merged;
        }
        let drawn = match run.backend.present(window, &mut frame, size) {
            Ok(Presented::Shown) => true,
            Ok(presented) => {
                if presented == Presented::Retry {
                    self.repaint.keep(RedrawReason::Surface);
                }
                run.stashed_textures = std::mem::take(&mut frame.textures_delta);
                false
            }
            Err(e) => {
                log::error!("present: {e}");
                false
            }
        };

        // Pointer hover state for the next move event.
        run.over_egui = egui_owns_pointer(&run.ctx, run.canvas_rect, self.last_pos);
        let (pending_ime, ime_for_platform) = (ime_changed, ime_rect);
        let _ = run;

        if pending_ime {
            self.platform.set_text_input(ime_for_platform);
        }
        for content in copies {
            self.clipboard_cache.remember(&content);
            self.platform.write_clipboard(&content);
        }
        self.flush_outbox(out);
        self.apply_repaint(frame_out.repaint);
        if let Some(delay) = settle_after {
            // The scene was drawn with less detail; one more frame finishes it when the gesture is over.
            self.apply_repaint(Repaint::After(delay, RedrawReason::Timer));
        }
        inc(Counter::SceneBuilds);
        drawn
    }

    // ----- GPU lifecycle -----

    fn start_gpu(&mut self, window: Arc<Window>, display: OwnedDisplayHandle) {
        let slot = self.gpu_slot.clone();
        let proxy = self.proxy.clone();
        let size = window.inner_size();
        let init = async move {
            let result = init_backend(window, display, [size.width, size.height]).await;
            *slot.borrow_mut() = Some(result);
            let _ = proxy.send_event(UserEvent::GpuReady);
        };
        #[cfg(target_arch = "wasm32")]
        wasm_bindgen_futures::spawn_local(init);
        #[cfg(not(target_arch = "wasm32"))]
        drafftink_profile::scope("startup_gpu", || pollster::block_on(init));
    }

    fn start_recovery(&mut self, window: &Arc<Window>, event_loop: &ActiveEventLoop) {
        log::warn!("GPU device lost; recreating");
        self.recovering = true;
        self.running = None;
        self.start_gpu(window.clone(), event_loop.owned_display_handle());
    }

    fn on_gpu_ready(&mut self, backend: Box<dyn Backend>) -> Result<(), GpuError> {
        let Some(window) = self.window.clone() else { return Ok(()) };
        log::info!("renderer: {}", backend.describe());
        let ctx = egui::Context::default();
        // egui's own repaint requests (animation, tooltip delay, `request_repaint_after`). The callback may
        // run on any thread, so it goes through the event loop.
        let proxy = self.proxy.clone();
        ctx.set_request_repaint_callback(move |info| {
            let _ = proxy.send_event(UserEvent::EguiRepaint(info.delay));
        });
        let mut state = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            &*window,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(backend.max_texture_side() as usize),
        );
        // The shell turns the shortcuts into clipboard events itself, so the app can answer first.
        state.set_clipboard_shortcuts(false);
        let view = ViewInfo { size_px: backend.size(), scale: ctx.pixels_per_point() };
        let touch_first = self.platform.coarse_pointer();
        self.app.setup(&mut SetupCx {
            ctx: &ctx,
            backend: backend.kind(),
            waker: self.waker.clone(),
            view,
            touch_first,
        });
        self.running = Some(Running {
            backend,
            ctx,
            state,
            canvas_rect: None,
            over_egui: false,
            held_cursor: HeldHover::default(),
            text_input: None,
            applied_text_input: None,
            last_cursor: None,
            stashed_textures: egui::TexturesDelta::default(),
            last_scale: window.scale_factor() as f32,
        });
        if self.recovering {
            self.recovering = false;
            self.scene_dirty = self.scene_submitted;
            self.dispatch(ShellEvent::GpuRecreated);
            self.want_frame(RedrawReason::Surface);
        } else {
            self.want_frame(RedrawReason::Startup);
        }
        Ok(())
    }

    // ----- native input -----

    #[cfg(not(target_arch = "wasm32"))]
    fn pointer_sample(
        &self,
        phase: PointerPhase,
        kind: PointerKind,
        pos: egui::Pos2,
        pressure: Option<f32>,
    ) -> PointerSample {
        PointerSample::new(phase, kind, PointerPoint { pos, pressure, time_ms: now_ms() }, self.modifiers)
    }

    /// Turn a winit pointer event into a [`PointerSample`] for the app. Web builds get samples from
    /// the DOM instead (pressure, tilt and coalesced points are only there).
    #[cfg(not(target_arch = "wasm32"))]
    fn native_pointer(&mut self, event: &WindowEvent) -> Option<PointerSample> {
        use winit::event::{MouseButton, TouchPhase};
        let ppp = self.running.as_ref()?.ctx.pixels_per_point();
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                let pos = egui::pos2(position.x as f32 / ppp, position.y as f32 / ppp);
                let phase = if self.buttons_down == 0 { PointerPhase::Hover } else { PointerPhase::Move };
                Some(self.pointer_sample(phase, PointerKind::Mouse, pos, None))
            }
            WindowEvent::CursorLeft { .. } => {
                let pos = self.last_pos.unwrap_or_default();
                Some(self.pointer_sample(PointerPhase::Leave, PointerKind::Mouse, pos, None))
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let index: u8 = match button {
                    MouseButton::Left => 0,
                    MouseButton::Middle => 1,
                    MouseButton::Right => 2,
                    MouseButton::Back => 3,
                    MouseButton::Forward => 4,
                    MouseButton::Other(n) => u8::try_from(*n).unwrap_or(u8::MAX),
                };
                let bit = 1u32 << u32::from(index.min(31));
                let phase = if *state == ElementState::Pressed {
                    self.buttons_down |= bit;
                    PointerPhase::Down
                } else {
                    self.buttons_down &= !bit;
                    PointerPhase::Up
                };
                let mut s = self.pointer_sample(phase, PointerKind::Mouse, self.last_pos.unwrap_or_default(), None);
                s.button = Some(index);
                Some(s)
            }
            WindowEvent::Touch(t) => {
                let pos = egui::pos2(t.location.x as f32 / ppp, t.location.y as f32 / ppp);
                let phase = match t.phase {
                    TouchPhase::Started => PointerPhase::Down,
                    TouchPhase::Moved => PointerPhase::Move,
                    TouchPhase::Ended => PointerPhase::Up,
                    TouchPhase::Cancelled => PointerPhase::Cancel,
                };
                // winit has no pointer type. A calibrated force with an altitude angle is an Apple Pencil.
                let pen = matches!(t.force, Some(winit::event::Force::Calibrated { altitude_angle: Some(_), .. }));
                let pressure = t.force.map(|f| f.normalized() as f32);
                let mut s =
                    self.pointer_sample(phase, if pen { PointerKind::Pen } else { PointerKind::Touch }, pos, pressure);
                s.id = t.id;
                Some(s)
            }
            _ => None,
        }
    }

    /// Ctrl/Cmd+C, X, V and the legacy Insert/Delete forms, on native.
    #[cfg(not(target_arch = "wasm32"))]
    fn native_clipboard_shortcut(&mut self, event: &winit::event::KeyEvent) {
        use winit::keyboard::{KeyCode, PhysicalKey};
        if event.state != ElementState::Pressed || event.repeat {
            return;
        }
        let PhysicalKey::Code(code) = event.physical_key else { return };
        let m = self.modifiers;
        let paste =
            (m.command && !m.alt && code == KeyCode::KeyV) || (m.shift && !m.command && code == KeyCode::Insert);
        let copy = (m.command && !m.alt && code == KeyCode::KeyC) || (m.ctrl && code == KeyCode::Insert);
        let cut = (m.command && !m.alt && code == KeyCode::KeyX) || (m.shift && code == KeyCode::Delete);
        if paste {
            let content = self.platform.read_clipboard();
            self.handle_paste(content);
        } else if copy {
            self.handle_copy(false);
        } else if cut {
            self.handle_copy(true);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn native_text_input(&mut self, event: &WindowEvent) {
        let active = self.running.as_ref().is_some_and(|r| r.text_input.is_some());
        if !active {
            return;
        }
        match event {
            WindowEvent::Ime(Ime::Preedit(text, cursor)) => {
                let ev = if text.is_empty() {
                    TextInput::Cancel
                } else {
                    TextInput::Preedit { text: text.clone(), cursor: *cursor }
                };
                self.dispatch(ShellEvent::TextInput(ev));
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                self.dispatch(ShellEvent::TextInput(TextInput::Commit(text.clone())));
            }
            // Plain typing without an IME arrives as key text.
            WindowEvent::KeyboardInput { event, .. }
                if !self.ime_enabled && event.state == ElementState::Pressed && !self.modifiers.command =>
            {
                if let Some(text) = event.text.as_ref().filter(|t| !t.chars().all(char::is_control)) {
                    self.dispatch(ShellEvent::TextInput(TextInput::Commit(text.to_string())));
                }
            }
            _ => {}
        }
    }
}

/// The shortest time between two frames on this display: the refresh period less 10%, so a vsync-paced
/// present never finds the next frame a hair too early. 60 Hz when the display does not say.
#[cfg(not(target_arch = "wasm32"))]
fn frame_interval(window: &Window) -> Duration {
    let millihertz =
        window.current_monitor().and_then(|m| m.refresh_rate_millihertz()).filter(|r| *r > 0).unwrap_or(60_000);
    Duration::from_secs_f64(0.9 * 1000.0 / f64::from(millihertz))
}

/// Shows `icon` over the window now. On the web winit sets the CSS cursor of the canvas.
fn set_window_cursor(window: &Window, icon: egui::CursorIcon) {
    match crate::cursor::cursor_icon(icon) {
        Some(c) => {
            window.set_cursor_visible(true);
            window.set_cursor(c);
        }
        None => window.set_cursor_visible(false),
    }
}

impl<A: ShellApp> Shell<A> {
    /// Hand a window event to egui and decide whether it needs a frame.
    fn route_to_egui(&mut self, window: &Arc<Window>, event: WindowEvent) {
        let ppp = match self.running.as_ref() {
            Some(run) => run.ctx.pixels_per_point().max(f32::EPSILON),
            None => return,
        };
        match &event {
            WindowEvent::CursorMoved { position, .. } => {
                self.last_pos = Some(egui::pos2(position.x as f32 / ppp, position.y as f32 / ppp));
            }
            WindowEvent::Touch(t) => {
                self.last_pos = Some(egui::pos2(t.location.x as f32 / ppp, t.location.y as f32 / ppp));
            }
            WindowEvent::CursorLeft { .. } => self.last_pos = None,
            _ => {}
        }
        let Some(run) = self.running.as_mut() else { return };

        // Hover over the canvas: remember the event, tell egui nothing, draw nothing.
        let event = if matches!(event, WindowEvent::CursorMoved { .. }) {
            let owns = egui_owns_pointer(&run.ctx, run.canvas_rect, self.last_pos) || run.over_egui;
            match run.held_cursor.on_move(event, owns) {
                Some(event) => event,
                None => return,
            }
        } else {
            event
        };
        // Events that act at the pointer position need egui to know it first.
        let needs_position = matches!(
            event,
            WindowEvent::MouseInput { .. }
                | WindowEvent::MouseWheel { .. }
                | WindowEvent::Touch(_)
                | WindowEvent::PinchGesture { .. }
                | WindowEvent::PanGesture { .. }
                | WindowEvent::DoubleTapGesture { .. }
                | WindowEvent::RotationGesture { .. }
                | WindowEvent::DroppedFile(_)
                | WindowEvent::HoveredFile(_)
        );
        if needs_position && let Some(held) = run.held_cursor.take() {
            let _ = run.state.on_window_event(window, &held);
        }
        if matches!(event, WindowEvent::AxisMotion { .. }) {
            return; // raw device axes: egui ignores them and they must not wake the loop
        }
        if matches!(event, WindowEvent::CursorLeft { .. }) {
            run.held_cursor.clear();
        }
        if matches!(
            event,
            WindowEvent::MouseWheel { .. } | WindowEvent::PinchGesture { .. } | WindowEvent::PanGesture { .. }
        ) {
            // Let egui's window layer translate the event, then offer the result to the app before egui's
            // context smooths it. Only what the app does not take stays in egui's input.
            let before = run.state.egui_input().events.len();
            let response = run.state.on_window_event(window, &event);
            let produced = run.state.egui_input_mut().events.split_off(before);
            if !produced.is_empty() && self.offer_wheel(&produced) {
                return;
            }
            if let Some(run) = self.running.as_mut() {
                run.state.egui_input_mut().events.extend(produced);
            }
            if response.repaint {
                self.want_frame(RedrawReason::Gesture);
            }
            return;
        }
        let response = run.state.on_window_event(window, &event);
        coalesce_pointer_moves(&mut run.state.egui_input_mut().events);
        let over_egui = run.over_egui;
        let reason = match &event {
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => Some(RedrawReason::Resize),
            WindowEvent::ThemeChanged(_) => Some(RedrawReason::Ui),
            // Moves that reach here belong to egui. Clicks and touches always get a pass so that
            // popups close and text fields lose focus when the canvas is clicked.
            WindowEvent::CursorMoved { .. } | WindowEvent::MouseInput { .. } | WindowEvent::Touch(_) => {
                Some(RedrawReason::Gesture)
            }
            WindowEvent::CursorLeft { .. } => over_egui.then_some(RedrawReason::Gesture),
            WindowEvent::MouseWheel { .. } | WindowEvent::PinchGesture { .. } | WindowEvent::PanGesture { .. } => {
                response.repaint.then_some(RedrawReason::Gesture)
            }
            _ => response.repaint.then_some(RedrawReason::Ui),
        };
        if let Some(reason) = reason {
            self.want_frame(reason);
        }
    }
}

impl<A: ShellApp> ApplicationHandler<UserEvent> for Shell<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return; // Android may resume repeatedly; the window is created once
        }
        let attrs = Window::default_attributes().with_title(self.options.title.clone());
        #[cfg(not(target_arch = "wasm32"))]
        let attrs = attrs.with_inner_size(winit::dpi::LogicalSize::new(
            f64::from(self.options.initial_size[0]),
            f64::from(self.options.initial_size[1]),
        ));
        #[cfg(target_arch = "wasm32")]
        let (attrs, canvas) = {
            use winit::platform::web::WindowAttributesExtWebSys;
            let canvas = match crate::web::lookup_canvas(&self.options.canvas_id) {
                Ok(c) => c,
                Err(e) => return self.fail(event_loop, ShellError::Window(e)),
            };
            // The browser must see key events, or `paste`, `copy` and `cut` never fire.
            (attrs.with_canvas(Some(canvas.clone())).with_prevent_default(false), canvas)
        };
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => return self.fail(event_loop, ShellError::Window(e.to_string())),
        };
        self.window = Some(window.clone());
        #[cfg(not(target_arch = "wasm32"))]
        self.repaint.set_min_interval(frame_interval(&window));
        #[cfg(target_arch = "wasm32")]
        {
            let proxy = self.proxy.clone();
            let sink = crate::host::HostSink::new(move |ev| {
                let _ = proxy.send_event(UserEvent::Host(ev));
            });
            match crate::web::WebHost::install(&canvas, sink) {
                Ok(host) => self.platform.host = Some(host),
                Err(e) => return self.fail(event_loop, ShellError::Window(e)),
            }
        }
        self.lose_device_next = false;
        self.start_gpu(window, event_loop.owned_display_handle());
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::GpuReady => match self.gpu_slot.take() {
                Some(Ok(backend)) => {
                    if let Err(e) = self.on_gpu_ready(backend) {
                        self.fail(event_loop, e.into());
                    }
                }
                Some(Err(e)) => self.fail(event_loop, e.into()),
                None => {}
            },
            UserEvent::EguiRepaint(delay) => {
                let reason = if delay.is_zero() { RedrawReason::Ui } else { RedrawReason::Timer };
                self.apply_repaint(Repaint::After(delay, reason));
            }
            UserEvent::Wake(reason) => {
                // The app answers with the repaint it needs (usually `Now(reason)`; none when the wake changed
                // nothing visible).
                self.dispatch(ShellEvent::Wake(reason));
            }
            UserEvent::Host(ev) => self.handle_host_event(ev),
            UserEvent::Exit => event_loop.exit(),
        }
    }

    fn new_events(&mut self, _: &ActiveEventLoop, cause: StartCause) {
        // A `WaitUntil` deadline passed. Usually the delayed request becomes a frame. A collaboration deadline
        // goes to the app as a wake instead: most of them (a heartbeat, a retry check) change nothing visible,
        // and the app answers with a repaint only when something did.
        if !matches!(cause, StartCause::ResumeTimeReached { .. }) {
            return;
        }
        // A frame that pacing held back is due now.
        if self.repaint.needs_redraw_call() {
            self.request_redraw();
        }
        match self.repaint.expired(Instant::now()) {
            Some(RedrawReason::Collab) => {
                self.dispatch(ShellEvent::Wake(RedrawReason::Collab));
            }
            Some(reason) => self.want_frame(reason),
            None => {}
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(window) = self.window.clone() else { return };
        match &event {
            WindowEvent::CloseRequested => return event_loop.exit(),
            WindowEvent::RedrawRequested => return self.redraw(event_loop),
            WindowEvent::ModifiersChanged(m) => self.modifiers = modifiers_from(m.state()),
            WindowEvent::Occluded(occluded) => self.set_visible(!occluded),
            WindowEvent::Focused(focused) => {
                // On the web the hidden textarea takes focus from the canvas while the user types. The
                // host reports that to egui itself; winit's `Focused(false)` would close the text field.
                #[cfg(target_arch = "wasm32")]
                if !*focused && self.platform.host.as_ref().is_some_and(|h| h.text_input_focused()) {
                    return;
                }
                self.dispatch(ShellEvent::Focus(*focused));
            }
            WindowEvent::Ime(Ime::Enabled) => self.ime_enabled = true,
            WindowEvent::Ime(Ime::Disabled) => self.ime_enabled = false,
            WindowEvent::HoveredFile(_) => {
                self.dispatch(ShellEvent::FileHover(true));
            }
            WindowEvent::HoveredFileCancelled => {
                self.dispatch(ShellEvent::FileHover(false));
            }
            #[cfg(not(target_arch = "wasm32"))]
            WindowEvent::DroppedFile(path) => {
                self.dispatch(ShellEvent::FileHover(false));
                let path = path.clone();
                let proxy = self.proxy.clone();
                std::thread::spawn(move || match crate::native::read_dropped(&path) {
                    Ok(file) => {
                        let _ = proxy.send_event(UserEvent::Host(HostEvent::App(ShellEvent::FilesDropped(vec![file]))));
                    }
                    Err(e) => log::warn!("dropped file not read: {e}"),
                });
            }
            _ => {}
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            if let WindowEvent::KeyboardInput { event: key, .. } = &event {
                self.native_clipboard_shortcut(key);
            }
            self.native_text_input(&event);
        }

        // Pointer events: the app first (it owns hover). Then egui, but a plain hover over the canvas is
        // held back from egui: delivering it would make egui ask for a follow-up pass per move and keep
        // the loop running for as long as the pointer moves.
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(sample) = self.native_pointer(&event) {
            self.dispatch(ShellEvent::Pointer(sample));
        }
        self.route_to_egui(&window, event);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(self.repaint.control_flow());
    }
}

/// Create the drawing back end for the window: WebGPU (or the native API) when there is one, WebGL2 when the build
/// has the fallback and the browser has no WebGPU.
async fn init_backend(
    window: Arc<Window>,
    display: OwnedDisplayHandle,
    size: [u32; 2],
) -> Result<Box<dyn Backend>, GpuError> {
    #[cfg(all(target_arch = "wasm32", feature = "webgl"))]
    if !cfg!(feature = "wgpu") || !crate::web::webgpu_available() {
        use winit::platform::web::WindowExtWebSys;
        let canvas = window.canvas().ok_or_else(|| GpuError::WebGl("the window has no canvas".into()))?;
        return Ok(Box::new(crate::gl::GlBackend::create(&canvas, size)?));
    }
    #[cfg(feature = "wgpu")]
    {
        Ok(Box::new(crate::wgpu_backend::WgpuBackend::create(window, display, size).await?))
    }
    #[cfg(not(feature = "wgpu"))]
    {
        let _ = (window, display, size);
        Err(GpuError::NoAdapter("this build has no WebGPU renderer".into()))
    }
}
