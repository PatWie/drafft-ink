//! Per-frame statistics: frame time, phase times, redraw reasons, rolling percentiles.
//!
//! [`FrameRecorder`] is a plain value that takes explicit timestamps, so its behaviour is
//! tested without a clock. The process-wide recorder in `lib.rs` feeds it from the real clock.

use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};

use crate::clock::ns_to_ms;
use crate::names::{Phase, RedrawReason};

/// Frames kept in the rolling window by default (about 17 s at 60 fps).
pub const DEFAULT_WINDOW: usize = 1024;
/// A frame longer than this misses 60 fps (16.7 ms).
pub const BUDGET_60FPS_NS: u64 = 16_700_000;
/// A frame longer than this misses 30 fps (33.3 ms).
pub const BUDGET_30FPS_NS: u64 = 33_300_000;

/// Upper bounds, in milliseconds, of the fixed histogram buckets. A final overflow bucket
/// holds everything above the last bound.
pub const HISTOGRAM_BOUNDS_MS: [f64; 12] = [
    1.0, 2.0, 4.0, 8.0, 12.0, 16.7, 25.0, 33.3, 50.0, 100.0, 250.0, 500.0,
];

/// One finished frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameRecord {
    /// When the frame began, in nanoseconds on the process clock.
    pub start_ns: u64,
    /// Begin to end.
    pub total_ns: u64,
    /// Since the previous frame began (0 for the first).
    pub interval_ns: u64,
    /// Time in each [`Phase`].
    pub phases_ns: [u64; Phase::COUNT],
    /// Redraw reasons pending when the frame began, one bit per [`RedrawReason`].
    pub reasons: u32,
}

impl FrameRecord {
    /// Frame time outside every phase.
    pub fn other_ns(&self) -> u64 {
        self.total_ns.saturating_sub(self.phases_ns.iter().sum())
    }
}

#[derive(Clone, Copy, Debug)]
struct Open {
    start_ns: u64,
    interval_ns: u64,
    reasons: u32,
    phases_ns: [u64; Phase::COUNT],
}

/// Rolling frame statistics.
#[derive(Debug)]
pub struct FrameRecorder {
    capacity: usize,
    ring: VecDeque<FrameRecord>,
    open: Option<Open>,
    last_begin_ns: Option<u64>,
    total: u64,
    over_budget_60: u64,
    over_budget_30: u64,
    unexplained: u64,
    by_reason: [u64; RedrawReason::COUNT],
}

impl FrameRecorder {
    /// A recorder whose rolling window holds the last `capacity` frames (at least 1).
    pub const fn new(capacity: usize) -> Self {
        Self {
            capacity: if capacity == 0 { 1 } else { capacity },
            ring: VecDeque::new(),
            open: None,
            last_begin_ns: None,
            total: 0,
            over_budget_60: 0,
            over_budget_30: 0,
            unexplained: 0,
            by_reason: [0; RedrawReason::COUNT],
        }
    }

    /// Window size.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Start a frame at `now_ns` that was requested for the reasons in the `reasons` bit mask.
    /// A frame still open is ended first, at `now_ns`.
    pub fn begin(&mut self, now_ns: u64, reasons: u32) {
        if self.open.is_some() {
            self.end(now_ns);
        }
        let interval_ns = self.last_begin_ns.map_or(0, |p| now_ns.saturating_sub(p));
        self.last_begin_ns = Some(now_ns);
        self.open = Some(Open {
            start_ns: now_ns,
            interval_ns,
            reasons,
            phases_ns: [0; Phase::COUNT],
        });
    }

    /// Add `ns` to a phase of the open frame. Ignored when no frame is open.
    pub fn add_phase(&mut self, phase: Phase, ns: u64) {
        if let Some(slot) = self
            .open
            .as_mut()
            .and_then(|o| o.phases_ns.get_mut(phase.index()))
        {
            *slot = slot.saturating_add(ns);
        }
    }

    /// Is a frame open?
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Finish the open frame at `now_ns`. Returns it, or `None` when no frame was open.
    pub fn end(&mut self, now_ns: u64) -> Option<FrameRecord> {
        let o = self.open.take()?;
        let rec = FrameRecord {
            start_ns: o.start_ns,
            total_ns: now_ns.saturating_sub(o.start_ns),
            interval_ns: o.interval_ns,
            phases_ns: o.phases_ns,
            reasons: o.reasons,
        };
        self.total = self.total.saturating_add(1);
        if rec.total_ns > BUDGET_60FPS_NS {
            self.over_budget_60 = self.over_budget_60.saturating_add(1);
        }
        if rec.total_ns > BUDGET_30FPS_NS {
            self.over_budget_30 = self.over_budget_30.saturating_add(1);
        }
        if rec.reasons == 0 {
            self.unexplained = self.unexplained.saturating_add(1);
        }
        for r in RedrawReason::ALL {
            if rec.reasons & (1u32 << r.index()) != 0
                && let Some(c) = self.by_reason.get_mut(r.index())
            {
                *c = c.saturating_add(1);
            }
        }
        if self.ring.len() >= self.capacity {
            self.ring.pop_front();
        }
        self.ring.push_back(rec);
        Some(rec)
    }

    /// Frames ended since creation or the last reset.
    pub fn total_frames(&self) -> u64 {
        self.total
    }

    /// Frames in the rolling window, oldest first.
    pub fn window(&self) -> impl Iterator<Item = &FrameRecord> {
        self.ring.iter()
    }

    /// Forget everything. The window size is kept.
    pub fn reset(&mut self) {
        *self = Self::new(self.capacity);
    }

    /// The statistics for the rolling window and the totals.
    pub fn stats(&self) -> FrameStats {
        let n = self.ring.len();
        let frame: Vec<u64> = self.ring.iter().map(|f| f.total_ns).collect();
        let interval: Vec<u64> = self.ring.iter().skip(1).map(|f| f.interval_ns).collect();
        let mut phase_ms = BTreeMap::new();
        for p in Phase::ALL {
            let v: Vec<u64> = self
                .ring
                .iter()
                .map(|f| f.phases_ns.get(p.index()).copied().unwrap_or(0))
                .collect();
            phase_ms.insert(p.name().to_string(), Summary::from_ns(v));
        }
        phase_ms.insert(
            "other".to_string(),
            Summary::from_ns(self.ring.iter().map(FrameRecord::other_ns).collect()),
        );

        let mut counts = vec![0u32; HISTOGRAM_BOUNDS_MS.len() + 1];
        for ns in &frame {
            let ms = *ns as f64 / 1e6;
            let b = HISTOGRAM_BOUNDS_MS
                .iter()
                .position(|bound| ms <= *bound)
                .unwrap_or(HISTOGRAM_BOUNDS_MS.len());
            if let Some(c) = counts.get_mut(b) {
                *c = c.saturating_add(1);
            }
        }
        let fps = match (self.ring.front(), self.ring.back()) {
            (Some(first), Some(last)) if n > 1 && last.start_ns > first.start_ns => {
                (n as f64 - 1.0) / ((last.start_ns - first.start_ns) as f64 / 1e9)
            }
            _ => 0.0,
        };
        let by_reason = RedrawReason::ALL
            .iter()
            .map(|r| {
                (
                    r.name().to_string(),
                    self.by_reason.get(r.index()).copied().unwrap_or(0),
                )
            })
            .collect();
        FrameStats {
            total: self.total,
            window: n as u64,
            over_16_7ms: self.over_budget_60,
            over_33_3ms: self.over_budget_30,
            unexplained: self.unexplained,
            fps: (fps * 100.0).round() / 100.0,
            frame_ms: Summary::from_ns(frame),
            interval_ms: Summary::from_ns(interval),
            phase_ms,
            histogram: Histogram {
                upper_ms: HISTOGRAM_BOUNDS_MS.to_vec(),
                counts,
            },
            by_reason,
        }
    }
}

/// Count, mean and percentiles of a set of durations, in milliseconds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    /// Number of samples.
    pub count: u64,
    /// Arithmetic mean.
    pub mean_ms: f64,
    /// Median (nearest rank).
    pub p50_ms: f64,
    /// 95th percentile (nearest rank).
    pub p95_ms: f64,
    /// 99th percentile (nearest rank).
    pub p99_ms: f64,
    /// Largest sample.
    pub max_ms: f64,
}

impl Summary {
    /// Summarize durations in nanoseconds. An empty set gives all zeros.
    pub fn from_ns(mut v: Vec<u64>) -> Self {
        if v.is_empty() {
            return Self::default();
        }
        v.sort_unstable();
        let n = v.len();
        let rank = |p: usize| -> u64 {
            // nearest rank: ceil(p/100 * n), 1-based
            let r = (p * n).div_ceil(100).clamp(1, n);
            v.get(r - 1).copied().unwrap_or(0)
        };
        let sum: u128 = v.iter().map(|x| u128::from(*x)).sum();
        Self {
            count: n as u64,
            mean_ms: ns_to_ms((sum / n as u128) as u64),
            p50_ms: ns_to_ms(rank(50)),
            p95_ms: ns_to_ms(rank(95)),
            p99_ms: ns_to_ms(rank(99)),
            max_ms: ns_to_ms(v.last().copied().unwrap_or(0)),
        }
    }
}

/// Fixed-bucket histogram of frame times in the rolling window.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Histogram {
    /// Bucket upper bounds in ms; bucket `i` holds frames with `upper_ms[i-1] < t <= upper_ms[i]`.
    pub upper_ms: Vec<f64>,
    /// One count per bound plus a last overflow bucket for frames above the last bound.
    pub counts: Vec<u32>,
}

/// The frame section of `PerfStats`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FrameStats {
    /// Frames since the last reset.
    pub total: u64,
    /// Frames in the rolling window.
    pub window: u64,
    /// Frames since the last reset that took longer than 16.7 ms.
    pub over_16_7ms: u64,
    /// Frames since the last reset that took longer than 33.3 ms.
    pub over_33_3ms: u64,
    /// Frames since the last reset that began with no pending redraw reason. Must be 0.
    pub unexplained: u64,
    /// Frames per second over the window, from first to last frame start. Includes idle gaps.
    pub fps: f64,
    /// Frame time (begin to end) over the window.
    pub frame_ms: Summary,
    /// Time between consecutive frame starts over the window.
    pub interval_ms: Summary,
    /// Per-phase time over the window, keyed by phase name plus `other`.
    pub phase_ms: BTreeMap<String, Summary>,
    /// Frame-time histogram over the window.
    pub histogram: Histogram,
    /// Frames since the last reset that began with each redraw reason pending.
    pub by_reason: BTreeMap<String, u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    fn frame(r: &mut FrameRecorder, start_ms: u64, dur_ms: u64, reasons: u32) {
        r.begin(start_ms * MS, reasons);
        r.end((start_ms + dur_ms) * MS);
    }

    #[test]
    fn percentiles_use_nearest_rank() {
        let mut r = FrameRecorder::new(1000);
        // frame times 1..=100 ms
        for i in 1..=100u64 {
            frame(&mut r, i * 200, i, 1);
        }
        let s = r.stats();
        assert_eq!(s.window, 100);
        assert_eq!(s.frame_ms.p50_ms, 50.0);
        assert_eq!(s.frame_ms.p95_ms, 95.0);
        assert_eq!(s.frame_ms.p99_ms, 99.0);
        assert_eq!(s.frame_ms.max_ms, 100.0);
        assert_eq!(s.frame_ms.mean_ms, 50.5);
    }

    #[test]
    fn window_rolls_but_totals_keep_counting() {
        let mut r = FrameRecorder::new(10);
        for i in 0..25u64 {
            frame(&mut r, i * 20, if i < 15 { 40 } else { 5 }, 1);
        }
        let s = r.stats();
        assert_eq!(s.total, 25);
        assert_eq!(s.window, 10);
        // the slow frames fell out of the window but stay in the totals
        assert_eq!(s.frame_ms.max_ms, 5.0);
        assert_eq!(s.over_33_3ms, 15);
        assert_eq!(s.over_16_7ms, 15);
    }

    #[test]
    fn budget_thresholds_are_strict() {
        let mut r = FrameRecorder::new(10);
        r.begin(0, 1);
        r.end(BUDGET_60FPS_NS);
        r.begin(100 * MS, 1);
        r.end(100 * MS + BUDGET_60FPS_NS + 1);
        assert_eq!(r.stats().over_16_7ms, 1);
    }

    #[test]
    fn phases_accumulate_and_the_rest_is_other() {
        let mut r = FrameRecorder::new(10);
        r.begin(0, 1);
        r.add_phase(Phase::Encode, 3 * MS);
        r.add_phase(Phase::Encode, 2 * MS);
        r.add_phase(Phase::Ui, MS);
        let rec = r.end(10 * MS).unwrap();
        assert_eq!(rec.phases_ns[Phase::Encode.index()], 5 * MS);
        assert_eq!(rec.other_ns(), 4 * MS);
        let s = r.stats();
        assert_eq!(s.phase_ms["encode"].p50_ms, 5.0);
        assert_eq!(s.phase_ms["ui"].max_ms, 1.0);
        assert_eq!(s.phase_ms["input"].max_ms, 0.0);
        assert_eq!(s.phase_ms["other"].p50_ms, 4.0);
        assert_eq!(s.phase_ms.len(), Phase::COUNT + 1);
    }

    #[test]
    fn phase_time_outside_a_frame_is_ignored() {
        let mut r = FrameRecorder::new(10);
        r.add_phase(Phase::Encode, MS);
        r.begin(0, 1);
        let rec = r.end(MS).unwrap();
        assert_eq!(rec.phases_ns, [0; Phase::COUNT]);
        assert!(r.end(2 * MS).is_none());
    }

    #[test]
    fn reasons_and_unexplained_frames() {
        let mut r = FrameRecorder::new(10);
        let hover = 1u32 << RedrawReason::HoverChange.index();
        let both = hover | (1u32 << RedrawReason::Collab.index());
        frame(&mut r, 0, 1, hover);
        frame(&mut r, 20, 1, both);
        frame(&mut r, 40, 1, 0);
        let s = r.stats();
        assert_eq!(s.unexplained, 1);
        assert_eq!(s.by_reason["hover_change"], 2);
        assert_eq!(s.by_reason["collab"], 1);
        assert_eq!(s.by_reason["timer"], 0);
        assert_eq!(s.by_reason.len(), RedrawReason::COUNT);
    }

    #[test]
    fn histogram_buckets_are_fixed() {
        let mut r = FrameRecorder::new(100);
        for (i, ms) in [0u64, 1, 2, 9, 16, 17, 600].into_iter().enumerate() {
            frame(&mut r, i as u64 * 1000, ms, 1);
        }
        let h = r.stats().histogram;
        assert_eq!(h.upper_ms.len() + 1, h.counts.len());
        assert_eq!(h.counts.iter().sum::<u32>(), 7);
        // 0 and 1 ms -> <=1; 2 -> <=2; 9 -> <=12; 16 -> <=16.7; 17 -> <=25; 600 -> overflow
        assert_eq!(h.counts[0], 2);
        assert_eq!(h.counts[1], 1);
        assert_eq!(h.counts[4], 1);
        assert_eq!(h.counts[5], 1);
        assert_eq!(h.counts[6], 1);
        assert_eq!(h.counts[HISTOGRAM_BOUNDS_MS.len()], 1);
    }

    #[test]
    fn intervals_and_fps() {
        let mut r = FrameRecorder::new(100);
        for i in 0..11u64 {
            frame(&mut r, i * 20, 2, 1); // 50 fps
        }
        let s = r.stats();
        assert_eq!(s.fps, 50.0);
        assert_eq!(s.interval_ms.count, 10);
        assert_eq!(s.interval_ms.p50_ms, 20.0);
        assert_eq!(s.interval_ms.max_ms, 20.0);
    }

    #[test]
    fn a_new_begin_ends_an_open_frame() {
        let mut r = FrameRecorder::new(10);
        r.begin(0, 1);
        r.begin(5 * MS, 1);
        assert_eq!(r.total_frames(), 1);
        assert!(r.is_open());
        assert_eq!(r.window().next().unwrap().total_ns, 5 * MS);
    }

    #[test]
    fn empty_recorder_reports_zeros() {
        let s = FrameRecorder::new(0).stats();
        assert_eq!(s.total, 0);
        assert_eq!(s.frame_ms, Summary::default());
        assert_eq!(s.fps, 0.0);
        assert_eq!(FrameRecorder::new(0).capacity(), 1);
    }

    #[test]
    fn reset_clears_but_keeps_capacity() {
        let mut r = FrameRecorder::new(7);
        frame(&mut r, 0, 1, 0);
        r.reset();
        assert_eq!(r.total_frames(), 0);
        assert_eq!(r.stats().unexplained, 0);
        assert_eq!(r.capacity(), 7);
    }
}
