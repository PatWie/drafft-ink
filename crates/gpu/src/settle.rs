//! How much detail a frame of the compatibility renderer draws, and when to draw the scene again in full.
//!
//! The WebGL2 back end turns every path into strips on the CPU in WebAssembly, so a busy scene costs tens of
//! milliseconds. While the user pans, zooms or drags (frames whose reason is [`RedrawReason::Gesture`]) it draws text as
//! bars. When the gestures stop, one more frame draws the retained scene in full. The wgpu back end is fast enough to
//! always draw everything, and never asks for any of this.

use std::time::Duration;

use drafftink_profile::RedrawReason;
use web_time::Instant;

use crate::backend::Detail;

/// How long after the last gesture frame the scene is drawn again in full.
pub(crate) const SETTLE: Duration = Duration::from_millis(160);

/// What to do for one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Plan {
    /// The detail to draw the scene with.
    pub detail: Detail,
    /// Draw the retained scene (again), although the app did not send a new one.
    pub redraw_scene: bool,
    /// Ask for a frame after this delay, to finish the scene in full.
    pub settle_after: Option<Duration>,
}

/// Tracks whether the retained scene was drawn with reduced detail.
#[derive(Debug, Default)]
pub(crate) struct Settle {
    last_gesture: Option<Instant>,
    reduced: bool,
}

impl Settle {
    /// The plan for a frame at `now`. `compat` is false for the wgpu back end, which always draws in full.
    /// `scene_new`: the app sent a scene this frame.
    pub(crate) fn plan(&mut self, now: Instant, compat: bool, reason: RedrawReason, scene_new: bool) -> Plan {
        if !compat {
            return Plan { detail: Detail::Full, redraw_scene: false, settle_after: None };
        }
        if reason == RedrawReason::Gesture {
            self.last_gesture = Some(now);
        }
        let quiet = self.last_gesture.map_or(Duration::MAX, |t| now.saturating_duration_since(t));
        if quiet < SETTLE {
            // Still moving, or only just stopped.
            self.reduced |= scene_new;
            return Plan {
                detail: Detail::Moving,
                redraw_scene: false,
                settle_after: self.reduced.then(|| SETTLE - quiet),
            };
        }
        let redraw = std::mem::take(&mut self.reduced);
        Plan { detail: Detail::Full, redraw_scene: redraw, settle_after: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn the_wgpu_back_end_always_draws_everything() {
        let mut s = Settle::default();
        let plan = s.plan(Instant::now(), false, RedrawReason::Gesture, true);
        assert_eq!(plan, Plan { detail: Detail::Full, redraw_scene: false, settle_after: None });
    }

    #[test]
    fn a_gesture_draws_less_and_asks_to_finish_later() {
        let t0 = Instant::now();
        let mut s = Settle::default();
        let plan = s.plan(t0, true, RedrawReason::Gesture, true);
        assert_eq!(plan.detail, Detail::Moving);
        assert!(!plan.redraw_scene);
        assert_eq!(plan.settle_after, Some(SETTLE));
        let later = s.plan(t0 + ms(100), true, RedrawReason::Gesture, true);
        assert_eq!(later.settle_after, Some(SETTLE), "each gesture frame moves the deadline");
    }

    #[test]
    fn the_frame_after_the_quiet_time_redraws_in_full_once() {
        let t0 = Instant::now();
        let mut s = Settle::default();
        s.plan(t0, true, RedrawReason::Gesture, true);
        let early = s.plan(t0 + ms(50), true, RedrawReason::Timer, false);
        assert_eq!((early.detail, early.redraw_scene), (Detail::Moving, false));
        assert_eq!(early.settle_after, Some(SETTLE - ms(50)));
        let done = s.plan(t0 + SETTLE, true, RedrawReason::Timer, false);
        assert_eq!(done, Plan { detail: Detail::Full, redraw_scene: true, settle_after: None });
        let again = s.plan(t0 + SETTLE + ms(10), true, RedrawReason::Ui, false);
        assert!(!again.redraw_scene, "nothing was reduced since");
    }

    #[test]
    fn a_frame_that_never_drew_a_reduced_scene_has_nothing_to_finish() {
        let t0 = Instant::now();
        let mut s = Settle::default();
        let hover = s.plan(t0, true, RedrawReason::Gesture, false);
        assert_eq!(hover.settle_after, None);
        assert!(!s.plan(t0 + SETTLE, true, RedrawReason::Timer, false).redraw_scene);
    }

    #[test]
    fn a_scene_the_app_sends_after_the_quiet_time_is_drawn_in_full() {
        let t0 = Instant::now();
        let mut s = Settle::default();
        s.plan(t0, true, RedrawReason::Gesture, true);
        let plan = s.plan(t0 + ms(500), true, RedrawReason::Command, true);
        assert_eq!(plan.detail, Detail::Full);
    }
}
