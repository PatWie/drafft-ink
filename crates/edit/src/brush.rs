//! How the samples of a freehand stroke are captured: the calligraphy brush and the pressure of the
//! stroke (`style.set` `freehand_style` and `pressure_mode`).
//!
//! The old app did both while the pointer moved and stored the result, so the brush and the speed
//! model never reached the renderer. This module ports them as pure state machines. The caller passes
//! the time of every sample in, so a recorded stroke replays to the same points.
//!
//! * **Calligraphy**: the pen is a mass on a spring (mass 0.5, stiffness 120, damping 15.49, which is
//!   critical damping) that is pulled towards the pointer. The stroke follows the mass, not the
//!   pointer, so corners are rounded and the line trails behind fast moves. A position is kept only
//!   once it is more than [`MIN_EMIT_DISTANCE`] away from the last kept one. The old app advanced the
//!   spring by 1/60 s for every pointer event, so the feel depended on the event rate. Here the
//!   spring advances by the real time between samples, in steps of at most 1/60 s. At exactly 60
//!   samples a second the result is the old one.
//! * **Simulated pressure**: pressure is `exp(-v / 800)` for a speed `v` in world units per second
//!   (at most three times 800, never below 0.2), blended into the previous value with weight 0.3.
//!   Slow is thick and fast is thin. It is used only when the device reports no pressure.
//!
//! `crates/edit/tests/brush_old_app.rs` compares both with the old app's output.

use drafftink_geom::kurbo::{Point, Vec2};
use drafftink_scene::{Brush, PressureMode};

/// Mass of the calligraphy pen. Old: `M`.
const MASS: f64 = 0.5;
/// Spring stiffness. Old: `K`.
const STIFFNESS: f64 = 120.0;
/// Damping, `2 * sqrt(MASS * STIFFNESS)`. Old: `C`.
const DAMPING: f64 = 15.49;
/// The longest time step of the spring in seconds. The old app always used this one.
const FRAME: f64 = 1.0 / 60.0;
/// A pause longer than this many frames counts as this many: the pen has settled by then.
const MAX_FRAMES: f64 = 60.0;
/// The calligraphy pen adds a point only when it moved this far (world units) from the last point.
pub const MIN_EMIT_DISTANCE: f64 = 2.0;

/// Speed in world units per second that maps to `exp(-1)`.
const SPEED_SCALE: f64 = 800.0;
/// Speeds above this many `SPEED_SCALE` count as this fast.
const MAX_SPEED_RATIO: f64 = 3.0;
/// The thinnest simulated pressure.
const MIN_PRESSURE: f64 = 0.2;
/// Weight of the newest pressure against the running value.
const BLEND: f64 = 0.3;
/// Time steps up to this many seconds are too short for a speed; the pressure then stays.
const MIN_INTERVAL: f64 = 0.001;

/// The calligraphy pen: a mass on a spring.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Spring {
    pos: Point,
    vel: Vec2,
}

impl Spring {
    fn at_rest(pos: Point) -> Self {
        Self { pos, vel: Vec2::ZERO }
    }

    /// Advances towards `target` over `dt` seconds in steps of at most one frame.
    ///
    /// `dt` that is not a positive finite number counts as one frame, as in the old app.
    fn advance(&mut self, target: Point, dt: f64) {
        let (steps, h) = if dt.is_finite() && dt > 0.0 {
            // The bounds make the cast exact.
            let frames = (dt / FRAME - 1e-6).ceil().clamp(1.0, MAX_FRAMES);
            (frames as u32, (dt / frames).min(FRAME))
        } else {
            (1, FRAME)
        };
        for _ in 0..steps {
            self.step(target, h);
        }
        if !(self.pos.x.is_finite() && self.pos.y.is_finite()) {
            // Only absurd coordinates get here. Start again from the target.
            *self = Self::at_rest(target);
        }
    }

    /// One semi-implicit Euler step: the velocity first, then the position with the new velocity.
    fn step(&mut self, target: Point, h: f64) {
        let d = target - self.pos;
        let accel = (d * STIFFNESS - self.vel * DAMPING) / MASS;
        self.vel += accel * h;
        self.pos += self.vel * h;
    }
}

/// Pressure from the speed of the pointer.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SpeedPressure {
    last_time_ms: f64,
    last_pos: Point,
    value: f64,
}

impl SpeedPressure {
    fn start(pos: Point, time_ms: f64) -> Self {
        Self { last_time_ms: time_ms, last_pos: pos, value: 1.0 }
    }

    /// The pressure at `pos` reached at `time_ms`.
    fn sample(&mut self, pos: Point, time_ms: f64) -> f64 {
        let dt = (time_ms - self.last_time_ms) / 1000.0;
        let raw = if dt > MIN_INTERVAL {
            let speed = self.last_pos.distance(pos) / dt;
            (-(speed / SPEED_SCALE).min(MAX_SPEED_RATIO)).exp().clamp(MIN_PRESSURE, 1.0)
        } else {
            self.value
        };
        self.value = self.value * (1.0 - BLEND) + raw * BLEND;
        self.last_time_ms = time_ms;
        self.last_pos = pos;
        self.value
    }
}

/// One point of a stroke with its pressure, if the stroke has pressures.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrokeSample {
    /// Where the point is.
    pub point: Point,
    /// Its pressure, 0 to 1. Either every point of a stroke has one or none has.
    pub pressure: Option<f64>,
}

/// Turns the pointer samples of one stroke into the points and pressures to store.
#[derive(Debug, Clone, PartialEq)]
pub struct StrokeSampler {
    spring: Option<Spring>,
    speed: Option<SpeedPressure>,
    last_time_ms: f64,
    last_kept: Point,
    /// A device that reports pressure was seen: strokes then never mix in simulated values.
    device: bool,
}

impl StrokeSampler {
    /// Starts a stroke at `at` at `time_ms` with the brush and pressure mode of the session.
    ///
    /// `device` is the pressure the pen reports for this first sample, `None` for a mouse or a finger.
    /// Returns the sampler and the first point of the stroke.
    pub fn start(
        brush: Brush,
        mode: PressureMode,
        at: Point,
        time_ms: f64,
        device: Option<f64>,
    ) -> (Self, StrokeSample) {
        let simulate = mode == PressureMode::Simulated && device.is_none();
        let sampler = Self {
            spring: (brush == Brush::Calligraphy).then(|| Spring::at_rest(at)),
            speed: (mode == PressureMode::Simulated).then(|| SpeedPressure::start(at, time_ms)),
            last_time_ms: time_ms,
            last_kept: at,
            device: device.is_some(),
        };
        let pressure = device.or(simulate.then_some(1.0));
        (sampler, StrokeSample { point: at, pressure })
    }

    /// Feeds the pointer at `at` at `time_ms`. Returns the point to add to the stroke, if any.
    ///
    /// The plain brush keeps every sample. The calligraphy brush keeps one when the pen moved more than
    /// [`MIN_EMIT_DISTANCE`]. A sample that is not finite is ignored. A time that stands still or goes
    /// back counts as one frame for the spring and leaves the simulated pressure as it is.
    pub fn push(&mut self, at: Point, time_ms: f64, device: Option<f64>) -> Option<StrokeSample> {
        if !(at.x.is_finite() && at.y.is_finite()) {
            return None;
        }
        let dt = (time_ms - self.last_time_ms) / 1000.0;
        self.last_time_ms = time_ms;
        let point = match &mut self.spring {
            Some(spring) => {
                spring.advance(at, dt);
                if spring.pos.distance(self.last_kept) <= MIN_EMIT_DISTANCE {
                    return None;
                }
                spring.pos
            }
            None => at,
        };
        self.last_kept = point;
        self.device |= device.is_some();
        // The speed state follows the kept points, as in the old app, but only while the device has none.
        let simulated = self.speed.as_mut().map(|s| s.sample(point, time_ms));
        let pressure = device.or(if self.device { None } else { simulated });
        Some(StrokeSample { point, pressure })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    #[test]
    fn the_spring_settles_on_a_fixed_target_without_overshoot() {
        let mut s = Spring::at_rest(p(0.0, 0.0));
        let mut worst = 0.0_f64;
        for _ in 0..120 {
            s.advance(p(100.0, 0.0), FRAME);
            worst = worst.max(s.pos.x);
        }
        assert!(worst <= 100.0 + 1e-9, "critical damping does not overshoot: {worst}");
        assert!((s.pos.x - 100.0).abs() < 0.01, "{:?}", s.pos);
    }

    #[test]
    fn the_spring_does_not_depend_on_the_event_rate() {
        // One second towards a target, as 60 samples or as 240 samples.
        let run = |hz: usize| {
            let mut s = Spring::at_rest(p(0.0, 0.0));
            for _ in 0..hz {
                s.advance(p(100.0, 50.0), 1.0 / hz as f64);
            }
            s.pos
        };
        let (a, b) = (run(60), run(240));
        assert!(a.distance(b) < 0.05, "{a:?} {b:?}");
    }

    #[test]
    fn a_missing_or_backwards_time_counts_as_one_frame() {
        let mut a = Spring::at_rest(p(0.0, 0.0));
        let mut b = a;
        a.advance(p(10.0, 0.0), FRAME);
        b.advance(p(10.0, 0.0), 0.0);
        assert_eq!(a, b);
        b = Spring::at_rest(p(0.0, 0.0));
        b.advance(p(10.0, 0.0), f64::NAN);
        assert_eq!(a, b);
        b = Spring::at_rest(p(0.0, 0.0));
        b.advance(p(10.0, 0.0), -5.0);
        assert_eq!(a, b);
    }

    #[test]
    fn a_long_pause_is_capped() {
        let mut s = Spring::at_rest(p(0.0, 0.0));
        s.advance(p(10.0, 0.0), 1e12);
        assert!((s.pos.x - 10.0).abs() < 1e-3, "{:?}", s.pos);
        let mut s = Spring::at_rest(p(0.0, 0.0));
        s.advance(p(10.0, 0.0), f64::INFINITY);
        assert!(s.pos.x.is_finite());
    }

    #[test]
    fn absurd_coordinates_do_not_poison_the_spring() {
        let mut s = Spring::at_rest(p(0.0, 0.0));
        s.advance(p(1e308, -1e308), FRAME);
        assert!(s.pos.x.is_finite() && s.pos.y.is_finite(), "{:?}", s.pos);
        s.advance(p(1.0, 1.0), FRAME);
        assert!(s.pos.x.is_finite());
    }

    #[test]
    fn slow_is_thick_and_fast_is_thin() {
        let run = |step: f64| {
            let mut s = SpeedPressure::start(p(0.0, 0.0), 0.0);
            let mut v = 1.0;
            for i in 1..40 {
                v = s.sample(p(step * f64::from(i), 0.0), f64::from(i) * 16.0);
            }
            v
        };
        let (slow, fast) = (run(0.5), run(40.0));
        assert!(slow > 0.95, "{slow}");
        assert!((MIN_PRESSURE..0.3).contains(&fast), "{fast}");
    }

    #[test]
    fn the_pressure_stays_between_the_bounds_for_hostile_input() {
        let mut s = SpeedPressure::start(p(0.0, 0.0), 0.0);
        for (x, t) in [(1e300, 1.0), (-1e300, 2.0), (0.0, f64::NAN), (5.0, f64::INFINITY), (7.0, -3.0), (9.0, 4.0)] {
            let v = s.sample(p(x, 0.0), t);
            assert!((MIN_PRESSURE..=1.0).contains(&v), "{v}");
        }
    }

    #[test]
    fn a_plain_mouse_stroke_keeps_every_point_and_has_no_pressure() {
        let (mut s, first) = StrokeSampler::start(Brush::Normal, PressureMode::Uniform, p(0.0, 0.0), 0.0, None);
        assert_eq!(first.pressure, None);
        let next = s.push(p(0.1, 0.0), 16.0, None);
        assert_eq!(next, Some(StrokeSample { point: p(0.1, 0.0), pressure: None }));
    }

    #[test]
    fn a_pen_keeps_its_own_pressure_even_when_the_mode_is_simulated() {
        let (mut s, first) = StrokeSampler::start(Brush::Normal, PressureMode::Simulated, p(0.0, 0.0), 0.0, Some(0.7));
        assert_eq!(first.pressure, Some(0.7));
        assert_eq!(s.push(p(50.0, 0.0), 16.0, Some(0.4)).and_then(|x| x.pressure), Some(0.4));
    }

    #[test]
    fn a_mouse_in_simulated_mode_gets_a_pressure_on_every_point() {
        let (mut s, first) = StrokeSampler::start(Brush::Normal, PressureMode::Simulated, p(0.0, 0.0), 0.0, None);
        assert_eq!(first.pressure, Some(1.0));
        let v = s.push(p(60.0, 0.0), 16.0, None).and_then(|x| x.pressure);
        assert!(v.is_some_and(|v| v < 1.0), "{v:?}");
    }

    #[test]
    fn the_calligraphy_pen_skips_small_moves_and_trails_behind_the_pointer() {
        let (mut s, _) = StrokeSampler::start(Brush::Calligraphy, PressureMode::Uniform, p(0.0, 0.0), 0.0, None);
        assert_eq!(s.push(p(1.0, 0.0), 16.7, None), None, "the pen moved less than the gap");
        let kept = (2..30).filter_map(|i| s.push(p(f64::from(i) * 10.0, 0.0), f64::from(i) * 16.7, None)).last();
        let kept = kept.map(|k| k.point.x).unwrap_or_default();
        assert!(kept < 290.0 && kept > 100.0, "the pen trails the pointer: {kept}");
    }

    #[test]
    fn a_sample_that_is_not_finite_is_ignored() {
        let (mut s, _) = StrokeSampler::start(Brush::Calligraphy, PressureMode::Simulated, p(0.0, 0.0), 0.0, None);
        assert_eq!(s.push(p(f64::NAN, 0.0), 16.0, None), None);
        assert_eq!(s.push(p(0.0, f64::INFINITY), 32.0, None), None);
    }

    #[test]
    fn the_same_samples_give_the_same_stroke() {
        let run = || {
            let (mut s, first) =
                StrokeSampler::start(Brush::Calligraphy, PressureMode::Simulated, p(5.0, 5.0), 100.0, None);
            let mut out = vec![first];
            for i in 1..200 {
                let t = 100.0 + f64::from(i) * 7.3;
                out.extend(s.push(p(5.0 + f64::from(i) * 3.1, 5.0 + (f64::from(i) * 0.2).sin() * 30.0), t, None));
            }
            out
        };
        assert_eq!(run(), run());
    }
}
