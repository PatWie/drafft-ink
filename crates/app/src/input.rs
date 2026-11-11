//! From shell events to the canvas library's input events, and from the interaction layer's cursor to egui's.

use drafftink_canvas::{
    Button, Buttons, Cursor, InputEvent, Modifiers, PointerEvent, PointerId, PointerKind, PointerPhase, Tilt,
    WheelEvent, WheelUnit,
};
use drafftink_geom::kurbo::{Point, Vec2};
use drafftink_gpu as gpu;

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
    let make = |phase: PointerPhase, pos: Point, pressure: f32, b: Button, buttons: Buttons| PointerEvent {
        id,
        kind,
        phase,
        pos,
        button: b,
        buttons,
        modifiers: mods,
        pressure,
        tilt,
        time_ms: now_ms,
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
                )));
            }
            out.push(InputEvent::Pointer(make(
                PointerPhase::Move,
                at(s.point.pos),
                pressure,
                Button::None,
                held.buttons(),
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
        Cursor::Grab => egui::CursorIcon::Grab,
        Cursor::Grabbing => egui::CursorIcon::Grabbing,
        Cursor::Crosshair => egui::CursorIcon::Crosshair,
        Cursor::ResizeNs => egui::CursorIcon::ResizeVertical,
        Cursor::ResizeEw => egui::CursorIcon::ResizeHorizontal,
        Cursor::ResizeNesw => egui::CursorIcon::ResizeNeSw,
        Cursor::ResizeNwse => egui::CursorIcon::ResizeNwSe,
        Cursor::Rotate => egui::CursorIcon::Alias,
    }
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
