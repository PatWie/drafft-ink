//! [`App`]: the `ShellApp` of Drafft.ink.
//!
//! One frame, in order: results of background work, bridge requests, the collaboration client, the
//! interaction layer's timers, the wheel, the keys the interaction layer owns, the UI, the text editor, then
//! the canvas scene when something about it changed, and last the next wake-up. Nothing here asks for a
//! frame by itself; a frame exists because the shell had a reason to draw one.

use std::collections::HashSet;
use std::time::Duration;

use drafftink_bridge::BridgeHost;
use drafftink_canvas::Camera;
use drafftink_engine::cmd::view::SetViewportArgs;
use drafftink_engine::interact::{Badge, ContextTarget, Interaction, InteractionOverlay, Outcome, UiRequest};
use drafftink_engine::services::SharedTypesetter;
use drafftink_engine::state::TabId;
use drafftink_engine::{Origin, Refusal, Session};
use drafftink_geom::kurbo::{Point, Size};
use drafftink_gpu::{
    ClipboardContent, DroppedFile, EventCx, FrameCx, FrameOutput, Handled, PointerPhase, RedrawReason, Repaint,
    SetupCx, ShellApp, ShellEvent,
};
use drafftink_render::{GridStyle, RemoteCursor, Theme};
use drafftink_scene::{ElementKind, SceneRead, SceneStore};
use drafftink_ui::{FrameInput, HostRequest, Pointer, Toast, UiShell};
use serde_json::{Value, json};
use vello::peniko::Color;

use crate::config::AppConfig;
use crate::editor::{KeyRoute, TextEditor, route};
use crate::input::{self, Held};
use crate::overlay;
use crate::perf::{Look, SceneInstall, SceneLoader, SceneSlot, scene_hook};
use crate::platform::{
    FontFile, Host, HostJob, Inbound, Inbox, InboxRx, PickKind, PickedFile, Platform, SharedClipboard, WakeCell, inbox,
};
use crate::view::{CanvasView, LASER_FADE_MS, TrailPoint};

/// Everything about the picture that, when it changes, makes the document scene stale.
#[derive(Clone, Debug, PartialEq)]
struct WorldKey {
    tab: TabId,
    revision: u64,
    camera: Camera,
    size: [f64; 2],
    dpr: f64,
    dark: bool,
    grid: GridStyle,
    /// The elements left out because the editor draws them.
    editing: Vec<drafftink_scene::ElementId>,
}

/// Everything the overlay shows, to tell whether it changed.
#[derive(Clone, Debug, PartialEq)]
struct OverlayKey {
    interaction: InteractionOverlay,
    /// Pointer (world), colour and name of each peer that has a pointer.
    peers: Vec<([f64; 2], String, String)>,
    /// The text editor: its picture, whether the caret shows.
    edit: Option<((u64, u64), bool)>,
}

/// The application. Build it with [`App::new`] and give it to `drafftink_gpu::run`.
pub struct App {
    session: Session,
    interaction: Interaction,
    ui: UiShell,
    canvas: CanvasView,
    editor: Option<TextEditor>,
    typesetter: SharedTypesetter,
    is_mac: bool,
    host: Box<dyn Host>,
    inbox: Inbox,
    inbox_rx: InboxRx,
    wake: WakeCell,
    clipboard: SharedClipboard,
    bridge: Option<BridgeHost>,
    /// Fonts the host was asked for, so that each is asked for once.
    fonts_requested: HashSet<FontFile>,
    notices: Vec<String>,
    /// Web addresses a link click asked for; the next event or frame hands them to the shell.
    open_urls: Vec<String>,
    held: Held,
    /// A pointer gesture began on the canvas and has not ended.
    canvas_drag: bool,
    canvas_rect: egui::Rect,
    egui_time: f64,
    told_viewport: Option<([f64; 2], f64)>,
    world_key: Option<WorldKey>,
    overlay_key: Option<OverlayKey>,
    in_room: bool,
    last_presence_ms: f64,
    collab_due_ms: Option<u64>,
    perf_overlay: bool,
    /// Where a scenario driver delivers a generated scene (`scene_loader`).
    scenes: Option<SceneSlot>,
    cursor: egui::CursorIcon,
    /// The picture of the text editor the engine was last told about.
    staged: Option<(u64, u64)>,
}

impl App {
    /// An app over a platform. Loads the settings and the workspace, so call it once, before the shell runs.
    pub fn new(platform: Platform, config: AppConfig) -> Self {
        let Platform { ports, host, wake, clipboard, typesetter, bridge, is_mac } = platform;
        let services = drafftink_engine::Services::sharing(typesetter.clone());
        let mut session = Session::with_services(ports, services);
        let notices = crate::startup::start(&mut session, &config);
        let (inbox, inbox_rx) = inbox(wake.clone());
        Self {
            session,
            interaction: Interaction::new(),
            ui: UiShell::new(is_mac),
            canvas: CanvasView::new(typesetter.clone()),
            editor: None,
            typesetter,
            is_mac,
            host,
            inbox,
            inbox_rx,
            wake,
            clipboard,
            bridge,
            fonts_requested: HashSet::new(),
            notices,
            open_urls: Vec::new(),
            held: Held::default(),
            canvas_drag: false,
            canvas_rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 720.0)),
            egui_time: 0.0,
            told_viewport: None,
            world_key: None,
            overlay_key: None,
            in_room: false,
            last_presence_ms: f64::NEG_INFINITY,
            collab_due_ms: Some(0),
            perf_overlay: config.perf_overlay,
            scenes: None,
            cursor: egui::CursorIcon::Default,
            staged: None,
        }
    }

    /// The session, for launchers that run a script against it (tests, the bridge's first connection).
    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    /// The door for a scenario driver: scenes sent through it replace the active tab's document at the next frame.
    /// Call it before the shell runs.
    pub fn scene_loader(&mut self) -> SceneLoader {
        let (loader, slot) = scene_hook(self.wake.clone());
        self.scenes = Some(slot);
        loader
    }

    /// Installs the scene a driver sent, if one is waiting: the document, the camera, the selection. Like the test
    /// rig's `load_generated`, it is not a command and leaves no history.
    fn install_scene(&mut self, canvas: Size) {
        let Some(install) = self.scenes.as_ref().and_then(SceneSlot::take) else { return };
        let SceneInstall { doc, look, zoom, select } = install;
        match drafftink_engine::store::TabDoc::from_document(&doc) {
            Ok(td) => {
                let tab = self.session.tab_mut();
                tab.saved_revision = td.revision();
                tab.doc = td;
                tab.observed = drafftink_engine::state::Observed::default();
                self.canvas.forget_index();
                let scroll = match look {
                    Look::Centre(p) => p,
                    Look::Corner { at, margin } => {
                        [at[0] + (canvas.width / 2.0 - margin) / zoom, at[1] + (canvas.height / 2.0 - margin) / zoom]
                    }
                };
                self.run(Origin::Script, "view.set", json!({ "scroll": scroll, "zoom": zoom }));
                if !select.is_empty() {
                    self.run(Origin::Script, "selection.select", json!({ "ids": select }));
                }
            }
            Err(e) => self.toast(format!("The scenario scene could not be loaded: {e}")),
        }
        if let Some(slot) = &self.scenes {
            slot.done();
        }
    }

    fn now_ms(&self) -> f64 {
        self.session.ports.clock.mono_ms() as f64
    }

    /// Runs a command for the person. A refusal becomes a toast with its text unchanged.
    fn run(&mut self, origin: Origin, id: &str, args: Value) -> Option<Value> {
        match self.session.execute_from(origin, id, args) {
            Ok(v) => Some(v),
            Err(r) => {
                self.toast_refusal(&r);
                None
            }
        }
    }

    fn toast_refusal(&mut self, r: &Refusal) {
        self.ui.state.push_toast(Toast::from_refusal(r, self.egui_time));
    }

    fn toast(&mut self, text: impl Into<String>) {
        self.ui.state.push_toast(Toast::note(text, self.egui_time));
    }

    // ---- interaction ----------------------------------------------------------------------------------------

    /// Applies what the interaction layer says: toasts for refusals, the UI it asks for. Returns whether a
    /// frame is needed.
    fn apply_outcome(&mut self, out: Outcome) -> bool {
        let need = out.redraw.is_some() || out.ui_request.is_some() || !out.refusals.is_empty();
        if let Some(c) = out.cursor {
            self.cursor = input::cursor_icon(c);
        }
        for r in &out.refusals {
            self.toast_refusal(r);
        }
        if let Some(request) = out.ui_request {
            self.open_ui(&request);
        }
        need
    }

    fn open_ui(&mut self, request: &UiRequest) {
        match request {
            UiRequest::OpenTextEditor { at, .. } => self.open_editor(request, Some(*at)),
            UiRequest::OpenEquationEditor { element } => {
                let latex = self.session.store().get(element).and_then(|e| match &e.kind {
                    ElementKind::Math(m) => Some(m.latex.clone()),
                    _ => None,
                });
                self.ui.state.open_equation(latex.unwrap_or_default());
            }
            UiRequest::OpenUrl { url } => self.open_urls.push(url.clone()),
            UiRequest::OpenContextMenu { at, target } => {
                let origin = self.canvas_rect.min;
                let screen = egui::pos2(at.x as f32 + origin.x, at.y as f32 + origin.y);
                let world = self.canvas_to_world(*at);
                let world = Some([world.x, world.y]);
                match target {
                    ContextTarget::Locked { element } => {
                        self.ui.state.open_context_menu_on_locked(screen, world, element.as_str());
                    }
                    ContextTarget::Canvas | ContextTarget::Selection => self.ui.state.open_context_menu(screen, world),
                }
            }
        }
    }

    fn canvas_input(&mut self, events: &[drafftink_canvas::InputEvent]) -> bool {
        let mut need = false;
        for ev in events {
            let out = self.interaction.input(&mut self.session, ev);
            need |= self.apply_outcome(out);
        }
        need
    }

    /// Where the pointer is in world units, if it was seen over the canvas.
    fn pointer_world(&self) -> Option<Point> {
        self.interaction.pointer_position().map(|p| self.canvas_to_world(p))
    }

    /// A point in canvas points (origin at the canvas corner) as world units.
    fn canvas_to_world(&self, p: Point) -> Point {
        let cam = self.interaction.camera();
        let size = self.canvas_rect.size();
        Point::new(
            (p.x - f64::from(size.x) / 2.0) / cam.zoom + cam.scroll.x,
            (p.y - f64::from(size.y) / 2.0) / cam.zoom + cam.scroll.y,
        )
    }

    fn handle_pointer(&mut self, cx: &mut EventCx<'_>, s: drafftink_gpu::PointerSample) -> Repaint {
        let pos = s.point.pos;
        let over_egui = input::egui_owns(cx.ctx, self.canvas_rect, pos);
        let now = self.now_ms();
        if let Some(repaint) = self.editor_pointer(&s, over_egui) {
            return repaint;
        }
        match s.phase {
            PointerPhase::Down => {
                self.canvas_drag = !over_egui && !self.ui.state.modal_open();
            }
            PointerPhase::Up | PointerPhase::Cancel => {}
            PointerPhase::Hover | PointerPhase::Move | PointerPhase::Leave => {
                if !self.canvas_drag && over_egui && s.phase != PointerPhase::Leave {
                    return Repaint::Idle;
                }
            }
        }
        if !self.canvas_drag && !matches!(s.phase, PointerPhase::Hover | PointerPhase::Leave) {
            if matches!(s.phase, PointerPhase::Up | PointerPhase::Cancel) {
                self.held = Held::default();
            }
            return Repaint::Idle;
        }
        let events = input::pointer_events(&s, &mut self.held, self.canvas_rect.min, now);
        let reason = if s.phase == PointerPhase::Hover { RedrawReason::HoverChange } else { RedrawReason::Gesture };
        let mut need = self.canvas_input(&events);
        for url in self.open_urls.drain(..) {
            cx.open_url(url, true);
        }
        if matches!(s.phase, PointerPhase::Up | PointerPhase::Cancel) && !self.held.any() {
            self.canvas_drag = false;
        }
        self.publish_pointer(now);
        if need {
            cx.set_cursor(self.cursor);
        }
        need |= self.session.dirty().is_some();
        if need { Repaint::Now(reason) } else { Repaint::Idle }
    }

    /// Wheel, scroll and pinch over the canvas, straight from the shell. `None`: not for the canvas now (a modal is
    /// open, or the text editor is), so egui gets the events.
    fn handle_wheel(&mut self, wheel: &drafftink_gpu::WheelInput) -> Option<Repaint> {
        if self.ui.state.modal_open() || self.editor.is_some() {
            return None;
        }
        let origin = self.canvas_rect.min;
        let at = Point::new(f64::from(wheel.pos.x - origin.x), f64::from(wheel.pos.y - origin.y));
        let events = input::wheel_events(&wheel.events, at, self.now_ms(), wheel.modifiers);
        let need = self.canvas_input(&events) | self.session.dirty().is_some();
        Some(if need { Repaint::Now(RedrawReason::Gesture) } else { Repaint::Idle })
    }

    /// Tells the room where the pointer is, at most every 33 ms. It changes nothing on this screen.
    fn publish_pointer(&mut self, now: f64) {
        if !self.in_room || now - self.last_presence_ms < 33.0 {
            return;
        }
        let Some(world) = self.pointer_world() else { return };
        self.last_presence_ms = now;
        let before = self.session.dirty();
        let _ = self.session.execute_from(Origin::Ui, "collab.set_pointer", json!({ "point": [world.x, world.y] }));
        if before.is_none() {
            let _ = self.session.take_dirty();
        }
    }

    // ---- text editor ----------------------------------------------------------------------------------------

    fn open_editor(&mut self, request: &UiRequest, caret_hint: Option<Point>) {
        let now = self.now_ms();
        let session = &self.session;
        let editor = self.typesetter.with(|ts| TextEditor::open(session, request, caret_hint, ts, now));
        self.editor = editor;
        self.staged = None;
        self.stage_editor();
    }

    /// Hands the editor's content to the engine, so that a command which ends the edit on its own (a toolbar
    /// click, `Mod+S`) keeps what was typed.
    fn stage_editor(&mut self) {
        let Some(editor) = self.editor.as_ref() else { return };
        let key = editor.picture_key();
        if self.staged == Some(key) {
            return;
        }
        self.staged = Some(key);
        self.session.stage_text_edit(editor.finish_args());
    }

    fn finish_editor(&mut self) {
        let Some(editor) = self.editor.take() else { return };
        if self.session.text_edit.is_some() {
            let args = editor.finish_args();
            self.run(Origin::Ui, "text.edit_end", args);
        }
        self.staged = None;
    }

    /// The engine's text edit and ours must agree: an edit that ended elsewhere (undo, a script, a tool
    /// change) closes the editor, and an edit a script began on an element opens one.
    fn sync_editor(&mut self) {
        match (&self.session.text_edit, &self.editor) {
            (None, Some(_)) => {
                self.editor = None;
                self.staged = None;
            }
            (Some(edit), None) if !self.ui.state.modal_open() => {
                if let Some(id) = &edit.element {
                    let req = UiRequest::OpenTextEditor { element: Some(id.clone()), at: Point::ZERO, width: None };
                    let is_math = self.session.store().get(id).is_some_and(|e| matches!(e.kind, ElementKind::Math(_)));
                    if !is_math {
                        self.open_editor(&req, None);
                    }
                }
            }
            _ => {}
        }
    }

    /// A pointer sample while a text is edited. `Some` when the editor took it: a press on the text, and the drag
    /// and the release that follow. A press anywhere else on the canvas ends the edit and goes on to the canvas.
    fn editor_pointer(&mut self, s: &drafftink_gpu::PointerSample, over_egui: bool) -> Option<Repaint> {
        self.editor.as_ref()?;
        let now = self.now_ms();
        let pos = s.point.pos;
        let canvas = Point::new(f64::from(pos.x - self.canvas_rect.min.x), f64::from(pos.y - self.canvas_rect.min.y));
        let world = self.canvas_to_world(canvas);
        let zoom = self.interaction.camera().zoom.max(1e-6);
        let primary = s.button.is_none_or(|b| b == 0);
        let editor = self.editor.as_mut()?;
        match s.phase {
            PointerPhase::Down if !over_egui => {
                let on_text = primary && self.typesetter.with(|ts| editor.hit(ts, world, 6.0 / zoom));
                if on_text {
                    let shift = s.modifiers.shift;
                    self.typesetter.with(|ts| editor.press(ts, world, canvas, shift, now));
                    self.stage_editor();
                    return Some(Repaint::Now(RedrawReason::Gesture));
                }
                // A press elsewhere on the canvas ends the edit with what was typed; the canvas then gets the press.
                self.stage_editor();
                self.finish_editor();
                None
            }
            PointerPhase::Move if editor.is_pressing() => {
                let changed = self.typesetter.with(|ts| editor.drag(ts, world, now));
                Some(if changed { Repaint::Now(RedrawReason::Gesture) } else { Repaint::Idle })
            }
            PointerPhase::Up | PointerPhase::Cancel if editor.is_pressing() => {
                editor.release();
                Some(Repaint::Now(RedrawReason::Gesture))
            }
            _ => None,
        }
    }

    /// Esc, `Mod+Enter` and the editing keys of this frame. Returns whether the picture changed.
    fn editor_keys(&mut self, ctx: &egui::Context, now_ms: f64) -> bool {
        let Some(editor) = self.editor.as_mut() else { return false };
        let mac = self.is_mac;
        let mut finish = false;
        let mut changed = false;
        let ts = &self.typesetter;
        ctx.input_mut(|i| {
            i.events.retain(|event| {
                let egui::Event::Key { key, pressed: true, modifiers, .. } = event else { return true };
                match route(*key, *modifiers, mac) {
                    Some(KeyRoute::Finish) => finish = true,
                    Some(KeyRoute::Editor(k)) => changed |= ts.with(|ts| editor.key(k, mac, ts, now_ms)),
                    None => return true,
                }
                false
            });
        });
        if finish {
            self.stage_editor();
            self.finish_editor();
            return true;
        }
        changed
    }

    fn editor_paste(&mut self, content: ClipboardContent) -> Repaint {
        let now = self.now_ms();
        let changed = match (self.editor.as_mut(), content.text) {
            (Some(editor), Some(text)) => editor.paste(&text, now),
            _ => false,
        };
        if changed { Repaint::Now(RedrawReason::Command) } else { Repaint::Idle }
    }

    fn editor_copy(&mut self, cx: &mut EventCx<'_>, cut: bool) -> Repaint {
        let now = self.now_ms();
        let Some(text) = self.editor.as_mut().and_then(|e| e.copy(cut, now)) else { return Repaint::Idle };
        cx.set_clipboard(ClipboardContent::text(text));
        if cut { Repaint::Now(RedrawReason::Command) } else { Repaint::Idle }
    }

    // ---- background results ---------------------------------------------------------------------------------

    fn drain_inbox(&mut self) {
        while let Ok(message) = self.inbox_rx.0.try_recv() {
            self.handle_inbound(message);
        }
    }

    fn handle_inbound(&mut self, message: Inbound) {
        match message {
            Inbound::Picked { kind, files } => match kind {
                PickKind::Document => {
                    for f in &files {
                        self.run(Origin::Ui, "doc.open", json!({ "source": source_of(f), "mode": "new_tab" }));
                    }
                }
                PickKind::Images => {
                    let images: Vec<Value> = files.iter().map(source_of).collect();
                    self.run(Origin::Ui, "image.insert", json!({ "images": images }));
                }
                PickKind::Library => {
                    for f in &files {
                        self.run(
                            Origin::Ui,
                            "library.import",
                            json!({ "source": source_of(f), "mode": "open_as_tab" }),
                        );
                    }
                }
                PickKind::LibraryImport => {
                    for f in &files {
                        self.run(Origin::Ui, "library.import", json!({ "source": source_of(f), "mode": "merge" }));
                    }
                }
            },
            Inbound::SaveTarget(to) => {
                self.run(Origin::Ui, "doc.save", json!({ "to": to, "overwrite": true }));
            }
            Inbound::ClipboardText(text) => self.ui.state.set_pasted_text(text),
            Inbound::Notice(text) => self.toast(text),
            Inbound::FontLoaded { file, bytes } => self.font_loaded(file, bytes),
        }
    }

    /// Asks the host for the fonts the typesetter and the UI need and do not have, once each. Nothing happens
    /// on a build that carries its fonts.
    fn request_fonts(&mut self) {
        let text = self.typesetter.peek(|ts| ts.wanted_fonts()).unwrap_or_default().into_iter().map(FontFile::Text);
        let ui = drafftink_ui::theme::wanted_fonts().into_iter().map(FontFile::Ui);
        for file in text.chain(ui) {
            if self.fonts_requested.insert(file) {
                self.host.start(HostJob::LoadFont(file), self.inbox.clone());
            }
        }
    }

    /// A font file arrived. The typesetter drops the layouts made without it, and the next frame draws again
    /// (`CanvasView::sync_fonts`); the UI installs it in its next pass.
    fn font_loaded(&mut self, file: FontFile, bytes: Vec<u8>) {
        match file {
            FontFile::Text(font) => {
                if let Err(e) = self.typesetter.with(|ts| ts.provide_font(font, bytes)) {
                    self.toast(format!("The font {} could not be used: {e}", file.file_name()));
                }
            }
            FontFile::Ui(font) => drafftink_ui::theme::provide_font(font, bytes),
        }
    }

    fn handle_host_request(&mut self, req: HostRequest, cx: &mut FrameCx<'_>) {
        let job = |h: &mut Box<dyn Host>, inbox: &Inbox, job: HostJob| h.start(job, inbox.clone());
        match req {
            HostRequest::PickFileToOpen => job(&mut self.host, &self.inbox, HostJob::PickDocument),
            HostRequest::PickImage => job(&mut self.host, &self.inbox, HostJob::PickImages),
            HostRequest::PickLibrary => job(&mut self.host, &self.inbox, HostJob::PickLibrary),
            HostRequest::PickLibraryImport => job(&mut self.host, &self.inbox, HostJob::PickLibraryImport),
            HostRequest::PickSaveTarget => {
                let name = self.session.tab().doc.store().meta().name.clone();
                let stem = if name.trim().is_empty() { "Untitled".to_owned() } else { name };
                job(&mut self.host, &self.inbox, HostJob::PickSaveTarget { suggested: format!("{stem}.drafftink") });
            }
            HostRequest::PasteText => job(&mut self.host, &self.inbox, HostJob::ReadClipboardText),
            HostRequest::PickRecent => {}
            HostRequest::CopyText(text) => cx.set_clipboard(ClipboardContent::text(text)),
            HostRequest::TextEditStyle(props) => {
                let now = self.now_ms();
                if let Some(editor) = self.editor.as_mut() {
                    editor.add_style(&props, now);
                }
            }
            HostRequest::OpenUrl(url) => cx.open_url(url, true),
        }
    }

    fn handle_files_dropped(&mut self, files: Vec<DroppedFile>) -> Repaint {
        use base64::Engine as _;
        let items: Vec<Value> = files
            .iter()
            .map(|f| {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&f.bytes);
                let mime = (!f.mime.is_empty()).then_some(f.mime.as_str());
                json!({ "name": f.name, "bytes": { "base64": b64, "name": f.name, "mime": mime }, "mime": mime })
            })
            .collect();
        let at = self.pointer_world().map(|p| json!({ "x": p.x, "y": p.y }));
        let mut args = json!({ "items": items });
        if let (Some(at), Some(map)) = (at, args.as_object_mut()) {
            map.insert("at".to_owned(), at);
        }
        self.run(Origin::Ui, "doc.drop", args);
        Repaint::Now(RedrawReason::Load)
    }

    // ---- per-frame work -------------------------------------------------------------------------------------

    fn sync_viewport(&mut self, size: Size, dpr: f64) {
        let now = ([size.width, size.height], dpr);
        if self.told_viewport == Some(now) || size.width < 1.0 || size.height < 1.0 {
            return;
        }
        let args = SetViewportArgs { width: size.width, height: size.height, scale_factor: dpr };
        if self.session.execute_typed(Origin::Ui, "view.set_viewport", &args).is_ok() {
            self.told_viewport = Some(now);
        }
    }

    /// Lets the collaboration client work. Returns whether it left something to show: a notice, or a change
    /// of the screen the session recorded.
    fn poll_collab(&mut self, now_ms: u64) -> bool {
        let due = self.collab_due_ms.is_some_and(|d| now_ms >= d);
        if !due && !self.in_room {
            return false;
        }
        let poll = self.session.collab_poll();
        self.collab_due_ms = poll.wake_in_ms.map(|ms| now_ms.saturating_add(ms.max(1)));
        self.in_room = self.session.collab_view().room.is_some();
        let mut shown = self.session.dirty().is_some();
        for notice in self.session.take_collab_notices() {
            shown = true;
            match notice {
                drafftink_engine::cmd::collab::CollabNotice::Info(t) => self.toast(t),
                drafftink_engine::cmd::collab::CollabNotice::Problem(p) => self.toast(p.text),
            }
        }
        shown
    }

    /// Esc and Enter belong to the interaction layer while it has something running (a drag, a line draft).
    fn feed_keys(&mut self, ctx: &egui::Context, now: f64) -> bool {
        if ctx.egui_wants_keyboard_input() || self.editor.is_some() {
            return false;
        }
        let mut need = false;
        for key in [egui::Key::Escape, egui::Key::Enter] {
            let pressed = ctx.input(|i| i.modifiers.is_none() && i.key_pressed(key));
            if !pressed {
                continue;
            }
            let k = if key == egui::Key::Escape {
                drafftink_engine::interact::Key::Escape
            } else {
                drafftink_engine::interact::Key::Enter
            };
            let out = self.interaction.key(&mut self.session, k, now);
            if out.handled {
                ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key));
            }
            need |= self.apply_outcome(out);
        }
        need
    }

    fn feed_wheel(&mut self, ctx: &egui::Context, now: f64) {
        if self.ui.state.modal_open() {
            return;
        }
        let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else { return };
        if input::egui_owns(ctx, self.canvas_rect, pos) {
            return;
        }
        let events = ctx.input(|i| (i.events.clone(), i.modifiers));
        let at = Point::new(f64::from(pos.x - self.canvas_rect.min.x), f64::from(pos.y - self.canvas_rect.min.y));
        let wheel = input::wheel_events(&events.0, at, now, events.1);
        self.canvas_input(&wheel);
    }

    fn theme_of(ctx: &egui::Context) -> Theme {
        if ctx.global_style().visuals.dark_mode { Theme::Dark } else { Theme::Light }
    }

    fn grid_of(&self) -> GridStyle {
        match self.session.settings.get("view.grid_style").and_then(Value::as_str) {
            Some("lines") => GridStyle::Lines,
            Some("horizontal_lines") => GridStyle::HorizontalLines,
            Some("crosses") => GridStyle::Crosses,
            Some("dots") => GridStyle::Dots,
            _ => GridStyle::None,
        }
    }

    /// Rebuilds the document and the overlay scenes if anything they show changed, and hands them to the shell.
    fn paint_canvas(&mut self, ctx: &egui::Context, cx: &mut FrameCx<'_>, now: f64) {
        let size = self.canvas_rect.size();
        let size = Size::new(f64::from(size.x), f64::from(size.y));
        let dpr = f64::from(cx.view.scale);
        let camera = self.interaction.camera();
        let theme = Self::theme_of(ctx);
        let grid = self.grid_of();
        let (hidden, edit_paint) = match self.editor.as_mut() {
            Some(editor) => {
                let arranged = self.typesetter.with(|ts| editor.paint(ts).clone());
                (editor.hidden(), Some((arranged, editor.picture_key())))
            }
            None => (Vec::new(), None),
        };
        let interaction_overlay = self.interaction.overlay(&self.session);
        let mut hide = hidden.clone();
        hide.extend(interaction_overlay.faded.iter().cloned());

        let tab = self.session.tab().id;
        let revision = self.session.store().revision();
        // Not drained while the index is current, so ids committed meanwhile are not lost.
        let touched =
            if self.canvas.index_is_current(tab, revision) { Vec::new() } else { self.session.tab().doc.take_touched() };
        self.canvas.sync_index(tab, revision, self.session.store(), &touched);
        let look = self.canvas.set_look(theme, grid) | self.canvas.sync_fonts();
        let hidden_changed = self.canvas.set_hidden(hide);
        let key = WorldKey {
            tab,
            revision,
            camera,
            size: [size.width, size.height],
            dpr,
            dark: theme == Theme::Dark,
            grid,
            editing: hidden.clone(),
        };
        let view = CanvasView::view_of(camera.scroll, camera.zoom, size, dpr);
        let world_stale = look || hidden_changed || self.world_key.as_ref() != Some(&key) || cx.resized;
        let peers: Vec<([f64; 2], String, String)> = self
            .session
            .collab_view()
            .peers
            .iter()
            .filter_map(|p| p.pointer.map(|pt| (pt, p.color.clone(), p.name.clone())))
            .collect();
        let caret_on = self.editor.as_ref().is_some_and(|e| e.caret_on(now));
        let edit_key = edit_paint.as_ref().map(|(_, picture)| (*picture, caret_on));
        let overlay_key = OverlayKey { interaction: interaction_overlay.clone(), peers: peers.clone(), edit: edit_key };
        let laser_running = !interaction_overlay.laser.is_empty();
        let overlay_stale = world_stale || laser_running || self.overlay_key.as_ref() != Some(&overlay_key);
        if !world_stale && !overlay_stale {
            return;
        }
        let _encode = drafftink_profile::phase(drafftink_profile::Phase::Encode);
        if world_stale {
            self.canvas.compose_world(self.session.store(), &view);
            self.world_key = Some(key);
        }
        let cursors: Vec<RemoteCursor> = peers
            .iter()
            .map(|(pt, color, name)| RemoteCursor {
                at: Point::new(pt[0], pt[1]),
                color: parse_hex(color),
                name: Some(name.clone()).filter(|n| !n.is_empty()),
            })
            .collect();
        let mut render_overlay = overlay::to_render(&interaction_overlay.canvas, &[]);
        render_overlay.cursors = cursors;
        if let Some(editor) = self.editor.as_mut() {
            render_overlay.text_edit = Some(self.typesetter.with(|ts| editor.overlay(ts, theme, now)));
        }
        let edited = edit_paint.as_ref().map(|(arranged, _)| arranged);
        self.canvas.compose_overlay(self.session.store(), &interaction_overlay, &render_overlay, edited, &view);
        let trail: Vec<TrailPoint> = interaction_overlay
            .laser
            .iter()
            .map(|p| TrailPoint { at: p.at, age_ms: (now - p.t_ms).max(0.0) })
            .filter(|p| p.age_ms < LASER_FADE_MS)
            .collect();
        let mut trails = vec![(Color::from_rgba8(239, 68, 68, 255), trail)];
        for peer in self.session.collab_view().peers {
            let points =
                peer.trail.iter().map(|d| TrailPoint { at: Point::new(d.x, d.y), age_ms: d.age_ms as f64 }).collect();
            trails.push((parse_hex(&peer.color), points));
        }
        self.canvas.add_trails(&trails, &view);
        self.overlay_key = Some(overlay_key);

        let base = self.canvas.background(self.session.store());
        self.canvas.write_to(cx.scene_mut(base));
    }

    /// Where the caret is in window points while a text is edited: the input method puts its window there, and the
    /// soft keyboard appears.
    fn text_input_rect(&mut self, canvas: egui::Vec2) -> Option<egui::Rect> {
        let editor = self.editor.as_mut()?;
        let camera = self.interaction.camera();
        let size = Size::new(f64::from(canvas.x), f64::from(canvas.y));
        let caret = self.typesetter.with(|ts| editor.caret_on_screen(ts, camera, size))?;
        let min = self.canvas_rect.min;
        Some(egui::Rect::from_min_max(
            egui::pos2(min.x + caret.x0 as f32, min.y + caret.y0 as f32),
            egui::pos2(min.x + caret.x1 as f32, min.y + caret.y1 as f32),
        ))
    }

    fn badge(&self, ctx: &egui::Context) {
        let overlay = self.interaction.overlay(&self.session);
        let Some(badge) = overlay.badge else { return };
        let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else { return };
        let text = match badge {
            Badge::Angle { degrees, .. } => format!("{degrees:.0}\u{b0}"),
            Badge::Size { w, h } => format!("{w:.0} \u{d7} {h:.0}"),
        };
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("drafftink-badge")));
        let at = pos + egui::vec2(14.0, 18.0);
        let galley = painter.layout_no_wrap(text, egui::FontId::proportional(12.0), egui::Color32::WHITE);
        let rect = egui::Align2::LEFT_TOP.anchor_size(at, galley.size()).expand2(egui::vec2(6.0, 3.0));
        painter.rect_filled(rect, 4.0, egui::Color32::from_rgba_unmultiplied(30, 30, 36, 220));
        painter.galley(rect.min + egui::vec2(6.0, 3.0), galley, egui::Color32::WHITE);
    }

    fn perf_hud(&self, ctx: &egui::Context) {
        if !self.perf_overlay {
            return;
        }
        let r = self.canvas.report();
        let text = format!(
            "frames {}  vello {}\nvisible {}/{}  drawn {}  cache hit {} miss {}",
            drafftink_gpu::frames_drawn(),
            drafftink_gpu::vello_runs(),
            r.in_view,
            r.total,
            r.drawn,
            r.cache_hits,
            r.cache_misses
        );
        egui::Area::new(egui::Id::unique("drafftink-perf-hud"))
            .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(8.0, -8.0))
            .show(ctx, |ui| {
                ui.label(egui::RichText::new(text).monospace().size(11.0));
            });
    }

    /// The earliest of the app's own timers as a repaint request.
    fn next_wake(&mut self, now: f64) -> Repaint {
        let mut best: Option<(f64, RedrawReason)> = None;
        let mut consider = |at_ms: f64, reason: RedrawReason| {
            if best.is_none_or(|(b, _)| at_ms < b) {
                best = Some((at_ms, reason));
            }
        };
        if let Some(d) = self.interaction.next_deadline() {
            consider(d, RedrawReason::Timer);
        }
        if let Some(d) = self.collab_due_ms {
            consider(d as f64, RedrawReason::Collab);
        }
        if let Some(editor) = self.editor.as_ref() {
            consider(editor.next_blink_ms(now), RedrawReason::CaretBlink);
        }
        if let Some(ms) = self.session.workspace_next_due_ms() {
            consider(now + ms as f64, RedrawReason::Autosave);
        }
        match best {
            None => Repaint::Idle,
            Some((at, reason)) => {
                let wait = (at - now).max(0.0);
                if wait < 1.0 {
                    Repaint::Now(reason)
                } else {
                    Repaint::After(Duration::from_secs_f64(wait / 1000.0), reason)
                }
            }
        }
    }
}

/// `#rrggbb` as a colour (grey when it cannot be read).
fn parse_hex(text: &str) -> Color {
    let hex = text.trim_start_matches('#');
    let channel = |i: usize| hex.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok());
    match (channel(0), channel(2), channel(4)) {
        (Some(r), Some(g), Some(b)) => Color::from_rgba8(r, g, b, 255),
        _ => Color::from_rgba8(120, 120, 120, 255),
    }
}

/// A picked file as the `source` of a command.
fn source_of(file: &PickedFile) -> Value {
    use base64::Engine as _;
    match file {
        PickedFile::Ref(r) => serde_json::to_value(r).unwrap_or(Value::Null),
        PickedFile::Bytes { name, bytes } => {
            json!({ "base64": base64::engine::general_purpose::STANDARD.encode(bytes), "name": name })
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.session.flush_workspace();
    }
}

impl ShellApp for App {
    fn setup(&mut self, cx: &mut SetupCx<'_>) {
        self.wake.set(cx.waker.clone());
        // A new egui context has no fonts or theme of ours; the UI installs them on its first pass.
        self.world_key = None;
        self.overlay_key = None;
    }

    fn event(&mut self, cx: &mut EventCx<'_>, event: ShellEvent) -> (Repaint, Handled) {
        self.egui_time = cx.ctx.input(|i| i.time);
        match event {
            ShellEvent::Pointer(sample) => (self.handle_pointer(cx, sample), Handled::No),
            ShellEvent::Paste(content) => {
                if self.editor.is_some() {
                    return (self.editor_paste(content), Handled::Yes);
                }
                let take = !cx.ctx.egui_wants_keyboard_input() && !self.ui.state.modal_open();
                if !take {
                    return (Repaint::Idle, Handled::No);
                }
                self.clipboard.set_incoming(Some(content));
                let args = crate::clip::paste_args(self.pointer_world());
                if let Err(r) = self.session.execute_from(Origin::Shortcut, "clipboard.paste", args)
                    && !drafftink_ui::action::quiet_from_keyboard(r.code)
                {
                    self.toast_refusal(&r);
                }
                (Repaint::Now(RedrawReason::Command), Handled::Yes)
            }
            ShellEvent::CopyRequested { cut } => {
                if self.editor.is_some() {
                    return (self.editor_copy(cx, cut), Handled::Yes);
                }
                if cx.ctx.egui_wants_keyboard_input() || self.ui.state.modal_open() {
                    return (Repaint::Idle, Handled::No);
                }
                let id = if cut { "clipboard.cut" } else { "clipboard.copy" };
                match self.session.execute_from(Origin::Shortcut, id, json!({})) {
                    Ok(_) => {
                        if let Some(content) = self.clipboard.take_outgoing() {
                            cx.set_clipboard(content);
                        }
                        (Repaint::Now(RedrawReason::Command), Handled::Yes)
                    }
                    Err(r) => {
                        if !drafftink_ui::action::quiet_from_keyboard(r.code) {
                            self.toast_refusal(&r);
                        }
                        (Repaint::Now(RedrawReason::Ui), Handled::No)
                    }
                }
            }
            ShellEvent::Wheel(wheel) => match self.handle_wheel(&wheel) {
                Some(repaint) => (repaint, Handled::Yes),
                None => (Repaint::Idle, Handled::No),
            },
            ShellEvent::FilesDropped(files) => (self.handle_files_dropped(files), Handled::No),
            ShellEvent::Visibility(false) | ShellEvent::Focus(false) => {
                if self.held.any() {
                    let out = self.interaction.input(&mut self.session, &drafftink_canvas::InputEvent::CancelAll);
                    self.apply_outcome(out);
                    self.held = Held::default();
                    self.canvas_drag = false;
                }
                self.session.flush_workspace();
                (Repaint::Idle, Handled::No)
            }
            ShellEvent::GpuRecreated => {
                self.world_key = None;
                self.overlay_key = None;
                (Repaint::Now(RedrawReason::Surface), Handled::No)
            }
            // Socket traffic (a server ping, a peer's unchanged presence) usually changes nothing on screen:
            // work it off here and ask for a frame only when something is to be shown.
            ShellEvent::Wake(RedrawReason::Collab) => {
                let now = self.now_ms();
                let repaint =
                    if self.poll_collab(now as u64) { Repaint::Now(RedrawReason::Collab) } else { self.next_wake(now) };
                (repaint, Handled::No)
            }
            ShellEvent::Wake(reason) => (Repaint::Now(reason), Handled::No),
            ShellEvent::TextInput(input) => {
                let now = self.now_ms();
                let changed = self.editor.as_mut().is_some_and(|e| e.text_input(&input, now));
                (if changed { Repaint::Now(RedrawReason::Command) } else { Repaint::Idle }, Handled::Yes)
            }
            ShellEvent::FileHover(_) | ShellEvent::Visibility(true) | ShellEvent::Focus(true) => {
                (Repaint::Idle, Handled::No)
            }
        }
    }

    fn frame(&mut self, ui: &mut egui::Ui, cx: &mut FrameCx<'_>) -> FrameOutput {
        let ctx = ui.ctx().clone();
        let now = self.now_ms();
        let now_ms = now as u64;
        self.egui_time = ctx.input(|i| i.time);
        self.canvas_rect = ctx.content_rect();
        for text in std::mem::take(&mut self.notices) {
            self.toast(text);
        }
        let size = self.canvas_rect.size();
        self.sync_viewport(Size::new(f64::from(size.x), f64::from(size.y)), f64::from(cx.view.scale));
        self.install_scene(Size::new(f64::from(size.x), f64::from(size.y)));
        // The spatial index of a big document takes a moment to build (0.7 s at 100,000 elements). Build it here,
        // in the frame that follows a load, so the first hover or drag does not pay for it. It follows the edits
        // of every later frame by the ids they touched, which is what the next hit test would do anyway.
        drop(self.session.tab().doc.scene_index());

        for url in self.open_urls.drain(..) {
            cx.open_url(url, true);
        }
        // Work that arrived while no frame ran.
        self.drain_inbox();
        if let Some(host) = self.bridge.as_mut() {
            host.poll(&mut self.session);
        }
        self.poll_collab(now_ms);
        self.session.workspace_tick();

        // The interaction layer follows the session (a command may have moved the camera) and runs its timers.
        let out = self.interaction.tick(&mut self.session, now);
        self.apply_outcome(out);
        self.feed_keys(&ctx, now);
        self.feed_wheel(&ctx, now);

        // The text editor goes first: it takes Esc, Ctrl+Enter and the editing keys before the keymap sees them.
        self.sync_editor();
        self.stage_editor();
        self.editor_keys(&ctx, now);
        self.stage_editor();

        // The UI.
        let collab = collab_view_of(&mut self.session);
        self.ui.set_collab(collab);
        let pointer = Pointer {
            cursor: self.pointer_world().map(|p| (p.x, p.y)),
            viewport_center: Some((self.interaction.camera().scroll.x, self.interaction.camera().scroll.y)),
        };
        let result = self.ui.frame(ui, &mut self.session, FrameInput { pointer });
        for request in result.report.host {
            self.handle_host_request(request, cx);
        }
        self.sync_editor();
        self.badge(&ctx);
        self.perf_hud(&ctx);
        if let Some(content) = self.clipboard.take_outgoing() {
            cx.set_clipboard(content);
        }

        // Commands may have changed the camera or the document again.
        let _ = self.session.take_dirty();
        let out = self.interaction.tick(&mut self.session, now);
        self.apply_outcome(out);
        self.paint_canvas(&ctx, cx, now);
        self.request_fonts();

        let over_canvas =
            ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| !input::egui_owns(&ctx, self.canvas_rect, p));
        let text_input = self.text_input_rect(size);
        FrameOutput {
            repaint: self.next_wake(now),
            canvas_rect: Some(self.canvas_rect),
            text_input,
            cursor: over_canvas.then_some(self.cursor),
        }
    }
}

/// What the UI shows about collaboration: `collab.status`, read as a query so it leaves no ring entry.
fn collab_view_of(s: &mut Session) -> drafftink_ui::CollabView {
    s.query("collab.status", json!({})).map(|out| drafftink_ui::CollabView::from_status(&out)).unwrap_or_default()
}
