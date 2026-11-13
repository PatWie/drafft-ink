//! A live frame meter for the debug panel: frames per second, frame time and a short history.
//!
//! [`crate::FrameRecorder`] keeps a long window for reports and sorts it to give percentiles. This meter is the
//! small, cheap one that the debug panel reads every refresh: it keeps the frames of the last second and a
//! few older ones for the sparkline, and takes explicit timestamps, so its behaviour is tested without a clock.
//!
//! The app redraws on demand. A meter that nothing feeds is not broken, it is idle: [`FrameReading::idle`] says
//! so, and the frame rate then is 0 because no frame was drawn, not because frames are slow.

use std::collections::VecDeque;

use crate::clock::ns_to_ms;

/// The span of time a [`FrameReading`] describes: one second.
pub const METER_WINDOW_NS: u64 = 1_000_000_000;

/// Frames the sparkline shows, at most.
pub const SPARKLINE_FRAMES: usize = 90;

/// Frames kept: a second at 240 Hz, and never fewer than the sparkline needs.
const KEPT: usize = 256;

/// One frame as the meter saw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Sample {
    /// When the frame began.
    start_ns: u64,
    /// CPU time from the start of the frame to the end of the present call.
    cpu_ns: u64,
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

    /// Records a frame that began at `start_ns` and took `cpu_ns` of CPU time.
    pub fn record(&mut self, start_ns: u64, cpu_ns: u64) {
        if self.samples.len() >= KEPT {
            self.samples.pop_front();
        }
        self.samples.push_back(Sample { start_ns, cpu_ns });
    }

    /// Forgets every frame (the panel was closed: its next opening must not show old numbers).
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// What the frames of the last second say, at `now_ns`.
    pub fn reading(&self, now_ns: u64) -> FrameReading {
        let from = now_ns.saturating_sub(METER_WINDOW_NS);
        let recent = || self.samples.iter().filter(move |s| s.start_ns >= from);
        let frames = recent().count();
        let total_ns: u128 = recent().map(|s| u128::from(s.cpu_ns)).sum();
        let avg_ms =
            if frames == 0 { 0.0 } else { ns_to_ms(u64::try_from(total_ns / frames as u128).unwrap_or(u64::MAX)) };
        FrameReading {
            frames: u32::try_from(frames).unwrap_or(u32::MAX),
            avg_ms,
            worst_ms: ns_to_ms(recent().map(|s| s.cpu_ns).max().unwrap_or(0)),
            last_ms: self.samples.back().map(|s| ns_to_ms(s.cpu_ns)),
            since_last_ms: self.samples.back().map(|s| ns_to_ms(now_ns.saturating_sub(s.start_ns))),
            idle: frames == 0,
            history_ms: self
                .samples
                .iter()
                .rev()
                .take(SPARKLINE_FRAMES)
                .rev()
                .map(|s| (s.cpu_ns as f64 / 1e6) as f32)
                .collect(),
        }
    }
}

/// What the frames of the last second say.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameReading {
    /// Frames drawn in the last second: the frame rate.
    pub frames: u32,
    /// Mean CPU time of those frames, in milliseconds.
    pub avg_ms: f64,
    /// Longest CPU time among them.
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
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    #[test]
    fn a_meter_without_frames_is_idle() {
        let r = FrameMeter::new().reading(5_000 * MS);
        assert!(r.idle);
        assert_eq!(r.frames, 0);
        assert_eq!(r.last_ms, None);
        assert!(r.history_ms.is_empty());
    }

    #[test]
    fn sixty_frames_in_a_second_read_as_sixty_fps_with_their_mean_and_worst() {
        let mut m = FrameMeter::new();
        for i in 0..60u64 {
            // 16.5 ms apart (just under a second for 60), 2 ms each, one slow frame of 9 ms.
            m.record(i * 16 * MS + 500 * MS, if i == 30 { 9 * MS } else { 2 * MS });
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
        m.record(100 * MS, 3 * MS);
        m.record(200 * MS, 5 * MS);
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
        m.record(1_000 * MS, MS);
        assert_eq!(m.reading(2_000 * MS).frames, 1, "exactly a second ago is still in");
        assert_eq!(m.reading(2_000 * MS + 1).frames, 0);
    }

    #[test]
    fn the_sparkline_is_capped_and_keeps_the_newest_frames() {
        let mut m = FrameMeter::new();
        for i in 0..300u64 {
            m.record(i * MS, i * 1000);
        }
        let r = m.reading(300 * MS);
        assert_eq!(r.history_ms.len(), SPARKLINE_FRAMES);
        assert_eq!(r.history_ms.last().copied(), Some(0.299));
        assert!(r.frames as usize <= KEPT);
    }

    #[test]
    fn clear_forgets_everything() {
        let mut m = FrameMeter::new();
        m.record(MS, MS);
        m.clear();
        assert_eq!(m.reading(2 * MS), FrameMeter::new().reading(2 * MS));
    }
}
