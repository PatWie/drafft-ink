//! A live frame meter for the debug panel: frames per second, frame time and a short history.
//!
//! [`crate::FrameRecorder`] keeps a long window for reports and sorts it to give percentiles. This meter is the
//! small, cheap one that the debug panel reads every refresh: it keeps the frames of the last second and a
//! few older ones for the sparkline, and takes explicit timestamps, so its behaviour is tested without a clock.
//!
//! The app redraws on demand. A meter that nothing feeds is not broken, it is idle: [`FrameReading::idle`] says
//! so, and the frame rate then is 0 because no frame was drawn, not because frames are slow.
//!
//! The panel that shows the numbers costs time itself. The meter keeps that apart: a frame is recorded as a
//! [`FrameCost`] with the panel's share named, the share is taken out of the frame time ([`FrameCost::counted_ns`]),
//! and it is reported on its own ([`FrameReading::panel_ms`]). Unreal's `stat unit` splits a frame into named
//! threads for the same reason: what is named can be left out.

use std::collections::VecDeque;

use crate::clock::ns_to_ms;

/// The span of time a [`FrameReading`] describes: one second.
pub const METER_WINDOW_NS: u64 = 1_000_000_000;

/// Frames the sparkline shows, at most.
pub const SPARKLINE_FRAMES: usize = 90;

/// Frames kept: a second at 240 Hz, and never fewer than the sparkline needs.
const KEPT: usize = 256;

/// A part of a frame, for the breakdown the panel shows. The parts are CPU time, in the order the frame runs them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// The egui pass and the app's own work around it, without the scene build and without the debug panel.
    Ui,
    /// Building the canvas scene.
    Scene,
    /// Handing the scene to the GPU renderer (Vello): the CPU side of it.
    Render,
    /// Tessellating egui, compositing and presenting, without the debug panel's share of the tessellation.
    Present,
}

impl Stage {
    /// Every stage, in frame order.
    pub const ALL: [Stage; 4] = [Stage::Ui, Stage::Scene, Stage::Render, Stage::Present];

    /// The name the panel shows.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ui => "ui",
            Self::Scene => "scene",
            Self::Render => "render",
            Self::Present => "present",
        }
    }

    const fn index(self) -> usize {
        self as usize
    }
}

/// The time of each [`Stage`] of one frame. A stage that was not timed (a shell without a window has no present) is
/// `None`, never 0.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StageTimes([Option<u64>; Stage::ALL.len()]);

impl StageTimes {
    /// The same times with `stage` set to `ns`.
    #[must_use]
    pub fn with(mut self, stage: Stage, ns: u64) -> Self {
        if let Some(slot) = self.0.get_mut(stage.index()) {
            *slot = Some(ns);
        }
        self
    }

    /// The time of `stage`, if it was timed.
    pub fn get(&self, stage: Stage) -> Option<u64> {
        self.0.get(stage.index()).copied().flatten()
    }
}

/// What one frame cost, as the shell measured it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameCost {
    /// CPU time from the start of the frame to the end of the present call, with everything in it.
    pub gross_ns: u64,
    /// The part of it that the debug panel itself took: building it, laying it out, tessellating it.
    pub panel_ns: u64,
    /// The breakdown, with the panel already taken out of the stages it ran in.
    pub stages: StageTimes,
}

impl FrameCost {
    /// A frame of `gross_ns` with no panel in it and no breakdown.
    pub const fn new(gross_ns: u64) -> Self {
        Self { gross_ns, panel_ns: 0, stages: StageTimes([None; Stage::ALL.len()]) }
    }

    /// The same frame with `panel_ns` of it spent on the debug panel.
    #[must_use]
    pub const fn with_panel(mut self, panel_ns: u64) -> Self {
        self.panel_ns = panel_ns;
        self
    }

    /// The same frame with `stage` timed.
    #[must_use]
    pub fn with_stage(mut self, stage: Stage, ns: u64) -> Self {
        self.stages = self.stages.with(stage, ns);
        self
    }

    /// The panel's share, never more than the frame it is part of: a clock that reads a little apart (the panel
    /// is timed in pieces) must not make the frame negative.
    pub const fn panel_share_ns(&self) -> u64 {
        if self.panel_ns < self.gross_ns { self.panel_ns } else { self.gross_ns }
    }

    /// The frame time that counts: the frame without the panel. Never negative.
    pub const fn counted_ns(&self) -> u64 {
        self.gross_ns - self.panel_share_ns()
    }
}

/// What a counted frame adds to the readings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Counted {
    /// The frame time without the panel.
    cpu_ns: u64,
    stages: StageTimes,
}

/// One frame as the meter saw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Sample {
    /// When the frame began.
    start_ns: u64,
    /// What the debug panel took of it.
    panel_ns: u64,
    /// The frame time, or `None` for a frame that exists only to refresh the panel: it is not counted in the
    /// frame rate or the frame time, but its panel cost is real and is reported.
    counted: Option<Counted>,
}

/// Keeps the recent frames and answers [`FrameMeter::reading`].
#[derive(Clone, Debug, Default)]
pub struct FrameMeter {
    samples: VecDeque<Sample>,
}

impl FrameMeter {
    /// An empty meter.
    pub const fn new() -> Self {
        Self { samples: VecDeque::new() }
    }

    /// Records a frame that began at `start_ns`. Its time is the frame without the debug panel's share.
    pub fn record(&mut self, start_ns: u64, cost: FrameCost) {
        let counted = Counted { cpu_ns: cost.counted_ns(), stages: cost.stages };
        self.push(Sample { start_ns, panel_ns: cost.panel_share_ns(), counted: Some(counted) });
    }

    /// Records a frame that exists only to refresh the panel: it takes `panel_ns` and counts for nothing else.
    pub fn record_refresh(&mut self, start_ns: u64, panel_ns: u64) {
        self.push(Sample { start_ns, panel_ns, counted: None });
    }

    fn push(&mut self, sample: Sample) {
        if self.samples.len() >= KEPT {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
    }

    /// Forgets every frame (the panel was closed: its next opening must not show old numbers).
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// What the frames of the last second say, at `now_ns`.
    pub fn reading(&self, now_ns: u64) -> FrameReading {
        let from = now_ns.saturating_sub(METER_WINDOW_NS);
        let counted = || self.samples.iter().filter_map(|s| s.counted.map(|c| (s.start_ns, c)));
        let recent = || counted().filter(move |(start, _)| *start >= from).map(|(_, c)| c);
        let frames = recent().count();
        let latest = counted().next_back();
        let mut history_ms: Vec<f32> =
            counted().rev().take(SPARKLINE_FRAMES).map(|(_, c)| (c.cpu_ns as f64 / 1e6) as f32).collect();
        history_ms.reverse();
        // The panel's cost counts every frame in the window, the refresh frames too; an idle canvas has only those.
        let panel_ns = mean_ns(self.samples.iter().filter(|s| s.start_ns >= from).map(|s| s.panel_ns))
            .or_else(|| self.samples.back().map(|s| s.panel_ns));
        FrameReading {
            frames: u32::try_from(frames).unwrap_or(u32::MAX),
            avg_ms: mean_ns(recent().map(|c| c.cpu_ns)).map_or(0.0, ns_to_ms),
            worst_ms: ns_to_ms(recent().map(|c| c.cpu_ns).max().unwrap_or(0)),
            last_ms: latest.map(|(_, c)| ns_to_ms(c.cpu_ns)),
            since_last_ms: latest.map(|(start, _)| ns_to_ms(now_ns.saturating_sub(start))),
            idle: frames == 0,
            history_ms,
            panel_ms: panel_ns.map(ns_to_ms),
            stages: Stage::ALL
                .into_iter()
                .filter_map(|stage| {
                    let avg = mean_ns(recent().filter_map(|c| c.stages.get(stage)))?;
                    Some(StageReading { stage, avg_ms: ns_to_ms(avg) })
                })
                .collect(),
        }
    }
}

/// The mean of some times, or `None` for no times at all.
fn mean_ns(times: impl Iterator<Item = u64>) -> Option<u64> {
    let (sum, n) = times.fold((0u128, 0u128), |(sum, n), t| (sum + u128::from(t), n + 1));
    sum.checked_div(n).map(|mean| u64::try_from(mean).unwrap_or(u64::MAX))
}

/// The mean time of one [`Stage`] over the counted frames of the last second.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StageReading {
    /// Which part of the frame.
    pub stage: Stage,
    /// Its mean CPU time, in milliseconds.
    pub avg_ms: f64,
}

/// What the frames of the last second say.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameReading {
    /// Frames drawn in the last second: the frame rate.
    pub frames: u32,
    /// Mean CPU time of those frames, in milliseconds, without the debug panel's own cost.
    pub avg_ms: f64,
    /// Longest CPU time among them, without the debug panel's own cost.
    pub worst_ms: f64,
    /// CPU time of the latest frame, however long ago it was. `None` before the first frame.
    pub last_ms: Option<f64>,
    /// How long ago the latest frame began. `None` before the first frame.
    pub since_last_ms: Option<f64>,
    /// No frame was drawn in the last second. The app draws on demand, so this is the normal state of a quiet
    /// canvas and not a fault.
    pub idle: bool,
    /// CPU time of the latest frames, oldest first, at most [`SPARKLINE_FRAMES`]: the sparkline.
    pub history_ms: Vec<f32>,
    /// What the debug panel itself cost per frame, left out of every time above: the mean of the last second (its
    /// refresh frames included), or of the latest frame when there was none in the last second. `None` before the
    /// first frame.
    pub panel_ms: Option<f64>,
    /// Where the counted frames spent their time, the stages that were timed. Empty when no frame is counted.
    pub stages: Vec<StageReading>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    fn frame(ms: u64) -> FrameCost {
        FrameCost::new(ms * MS)
    }

    #[test]
    fn a_meter_without_frames_is_idle() {
        let r = FrameMeter::new().reading(5_000 * MS);
        assert!(r.idle);
        assert_eq!(r.frames, 0);
        assert_eq!(r.last_ms, None);
        assert_eq!(r.panel_ms, None, "no frame, no panel cost");
        assert!(r.history_ms.is_empty());
        assert!(r.stages.is_empty());
    }

    #[test]
    fn sixty_frames_in_a_second_read_as_sixty_fps_with_their_mean_and_worst() {
        let mut m = FrameMeter::new();
        for i in 0..60u64 {
            // 16 ms apart, 2 ms each, one slow frame of 9 ms.
            m.record(i * 16 * MS + 500 * MS, frame(if i == 30 { 9 } else { 2 }));
        }
        let r = m.reading(1_400 * MS);
        assert_eq!(r.frames, 60);
        assert!(!r.idle);
        assert_eq!(r.worst_ms, 9.0);
        assert!((r.avg_ms - (59.0 * 2.0 + 9.0) / 60.0).abs() < 0.001, "{}", r.avg_ms);
        assert_eq!(r.history_ms.len(), 60);
    }

    #[test]
    fn frames_older_than_a_second_leave_the_rate_but_not_the_last_frame() {
        let mut m = FrameMeter::new();
        m.record(100 * MS, frame(3));
        m.record(200 * MS, frame(5));
        let r = m.reading(2_000 * MS);
        assert!(r.idle, "no frame in the last second");
        assert_eq!(r.frames, 0);
        assert_eq!(r.avg_ms, 0.0);
        assert_eq!(r.last_ms, Some(5.0));
        assert_eq!(r.since_last_ms, Some(1_800.0));
        assert_eq!(r.history_ms, vec![3.0, 5.0], "the sparkline keeps the last burst");
    }

    #[test]
    fn the_window_is_closed_at_one_second_back() {
        let mut m = FrameMeter::new();
        m.record(1_000 * MS, frame(1));
        assert_eq!(m.reading(2_000 * MS).frames, 1, "exactly a second ago is still in");
        assert_eq!(m.reading(2_000 * MS + 1).frames, 0);
    }

    #[test]
    fn the_sparkline_is_capped_and_keeps_the_newest_frames() {
        let mut m = FrameMeter::new();
        for i in 0..300u64 {
            m.record(i * MS, FrameCost::new(i * 1000));
        }
        let r = m.reading(300 * MS);
        assert_eq!(r.history_ms.len(), SPARKLINE_FRAMES);
        assert_eq!(r.history_ms.last().copied(), Some(0.299));
        assert!(r.frames as usize <= KEPT);
    }

    #[test]
    fn clear_forgets_everything() {
        let mut m = FrameMeter::new();
        m.record(MS, frame(1));
        m.clear();
        assert_eq!(m.reading(2 * MS), FrameMeter::new().reading(2 * MS));
    }

    #[test]
    fn the_panel_is_taken_out_of_the_frame_and_reported_on_its_own() {
        let mut m = FrameMeter::new();
        m.record(100 * MS, frame(5).with_panel(2 * MS));
        let r = m.reading(200 * MS);
        assert_eq!(r.avg_ms, 3.0, "5 ms of frame, 2 ms of it the panel");
        assert_eq!(r.worst_ms, 3.0);
        assert_eq!(r.last_ms, Some(3.0));
        assert_eq!(r.history_ms, vec![3.0]);
        assert_eq!(r.panel_ms, Some(2.0));
    }

    #[test]
    fn a_panel_dearer_than_its_frame_leaves_zero_and_never_a_negative_or_wrapped_time() {
        // The panel is timed in pieces and the frame as a whole, so the pieces can add up to more than the whole.
        for (gross, panel) in
            [(0, 0), (0, 5), (5, 0), (5, 5), (5, 6), (1, u64::MAX), (u64::MAX, u64::MAX), (u64::MAX, 1)]
        {
            let cost = FrameCost::new(gross).with_panel(panel);
            assert!(cost.counted_ns() <= gross, "{gross} {panel}");
            assert!(cost.panel_share_ns() <= gross, "{gross} {panel}");
            assert_eq!(cost.counted_ns() + cost.panel_share_ns(), gross, "nothing is lost or made up: {gross} {panel}");
        }
        let mut m = FrameMeter::new();
        m.record(MS, frame(2).with_panel(9 * MS));
        let r = m.reading(2 * MS);
        assert_eq!(r.avg_ms, 0.0);
        assert_eq!(r.worst_ms, 0.0);
        assert_eq!(r.panel_ms, Some(2.0), "what the panel is reported at cannot be more than the frame it was in");
        assert!(r.history_ms.iter().all(|ms| *ms >= 0.0));
    }

    #[test]
    fn a_refresh_frame_costs_the_panel_and_counts_for_nothing_else() {
        let mut m = FrameMeter::new();
        m.record_refresh(100 * MS, MS);
        let r = m.reading(200 * MS);
        assert!(r.idle, "a refresh is not a drawn frame");
        assert_eq!(r.frames, 0);
        assert_eq!(r.last_ms, None);
        assert!(r.history_ms.is_empty());
        assert_eq!(r.panel_ms, Some(1.0), "but the panel's cost is known from it");
    }

    #[test]
    fn the_panel_cost_is_the_mean_of_every_frame_in_the_window_refreshes_included() {
        let mut m = FrameMeter::new();
        m.record(100 * MS, frame(4).with_panel(MS));
        m.record_refresh(200 * MS, 3 * MS);
        let r = m.reading(300 * MS);
        assert_eq!(r.panel_ms, Some(2.0));
        assert_eq!(r.frames, 1);
        assert_eq!(r.avg_ms, 3.0);
    }

    #[test]
    fn the_panel_cost_of_a_quiet_second_is_the_latest_frames() {
        let mut m = FrameMeter::new();
        m.record(100 * MS, frame(4).with_panel(MS));
        assert_eq!(m.reading(5_000 * MS).panel_ms, Some(1.0));
    }

    #[test]
    fn stages_are_averaged_over_the_frames_that_timed_them() {
        let mut m = FrameMeter::new();
        m.record(100 * MS, frame(6).with_stage(Stage::Ui, 2 * MS).with_stage(Stage::Scene, MS));
        m.record(200 * MS, frame(8).with_stage(Stage::Ui, 4 * MS).with_stage(Stage::Present, 3 * MS));
        let r = m.reading(300 * MS);
        let stage = |s: Stage| r.stages.iter().find(|x| x.stage == s).map(|x| x.avg_ms);
        assert_eq!(stage(Stage::Ui), Some(3.0));
        assert_eq!(stage(Stage::Scene), Some(1.0), "the frame that did not time it does not pull it down");
        assert_eq!(stage(Stage::Render), None, "never timed is not 0");
        assert_eq!(stage(Stage::Present), Some(3.0));
        let order: Vec<Stage> = r.stages.iter().map(|x| x.stage).collect();
        assert_eq!(order, vec![Stage::Ui, Stage::Scene, Stage::Present], "in the order the frame runs them");
    }
}
