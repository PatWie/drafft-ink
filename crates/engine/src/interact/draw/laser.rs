//! The laser pointer (E12): a local trail that fades by the clock, and throttled `collab.laser` sends.
//! Nothing is stored in the document, and nothing touches the history.

use drafftink_geom::kurbo::Point;
use serde_json::json;

use crate::interact::ctx::Ctx;
use crate::interact::events::Ev;
use crate::interact::overlay::LaserPoint;

/// The trail holds at most this many points.
const MAX_POINTS: usize = 50;
/// A point older than this (ms) is gone. Also sent to peers as `fade_ms`.
const FADE_MS: u64 = 330;
/// New points go to peers at most this often (ms).
const SEND_EVERY_MS: f64 = 33.0;
/// The fade is redrawn this often while a trail is showing (ms).
const TICK_MS: f64 = 16.0;

/// Laser state between events.
#[derive(Debug, Default)]
pub(super) struct Laser {
    /// Points not sent to peers yet.
    pending: Vec<[f64; 2]>,
    /// Time of the last send.
    last_sent_ms: f64,
    /// A trail is showing, so the fade needs ticks.
    live: bool,
    /// When the next tick is due.
    next_tick_ms: f64,
}

impl Laser {
    /// The drag starts: the press position is the first point of a new trail, and it goes out at once.
    pub(super) fn begin(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>) {
        self.last_sent_ms = cx.now_ms - SEND_EVERY_MS;
        self.add(cx, ev.origin.world);
    }

    /// The pointer moved with the laser down.
    pub(super) fn update(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>) {
        self.add(cx, ev.at.world);
        self.fade(cx);
        self.send(cx, false);
    }

    /// The pointer was released or the drag was cancelled. The trail stays and fades.
    pub(super) fn end(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>, cancelled: bool) {
        if cancelled {
            self.pending.clear();
            return;
        }
        self.add(cx, ev.at.world);
        self.fade(cx);
        self.send(cx, true);
    }

    /// Drops what is waiting to be sent. The trail on screen stays: it fades by the clock.
    pub(super) fn cancel(&mut self) {
        self.pending.clear();
    }

    fn add(&mut self, cx: &mut Ctx<'_>, p: Point) {
        if cx.parts.laser.last().is_some_and(|l| l.at == p) {
            return;
        }
        cx.parts.laser.push(LaserPoint { at: p, t_ms: cx.now_ms });
        let excess = cx.parts.laser.len().saturating_sub(MAX_POINTS);
        cx.parts.laser.drain(..excess);
        self.pending.push([p.x, p.y]);
        self.live = true;
        self.next_tick_ms = cx.now_ms + TICK_MS;
        cx.overlay_changed();
    }

    /// Drops the points that are older than the fade time.
    fn fade(&mut self, cx: &mut Ctx<'_>) {
        let now = cx.now_ms;
        let before = cx.parts.laser.len();
        cx.parts.laser.retain(|p| now - p.t_ms <= FADE_MS as f64);
        if cx.parts.laser.len() != before {
            cx.overlay_changed();
        }
    }

    /// Sends the new points when 33 ms have passed since the last send, or at once with `force`.
    fn send(&mut self, cx: &mut Ctx<'_>, force: bool) {
        if self.pending.is_empty() || (!force && cx.now_ms - self.last_sent_ms < SEND_EVERY_MS) {
            return;
        }
        self.last_sent_ms = cx.now_ms;
        let points = std::mem::take(&mut self.pending);
        match cx.try_exec("collab.laser", json!({ "points": points, "fade_ms": FADE_MS })) {
            Ok(_) => {}
            // The collab family or its port may not exist (a tab with no network): the trail is local then.
            Err(r) if matches!(r.code, "unknown_command" | "port_unavailable") => {}
            Err(r) => cx.out.refusals.push(r),
        }
    }

    /// Time passes: the trail fades, and points that waited for the throttle go out.
    pub(super) fn tick(&mut self, cx: &mut Ctx<'_>) {
        if !self.live {
            return;
        }
        self.fade(cx);
        self.send(cx, false);
        self.live = !cx.parts.laser.is_empty();
        self.next_tick_ms = cx.now_ms + TICK_MS;
        if self.live {
            cx.overlay_changed();
        }
    }

    /// When the next tick is due, while a trail shows.
    pub(super) fn deadline(&self) -> Option<f64> {
        self.live.then_some(self.next_tick_ms)
    }
}
