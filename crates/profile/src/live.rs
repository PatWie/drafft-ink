//! The process-wide recorder: frames, phases, snapshot and reset.

use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::clock::now_ns;
use crate::frame::{DEFAULT_WINDOW, FrameRecorder};
use crate::names::Phase;
use crate::spans::{self, Span};
use crate::stats::PerfStats;
use crate::work;

static FRAMES: Mutex<FrameRecorder> = Mutex::new(FrameRecorder::new(DEFAULT_WINDOW));

fn frames() -> MutexGuard<'static, FrameRecorder> {
    FRAMES.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Mark the start of a frame. Takes the redraw reasons requested since the previous frame.
/// Does nothing but clear them when recording is off.
pub fn begin_frame() {
    let reasons = work::take_pending_reasons();
    if spans::enabled() {
        frames().begin(now_ns(), reasons);
    }
}

/// Mark the end of the frame (after present). Does nothing when recording is off.
pub fn end_frame() {
    if spans::enabled() {
        frames().end(now_ns());
    }
}

/// Resize the rolling frame window (default 1024 frames) and clear the frame statistics.
pub fn set_frame_window(frames_kept: usize) {
    *frames() = FrameRecorder::new(frames_kept);
}

/// Time one phase of the current frame: `let _p = profile::phase(Phase::Encode);`.
/// The time is added to the open frame's phase total, and a span of the same name is recorded.
/// Off, it costs one relaxed atomic load.
#[must_use = "a phase measures until it is dropped"]
pub struct PhaseGuard {
    phase: Phase,
    /// When timing started; `None` while recording is off.
    start_ns: Option<u64>,
    /// What the phase that was open when this one began had already counted for its children.
    outer_children_ns: u64,
    _span: Span,
}

thread_local! {
    /// Time of the phases that ended inside the innermost open phase. A phase counts its own time only, so the
    /// phases of a frame never add up to more than the frame (`encode` inside `ui` is not `ui` as well).
    static CHILDREN_NS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Start timing `phase` of the current frame.
#[inline]
pub fn phase(phase: Phase) -> PhaseGuard {
    let start_ns = spans::enabled().then(now_ns);
    let outer_children_ns = if start_ns.is_some() { CHILDREN_NS.with(|c| c.replace(0)) } else { 0 };
    PhaseGuard { phase, start_ns, outer_children_ns, _span: spans::span(phase.name()) }
}

impl Drop for PhaseGuard {
    #[inline]
    fn drop(&mut self) {
        if let Some(start_ns) = self.start_ns {
            let took = now_ns().saturating_sub(start_ns);
            let inside = CHILDREN_NS.with(|c| c.replace(self.outer_children_ns.saturating_add(took)));
            frames().add_phase(self.phase, took.saturating_sub(inside));
        }
    }
}

/// Run `f` as a phase of the current frame.
#[inline]
pub fn in_phase<T>(p: Phase, f: impl FnOnce() -> T) -> T {
    let _g = phase(p);
    f()
}

/// Everything recorded since the last [`reset`].
pub fn snapshot() -> PerfStats {
    let data = spans::data();
    let frame_stats = frames().stats();
    let mut named = work::global_named();
    named.sort_unstable();
    PerfStats::assemble(
        frame_stats,
        work::global_counters(),
        work::global_gauges(),
        work::global_redraws(),
        named,
        work::recent_redraws(),
        data.agg,
        data.dropped,
    )
}

/// Clear spans, frames, counters, gauges and redraw counts. Thread-local scopes are
/// unaffected. Whether recording is on stays as it was.
pub fn reset() {
    spans::reset();
    frames().reset();
    work::reset_global();
}

/// Copies of the frames in the rolling window and the finished spans, for the trace.
pub(crate) fn frame_records() -> Vec<crate::frame::FrameRecord> {
    frames().window().copied().collect()
}

#[cfg(all(test, feature = "enabled"))]
mod tests {
    use super::*;
    use crate::names::{Counter, RedrawReason};
    use crate::test_lock;

    #[test]
    fn frames_phases_and_reasons_flow_into_the_snapshot() {
        let _l = test_lock();
        reset();
        spans::set_enabled(true);
        work::redraw(RedrawReason::Gesture);
        work::redraw(RedrawReason::HoverChange);
        begin_frame();
        {
            let _p = phase(Phase::Encode);
            work::count(Counter::ElementsEncoded, 7);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        in_phase(Phase::Ui, || ());
        end_frame();
        // a frame with nothing requested is unexplained
        begin_frame();
        end_frame();
        spans::set_enabled(false);

        let s = snapshot();
        assert_eq!(s.frames.total, 2);
        assert_eq!(s.frames.unexplained, 1);
        assert_eq!(s.frames.by_reason["gesture"], 1);
        assert_eq!(s.frames.by_reason["hover_change"], 1);
        assert!(s.frames.phase_ms["encode"].max_ms >= 2.0);
        assert_eq!(s.redraws.total, 2);
        assert_eq!(s.redraws.recent.len(), 2);
        assert_eq!(s.redraws.recent[0].reason, "gesture");
        assert_eq!(s.counters["elements_encoded"], 7);
        assert!(s.spans.iter().any(|r| r.name == "encode" && r.count == 1));
        assert!(s.compiled);
    }

    #[test]
    fn a_phase_inside_another_counts_for_itself_only() {
        let _l = test_lock();
        reset();
        spans::set_enabled(true);
        work::redraw(RedrawReason::Gesture);
        begin_frame();
        in_phase(Phase::Ui, || {
            std::thread::sleep(std::time::Duration::from_millis(2));
            in_phase(Phase::Encode, || std::thread::sleep(std::time::Duration::from_millis(6)));
            // A second inner phase after the first one ended.
            in_phase(Phase::Encode, || std::thread::sleep(std::time::Duration::from_millis(2)));
        });
        end_frame();
        spans::set_enabled(false);
        let s = snapshot();
        let (ui, encode, other) = (s.frames.phase_ms["ui"].max_ms, s.frames.phase_ms["encode"].max_ms, s.frames.phase_ms["other"].max_ms);
        assert!((8.0..11.0).contains(&encode), "encode {encode}");
        assert!((2.0..5.0).contains(&ui), "ui keeps its own time only: {ui}");
        assert!(other < 2.0, "the phases add up to the frame: other {other}");
    }

    #[test]
    fn frames_are_not_recorded_while_off_but_reasons_are_consumed() {
        let _l = test_lock();
        reset();
        spans::set_enabled(false);
        work::redraw(RedrawReason::Command);
        begin_frame();
        {
            let _p = phase(Phase::Layout);
        }
        end_frame();
        assert_eq!(snapshot().frames.total, 0);
        // the stale reason must not leak into the first frame recorded after enabling
        spans::set_enabled(true);
        begin_frame();
        end_frame();
        spans::set_enabled(false);
        assert_eq!(snapshot().frames.unexplained, 1);
    }

    #[test]
    fn reset_clears_global_state_but_not_thread_scopes() {
        let _l = test_lock();
        let scope = work::WorkScope::begin();
        work::count(Counter::CacheHit, 3);
        work::count_named("live_test_name", 1);
        assert!(snapshot().counters["cache_hit"] >= 3);
        reset();
        let s = snapshot();
        assert_eq!(s.counters["cache_hit"], 0);
        assert!(!s.counters.contains_key("live_test_name"));
        assert_eq!(scope.count(Counter::CacheHit), 3);
    }

    #[test]
    fn frame_window_can_be_resized() {
        let _l = test_lock();
        reset();
        set_frame_window(2);
        spans::set_enabled(true);
        for _ in 0..5 {
            work::redraw(RedrawReason::Timer);
            begin_frame();
            end_frame();
        }
        spans::set_enabled(false);
        let s = snapshot();
        assert_eq!((s.frames.total, s.frames.window), (5, 2));
        set_frame_window(DEFAULT_WINDOW);
    }
}
