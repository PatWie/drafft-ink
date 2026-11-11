//! From pointer and touch input to commands (docs/commands.md section 7.3, docs/exceptions.md).
//!
//! [`Interaction`] is the layer between the recognizers of `drafftink-canvas` and the command registry, so that
//! the shells stay thin and a test can drive a real pointer sequence without a window. It takes the
//! [`Intent`]s of a [`Recognizer`] (press, click, drag, hover, pan, zoom, ...), knows the current tool, the
//! selection and the editing group (it reads them from the [`Session`]), and produces:
//!
//! * **commands** through `Session::execute` and the typed gesture methods: `selection.select_at`,
//!   `element.create` with `from_tool`, `gesture.begin/update/end` of `element.move`, `element.resize`,
//!   `element.rotate`, `arrow.move_point` and `arrow.move_segment`, `selection.select_region`, `group.enter`,
//!   `text.edit_begin`, `element.erase`, `view.set`;
//! * **overlay state** ([`InteractionOverlay`]): the drawing preview, the marquee, snapping guides, binding
//!   candidates, the hover outline, handles;
//! * an [`Outcome`] per call: whether and how much to redraw, the mouse cursor, UI to open, refusals to show.
//!
//! Each row of `docs/exceptions.md` that concerns a pointer is implemented here exactly as documented there:
//! drag-move, resize, rotate and point dragging stream typed gesture frames (class A); drawing, freehand, the
//! eraser and the marquee keep a preview and run one command at the end (class B); pan and zoom move the
//! camera directly and report it with `view.set` (class C); the laser trail is ephemeral (class E).
//!
//! ## Entry points
//!
//! | Call | Use |
//! |---|---|
//! | [`Interaction::input`] | the normal door: a platform-neutral [`InputEvent`] (pointer, wheel, leave, cancel) |
//! | [`Interaction::run_script`] | a recorded list of input events (the test harness's pointer script) |
//! | [`Interaction::handle`] | an already recognized [`Intent`], for tests that script intents |
//! | [`Interaction::key`] | Enter and Escape, which the layer owns while a drawing or a drag is running |
//! | [`Interaction::tick`] | time passes: long press, the throttled camera report, the laser fade |
//! | [`Interaction::next_deadline`] | when to call `tick` next (`None`: no timer at all) |
//! | [`Interaction::overlay`] | what to draw above the scene |
//! | [`Interaction::camera`] | the live camera, which can be ahead of `Session::view` during a pan |
//!
//! No UI toolkit type appears here, and time is always passed in. `docs/interaction.md` is the guide.

mod ctx;
mod events;
mod frame_out;
mod link;
mod outcome;
mod overlay;
mod view;
mod world;

mod draw;
mod select;

#[cfg(test)]
mod testutil;

#[cfg(test)]
mod interaction_tests;

use drafftink_canvas::{
    Camera, Cursor, InputEvent, Intent, Modifiers, PointerEvent, PointerKind, PointerPhase, Recognizer, Target,
    ViewSource,
};
use drafftink_geom::kurbo::Point;
use drafftink_scene::{ElementId, SceneStore};

use ctx::Ctx;
use draw::DrawTools;
pub use draw::NewArrowPath;
use events::Ev;
use select::SelectTool;
use view::ViewSync;
use world::{World, pointer_mode};

pub use events::Key;
pub use outcome::{ContextTarget, Outcome, UiRequest};
pub use overlay::{Badge, InteractionOverlay, LaserPoint, LinkIcon, OverlayParts};
pub use view::REPORT_EVERY_MS;

use crate::session::Session;
use crate::spec::Redraw;
use crate::state::Tool;

/// Which machine owns the pointer drag that is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    Select,
    Draw,
}

/// Turns input into commands for one session (one per tab is fine; it follows the active tab).
pub struct Interaction {
    recognizer: Recognizer,
    world: World,
    view: ViewSync,
    parts: OverlayParts,
    select: SelectTool,
    draw: DrawTools,
    drag_owner: Option<Owner>,
    /// The element under an idle pointer, for the hover outline.
    hover: Option<ElementId>,
    cursor: Cursor,
    /// The last pointer position seen in an input event, in screen pixels.
    pointer: Option<(Point, PointerKind)>,
    /// The active tool at the last event (a change drops drafts).
    last_tool: Tool,
    /// Store revision and camera at the last hover refresh.
    refreshed: Option<(u64, Camera)>,
}

impl Default for Interaction {
    fn default() -> Self {
        Self::new()
    }
}

impl Interaction {
    /// A new interaction layer with default thresholds.
    pub fn new() -> Self {
        Self {
            recognizer: Recognizer::default(),
            world: World::new(),
            view: ViewSync::default(),
            parts: OverlayParts::default(),
            select: SelectTool::default(),
            draw: DrawTools::default(),
            drag_owner: None,
            hover: None,
            cursor: Cursor::Default,
            pointer: None,
            last_tool: Tool::Select,
            refreshed: None,
        }
    }

    // ---- entry points -----------------------------------------------------------------------------------------

    /// Feeds one platform-neutral input event through the recognizer and acts on the intents it produces.
    pub fn input(&mut self, s: &mut Session, ev: &InputEvent) -> Outcome {
        let (now, mods) = match ev {
            InputEvent::Pointer(p) => (p.time_ms, p.modifiers),
            InputEvent::Wheel(w) => (w.time_ms, w.modifiers),
            InputEvent::PointerLeave { .. } | InputEvent::CancelAll => (self.last_now(s), Modifiers::NONE),
        };
        let mut out = Outcome::default();
        self.begin(s, &mut out);
        if let InputEvent::Pointer(p) = ev {
            self.pointer = Some((p.pos, p.kind));
        }
        let mut intents = Vec::new();
        {
            let pick = self.world.picker(s);
            self.recognizer.handle(ev, &pick, &mut intents);
        }
        if let InputEvent::Pointer(p) = ev
            && p.phase == PointerPhase::Move
            && p.buttons.is_empty()
        {
            self.pointer_moved(s, p, &mut out);
        }
        for intent in intents {
            self.dispatch(s, intent, now, mods, &mut out);
        }
        if matches!(ev, InputEvent::CancelAll) {
            self.cancel_machines(s, now, &mut out);
        }
        self.finish(s, now, &mut out);
        out
    }

    /// Feeds a recorded or scripted sequence of events (the pointer script of the test harness) and returns the
    /// outcome of each. Events carry their own times, so the run is deterministic.
    pub fn run_script(&mut self, s: &mut Session, events: &[InputEvent]) -> Vec<Outcome> {
        events.iter().map(|ev| self.input(s, ev)).collect()
    }

    /// Convenience for a pointer event.
    pub fn pointer(&mut self, s: &mut Session, ev: PointerEvent) -> Outcome {
        self.input(s, &InputEvent::Pointer(ev))
    }

    /// Acts on one recognized intent. `mods` are the modifier keys now; intents that carry their own
    /// (`Meta`) use those for the pointer, and `mods` for the rest.
    pub fn handle(&mut self, s: &mut Session, intent: Intent, now_ms: f64, mods: Modifiers) -> Outcome {
        let mut out = Outcome::default();
        self.begin(s, &mut out);
        self.dispatch(s, intent, now_ms, mods, &mut out);
        self.finish(s, now_ms, &mut out);
        out
    }

    /// Enter or Escape. `Outcome::handled` tells the shell whether the layer used the key; when it did not,
    /// the shell runs the key's command (`tool.cancel` for Escape).
    pub fn key(&mut self, s: &mut Session, key: Key, now_ms: f64) -> Outcome {
        let mut out = Outcome::default();
        self.begin(s, &mut out);
        let mods = Modifiers::NONE;
        if key == Key::Escape {
            // A running pointer drag is cancelled first: the recognizer reports it as a cancelled drag end.
            let mut intents = Vec::new();
            {
                let pick = self.world.picker(s);
                self.recognizer.handle(&InputEvent::CancelAll, &pick, &mut intents);
            }
            out.handled |= !intents.is_empty();
            for intent in intents {
                self.dispatch(s, intent, now_ms, mods, &mut out);
            }
            if s.gesture_running() && !out.handled {
                // A gesture the recognizer does not know about (a script began it): cancel it too.
                let _ = s.gesture_cancel(None);
                out.handled = true;
            }
        }
        if !out.handled {
            let used = self.with_ctx(s, &mut out, now_ms, mods, |cx, m| m.draw.key(cx, key) || m.select.key(cx, key));
            out.handled |= used;
        }
        self.finish(s, now_ms, &mut out);
        out
    }

    /// Time passes. Produces a long press when one is due, reports a throttled camera move, and ages the
    /// laser trail. Call it at [`Interaction::next_deadline`].
    pub fn tick(&mut self, s: &mut Session, now_ms: f64) -> Outcome {
        let mut out = Outcome::default();
        self.begin(s, &mut out);
        let mut intents = Vec::new();
        self.recognizer.tick(now_ms, &mut intents);
        for intent in intents {
            self.dispatch(s, intent, now_ms, Modifiers::NONE, &mut out);
        }
        self.with_ctx(s, &mut out, now_ms, Modifiers::NONE, |cx, m| m.draw.tick(cx));
        self.finish(s, now_ms, &mut out);
        out
    }

    /// When [`Interaction::tick`] next has something to do, if ever. `None` means: no timer.
    pub fn next_deadline(&self) -> Option<f64> {
        [self.recognizer.next_deadline(), self.view.deadline(), self.draw.deadline()]
            .into_iter()
            .flatten()
            .min_by(f64::total_cmp)
    }

    // ---- reading ----------------------------------------------------------------------------------------------

    /// Everything to draw above the scene.
    pub fn overlay(&self, s: &Session) -> InteractionOverlay {
        overlay::build(&self.world, s, &self.parts, self.hover.as_ref())
    }

    /// The live camera. During a pan or zoom it can be ahead of `Session::view`, which is told with `view.set`
    /// at the end and at most every [`REPORT_EVERY_MS`].
    pub fn camera(&self) -> Camera {
        self.world.viewport.camera()
    }

    /// The mouse cursor to show now.
    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// True while a pointer gesture, a drawing preview or a multi-point line draft is running.
    pub fn is_busy(&self) -> bool {
        self.drag_owner.is_some() || self.draw.is_active() || self.select.is_active() || !self.recognizer.is_idle()
    }

    /// The last pointer position seen in an input event (screen pixels).
    pub fn pointer_position(&self) -> Option<Point> {
        self.pointer.map(|(p, _)| p)
    }

    /// The overlay state the tool machines own (read only; for tests and debugging).
    pub fn parts(&self) -> &OverlayParts {
        &self.parts
    }

    // ---- internals --------------------------------------------------------------------------------------------

    fn last_now(&self, s: &Session) -> f64 {
        // Events without a timestamp (leave, cancel) use the session clock.
        s.ports.clock.mono_ms() as f64
    }

    /// Start of every call: the world follows the session, drafts die with a tool change.
    fn begin(&mut self, s: &mut Session, _out: &mut Outcome) {
        self.view.begin(&mut self.world, s);
        self.world.sync(s);
        let active = s.tool.active;
        if active != self.last_tool {
            self.last_tool = active;
            self.drop_machines(s);
        }
        self.recognizer.set_mode(pointer_mode(s.tool.effective()));
    }

    /// End of every call: report the camera when the throttle allows, pick again under a resting pointer
    /// when the scene or the camera moved under it.
    fn finish(&mut self, s: &mut Session, now_ms: f64, out: &mut Outcome) {
        self.view.report(&self.world, s, now_ms, false, out);
        self.world.sync(s);
        let key = (s.store().revision(), self.world.viewport.camera());
        if self.refreshed != Some(key) {
            self.refreshed = Some(key);
            let mut intents = Vec::new();
            {
                let pick = self.world.picker(s);
                self.recognizer.refresh_hover(&pick, &mut intents);
            }
            for intent in intents {
                self.dispatch(s, intent, now_ms, Modifiers::NONE, out);
            }
        }
    }

    fn pointer_moved(&mut self, s: &mut Session, p: &PointerEvent, out: &mut Outcome) {
        let world = self.world.viewport.screen_to_world(p.pos);
        let (pos, mods, now) = (p.pos, p.modifiers, p.time_ms);
        if !matches!(s.tool.effective(), Tool::Select | Tool::Hand) {
            self.with_ctx(s, out, now, mods, |cx, m| m.draw.pointer_moved(cx, pos, world));
        }
    }

    /// Drops the machines' state without a command (tool change, tab switch).
    fn drop_machines(&mut self, s: &mut Session) {
        let mut out = Outcome::default();
        let now = self.last_now(s);
        self.cancel_machines(s, now, &mut out);
    }

    fn cancel_machines(&mut self, s: &mut Session, now_ms: f64, out: &mut Outcome) {
        self.with_ctx(s, out, now_ms, Modifiers::NONE, |cx, m| {
            m.select.cancel(cx);
            m.draw.cancel(cx);
        });
        self.drag_owner = None;
        self.parts.clear_gesture();
        out.redraw_at_least(Redraw::Overlay);
    }

    fn with_ctx<R>(
        &mut self,
        s: &mut Session,
        out: &mut Outcome,
        now_ms: f64,
        mods: Modifiers,
        f: impl FnOnce(&mut Ctx<'_>, &mut Machines<'_>) -> R,
    ) -> R {
        let mut cx = Ctx { s, world: &self.world, parts: &mut self.parts, out, now_ms, mods };
        let mut machines = Machines { select: &mut self.select, draw: &mut self.draw };
        f(&mut cx, &mut machines)
    }

    fn dispatch(&mut self, s: &mut Session, intent: Intent, now: f64, mods: Modifiers, out: &mut Outcome) {
        match intent {
            Intent::Pan { delta, source } => {
                self.view.pan(&mut self.world, delta, out);
                if matches!(source, ViewSource::HandTool | ViewSource::MiddleButton) {
                    self.set_cursor(Cursor::Grabbing, out);
                }
                self.view.report(&self.world, s, now, false, out);
            }
            Intent::Zoom { at, factor, .. } => {
                self.view.zoom(&mut self.world, at, factor, out);
                self.view.report(&self.world, s, now, false, out);
            }
            Intent::ViewEnd => {
                self.view.report(&self.world, s, now, true, out);
                let rest = if s.tool.effective() == Tool::Hand { Cursor::Grab } else { Cursor::Default };
                self.set_cursor(rest, out);
            }
            Intent::Hover { to, cursor, .. } => self.hovered(s, &to, cursor, out),
            other => {
                // Every other intent may run a command: the session must know the camera first.
                self.view.report(&self.world, s, now, true, out);
                self.pointer_intent(s, other, now, mods, out);
            }
        }
    }

    fn set_cursor(&mut self, cursor: Cursor, out: &mut Outcome) {
        if cursor != self.cursor {
            self.cursor = cursor;
            out.cursor = Some(cursor);
        }
    }

    /// E1: hover costs a redraw only when the outline it shows changes.
    fn hovered(&mut self, s: &Session, to: &Target, cursor: Cursor, out: &mut Outcome) {
        self.set_cursor(cursor, out);
        let shows_outline = matches!(s.tool.effective(), Tool::Select | Tool::Text | Tool::Math);
        let next = to.element().filter(|_| shows_outline).cloned();
        if next != self.hover {
            self.hover = next;
            out.redraw_at_least(Redraw::Overlay);
        }
    }

    fn pointer_intent(&mut self, s: &mut Session, intent: Intent, now: f64, mods: Modifiers, out: &mut Outcome) {
        let tool = s.tool.effective();
        match intent {
            Intent::Press { at, target, meta } => {
                let ev = Ev {
                    origin: at,
                    at,
                    delta_screen: Default::default(),
                    delta_world: Default::default(),
                    target: &target,
                    meta: &meta,
                };
                let mods = meta.modifiers;
                match route(tool, &target) {
                    Route::Select => self.with_ctx(s, out, now, mods, |cx, m| m.select.press(cx, &ev)),
                    Route::Draw => self.with_ctx(s, out, now, mods, |cx, m| m.draw.press(cx, &ev)),
                    Route::None => {}
                }
            }
            Intent::Click { at, target, meta } => {
                let ev = Ev {
                    origin: at,
                    at,
                    delta_screen: Default::default(),
                    delta_world: Default::default(),
                    target: &target,
                    meta: &meta,
                };
                let mods = meta.modifiers;
                match click_route(tool) {
                    Route::Select => self.with_ctx(s, out, now, mods, |cx, m| {
                        if !link::click(cx, &ev) {
                            m.select.click(cx, &ev);
                        }
                    }),
                    Route::Draw => self.with_ctx(s, out, now, mods, |cx, m| m.draw.click(cx, &ev)),
                    Route::None => {}
                }
            }
            Intent::DoubleClick { at, target, meta } => {
                let ev = Ev {
                    origin: at,
                    at,
                    delta_screen: Default::default(),
                    delta_world: Default::default(),
                    target: &target,
                    meta: &meta,
                };
                let mods = meta.modifiers;
                match click_route(tool) {
                    Route::Select => self.with_ctx(s, out, now, mods, |cx, m| m.select.double_click(cx, &ev)),
                    Route::Draw => self.with_ctx(s, out, now, mods, |cx, m| m.draw.double_click(cx, &ev)),
                    Route::None => {}
                }
            }
            Intent::LongPress { at, target, meta } | Intent::ContextMenu { at, target, meta } => {
                self.context_menu(s, at.screen, at.world, &target, meta.modifiers, now, out);
            }
            Intent::DragStart { origin, at, target, meta } => {
                let owner = match route(tool, &target) {
                    Route::Select => Some(Owner::Select),
                    Route::Draw => Some(Owner::Draw),
                    Route::None => None,
                };
                self.drag_owner = owner;
                let ev = Ev {
                    origin,
                    at,
                    delta_screen: at.screen - origin.screen,
                    delta_world: at.world - origin.world,
                    target: &target,
                    meta: &meta,
                };
                let mods = meta.modifiers;
                match owner {
                    Some(Owner::Select) => self.with_ctx(s, out, now, mods, |cx, m| m.select.drag_start(cx, &ev)),
                    Some(Owner::Draw) => self.with_ctx(s, out, now, mods, |cx, m| m.draw.drag_start(cx, &ev)),
                    None => {}
                }
            }
            Intent::DragUpdate { origin, at, delta_screen, delta_world, target, meta } => {
                let ev = Ev { origin, at, delta_screen, delta_world, target: &target, meta: &meta };
                let mods = meta.modifiers;
                match self.drag_owner {
                    Some(Owner::Select) => self.with_ctx(s, out, now, mods, |cx, m| m.select.drag_update(cx, &ev)),
                    Some(Owner::Draw) => self.with_ctx(s, out, now, mods, |cx, m| m.draw.drag_update(cx, &ev)),
                    None => {}
                }
            }
            Intent::DragEnd { origin, at, delta_screen, delta_world, target, cancelled, meta } => {
                let ev = Ev { origin, at, delta_screen, delta_world, target: &target, meta: &meta };
                let mods = meta.modifiers;
                let owner = self.drag_owner.take();
                match owner {
                    Some(Owner::Select) => {
                        self.with_ctx(s, out, now, mods, |cx, m| m.select.drag_end(cx, &ev, cancelled))
                    }
                    Some(Owner::Draw) => self.with_ctx(s, out, now, mods, |cx, m| m.draw.drag_end(cx, &ev, cancelled)),
                    None => {}
                }
                if owner.is_some() {
                    out.redraw_at_least(Redraw::Overlay);
                }
            }
            Intent::Pan { .. } | Intent::Zoom { .. } | Intent::ViewEnd | Intent::Hover { .. } => {
                // Routed by `dispatch`; listed so a new intent is a compile error here.
                let _ = mods;
            }
        }
    }

    /// A right click (or a long press): select what is under the pointer when it is not selected, then ask
    /// the shell for the menu.
    #[allow(clippy::too_many_arguments)]
    fn context_menu(
        &mut self,
        s: &mut Session,
        screen: Point,
        world: Point,
        target: &Target,
        mods: Modifiers,
        now: f64,
        out: &mut Outcome,
    ) {
        let on_element = target.element().cloned();
        let tolerance = self.world.config.hit_tolerance_px;
        // The normal hit test skips locked elements. When one is on top of everything it found, the menu is about
        // that one (it offers Unlock). It cannot be selected, so the selection is cleared.
        let locked = (!matches!(target, Target::Handle(_)))
            .then(|| self.world.locked_on_top(s, world, tolerance))
            .flatten();
        let menu_target = self.with_ctx(s, out, now, mods, |cx, _| {
            if let Some(element) = locked {
                cx.exec("selection.clear", serde_json::json!({}));
                return ContextTarget::Locked { element };
            }
            match on_element {
                Some(id) => {
                    if !cx.s.selection().contains(&id) {
                        cx.exec(
                            "selection.select_at",
                            serde_json::json!({ "point": [world.x, world.y], "tolerance": cx.world.config.hit_tolerance_px }),
                        );
                    }
                    ContextTarget::Selection
                }
                None => {
                    if matches!(target, Target::Empty) {
                        cx.exec("selection.clear", serde_json::json!({}));
                    }
                    ContextTarget::Canvas
                }
            }
        });
        out.ui_request = Some(UiRequest::OpenContextMenu { at: screen, target: menu_target });
    }
}

/// The two machines, borrowed together with the context.
pub(crate) struct Machines<'a> {
    pub(crate) select: &'a mut SelectTool,
    pub(crate) draw: &'a mut DrawTools,
}

/// Which machine handles a press or a drag that starts on `target`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    Select,
    Draw,
    None,
}

fn route(tool: Tool, target: &Target) -> Route {
    match tool {
        Tool::Select => Route::Select,
        Tool::Hand => Route::None,
        // The text and math tools click to edit, but their handles and bodies move and resize like any element.
        Tool::Text | Tool::Math => match target {
            Target::Handle(_) | Target::Element { .. } => Route::Select,
            Target::Empty | Target::None => Route::Draw,
        },
        _ => Route::Draw,
    }
}

fn click_route(tool: Tool) -> Route {
    match tool {
        Tool::Select => Route::Select,
        Tool::Hand => Route::None,
        _ => Route::Draw,
    }
}
