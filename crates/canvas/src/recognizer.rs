//! The gesture recognizer: a state machine from raw input events to [`Intent`]s.
//!
//! It is pure: it reads the world only through a [`Picker`] (what is under a screen point, and the viewport for
//! coordinate conversion), keeps its own small state, and takes time from the events. Feed it the same events and
//! you get the same intents, so gestures can be tested with scripted sequences and replayed from recordings.
//!
//! # States
//!
//! ```text
//!             down                    move > slop
//!   Idle ───────────▶ Pressed ─────────────────────────▶ Dragging ──up/cancel──▶ Idle
//!    ▲  ▲               │  │ up (no drag)                    (DragStart, DragUpdate*, DragEnd)
//!    │  │               │  └──▶ Click [DoubleClick] ──▶ Idle
//!    │  │               │ held, no move: LongPress (touch, pen)
//!    │  │               │ second finger down
//!    │  │               ▼
//!    │  └─ all up ── Ignoring ◀── a finger lifts ── Multi (Pan + Zoom, ViewEnd)
//! ```
//!
//! * One pointer at a time owns a gesture. While it is down, other mouse or pen events are ignored; a second
//!   finger turns a one-finger gesture into a two-finger view gesture and cancels the first (a running drag ends
//!   with `cancelled: true`).
//! * A press that never leaves the slop circle is a click; a held touch becomes a long press once
//!   [`InputConfig::long_press_ms`] has passed (call [`Recognizer::tick`], or just keep sending events).
//! * Hover is emitted only when the target under an idle mouse or pen changes.

use drafftink_geom::kurbo::{Point, Vec2};

use crate::config::{InputConfig, PointerMode};
use crate::input::{
    Button, InputEvent, Modifiers, PointerEvent, PointerId, PointerKind, PointerPhase, WheelEvent, WheelUnit,
};
use crate::intent::{Cursor, Intent, Meta, Pos, Target, ViewSource};
use crate::viewport::Viewport;

/// What the recognizer needs to know about the scene.
pub trait Picker {
    /// The viewport, for converting screen positions to world positions.
    fn viewport(&self) -> &Viewport;
    /// What is under a screen point: a handle, an element, or empty space.
    fn pick(&self, screen: Point, kind: PointerKind) -> Target;
    /// The cursor to show over a target.
    fn cursor(&self, target: &Target) -> Cursor {
        match target {
            Target::Element { .. } => Cursor::Move,
            Target::Handle(_) => Cursor::Crosshair,
            Target::Empty | Target::None | Target::LinkIcon { .. } => Cursor::Default,
        }
    }
}

#[derive(Debug, Clone)]
struct Press {
    id: PointerId,
    kind: PointerKind,
    button: Button,
    origin: Pos,
    time_ms: f64,
    target: Target,
    meta: Meta,
    long_pressed: bool,
    /// The press can no longer become a click or a drag (a secondary press that moved).
    dead: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum DragPurpose {
    Content,
    Pan(ViewSource),
}

#[derive(Debug, Clone)]
struct Drag {
    id: PointerId,
    kind: PointerKind,
    button: Button,
    origin: Pos,
    last: Point,
    target: Target,
    purpose: DragPurpose,
}

#[derive(Debug, Clone)]
struct Multi {
    a: PointerId,
    b: PointerId,
    last_dist: f64,
    last_center: Point,
}

#[derive(Debug, Clone, Default)]
enum State {
    #[default]
    Idle,
    Pressed(Press),
    Dragging(Drag),
    Multi(Multi),
    /// A two-finger gesture ended but fingers are still down: ignore them until all are up.
    Ignoring,
}

#[derive(Debug, Clone)]
struct LastClick {
    kind: PointerKind,
    target: Target,
    pos: Point,
    time_ms: f64,
}

/// See the module docs.
#[derive(Debug, Clone)]
pub struct Recognizer {
    cfg: InputConfig,
    mode: PointerMode,
    state: State,
    touches: Vec<(PointerId, Point)>,
    hover: Target,
    hover_at: Option<(Point, PointerKind)>,
    last_click: Option<LastClick>,
}

impl Default for Recognizer {
    fn default() -> Self {
        Self::new(InputConfig::default())
    }
}

fn meta_of(ev: &PointerEvent) -> Meta {
    Meta {
        pointer: ev.id,
        kind: ev.kind,
        button: ev.button,
        modifiers: ev.modifiers,
        pressure: ev.pressure,
        tilt: ev.tilt,
        time_ms: ev.time_ms,
    }
}

/// The end of a drag that is aborted without a pointer event to carry the details.
fn cancelled_drag_end<P: Picker + ?Sized>(d: &Drag, picker: &P) -> Intent {
    let at = pos_of(picker, d.last);
    Intent::DragEnd {
        origin: d.origin,
        at,
        delta_screen: at.screen - d.origin.screen,
        delta_world: at.world - d.origin.world,
        target: d.target.clone(),
        cancelled: true,
        meta: Meta {
            pointer: d.id,
            kind: d.kind,
            button: d.button,
            modifiers: Modifiers::NONE,
            pressure: 0.5,
            tilt: Default::default(),
            time_ms: 0.0,
        },
    }
}

fn pos_of<P: Picker + ?Sized>(p: &P, screen: Point) -> Pos {
    Pos { screen, world: p.viewport().screen_to_world(screen) }
}

impl Recognizer {
    /// A recognizer with the given thresholds.
    pub fn new(cfg: InputConfig) -> Self {
        Self {
            cfg,
            mode: PointerMode::Select,
            state: State::Idle,
            touches: Vec::new(),
            hover: Target::None,
            hover_at: None,
            last_click: None,
        }
    }

    /// The thresholds.
    pub fn config(&self) -> &InputConfig {
        &self.cfg
    }

    /// Replaces the thresholds. A running gesture keeps going with the new values.
    pub fn set_config(&mut self, cfg: InputConfig) {
        self.cfg = cfg;
    }

    /// What a primary drag means from the next press on.
    pub fn set_mode(&mut self, mode: PointerMode) {
        self.mode = mode;
    }

    /// The current pointer mode.
    pub fn mode(&self) -> PointerMode {
        self.mode
    }

    /// The current hover target ([`Target::None`] when the pointer is away).
    pub fn hover(&self) -> &Target {
        &self.hover
    }

    /// True when no gesture is running.
    pub fn is_idle(&self) -> bool {
        matches!(self.state, State::Idle)
    }

    /// True while a drag of content (not a pan) is running.
    pub fn is_dragging(&self) -> bool {
        matches!(&self.state, State::Dragging(d) if d.purpose == DragPurpose::Content)
    }

    /// The time at which [`Recognizer::tick`] will produce a long press, if a press is waiting for one. A host
    /// schedules one wake-up for this time and otherwise sleeps.
    pub fn next_deadline(&self) -> Option<f64> {
        match &self.state {
            State::Pressed(pr) if self.long_press_eligible(pr) && !pr.long_pressed => {
                self.cfg.long_press_ms.map(|ms| pr.time_ms + ms)
            }
            _ => None,
        }
    }

    fn long_press_eligible(&self, pr: &Press) -> bool {
        self.cfg.long_press_ms.is_some()
            && self.mode == PointerMode::Select
            && pr.button == Button::Primary
            && !pr.dead
            && (pr.kind != PointerKind::Mouse || self.cfg.long_press_mouse)
    }

    fn slop(&self, kind: PointerKind) -> f64 {
        match (self.mode, kind) {
            (PointerMode::Draw, _) => self.cfg.draw_slop_px,
            (_, PointerKind::Touch) => self.cfg.drag_slop_touch_px,
            _ => self.cfg.drag_slop_px,
        }
    }

    /// Advances time without an event: produces a long press when one is due.
    pub fn tick(&mut self, now_ms: f64, out: &mut Vec<Intent>) {
        if !now_ms.is_finite() {
            return;
        }
        let due = match &self.state {
            State::Pressed(pr) => self.long_press_due(pr, now_ms),
            _ => false,
        };
        if due && let State::Pressed(pr) = &mut self.state {
            pr.long_pressed = true;
            let mut meta = pr.meta.clone();
            meta.time_ms = now_ms;
            out.push(Intent::LongPress { at: pr.origin, target: pr.target.clone(), meta });
        }
    }

    fn long_press_due(&self, pr: &Press, now_ms: f64) -> bool {
        self.long_press_eligible(pr)
            && !pr.long_pressed
            && self.cfg.long_press_ms.is_some_and(|ms| now_ms - pr.time_ms >= ms)
    }

    /// Consumes one event and appends the intents it produces.
    pub fn handle<P: Picker + ?Sized>(&mut self, ev: &InputEvent, picker: &P, out: &mut Vec<Intent>) {
        match ev {
            InputEvent::Pointer(p) => self.pointer(p, picker, out),
            InputEvent::Wheel(w) => self.wheel(w, picker.viewport(), out),
            InputEvent::PointerLeave { .. } => self.leave(picker, out),
            InputEvent::CancelAll => self.cancel_all(picker, out),
        }
    }

    /// Picks again at the last pointer position and emits a hover change if the target under it changed. Call it
    /// after the scene, the selection or the viewport changed. Does nothing during a gesture or without a pointer.
    pub fn refresh_hover<P: Picker + ?Sized>(&mut self, picker: &P, out: &mut Vec<Intent>) {
        if !self.is_idle() {
            return;
        }
        if let Some((screen, kind)) = self.hover_at {
            self.set_hover(screen, kind, true, picker, out);
        }
    }

    fn pointer<P: Picker + ?Sized>(&mut self, ev: &PointerEvent, picker: &P, out: &mut Vec<Intent>) {
        if !ev.is_finite() {
            return;
        }
        match ev.phase {
            PointerPhase::Down => self.down(ev, picker, out),
            PointerPhase::Move => self.moved(ev, picker, out),
            PointerPhase::Up => self.up(ev, false, picker, out),
            PointerPhase::Cancel => self.up(ev, true, picker, out),
        }
    }

    fn down<P: Picker + ?Sized>(&mut self, ev: &PointerEvent, picker: &P, out: &mut Vec<Intent>) {
        if ev.kind == PointerKind::Touch {
            match self.touches.iter_mut().find(|(id, _)| *id == ev.id) {
                Some(t) => t.1 = ev.pos,
                None => self.touches.push((ev.id, ev.pos)),
            }
        }
        // A press from the pointer that already owns the gesture means its release was lost (the button went up
        // outside the window): drop the old gesture and start over.
        if matches!(&self.state, State::Pressed(Press { id, .. }) | State::Dragging(Drag { id, .. }) if *id == ev.id)
            && ev.kind != PointerKind::Touch
        {
            self.abort_gesture(picker, out);
        }
        match &self.state {
            State::Idle => self.begin_press(ev, picker, out),
            State::Pressed(Press { id, kind: PointerKind::Touch, .. })
            | State::Dragging(Drag { id, kind: PointerKind::Touch, .. })
                if ev.kind == PointerKind::Touch && *id != ev.id && self.touches.len() >= 2 =>
            {
                self.begin_multi(picker, out);
            }
            _ => {}
        }
    }

    fn begin_press<P: Picker + ?Sized>(&mut self, ev: &PointerEvent, picker: &P, out: &mut Vec<Intent>) {
        let target = picker.pick(ev.pos, ev.kind);
        let origin = pos_of(picker, ev.pos);
        let meta = meta_of(ev);
        out.push(Intent::Press { at: origin, target: target.clone(), meta: meta.clone() });
        self.state = State::Pressed(Press {
            id: ev.id,
            kind: ev.kind,
            button: ev.button,
            origin,
            time_ms: ev.time_ms,
            target,
            meta,
            long_pressed: false,
            dead: false,
        });
    }

    fn moved<P: Picker + ?Sized>(&mut self, ev: &PointerEvent, picker: &P, out: &mut Vec<Intent>) {
        if ev.kind == PointerKind::Touch
            && let Some(t) = self.touches.iter_mut().find(|(id, _)| *id == ev.id)
        {
            t.1 = ev.pos;
        }
        match std::mem::take(&mut self.state) {
            State::Idle => {
                if ev.kind != PointerKind::Touch {
                    self.set_hover(ev.pos, ev.kind, false, picker, out);
                }
            }
            State::Pressed(mut pr) => {
                if pr.id != ev.id {
                    self.state = State::Pressed(pr);
                    return;
                }
                if self.long_press_due(&pr, ev.time_ms) {
                    pr.long_pressed = true;
                    let mut meta = pr.meta.clone();
                    meta.time_ms = ev.time_ms;
                    out.push(Intent::LongPress { at: pr.origin, target: pr.target.clone(), meta });
                }
                let slop = self.slop(pr.kind);
                if pr.dead || (ev.pos - pr.origin.screen).hypot2() <= slop * slop || pr.long_pressed {
                    // A long press is final: moving afterwards neither drags nor clicks.
                    self.state = State::Pressed(pr);
                    return;
                }
                self.begin_drag(pr, ev, picker, out);
            }
            State::Dragging(mut d) => {
                if d.id == ev.id {
                    self.drag_move(&mut d, ev, picker, out);
                }
                self.state = State::Dragging(d);
            }
            State::Multi(mut m) => {
                if ev.id == m.a || ev.id == m.b {
                    self.multi_move(&mut m, picker, out);
                }
                self.state = State::Multi(m);
            }
            State::Ignoring => self.state = State::Ignoring,
        }
    }

    fn begin_drag<P: Picker + ?Sized>(&mut self, mut pr: Press, ev: &PointerEvent, picker: &P, out: &mut Vec<Intent>) {
        let purpose = match (pr.button, self.mode) {
            (Button::Middle, _) if self.cfg.middle_button_pans => Some(DragPurpose::Pan(ViewSource::MiddleButton)),
            (Button::Primary, PointerMode::Hand) => Some(DragPurpose::Pan(ViewSource::HandTool)),
            (Button::Primary, _) => Some(DragPurpose::Content),
            _ => None,
        };
        let Some(purpose) = purpose else {
            pr.dead = true;
            self.state = State::Pressed(pr);
            return;
        };
        let mut d = Drag {
            id: pr.id,
            kind: pr.kind,
            button: pr.button,
            origin: pr.origin,
            last: pr.origin.screen,
            target: pr.target.clone(),
            purpose,
        };
        if purpose == DragPurpose::Content {
            let mut meta = pr.meta.clone();
            meta.time_ms = ev.time_ms;
            meta.modifiers = ev.modifiers;
            out.push(Intent::DragStart { origin: pr.origin, at: pos_of(picker, ev.pos), target: pr.target, meta });
        }
        self.drag_move(&mut d, ev, picker, out);
        self.state = State::Dragging(d);
    }

    fn drag_move<P: Picker + ?Sized>(&mut self, d: &mut Drag, ev: &PointerEvent, picker: &P, out: &mut Vec<Intent>) {
        match d.purpose {
            DragPurpose::Content => {
                let at = pos_of(picker, ev.pos);
                out.push(Intent::DragUpdate {
                    origin: d.origin,
                    at,
                    delta_screen: at.screen - d.origin.screen,
                    delta_world: at.world - d.origin.world,
                    target: d.target.clone(),
                    meta: meta_of(ev),
                });
            }
            DragPurpose::Pan(source) => {
                let delta = ev.pos - d.last;
                if delta != Vec2::ZERO {
                    out.push(Intent::Pan { delta, source });
                }
            }
        }
        d.last = ev.pos;
    }

    fn up<P: Picker + ?Sized>(&mut self, ev: &PointerEvent, cancelled: bool, picker: &P, out: &mut Vec<Intent>) {
        if ev.kind == PointerKind::Touch {
            self.touches.retain(|(id, _)| *id != ev.id);
        }
        // A different mouse button going up is not the end of the gesture.
        let other_button = |b: Button| ev.kind == PointerKind::Mouse && ev.button != Button::None && ev.button != b;
        match std::mem::take(&mut self.state) {
            State::Pressed(mut pr) if pr.id == ev.id && !(other_button(pr.button) && !cancelled) => {
                if !cancelled {
                    if self.long_press_due(&pr, ev.time_ms) {
                        pr.long_pressed = true;
                        let mut meta = pr.meta.clone();
                        meta.time_ms = ev.time_ms;
                        out.push(Intent::LongPress { at: pr.origin, target: pr.target.clone(), meta });
                    }
                    if !pr.long_pressed && !pr.dead {
                        self.click(&pr, ev, picker, out);
                    }
                }
                self.released(ev, picker, out);
            }
            State::Dragging(d) if d.id == ev.id && !(other_button(d.button) && !cancelled) => {
                match d.purpose {
                    DragPurpose::Content => {
                        let at = pos_of(picker, ev.pos);
                        out.push(Intent::DragEnd {
                            origin: d.origin,
                            at,
                            delta_screen: at.screen - d.origin.screen,
                            delta_world: at.world - d.origin.world,
                            target: d.target,
                            cancelled,
                            meta: meta_of(ev),
                        });
                    }
                    DragPurpose::Pan(_) => out.push(Intent::ViewEnd),
                }
                self.released(ev, picker, out);
            }
            State::Multi(m) if ev.id == m.a || ev.id == m.b => {
                out.push(Intent::ViewEnd);
                self.state = if self.touches.is_empty() { State::Idle } else { State::Ignoring };
            }
            State::Ignoring => {
                self.state = if self.touches.is_empty() { State::Idle } else { State::Ignoring };
            }
            other => self.state = other,
        }
    }

    /// Back to idle after a gesture: a mouse or pen picks the hover again at the release point.
    fn released<P: Picker + ?Sized>(&mut self, ev: &PointerEvent, picker: &P, out: &mut Vec<Intent>) {
        self.state = State::Idle;
        if ev.kind != PointerKind::Touch {
            self.set_hover(ev.pos, ev.kind, true, picker, out);
        }
    }

    fn click<P: Picker + ?Sized>(&mut self, pr: &Press, ev: &PointerEvent, picker: &P, out: &mut Vec<Intent>) {
        let at = pos_of(picker, ev.pos);
        let mut meta = pr.meta.clone();
        meta.time_ms = ev.time_ms;
        meta.modifiers = ev.modifiers;
        match pr.button {
            Button::Secondary => {
                out.push(Intent::ContextMenu { at, target: pr.target.clone(), meta });
                return;
            }
            Button::Primary | Button::Middle | Button::Other | Button::None => {}
        }
        out.push(Intent::Click { at, target: pr.target.clone(), meta: meta.clone() });
        if pr.button != Button::Primary {
            return;
        }
        let double = self.last_click.as_ref().is_some_and(|lc| {
            let dt = ev.time_ms - lc.time_ms;
            lc.kind == pr.kind
                && lc.target == pr.target
                && (0.0..=self.cfg.double_click_ms).contains(&dt)
                && (ev.pos - lc.pos).hypot() <= self.cfg.double_click_dist_px
        });
        if double {
            self.last_click = None;
            out.push(Intent::DoubleClick { at, target: pr.target.clone(), meta });
        } else {
            self.last_click =
                Some(LastClick { kind: pr.kind, target: pr.target.clone(), pos: ev.pos, time_ms: ev.time_ms });
        }
    }

    fn begin_multi<P: Picker + ?Sized>(&mut self, picker: &P, out: &mut Vec<Intent>) {
        self.abort_gesture(picker, out);
        let (Some(&(a, pa)), Some(&(b, pb))) = (self.touches.first(), self.touches.get(1)) else {
            self.state = State::Idle;
            return;
        };
        self.last_click = None;
        self.state = State::Multi(Multi { a, b, last_dist: (pb - pa).hypot(), last_center: pa.midpoint(pb) });
    }

    fn multi_move<P: Picker + ?Sized>(&self, m: &mut Multi, _picker: &P, out: &mut Vec<Intent>) {
        let find = |id: PointerId| self.touches.iter().find(|(i, _)| *i == id).map(|(_, p)| *p);
        let (Some(pa), Some(pb)) = (find(m.a), find(m.b)) else { return };
        let center = pa.midpoint(pb);
        let dist = (pb - pa).hypot();
        // Pan first, then zoom about the new centre: the order the viewport needs to keep the fingers on their
        // content.
        let delta = center - m.last_center;
        if delta != Vec2::ZERO {
            out.push(Intent::Pan { delta, source: ViewSource::Touch });
            m.last_center = center;
        }
        let min = self.cfg.pinch_min_distance_px;
        if m.last_dist >= min && dist >= min {
            let ratio = dist / m.last_dist;
            if (ratio - 1.0).abs() > self.cfg.pinch_epsilon {
                out.push(Intent::Zoom { at: center, factor: ratio, source: ViewSource::Touch });
                m.last_dist = dist;
            }
        } else if dist >= min {
            m.last_dist = dist;
        }
    }

    fn wheel(&mut self, w: &WheelEvent, vp: &Viewport, out: &mut Vec<Intent>) {
        if !(w.pos.x.is_finite() && w.pos.y.is_finite() && w.delta.x.is_finite() && w.delta.y.is_finite()) {
            return;
        }
        let unit = match w.unit {
            WheelUnit::Pixel => 1.0,
            WheelUnit::Line => self.cfg.wheel_line_px,
            WheelUnit::Page => vp.size().height,
        };
        let mut d = w.delta * unit;
        if w.modifiers.command() != self.cfg.wheel_zooms {
            // One notch is 100 pixels of delta; a trackpad pinch sends small deltas and gets a smooth zoom.
            let factor = self.cfg.wheel_zoom_step.powf(-d.y / 100.0).clamp(0.25, 4.0);
            if factor.is_finite() && (factor - 1.0).abs() > 1e-12 {
                out.push(Intent::Zoom { at: w.pos, factor, source: ViewSource::Wheel });
            }
        } else {
            if w.modifiers.shift && d.x == 0.0 {
                d = Vec2::new(d.y, 0.0);
            }
            if d != Vec2::ZERO {
                out.push(Intent::Pan { delta: -d, source: ViewSource::Wheel });
            }
        }
    }

    fn leave<P: Picker + ?Sized>(&mut self, picker: &P, out: &mut Vec<Intent>) {
        if !self.is_idle() {
            return;
        }
        self.hover_at = None;
        if self.hover != Target::None {
            let from = std::mem::replace(&mut self.hover, Target::None);
            let at = self.last_hover_pos(picker);
            out.push(Intent::Hover { from, to: Target::None, at, cursor: Cursor::Default });
        }
    }

    fn last_hover_pos<P: Picker + ?Sized>(&self, picker: &P) -> Pos {
        pos_of(picker, self.hover_at.map_or(Point::ZERO, |(p, _)| p))
    }

    fn cancel_all<P: Picker + ?Sized>(&mut self, picker: &P, out: &mut Vec<Intent>) {
        self.abort_gesture(picker, out);
        self.touches.clear();
        self.last_click = None;
    }

    /// Ends whatever gesture is running without a trace: a drag of content ends with `cancelled: true`, a view
    /// gesture with `ViewEnd`, a press is dropped.
    fn abort_gesture<P: Picker + ?Sized>(&mut self, picker: &P, out: &mut Vec<Intent>) {
        match std::mem::take(&mut self.state) {
            State::Dragging(d) => match d.purpose {
                DragPurpose::Content => out.push(cancelled_drag_end(&d, picker)),
                DragPurpose::Pan(_) => out.push(Intent::ViewEnd),
            },
            State::Multi(_) => out.push(Intent::ViewEnd),
            State::Pressed(_) | State::Idle | State::Ignoring => {}
        }
    }

    /// Picks at `screen` and emits a change. Without `force`, a move to the position of the last pick is skipped:
    /// the answer cannot have changed unless the scene did.
    fn set_hover<P: Picker + ?Sized>(
        &mut self,
        screen: Point,
        kind: PointerKind,
        force: bool,
        picker: &P,
        out: &mut Vec<Intent>,
    ) {
        if !force && self.hover_at == Some((screen, kind)) && self.hover != Target::None {
            return;
        }
        self.hover_at = Some((screen, kind));
        let target = picker.pick(screen, kind);
        if target != self.hover {
            let cursor = picker.cursor(&target);
            let from = std::mem::replace(&mut self.hover, target.clone());
            out.push(Intent::Hover { from, to: target, at: pos_of(picker, screen), cursor });
        }
    }
}
