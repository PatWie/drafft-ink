//! The repaint rules, as plain data so tests can drive them without a window.
//!
//! A frame is drawn only when something asked for it with a reason. Requests are recorded with
//! `drafftink_profile::redraw`, so every frame can be explained. A delayed request becomes a
//! `ControlFlow::WaitUntil` deadline; with nothing pending the loop sleeps in `ControlFlow::Wait`.

use std::time::Duration;

use drafftink_profile::{RedrawReason, redraw};
use web_time::Instant;
use winit::event_loop::ControlFlow;

use crate::events::Repaint;

/// Pending repaint work.
#[derive(Debug, Default)]
pub(crate) struct RepaintState {
    /// A frame is due now; the first reason that asked.
    now: Option<RedrawReason>,
    /// The earliest delayed request.
    later: Option<(Instant, RedrawReason)>,
    hidden: bool,
    /// `Window::request_redraw` was called and the matching `RedrawRequested` has not arrived yet.
    requested: bool,
}

impl RepaintState {
    /// Ask for a frame now. Returns true when the caller must call `Window::request_redraw`
    /// (a frame was not already requested, and the window is visible).
    pub(crate) fn request_now(&mut self, reason: RedrawReason) -> bool {
        redraw(reason);
        if self.now.is_none() {
            self.now = Some(reason);
        }
        self.needs_redraw_call()
    }

    /// True once per pending frame when the caller must call `Window::request_redraw`: a frame is
    /// pending, none has been requested yet, and the window is visible.
    pub(crate) fn needs_redraw_call(&mut self) -> bool {
        if self.now.is_some() && !self.requested && !self.hidden {
            self.requested = true;
            return true;
        }
        false
    }

    /// Ask for a frame after `delay`. Keeps the earliest deadline. A delay too large to represent means never.
    pub(crate) fn request_after(&mut self, now: Instant, delay: Duration, reason: RedrawReason) {
        let Some(at) = now.checked_add(delay) else { return };
        match self.later {
            Some((old, _)) if old <= at => {}
            _ => self.later = Some((at, reason)),
        }
    }

    /// Apply an app answer. Returns true when `Window::request_redraw` must be called.
    pub(crate) fn apply(&mut self, now: Instant, repaint: Repaint) -> bool {
        match repaint {
            Repaint::Idle => false,
            Repaint::Now(reason) => self.request_now(reason),
            Repaint::After(delay, reason) if delay.is_zero() => self.request_now(reason),
            Repaint::After(delay, reason) => {
                self.request_after(now, delay, reason);
                false
            }
        }
    }

    /// A wake-up arrived. If the deadline passed, returns the reason it was asked with (`timer` unless the
    /// request named another) and clears it; the caller decides whether that becomes a frame
    /// ([`RepaintState::request_now`]) or goes to the app first.
    pub(crate) fn expired(&mut self, now: Instant) -> Option<RedrawReason> {
        match self.later {
            Some((at, reason)) if at <= now => {
                self.later = None;
                Some(reason)
            }
            _ => None,
        }
    }

    /// The window is about to draw. Returns why, and clears the request and any older deadline.
    /// `None` means nobody asked: the OS exposed the window (X11 expose, a compositor request).
    pub(crate) fn take(&mut self) -> Option<RedrawReason> {
        self.later = None;
        self.requested = false;
        self.now.take()
    }

    /// A frame was due but could not be drawn (surface not ready, GPU not ready). Keep it pending;
    /// the next `needs_redraw_call` asks for the redraw again.
    pub(crate) fn keep(&mut self, reason: RedrawReason) {
        if self.now.is_none() {
            self.now = Some(reason);
        }
        self.requested = false;
    }

    #[cfg(test)]
    pub(crate) fn pending_now(&self) -> bool {
        self.now.is_some()
    }

    #[cfg(test)]
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.later.map(|(at, _)| at)
    }

    pub(crate) fn hidden(&self) -> bool {
        self.hidden
    }

    /// Set visibility. Returns true when the window just became visible with a frame pending.
    pub(crate) fn set_hidden(&mut self, hidden: bool) -> bool {
        let was = self.hidden;
        self.hidden = hidden;
        was && !hidden && self.needs_redraw_call()
    }

    /// The control flow for the event loop: `Wait`, or `WaitUntil` the earliest deadline.
    pub(crate) fn control_flow(&self) -> ControlFlow {
        match self.later {
            Some((at, _)) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        }
    }
}

/// egui queues one `PointerMoved` per hover event. While no frame is drawn they would pile up for
/// as long as the pointer moves over the canvas. Keep only the newest of a run.
pub(crate) fn coalesce_pointer_moves(events: &mut Vec<egui::Event>) {
    let n = events.len();
    if n >= 2
        && matches!(events[n - 1], egui::Event::PointerMoved(_))
        && matches!(events[n - 2], egui::Event::PointerMoved(_))
    {
        events.swap_remove(n - 2);
    }
}

/// Does egui, not the app, own the pointer at `pos`? True while egui drags something, over a floating
/// egui area (window, popup, menu), and outside the canvas rectangle (toolbars and panels).
pub(crate) fn egui_owns_pointer(ctx: &egui::Context, canvas: Option<egui::Rect>, pos: Option<egui::Pos2>) -> bool {
    if ctx.egui_is_using_pointer() {
        return true;
    }
    let Some(pos) = pos else { return false };
    if canvas.is_some_and(|rect| !rect.contains(pos)) {
        return true;
    }
    ctx.layer_id_at(pos).is_some_and(|layer| layer.order != egui::Order::Background)
}

#[cfg(test)]
mod tests {
    use super::*;
    use drafftink_profile::WorkScope;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn idle_state_waits_forever() {
        let s = RepaintState::default();
        assert!(matches!(s.control_flow(), ControlFlow::Wait));
        assert!(!s.pending_now());
    }

    #[test]
    fn a_request_is_recorded_with_its_reason_and_asks_for_one_redraw() {
        let scope = WorkScope::begin();
        let mut s = RepaintState::default();
        assert!(s.request_now(RedrawReason::Command));
        assert!(!s.request_now(RedrawReason::Gesture), "second request joins the pending frame");
        assert!(!s.needs_redraw_call(), "already requested");
        assert_eq!(scope.redraw_reasons(), vec![RedrawReason::Command, RedrawReason::Gesture]);
        assert_eq!(s.take(), Some(RedrawReason::Command));
        assert_eq!(s.take(), None, "an OS expose has no reason of ours");
    }

    #[test]
    fn delayed_requests_become_wait_until_and_fire_once() {
        let t0 = Instant::now();
        let mut s = RepaintState::default();
        assert!(!s.apply(t0, Repaint::After(ms(100), RedrawReason::Autosave)));
        assert!(!s.apply(t0, Repaint::After(ms(300), RedrawReason::Timer)), "later request does not replace");
        assert!(matches!(s.control_flow(), ControlFlow::WaitUntil(at) if at == t0 + ms(100)));
        assert_eq!(s.expired(t0 + ms(50)), None, "too early");
        let scope = WorkScope::begin();
        assert_eq!(s.expired(t0 + ms(100)), Some(RedrawReason::Autosave));
        assert!(scope.redraw_reasons().is_empty(), "an expired deadline is not a frame request by itself");
        assert!(s.request_now(RedrawReason::Autosave));
        assert_eq!(scope.redraw_reasons(), vec![RedrawReason::Autosave]);
        assert!(matches!(s.control_flow(), ControlFlow::Wait));
        assert_eq!(s.take(), Some(RedrawReason::Autosave));
    }

    #[test]
    fn earlier_deadline_wins() {
        let t0 = Instant::now();
        let mut s = RepaintState::default();
        s.request_after(t0, ms(300), RedrawReason::Timer);
        s.request_after(t0, ms(20), RedrawReason::CaretBlink);
        assert_eq!(s.deadline(), Some(t0 + ms(20)));
    }

    #[test]
    fn huge_delay_means_never() {
        let mut s = RepaintState::default();
        s.request_after(Instant::now(), Duration::MAX, RedrawReason::Timer);
        assert_eq!(s.deadline(), None);
    }

    #[test]
    fn zero_delay_is_now() {
        let mut s = RepaintState::default();
        assert!(s.apply(Instant::now(), Repaint::After(Duration::ZERO, RedrawReason::Ui)));
        assert!(s.pending_now());
    }

    #[test]
    fn hidden_window_keeps_the_request_and_draws_when_visible_again() {
        let mut s = RepaintState::default();
        s.set_hidden(true);
        assert!(!s.request_now(RedrawReason::Collab), "no redraw call while hidden");
        assert!(s.pending_now());
        assert!(s.set_hidden(false), "visible again with a frame pending");
        assert!(!s.set_hidden(false));
    }

    #[test]
    fn a_kept_frame_asks_for_its_redraw_again() {
        let mut s = RepaintState::default();
        assert!(s.request_now(RedrawReason::Startup));
        assert_eq!(s.take(), Some(RedrawReason::Startup));
        s.keep(RedrawReason::Startup); // the surface was not ready
        assert!(s.pending_now());
        assert!(s.needs_redraw_call());
        assert!(!s.needs_redraw_call());
    }

    #[test]
    fn idle_answer_requests_nothing() {
        let mut s = RepaintState::default();
        let scope = WorkScope::begin();
        assert!(!s.apply(Instant::now(), Repaint::Idle));
        assert_eq!(scope.redraws(), 0);
        assert!(!s.pending_now());
    }

    #[test]
    fn pointer_moves_coalesce() {
        let mut ev = vec![egui::Event::PointerGone];
        for i in 0..1000 {
            ev.push(egui::Event::PointerMoved(egui::pos2(i as f32, 0.0)));
            coalesce_pointer_moves(&mut ev);
        }
        assert_eq!(ev.len(), 2);
        assert!(matches!(ev[1], egui::Event::PointerMoved(p) if p.x == 999.0));
        ev.push(egui::Event::PointerGone);
        ev.push(egui::Event::PointerMoved(egui::pos2(1.0, 1.0)));
        coalesce_pointer_moves(&mut ev);
        assert_eq!(ev.len(), 4, "a move after another event type is kept");
    }
}
