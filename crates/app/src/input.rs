//! From shell events to the canvas library's input events, and from the interaction layer's cursor to egui's.

use drafftink_canvas::{
    Button, Buttons, Cursor, FingerMode, InputEvent, Modifiers, PointerEvent, PointerId, PointerKind, PointerPhase,
    Tilt, WheelEvent, WheelUnit,
};
use drafftink_geom::kurbo::{Point, Vec2};
use drafftink_gpu as gpu;

/// The pointers that press on the canvas, each with the buttons it holds. A finger, a pen and a mouse are tracked
/// apart: a palm that lifts must not end the stroke of the pen, and the first finger of a pinch that lifts must not
/// cut the second one off from the recognizer (which would then wait for it for ever).
#[derive(Debug, Default)]
pub(crate) struct Contacts(Vec<(PointerKey, Held)>);

/// Which pointer a sample is from: its kind and the browser's id (0 for the mouse).
type PointerKey = (gpu::PointerKind, u64);

fn key_of(s: &gpu::PointerSample) -> PointerKey {
    (s.kind, if s.kind == gpu::PointerKind::Touch { s.id } else { 0 })
}

impl Contacts {
    /// The pointer of this sample pressed on the canvas (not on a bar or a dialog). Idempotent.
    pub(crate) fn begin(&mut self, s: &gpu::PointerSample) {
        let key = key_of(s);
        if !self.0.iter().any(|(k, _)| *k == key) {
            self.0.push((key, Held::default()));
        }
    }

    /// Did the press of this sample's pointer begin on the canvas, and is it still down?
    pub(crate) fn owns(&self, s: &gpu::PointerSample) -> bool {
        let key = key_of(s);
        self.0.iter().any(|(k, _)| *k == key)
    }

    /// The buttons of this sample's pointer. A pointer that is not down holds none: a fresh set.
    pub(crate) fn held_mut(&mut self, s: &gpu::PointerSample) -> Option<&mut Held> {
        let key = key_of(s);
        self.0.iter_mut().find(|(k, _)| *k == key).map(|(_, held)| held)
    }

    /// The release of this sample's pointer was handled: forget it once it holds nothing.
    pub(crate) fn release(&mut self, s: &gpu::PointerSample) {
        let key = key_of(s);
        self.0.retain(|(k, held)| *k != key || held.any());
    }

    /// Is any pointer down on the canvas?
    pub(crate) fn any(&self) -> bool {
        !self.0.is_empty()
    }

    /// Forget every pointer (the window lost focus).
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }
}

/// The longest a coalesced point can lie before its sample. A browser merges the moves of one frame, a few
/// milliseconds; anything longer is a clock that jumped.
const MAX_COALESCED_AGE_MS: f64 = 250.0;

/// Which buttons are down, so a move can say whether it is a drag.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Held(u8);

impl Held {
    fn bit(button: u8) -> u8 {
        match button {
            0 => 1,
            2 => 2,
            1 => 4,
            _ => 0,
        }
    }

    pub(crate) fn buttons(self) -> Buttons {
        Buttons(self.0)
    }

    pub(crate) fn any(self) -> bool {
        self.0 != 0
    }
}

/// How long after the last sample of a stylus a finger still only pans (palm rejection), in milliseconds. A palm
/// lifts a moment after the pen does; a person who puts the pen down to draw with a finger waits this long.
const PEN_GRACE_MS: f64 = 2_000.0;

/// Remembers when a stylus was last seen, to tell a finger that draws from a palm that rests.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct PenWatch {
    last_seen_ms: Option<f64>,
}

impl PenWatch {
    /// Notes a sample: a stylus (in contact or hovering) starts or extends the grace period.
    pub(crate) fn observe(&mut self, kind: gpu::PointerKind, now_ms: f64) {
        if kind == gpu::PointerKind::Pen {
            self.last_seen_ms = Some(now_ms);
        }
    }

    /// What a single finger does now: it only pans while a stylus is, or just was, in use.
    pub(crate) fn fingers(&self, now_ms: f64) -> FingerMode {
        match self.last_seen_ms {
            Some(seen) if now_ms - seen <= PEN_GRACE_MS => FingerMode::PanOnly,
            _ => FingerMode::Full,
        }
    }
}

/// Canvas modifiers from egui's.
pub(crate) fn modifiers(m: egui::Modifiers) -> Modifiers {
    Modifiers { shift: m.shift, ctrl: m.ctrl, alt: m.alt, meta: m.mac_cmd }
}

fn button(b: Option<u8>) -> Button {
    match b {
        Some(0) | None => Button::Primary,
        Some(1) => Button::Middle,
        Some(2) => Button::Secondary,
        Some(_) => Button::Other,
    }
}

fn kind_and_id(s: &gpu::PointerSample) -> (PointerKind, PointerId) {
    match s.kind {
        gpu::PointerKind::Mouse => (PointerKind::Mouse, PointerId(0)),
        gpu::PointerKind::Touch => (PointerKind::Touch, PointerId(s.id.saturating_add(1))),
        gpu::PointerKind::Pen => (PointerKind::Pen, PointerId(100)),
    }
}

/// The canvas events for one shell sample, oldest first. `origin` is the canvas corner in window points and
/// `now_ms` the app clock (the shell's own time stamps start somewhere else). Updates `held`.
pub(crate) fn pointer_events(
    s: &gpu::PointerSample,
    held: &mut Held,
    origin: egui::Pos2,
    now_ms: f64,
) -> Vec<InputEvent> {
    let (kind, id) = kind_and_id(s);
    let at = |p: egui::Pos2| Point::new(f64::from(p.x - origin.x), f64::from(p.y - origin.y));
    let mods = modifiers(s.modifiers);
    let tilt = s.tilt.map_or(Tilt::default(), |[x, y]| Tilt { x, y });
    let pressure = s.point.pressure.unwrap_or(0.5);
    let make = |phase: PointerPhase, pos: Point, pressure: f32, b: Button, buttons: Buttons, time_ms: f64| {
        PointerEvent { id, kind, phase, pos, button: b, buttons, modifiers: mods, pressure, tilt, time_ms }
    };
    // A coalesced point happened some milliseconds before the sample. The app clock is not the shell's, so the
    // point keeps its distance in time from the sample instead of its own stamp: a pen reports 240 points a
    // second, and the stroke's smoothing and speed pressure need to see the time between them.
    let before_sample = |p: &gpu::PointerPoint| {
        let age = s.point.time_ms - p.time_ms;
        now_ms - if age.is_finite() { age.clamp(0.0, MAX_COALESCED_AGE_MS) } else { 0.0 }
    };
    let mut out = Vec::new();
    match s.phase {
        gpu::PointerPhase::Hover => {
            out.push(InputEvent::Pointer(make(
                PointerPhase::Move,
                at(s.point.pos),
                pressure,
                Button::None,
                Buttons::NONE,
                now_ms,
            )));
        }
        gpu::PointerPhase::Down => {
            held.0 |= Held::bit(s.button.unwrap_or(0));
            out.push(InputEvent::Pointer(make(
                PointerPhase::Down,
                at(s.point.pos),
                pressure,
                button(s.button),
                held.buttons(),
                now_ms,
            )));
        }
        gpu::PointerPhase::Move => {
            // Intermediate samples first (a pen reports many between two frames), then the latest.
            for p in &s.coalesced {
                out.push(InputEvent::Pointer(make(
                    PointerPhase::Move,
                    at(p.pos),
                    p.pressure.unwrap_or(pressure),
                    Button::None,
                    held.buttons(),
                    before_sample(p),
                )));
            }
            out.push(InputEvent::Pointer(make(
                PointerPhase::Move,
                at(s.point.pos),
                pressure,
                Button::None,
                held.buttons(),
                now_ms,
            )));
        }
        gpu::PointerPhase::Up => {
            held.0 &= !Held::bit(s.button.unwrap_or(0));
            out.push(InputEvent::Pointer(make(
                PointerPhase::Up,
                at(s.point.pos),
                pressure,
                button(s.button),
                held.buttons(),
                now_ms,
            )));
        }
        gpu::PointerPhase::Cancel => {
            held.0 = 0;
            out.push(InputEvent::Pointer(make(
                PointerPhase::Cancel,
                at(s.point.pos),
                pressure,
                Button::Primary,
                Buttons::NONE,
                now_ms,
            )));
        }
        gpu::PointerPhase::Leave => {
            // A pointer that leaves with a button down keeps dragging: the web captures it, winit reports the
            // release later. Only an idle pointer clears the hover.
            if !held.any() {
                out.push(InputEvent::PointerLeave { id });
            }
        }
    }
    out
}

/// Wheel and pinch events from egui's raw input of this pass. Positive `delta.y` scrolls down, as in the DOM.
pub(crate) fn wheel_events(
    events: &[egui::Event],
    pos: Point,
    now_ms: f64,
    fallback: egui::Modifiers,
) -> Vec<InputEvent> {
    events
        .iter()
        .filter_map(|e| match e {
            egui::Event::MouseWheel { unit, delta, modifiers: m, .. } => Some(InputEvent::Wheel(WheelEvent {
                pos,
                delta: Vec2::new(-f64::from(delta.x), -f64::from(delta.y)),
                unit: match unit {
                    egui::MouseWheelUnit::Point => WheelUnit::Pixel,
                    egui::MouseWheelUnit::Line => WheelUnit::Line,
                    egui::MouseWheelUnit::Page => WheelUnit::Page,
                },
                modifiers: modifiers(*m),
                time_ms: now_ms,
            })),
            // A pinch is a wheel event with Ctrl and `-scale_change * 100` (docs/canvas-api.md).
            egui::Event::Zoom(factor) => Some(InputEvent::Wheel(WheelEvent {
                pos,
                delta: Vec2::new(0.0, -f64::from(factor - 1.0) * 100.0),
                unit: WheelUnit::Pixel,
                modifiers: Modifiers { ctrl: true, ..modifiers(fallback) },
                time_ms: now_ms,
            })),
            _ => None,
        })
        .collect()
}

/// egui's cursor for the interaction layer's.
pub(crate) fn cursor_icon(c: Cursor) -> egui::CursorIcon {
    match c {
        Cursor::Default => egui::CursorIcon::Default,
        Cursor::Move => egui::CursorIcon::Move,
        Cursor::Pointer => egui::CursorIcon::PointingHand,
        Cursor::Grab => egui::CursorIcon::Grab,
        Cursor::Grabbing => egui::CursorIcon::Grabbing,
        Cursor::Text => egui::CursorIcon::Text,
        Cursor::Crosshair => egui::CursorIcon::Crosshair,
        Cursor::ResizeNs => egui::CursorIcon::ResizeVertical,
        Cursor::ResizeEw => egui::CursorIcon::ResizeHorizontal,
        Cursor::ResizeNesw => egui::CursorIcon::ResizeNeSw,
        Cursor::ResizeNwse => egui::CursorIcon::ResizeNwSe,
        Cursor::Rotate => egui::CursorIcon::Alias,
    }
}

/// The pointer's icon while a text is edited. The I-beam shows only over the edited text, where a click places the
/// caret; anywhere else it is the cursor the canvas chose for what lies under the pointer.
pub(crate) fn editing_cursor(over_edited_text: bool, canvas: Cursor) -> egui::CursorIcon {
    if over_edited_text { egui::CursorIcon::Text } else { cursor_icon(canvas) }
}

/// Does egui, not the canvas, own the pointer at `pos`? True while egui drags something, over a floating
/// egui area (a panel, a popup) and outside the canvas rectangle. The same rule as the shell's.
pub(crate) fn egui_owns(ctx: &egui::Context, canvas: egui::Rect, pos: egui::Pos2) -> bool {
    ctx.egui_is_using_pointer()
        || !canvas.contains(pos)
        || ctx.layer_id_at(pos).is_some_and(|layer| layer.order != egui::Order::Background)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(phase: gpu::PointerPhase, x: f32, y: f32) -> gpu::PointerSample {
        gpu::PointerSample::new(
            phase,
            gpu::PointerKind::Mouse,
            gpu::PointerPoint { pos: egui::pos2(x, y), pressure: None, time_ms: 0.0 },
            egui::Modifiers::NONE,
        )
    }

    #[test]
    fn every_cursor_has_an_egui_cursor_and_the_i_beam_is_the_text_cursor() {
        assert_eq!(cursor_icon(Cursor::Text), egui::CursorIcon::Text);
        assert_eq!(cursor_icon(Cursor::Pointer), egui::CursorIcon::PointingHand);
        assert_eq!(cursor_icon(Cursor::Default), egui::CursorIcon::Default);
    }

    #[test]
    fn while_editing_the_i_beam_shows_over_the_edited_text_only() {
        assert_eq!(editing_cursor(true, Cursor::Move), egui::CursorIcon::Text, "over the edited text");
        assert_eq!(editing_cursor(false, Cursor::Move), cursor_icon(Cursor::Move), "over another element");
        assert_ne!(editing_cursor(false, Cursor::Move), egui::CursorIcon::Text);
        assert_eq!(editing_cursor(false, Cursor::Default), egui::CursorIcon::Default, "over empty canvas");
        assert_eq!(editing_cursor(false, Cursor::Pointer), egui::CursorIcon::PointingHand, "over a link icon");
    }

    #[test]
    fn a_drag_is_down_moves_up_with_the_button_held() {
        let mut held = Held::default();
        let o = egui::Pos2::ZERO;
        let down = pointer_events(&sample(gpu::PointerPhase::Down, 10.0, 20.0), &mut held, o, 1.0);
        let mv = pointer_events(&sample(gpu::PointerPhase::Move, 15.0, 25.0), &mut held, o, 2.0);
        let up = pointer_events(&sample(gpu::PointerPhase::Up, 15.0, 25.0), &mut held, o, 3.0);
        let phases: Vec<_> = [&down, &mv, &up]
            .iter()
            .map(|v| match &v[0] {
                InputEvent::Pointer(p) => (p.phase, p.buttons),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(
            phases,
            [
                (PointerPhase::Down, Buttons::PRIMARY),
                (PointerPhase::Move, Buttons::PRIMARY),
                (PointerPhase::Up, Buttons::NONE)
            ]
        );
        assert!(!held.any());
    }

    #[test]
    fn hover_has_no_buttons_and_the_origin_is_subtracted() {
        let mut held = Held::default();
        let ev =
            pointer_events(&sample(gpu::PointerPhase::Hover, 110.0, 70.0), &mut held, egui::pos2(100.0, 50.0), 0.0);
        let InputEvent::Pointer(p) = &ev[0] else { unreachable!() };
        assert_eq!((p.phase, p.buttons, p.pos), (PointerPhase::Move, Buttons::NONE, Point::new(10.0, 20.0)));
    }

    #[test]
    fn coalesced_points_come_before_the_latest() {
        let mut s = sample(gpu::PointerPhase::Move, 3.0, 3.0);
        s.coalesced = vec![gpu::PointerPoint { pos: egui::pos2(1.0, 1.0), pressure: Some(0.2), time_ms: 0.0 }];
        let mut held = Held(1);
        let ev = pointer_events(&s, &mut held, egui::Pos2::ZERO, 0.0);
        assert_eq!(ev.len(), 2);
        let (InputEvent::Pointer(a), InputEvent::Pointer(b)) = (&ev[0], &ev[1]) else { unreachable!() };
        assert_eq!((a.pos.x, a.pressure, b.pos.x), (1.0, 0.2, 3.0));
    }

    fn touch_sample(phase: gpu::PointerPhase, id: u64) -> gpu::PointerSample {
        let mut s = sample(phase, 0.0, 0.0);
        s.kind = gpu::PointerKind::Touch;
        s.id = id;
        s
    }

    #[test]
    fn coalesced_points_keep_their_distance_in_time_from_the_sample() {
        let mut s = sample(gpu::PointerPhase::Move, 30.0, 0.0);
        s.point.time_ms = 5_000.0;
        s.coalesced = [(10.0, 4_976.0), (20.0, 4_992.0)]
            .map(|(x, time_ms)| gpu::PointerPoint { pos: egui::pos2(x, 0.0), pressure: None, time_ms })
            .to_vec();
        let ev = pointer_events(&s, &mut Held(1), egui::Pos2::ZERO, 100.0);
        let times: Vec<f64> = ev
            .iter()
            .map(|e| match e {
                InputEvent::Pointer(p) => p.time_ms,
                _ => unreachable!(),
            })
            .collect();
        // The app clock reads 100 at the sample; the points were 24 and 8 milliseconds before it.
        assert_eq!(times, [76.0, 92.0, 100.0]);
    }

    #[test]
    fn a_coalesced_point_with_a_hostile_time_stamp_is_kept_in_order() {
        let mut s = sample(gpu::PointerPhase::Move, 30.0, 0.0);
        s.coalesced = vec![
            gpu::PointerPoint { pos: egui::pos2(10.0, 0.0), pressure: None, time_ms: f64::NAN },
            gpu::PointerPoint { pos: egui::pos2(20.0, 0.0), pressure: None, time_ms: f64::NEG_INFINITY },
        ];
        s.point.time_ms = 1.0;
        let ev = pointer_events(&s, &mut Held(1), egui::Pos2::ZERO, 100.0);
        for e in &ev {
            let InputEvent::Pointer(p) = e else { unreachable!() };
            assert!(p.time_ms.is_finite() && p.time_ms <= 100.0 && p.time_ms >= 100.0 - MAX_COALESCED_AGE_MS, "{p:?}");
        }
    }

    #[test]
    fn a_stylus_makes_fingers_pan_until_two_seconds_after_its_last_sample() {
        let mut watch = PenWatch::default();
        assert_eq!(watch.fingers(0.0), FingerMode::Full, "no stylus seen");
        watch.observe(gpu::PointerKind::Touch, 10.0);
        watch.observe(gpu::PointerKind::Mouse, 10.0);
        assert_eq!(watch.fingers(11.0), FingerMode::Full, "fingers and mice do not count");
        watch.observe(gpu::PointerKind::Pen, 1_000.0);
        assert_eq!(watch.fingers(1_001.0), FingerMode::PanOnly);
        assert_eq!(watch.fingers(3_000.0), FingerMode::PanOnly, "still inside the grace time");
        assert_eq!(watch.fingers(3_001.0), FingerMode::Full, "a person who draws with a finger again is let");
        watch.observe(gpu::PointerKind::Pen, 5_000.0);
        assert_eq!(watch.fingers(5_500.0), FingerMode::PanOnly, "every sample of the stylus extends it");
    }

    #[test]
    fn each_pointer_has_its_own_press_and_buttons() {
        let mut contacts = Contacts::default();
        let (a, b) = (touch_sample(gpu::PointerPhase::Down, 1), touch_sample(gpu::PointerPhase::Down, 2));
        let mut pen = sample(gpu::PointerPhase::Down, 0.0, 0.0);
        pen.kind = gpu::PointerKind::Pen;
        assert!(!contacts.any() && !contacts.owns(&a));
        contacts.begin(&a);
        contacts.begin(&pen);
        contacts.begin(&a);
        assert!(contacts.owns(&a) && contacts.owns(&pen) && !contacts.owns(&b), "two pointers, the third is not down");

        // A finger that lifts leaves the other pointers down.
        pointer_events(&a, &mut Held::default(), egui::Pos2::ZERO, 0.0);
        if let Some(held) = contacts.held_mut(&a) {
            *held = Held(1);
        }
        let mut up = touch_sample(gpu::PointerPhase::Up, 1);
        up.button = Some(0);
        let held = contacts.held_mut(&up).expect("the finger is down");
        pointer_events(&up, held, egui::Pos2::ZERO, 0.0);
        contacts.release(&up);
        assert!(!contacts.owns(&a), "the finger is gone");
        assert!(contacts.owns(&pen), "the pen is still down");
        contacts.clear();
        assert!(!contacts.any());
    }

    /// Two passes of egui with a bar at the bottom left and the events of a finger, the way the shell feeds them.
    fn press_with_a_finger(at: egui::Pos2) -> (egui::Context, egui::Rect) {
        let canvas = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(820.0, 1180.0));
        let ctx = egui::Context::default();
        let bar = |ui: &egui::Ui| {
            egui::Area::new(egui::Id::unique("bottom_bar"))
                .order(egui::Order::Middle)
                .fixed_pos(egui::pos2(12.0, 1120.0))
                .show(ui.ctx(), |ui| {
                    ui.add_sized(egui::vec2(200.0, 44.0), egui::Button::new("bar"));
                });
        };
        let mut events = Vec::new();
        for pass in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(canvas),
                events: std::mem::take(&mut events),
                ..egui::RawInput::default()
            };
            let mut output = ctx.run_ui(input, |ui| bar(ui));
            output.textures_delta.clear(); // egui asserts that a pass's texture changes are handled
            if pass == 1 {
                // What egui-winit makes of a touch: the touch itself, and for the first finger a pointer press.
                events = vec![
                    egui::Event::Touch {
                        device_id: egui::TouchDeviceId(0),
                        id: egui::TouchId(1),
                        phase: egui::TouchPhase::Start,
                        pos: at,
                        force: None,
                    },
                    egui::Event::PointerMoved(at),
                    egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                ];
            }
        }
        (ctx, canvas)
    }

    #[test]
    fn a_finger_on_the_empty_canvas_is_the_canvas_and_a_finger_on_the_bar_is_egui() {
        let on_canvas = egui::pos2(400.0, 500.0);
        let (ctx, canvas) = press_with_a_finger(on_canvas);
        assert!(!egui_owns(&ctx, canvas, on_canvas), "a touch that presses on empty canvas is not egui's");

        let on_bar = egui::pos2(100.0, 1140.0);
        let (ctx, canvas) = press_with_a_finger(on_bar);
        assert!(egui_owns(&ctx, canvas, on_bar), "a touch on the bar is");
    }

    #[test]
    fn a_finger_outside_the_canvas_rectangle_is_egui() {
        let (ctx, canvas) = press_with_a_finger(egui::pos2(400.0, 500.0));
        assert!(egui_owns(&ctx, canvas, egui::pos2(900.0, 500.0)));
        assert!(egui_owns(&ctx, canvas, egui::pos2(400.0, 1200.0)));
    }

    #[test]
    fn leave_while_dragging_is_ignored() {
        let mut held = Held(1);
        assert!(
            pointer_events(&sample(gpu::PointerPhase::Leave, 0.0, 0.0), &mut held, egui::Pos2::ZERO, 0.0).is_empty()
        );
        let mut idle = Held::default();
        assert_eq!(
            pointer_events(&sample(gpu::PointerPhase::Leave, 0.0, 0.0), &mut idle, egui::Pos2::ZERO, 0.0).len(),
            1
        );
    }

    #[test]
    fn wheel_scrolls_down_with_positive_y_and_a_pinch_is_ctrl_wheel() {
        let events = [
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -30.0),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Wheel,
            },
            egui::Event::Zoom(1.1),
        ];
        let out = wheel_events(&events, Point::new(5.0, 5.0), 0.0, egui::Modifiers::NONE);
        let (InputEvent::Wheel(a), InputEvent::Wheel(b)) = (&out[0], &out[1]) else { unreachable!() };
        assert_eq!(a.delta.y, 30.0);
        assert!(b.modifiers.ctrl && b.delta.y < 0.0);
    }
}
