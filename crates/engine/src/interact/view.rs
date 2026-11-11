//! Pan and zoom (docs/exceptions.md E10 and E11, class C).
//!
//! The camera of the interaction layer's viewport moves at once, with no command. The session learns the camera
//! with `view.set` when the gesture ends ([`Intent::ViewEnd`](drafftink_canvas::Intent)) and at most every
//! [`REPORT_EVERY_MS`] while it goes on, so `view.get`, screenshots and follow mode stay close to the screen. A
//! move that is not reported yet is reported before the next command of the interaction layer runs, and
//! [`Interaction::next_deadline`](super::Interaction::next_deadline) says when the throttle allows the next report.

use drafftink_canvas::Camera;
use drafftink_geom::kurbo::{Point, Vec2};

use super::outcome::Outcome;
use super::world::World;
use crate::cmd::view::SetArgs;
use crate::ring::Origin;
use crate::session::Session;
use crate::spec::Redraw;

/// The shortest time between two camera reports during a pan or zoom.
pub const REPORT_EVERY_MS: f64 = 100.0;

/// Tracks what the session knows about the camera.
#[derive(Debug, Clone, Default)]
pub(crate) struct ViewSync {
    /// The camera the session had when we last looked, or the one we sent.
    known: Option<Camera>,
    /// The viewport camera moved and the session has not been told.
    unreported: bool,
    /// Time of the last report, if any.
    last_report_ms: Option<f64>,
}

impl ViewSync {
    /// Start of every event: size and camera follow the session, unless we moved the camera ourselves.
    pub(crate) fn begin(&mut self, world: &mut World, s: &Session) {
        world.adopt_viewport(s);
        let session_cam = Camera { scroll: s.view().scroll, zoom: s.view().zoom };
        if self.known != Some(session_cam) {
            // A command moved the camera (fit, zoom_in, a script): the session is right.
            world.adopt_camera(s);
            self.known = Some(session_cam);
            self.unreported = false;
        }
    }

    /// Moves the content by `delta` screen pixels.
    pub(crate) fn pan(&mut self, world: &mut World, delta: Vec2, out: &mut Outcome) {
        if world.viewport.pan_by_screen(delta) {
            self.moved(out);
        }
    }

    /// Zooms by `factor` about a screen point.
    pub(crate) fn zoom(&mut self, world: &mut World, at: Point, factor: f64, out: &mut Outcome) {
        if world.viewport.zoom_at(at, factor) {
            self.moved(out);
        }
    }

    fn moved(&mut self, out: &mut Outcome) {
        self.unreported = true;
        out.redraw_at_least(Redraw::Camera);
    }

    /// Reports the camera when a move is waiting and the throttle allows it (`force` ignores the throttle).
    pub(crate) fn report(&mut self, world: &World, s: &mut Session, now_ms: f64, force: bool, out: &mut Outcome) {
        if !self.unreported || !(force || self.last_report_ms.is_none_or(|t| now_ms - t >= REPORT_EVERY_MS)) {
            return;
        }
        let cam = world.viewport.camera();
        let args = SetArgs { scroll: Some([cam.scroll.x, cam.scroll.y]), zoom: Some(cam.zoom) };
        match s.execute_typed(Origin::Ui, "view.set", &args) {
            Ok(_) => {
                self.known = Some(Camera { scroll: s.view().scroll, zoom: s.view().zoom });
                self.unreported = false;
                self.last_report_ms = Some(now_ms);
            }
            Err(r) => out.refusals.push(r),
        }
    }

    /// When the next throttled report is due, if one is waiting.
    pub(crate) fn deadline(&self) -> Option<f64> {
        self.unreported.then(|| self.last_report_ms.map_or(0.0, |t| t + REPORT_EVERY_MS))
    }
}
