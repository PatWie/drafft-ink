//! Freehand and highlighter strokes (E7).

use drafftink_canvas::PointerKind;
use drafftink_edit::brush::{StrokeSample, StrokeSampler};
use drafftink_geom::kurbo::Point;
use serde_json::json;

use super::preview;
use crate::interact::ctx::Ctx;
use crate::interact::events::Ev;
use crate::state::Tool;

/// Samples closer than this to the last one (in screen pixels) are skipped.
const MIN_SPACING_PX: f64 = 0.5;
/// A stroke never holds more samples than this; once full, the newest sample replaces the last one.
const MAX_SAMPLES: usize = 10_000;
/// The tolerance of the simplification at the end (`docs/exceptions.md` E7).
const SIMPLIFY: f64 = 2.0;

/// The samples of a stroke being drawn.
#[derive(Debug)]
pub(super) struct Stroke {
    points: Vec<Point>,
    /// One pressure per point: the pen's, or simulated from the speed. Empty for a mouse or a finger
    /// with the uniform mode.
    pressures: Vec<f64>,
    highlighter: bool,
    /// Brush and pressure model of this stroke. It starts with the first sample, which brings the time.
    sampler: Option<StrokeSampler>,
}

/// The pressure of a sample: only a pen reports one.
fn pressure_of(ev: &Ev<'_>) -> Option<f64> {
    (ev.meta.kind == PointerKind::Pen).then(|| f64::from(ev.meta.pressure).clamp(0.0, 1.0))
}

impl Stroke {
    pub(super) fn start(tool: Tool) -> Self {
        Self { points: Vec::new(), pressures: Vec::new(), highlighter: tool == Tool::Highlighter, sampler: None }
    }

    /// Adds the sample at the pointer (and the press position for the first one).
    ///
    /// The brush and the pressure mode of the session shape the sample here, while it is drawn, as in the old
    /// app: the stored stroke holds the result (`edit::brush`).
    pub(super) fn update(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>) {
        let mut changed = self.begin(cx, ev);
        changed |= self.add(cx.px(MIN_SPACING_PX), ev);
        if changed {
            preview::set(cx, preview::stroke(cx, &self.points, &self.pressures, self.highlighter));
        }
    }

    /// The first point of the stroke: the press position, with what the pen did at the press. The drag start carries
    /// the press, and the drag update that follows it carries the first move: the move's position and pressure must
    /// not be taken from the press (a pen touches down with almost no pressure). Does nothing once the stroke has
    /// begun. True when it added the point.
    pub(super) fn begin(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>) -> bool {
        if self.sampler.is_some() {
            return false;
        }
        let defaults = cx.s.tool.defaults();
        let (sampler, first) = StrokeSampler::start(
            defaults.brush,
            defaults.pressure_mode,
            ev.origin.world,
            ev.meta.time_ms,
            pressure_of(ev),
        );
        self.sampler = Some(sampler);
        self.push(first.point, first.pressure);
        true
    }

    /// Runs the pointer sample through the brush and keeps what comes out. True when the stroke changed.
    fn add(&mut self, min_gap: f64, ev: &Ev<'_>) -> bool {
        let sample = self.sampler.as_mut().and_then(|s| s.push(ev.at.world, ev.meta.time_ms, pressure_of(ev)));
        sample.is_some_and(|StrokeSample { point, pressure }| self.push_spaced(min_gap, point, pressure))
    }

    fn push(&mut self, p: Point, pressure: Option<f64>) {
        self.points.push(p);
        if let Some(v) = pressure {
            self.pressures.push(v);
        }
    }

    /// Adds a sample when it is more than `min_gap` (world units) from the last one. Returns true when the
    /// stroke changed.
    fn push_spaced(&mut self, min_gap: f64, p: Point, pressure: Option<f64>) -> bool {
        let Some(last) = self.points.last().copied() else {
            self.push(p, pressure);
            return true;
        };
        if last.distance(p) < min_gap {
            return false;
        }
        if self.points.len() >= MAX_SAMPLES {
            // Full: the end of the stroke keeps following the pointer.
            if let Some(slot) = self.points.last_mut() {
                *slot = p;
            }
            if let (Some(slot), Some(v)) = (self.pressures.last_mut(), pressure) {
                *slot = v;
            }
            return true;
        }
        self.push(p, pressure);
        true
    }

    /// The pointer was released: one `element.create`.
    pub(super) fn finish(mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>) {
        self.add(cx.px(MIN_SPACING_PX), ev);
        create(cx, &self.points, &self.pressures, self.highlighter);
    }
}

/// A click without a drag makes a dot.
pub(super) fn dot(cx: &mut Ctx<'_>, ev: &Ev<'_>) {
    let pressures: Vec<f64> = pressure_of(ev).into_iter().collect();
    create(cx, &[ev.at.world], &pressures, cx.tool() == Tool::Highlighter);
}

fn create(cx: &mut Ctx<'_>, points: &[Point], pressures: &[f64], highlighter: bool) {
    if points.is_empty() {
        return;
    }
    let pts: Vec<[f64; 2]> = points.iter().map(|p| [p.x, p.y]).collect();
    let mut args = json!({
        "kind": "freehand",
        "points": pts,
        "highlighter": highlighter,
        "simplify": SIMPLIFY,
        "from_tool": true,
    });
    if !pressures.is_empty() {
        args["pressures"] = json!(pressures);
    }
    cx.exec("element.create", args);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_closer_than_the_gap_are_skipped() {
        let mut s = Stroke::start(Tool::Freehand);
        assert!(s.push_spaced(1.0, Point::new(0.0, 0.0), None));
        assert!(!s.push_spaced(1.0, Point::new(0.4, 0.0), None));
        assert!(s.push_spaced(1.0, Point::new(2.0, 0.0), None));
        assert_eq!(s.points.len(), 2);
    }

    #[test]
    fn a_runaway_pen_cannot_grow_the_buffer_past_the_cap() {
        let mut s = Stroke::start(Tool::Freehand);
        for i in 0..(MAX_SAMPLES * 3) {
            s.push_spaced(0.1, Point::new(i as f64, 0.0), Some(0.5));
        }
        assert_eq!(s.points.len(), MAX_SAMPLES);
        assert_eq!(s.pressures.len(), MAX_SAMPLES);
        // The end of the stroke follows the newest sample.
        assert_eq!(s.points.last().copied(), Some(Point::new((MAX_SAMPLES * 3 - 1) as f64, 0.0)));
        assert_eq!(s.points.first().copied(), Some(Point::new(0.0, 0.0)));
    }
}
