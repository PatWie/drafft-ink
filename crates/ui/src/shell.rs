//! One frame of the UI: refresh the model, read the keys, draw every surface, execute the actions.

use drafftink_engine::Session;
use egui::Rect;

use crate::action::{Report, UiAction, dispatch};
use crate::keymap::{KeyContext, Pointer, resolve_keys};
use crate::model::{AgentServerView, CollabView, Model, TextFaces};
use crate::state::{TOAST_FADE_S, TOAST_HOLD_S, UiState};
use crate::surfaces;
use crate::theme::{self, Palette};
use crate::thumbs::Want;
use crate::widgets::{Cx, Layout};

/// What the app tells the UI about the frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameInput {
    /// Pointer placeholders for shortcuts that take a position (`$cursor`).
    pub pointer: Pointer,
    /// The host delivers copy, cut and paste as clipboard events and runs those commands from them. The keymap
    /// then leaves the clipboard chords alone, so one key press is one command (`Binding::host_clipboard`).
    pub host_clipboard: bool,
}

/// What the UI tells the app about the frame.
#[derive(Debug)]
pub struct FrameResult {
    /// The commands that ran and the requests for the app.
    pub report: Report,
    /// The area the canvas has: the whole window.
    pub canvas_rect: Rect,
    /// A modal dialog is open, so pointer events must not reach the canvas.
    pub modal: bool,
    /// What the debug panel cost in this frame (nothing while it is closed).
    pub debug: crate::debug::DebugCost,
}

/// The UI: its own state and the model snapshot.
pub struct UiShell {
    /// What the UI remembers between frames.
    pub state: UiState,
    /// The snapshot of the session.
    pub model: Model,
}

impl UiShell {
    /// A UI. `mac` is true on macOS, where `Mod` is Cmd.
    pub fn new(mac: bool) -> Self {
        Self { state: UiState::new(), model: Model::new(mac) }
    }

    /// Presence data from the collaboration client, which the app owns.
    pub fn set_collab(&mut self, collab: CollabView) {
        self.model.collab = collab;
    }

    /// What the debug panel shows. The app sets it in every frame while [`UiState::debug_open`] is set and clears it
    /// (`None`) when the panel is closed.
    pub fn set_debug(&mut self, debug: Option<crate::debug::DebugView>) {
        self.model.debug = debug;
    }

    /// Bold and italic at the selection of the running text editor, which the app owns (`None`: no editor).
    pub fn set_editor_faces(&mut self, faces: Option<TextFaces>) {
        self.model.editor_faces = faces;
    }

    /// What the About dialog shows: the commit, and the source address of a hosted copy.
    pub fn set_about(&mut self, about: crate::model::AboutInfo) {
        self.model.about = about;
    }

    /// The state of the MCP server over HTTP, which the app owns. `None` where there is none (the web).
    pub fn set_agent_server(&mut self, view: Option<AgentServerView>) {
        self.model.agent_server = view;
    }

    /// The device is touch-first (a coarse pointer on the web, a tablet). Touch events set it too. The touch
    /// layout follows it unless the preferences say otherwise.
    pub fn set_touch_first(&mut self, touch_first: bool) {
        self.state.set_touch_first(touch_first);
    }

    /// Draws one frame and executes the actions it produced. Call once per egui pass.
    pub fn frame(&mut self, ui: &mut egui::Ui, session: &mut Session, input: FrameInput) -> FrameResult {
        let ctx = ui.ctx().clone();
        self.model.mac = ctx.os().is_mac();
        self.sync_library();
        self.model.refresh(session);
        self.apply_theme(&ctx);

        let events = ctx.input(|i| i.events.clone());
        let wants_keyboard = ctx.egui_wants_keyboard_input();
        let mut actions = resolve_keys(
            &events,
            &KeyContext {
                model: &self.model,
                state: &self.state,
                egui_wants_keyboard: wants_keyboard,
                pointer: input.pointer,
                host_clipboard: input.host_clipboard,
            },
        );
        actions.extend(draw(ui, &self.model, &mut self.state));
        // Commands that wait for a picture run again before anything new.
        actions.splice(0..0, self.state.retries().into_iter().map(UiAction::Command));

        let now = ctx.input(|i| i.time);
        let report = dispatch(session, &mut self.state, actions, now);
        if self.sync_library() {
            // The library panel was opened or closed by a verb: read (or stop reading) the library.
            self.model.refresh(session);
            ctx.request_repaint();
        }
        if !report.ran.is_empty() {
            self.model.invalidate();
            self.model.refresh(session);
            // The state the user sees changed: one more frame to show it.
            ctx.request_repaint();
        }
        self.draw_pictures(&ctx, session);
        self.schedule_toasts(&ctx, now);
        FrameResult {
            report,
            canvas_rect: ctx.content_rect(),
            modal: self.state.modal_open(),
            debug: self.state.debug_cost,
        }
    }

    /// Asks the session for the next few pictures of the library items, for the tiles the drawer just drew. A
    /// picture that arrives, or work left over the budget, asks for one more frame; a picture the browser is still
    /// drawing does not (the renderer wakes the app when it is done).
    fn draw_pictures(&mut self, ctx: &egui::Context, session: &mut Session) {
        let want = Want {
            dark: Palette::of(ctx).dark,
            visible: &self.state.library_visible,
            hovered: self.state.library_hover.as_deref(),
        };
        let progress = self.model.read_thumbnails(session, &want);
        if progress.changed || progress.more {
            ctx.request_repaint();
        }
    }

    /// The model reads `library.list` only while the panel is open. Returns true when that changed.
    fn sync_library(&mut self) -> bool {
        let changed = self.model.wants_library != self.state.library_open;
        if changed {
            self.model.wants_library = self.state.library_open;
            self.model.invalidate();
        }
        changed
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        let want = self.model.setting_str("theme").unwrap_or("system").to_owned();
        if self.state.applied_theme.as_deref() != Some(want.as_str()) {
            theme::apply(ctx, Some(&want));
            self.state.applied_theme = Some(want);
        }
    }

    /// Repaints only while a toast is on screen: once when it starts to fade, then at 30 Hz during the fade.
    fn schedule_toasts(&mut self, ctx: &egui::Context, now: f64) {
        self.state.expire_toasts(now);
        let mut next: Option<f64> = None;
        for t in &self.state.toasts {
            let age = now - t.born;
            let wait = if age < TOAST_HOLD_S {
                TOAST_HOLD_S - age
            } else if age < TOAST_HOLD_S + TOAST_FADE_S {
                1.0 / 30.0
            } else {
                0.0
            };
            next = Some(next.map_or(wait, |n: f64| n.min(wait)));
        }
        if let Some(wait) = next {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait.max(0.0)));
        }
    }
}

/// Draws every surface from the model and returns the actions of the frame, in drawing order.
/// It reads the session through the model only and changes nothing but `state`.
pub fn draw(ui: &mut egui::Ui, model: &Model, state: &mut UiState) -> Vec<UiAction> {
    let mut out = Vec::new();
    let ctx = ui.ctx().clone();
    theme::ensure_fonts(&ctx, theme::UiFontChoice::of_model(model));
    state.chrome = crate::chrome::ChromeRects::default();
    let pal = Palette::of(&ctx);
    state.observe_input(&ctx);
    state.dark = pal.dark;
    let screen = ctx.content_rect().size();
    let layout = Layout { compact: state.compact(screen.x), narrow: state.narrow(screen.x), screen };
    let mut cx = Cx::new(model, pal, &mut out).with_layout(layout).with_scrub(state.scrub.frame());
    surfaces::show_all(&ctx, &mut cx, state);
    let scrubs = std::mem::take(&mut cx.scrub);
    let focus_lost = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::WindowFocused(false))));
    state.scrub.settle(scrubs, focus_lost, &mut out);
    out
}
