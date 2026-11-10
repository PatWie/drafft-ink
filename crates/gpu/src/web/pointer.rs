//! Raw pointer samples for every pointer type, with pressure, tilt and coalesced points.
//!
//! winit's web backend turns pen and touch input into plain cursor and touch events and drops pressure
//! (for pens), tilt and the coalesced intermediate positions. These listeners sit next to winit's own on
//! the same canvas; they never stop propagation, so winit still sees every event.
//!
//! Positions are CSS pixels relative to the canvas, which is egui points at zoom 1. Times are
//! `event.timeStamp` mapped onto the `host::now_ms` clock, so samples keep their real spacing even when
//! the page handles them late.

use std::rc::Rc;

use wasm_bindgen::prelude::*;
use web_sys::{EventTarget, HtmlCanvasElement, PointerEvent};

use super::dom::{self, ListenOptions, Listener};
use super::text_agent::Core;
use crate::events::{PointerKind, PointerPhase, PointerPoint, PointerSample, ShellEvent};
use crate::host::{HostSink, now_ms};

#[wasm_bindgen]
extern "C" {
    /// The same object as `PointerEvent`, with the fractional coordinates. web-sys exposes `clientX` as
    /// `i32`, which would round pen strokes to whole pixels.
    type FractionalPointer;
    #[wasm_bindgen(method, getter = clientX)]
    fn client_x(this: &FractionalPointer) -> f64;
    #[wasm_bindgen(method, getter = clientY)]
    fn client_y(this: &FractionalPointer) -> f64;
    #[wasm_bindgen(method, getter = timeStamp)]
    fn time_stamp(this: &FractionalPointer) -> f64;
}

struct Pointers {
    canvas: HtmlCanvasElement,
    sink: HostSink,
    is_mac: bool,
    /// `now_ms() - performance.now()`: add to an event time stamp to get the shell clock.
    clock_offset_ms: f64,
}

pub(super) fn install(
    canvas: &HtmlCanvasElement,
    sink: &HostSink,
    is_mac: bool,
    text: Rc<Core>,
) -> Result<Vec<Listener>, String> {
    let performance_now = dom::window()?.performance().map_or(0.0, |p| p.now());
    let pointers = Rc::new(Pointers {
        canvas: canvas.clone(),
        sink: sink.clone(),
        is_mac,
        clock_offset_ms: now_ms() - performance_now,
    });
    let target: &EventTarget = canvas.as_ref();
    let opts = ListenOptions::default();
    let mut out = Vec::new();

    let p = Rc::clone(&pointers);
    let t = Rc::clone(&text);
    out.push(Listener::new(target, "pointerdown", opts, move |e: PointerEvent| {
        // Keeps text selection and the default focus change away, like winit does when its
        // `preventDefault` is on (the shell turns that off for the keyboard). Other listeners still run.
        e.prevent_default();
        t.focus_for_pointer_down();
        // Keep receiving the drag when it leaves the canvas.
        if let Err(err) = p.canvas.set_pointer_capture(e.pointer_id()) {
            log::debug!("setPointerCapture: {}", dom::js_error(&err));
        }
        p.send(&e, PointerPhase::Down);
    })?);

    let p = Rc::clone(&pointers);
    out.push(Listener::new(target, "pointermove", opts, move |e: PointerEvent| {
        let phase = if e.buttons() == 0 { PointerPhase::Hover } else { PointerPhase::Move };
        p.send(&e, phase);
    })?);

    let p = Rc::clone(&pointers);
    let t = Rc::clone(&text);
    out.push(Listener::new(target, "pointerup", opts, move |e: PointerEvent| {
        p.send(&e, PointerPhase::Up);
        p.release(&e);
        // A user gesture: the place to bring the soft keyboard back (see `Core::refocus_if_wanted`).
        t.refocus_if_wanted();
    })?);

    let p = Rc::clone(&pointers);
    out.push(Listener::new(target, "pointercancel", opts, move |e: PointerEvent| {
        p.send(&e, PointerPhase::Cancel);
        p.release(&e);
    })?);

    let p = Rc::clone(&pointers);
    out.push(Listener::new(target, "pointerleave", opts, move |e: PointerEvent| p.send(&e, PointerPhase::Leave))?);

    // The rest of what winit's `preventDefault` would have done.
    out.push(Listener::new(target, "contextmenu", opts, |e: web_sys::Event| e.prevent_default())?);
    Ok(out)
}

impl Pointers {
    fn release(&self, event: &PointerEvent) {
        // Capture ends by itself on pointerup; releasing explicitly covers cancel and old engines.
        if self.canvas.has_pointer_capture(event.pointer_id())
            && let Err(err) = self.canvas.release_pointer_capture(event.pointer_id())
        {
            log::debug!("releasePointerCapture: {}", dom::js_error(&err));
        }
    }

    fn send(&self, event: &PointerEvent, phase: PointerPhase) {
        let kind = match event.pointer_type().as_str() {
            "pen" => PointerKind::Pen,
            "touch" => PointerKind::Touch,
            _ => PointerKind::Mouse,
        };
        let bounds = self.canvas.get_bounding_client_rect();
        let (left, top) = (bounds.left(), bounds.top());
        let in_contact = event.buttons() != 0;
        let pressure = match kind {
            PointerKind::Mouse => None,
            // A pen reports its pressure also while hovering (0).
            PointerKind::Pen => Some(event.pressure()),
            // Browsers answer 0.5 for a touch screen without pressure sensing. That fixed value is
            // indistinguishable from a real 0.5, so it counts as "not reported".
            PointerKind::Touch => Some(event.pressure()).filter(|p| in_contact && (*p - 0.5).abs() > f32::EPSILON),
        };
        let point = self.point(event, left, top, pressure);

        let mut coalesced = Vec::new();
        if phase == PointerPhase::Move || phase == PointerPhase::Hover {
            coalesced = self.coalesced(event, &point, left, top, kind, in_contact);
        }

        let button = match phase {
            PointerPhase::Down | PointerPhase::Up => u8::try_from(event.button()).ok(),
            _ => None,
        };
        let tilt = (kind == PointerKind::Pen).then(|| [event.tilt_x() as f32, event.tilt_y() as f32]);
        self.sink.app(ShellEvent::Pointer(PointerSample {
            phase,
            kind,
            id: u64::try_from(event.pointer_id()).unwrap_or(0),
            point,
            tilt,
            button,
            modifiers: dom::modifiers_from_mouse(self.is_mac, event),
            coalesced,
        }));
    }

    fn point(&self, event: &PointerEvent, left: f64, top: f64, pressure: Option<f32>) -> PointerPoint {
        let fractional: &FractionalPointer = event.unchecked_ref();
        let (mut x, mut y) = (fractional.client_x(), fractional.client_y());
        if !x.is_finite() || !y.is_finite() {
            x = f64::from(event.client_x());
            y = f64::from(event.client_y());
        }
        let stamp = fractional.time_stamp();
        let time_ms = if stamp.is_finite() && stamp > 0.0 { stamp + self.clock_offset_ms } else { now_ms() };
        PointerPoint { pos: egui::pos2((x - left) as f32, (y - top) as f32), pressure, time_ms }
    }

    /// The positions the browser merged into this move event, oldest first, without the event itself.
    fn coalesced(
        &self,
        event: &PointerEvent,
        main: &PointerPoint,
        left: f64,
        top: f64,
        kind: PointerKind,
        in_contact: bool,
    ) -> Vec<PointerPoint> {
        // Not every browser has it (Safari before 18, some WebViews). Called through Reflect so a missing
        // or throwing method gives an error value instead of a JS exception inside wasm.
        let Ok(list) = dom::call_method(event, "getCoalescedEvents", &[]) else { return Vec::new() };
        let list: js_sys::Array = list.unchecked_into();
        let mut points: Vec<PointerPoint> = list
            .iter()
            .filter_map(|e| e.dyn_into::<PointerEvent>().ok())
            .map(|e| {
                let pressure = match kind {
                    PointerKind::Mouse => None,
                    PointerKind::Pen => Some(e.pressure()),
                    PointerKind::Touch => Some(e.pressure()).filter(|p| in_contact && (*p - 0.5).abs() > f32::EPSILON),
                };
                self.point(&e, left, top, pressure)
            })
            .collect();
        // Chromium lists the dispatched event itself last.
        if points.last().is_some_and(|last| last.pos == main.pos && (last.time_ms - main.time_ms).abs() < 0.5) {
            points.pop();
        }
        points
    }
}
