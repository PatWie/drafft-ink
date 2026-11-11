//! Assembly: input description in, [`SketchGeometry`] out.

use kurbo::{BezPath, Point, Shape, Vec2};

use crate::arrowhead::{self, HeadSpan};
use crate::effect::{sketch_pass, split_subpaths};
use crate::fill;
use crate::freehand::pressure_outline;
use crate::input::{Arrowhead, Curve, FillStyle, LineDash, PressureProfile, SketchInput, SketchParams, SketchShape};
use crate::limits::{MAX_COORD, MAX_HEAD_SIZE, MAX_ROUGHNESS, MAX_STROKE_WIDTH, bounded, clean_points, length};
use crate::outline;
use crate::output::{FillKind, SketchGeometry, StrokeStyle};

/// Alpha of the pale background under hatch lines (the old app).
const TINT_ALPHA: f64 = 0.15;
/// Roughness of the fill outline relative to the stroke roughness (the old app).
const FILL_ROUGHNESS_SCALE: f64 = 0.3;

/// Sanitised parameters.
struct Params {
    roughness: f64,
    bowing: f64,
    seed: u32,
    stroke_width: f64,
    line: LineDash,
    fill: FillStyle,
    fill_gap: Option<f64>,
    fill_angle: Option<f64>,
}

impl Params {
    fn new(p: &SketchParams) -> Self {
        Self {
            roughness: bounded(p.roughness, 0.0, MAX_ROUGHNESS),
            bowing: bounded(p.bowing, 1.0, MAX_ROUGHNESS),
            seed: p.seed,
            stroke_width: bounded(p.stroke_width, 0.0, MAX_STROKE_WIDTH),
            line: p.line,
            fill: p.fill,
            fill_gap: p.fill_gap.filter(|g| g.is_finite()).map(|g| g.clamp(0.0, MAX_COORD)),
            fill_angle: p.fill_angle.filter(|a| a.is_finite()),
        }
    }

    fn stroke(&self, dash: LineDash) -> StrokeStyle {
        let w = self.stroke_width;
        let dash = if w <= 0.0 {
            None
        } else {
            match dash {
                LineDash::Solid => None,
                LineDash::Dashed => Some([w * 4.0, w * 2.0]),
                LineDash::Dotted => Some([w, w * 2.0]),
            }
        };
        StrokeStyle { width: w, dash }
    }

    /// The stroke passes: two for the double-stroke look, one for clean geometry.
    fn passes(&self) -> &'static [u32] {
        if self.roughness > 0.0 { &[0, 1] } else { &[0] }
    }
}

/// Generate the sketchy geometry for `input`. Pure and deterministic: the same input always gives
/// the same output, on every platform.
///
/// Hostile input is tolerated: NaN and infinite points are dropped, sizes and coordinates are
/// clamped to [`MAX_COORD`], over-long point lists are decimated, and fills never produce more
/// than the limits in the crate docs. The result never contains a non-finite coordinate.
pub fn sketch(input: &SketchInput) -> SketchGeometry {
    let p = Params::new(&input.params);
    let mut g = match &input.shape {
        SketchShape::Rect { width, height, roundness } => {
            let (w, h) = (length(*width), length(*height));
            if w == 0.0 && h == 0.0 {
                SketchGeometry::default()
            } else {
                closed_shape(&outline::rect(w, h, roundness.radius(w, h)), &p)
            }
        }
        SketchShape::Ellipse { width, height } => {
            let (w, h) = (length(*width), length(*height));
            if w == 0.0 && h == 0.0 { SketchGeometry::default() } else { closed_shape(&outline::ellipse(w, h), &p) }
        }
        SketchShape::Diamond { width, height, roundness } => {
            let (w, h) = (length(*width), length(*height));
            if w == 0.0 && h == 0.0 {
                SketchGeometry::default()
            } else {
                closed_shape(&outline::diamond(w, h, roundness.radius(w, h)), &p)
            }
        }
        SketchShape::Polyline { points, curve, closed } => {
            let pts: Vec<Point> = clean_points(points).into_iter().map(|(_, q)| q).collect();
            polyline(&pts, *curve, *closed, &p)
        }
        SketchShape::Arrow { points, curve, start, end, head_size } => {
            let pts: Vec<Point> = clean_points(points).into_iter().map(|(_, q)| q).collect();
            arrow(&pts, *curve, *start, *end, bounded(*head_size, 0.0, MAX_HEAD_SIZE), &p)
        }
        SketchShape::Freehand { points, pressures, closed, profile } => {
            freehand(points, pressures, *closed, *profile, &p)
        }
    };
    g.drop_non_finite();
    g
}

fn polyline(pts: &[Point], curve: Curve, closed: bool, p: &Params) -> SketchGeometry {
    if pts.len() < 2 || outline::all_equal(pts) {
        return SketchGeometry::default();
    }
    let path = outline::through_points(pts, curve, closed);
    if closed { closed_shape(&path, p) } else { open_stroke(&path, p, p.line) }
}

fn freehand(points: &[Point], pressures: &[f64], closed: bool, profile: PressureProfile, p: &Params) -> SketchGeometry {
    let kept = clean_points(points);
    let pts: Vec<Point> = kept.iter().map(|(_, q)| *q).collect();
    if pts.len() < 2 || outline::all_equal(&pts) {
        return SketchGeometry::default();
    }
    if closed {
        return closed_shape(&outline::through_points(&pts, Curve::Straight, true), p);
    }
    if pressures.is_empty() {
        return open_stroke(&outline::through_points(&pts, Curve::Straight, false), p, p.line);
    }
    let pressure = |i: usize| {
        kept.get(i).and_then(|(orig, _)| pressures.get(*orig)).copied().filter(|v| v.is_finite()).unwrap_or(1.0)
    };
    let profile = PressureProfile {
        size_scale: bounded(profile.size_scale, 2.3, 1000.0),
        thinning: bounded(profile.thinning, 0.6, 1.0),
    };
    let mut g = SketchGeometry::default();
    if let Some(path) = pressure_outline(&pts, pressure, p.stroke_width, profile) {
        g.fills.push((path, FillKind::Ink));
    }
    g
}

/// Outline plus optional fill of a closed path.
fn closed_shape(path: &BezPath, p: &Params) -> SketchGeometry {
    let mut g = SketchGeometry::default();
    if p.fill != FillStyle::None {
        let region = if p.roughness > 0.0 {
            sketch_pass(path, p.roughness * FILL_ROUGHNESS_SCALE, p.bowing, p.seed, 0)
        } else {
            path.clone()
        };
        if p.fill == FillStyle::Solid {
            g.fills.push((region, FillKind::Solid));
        } else {
            let lines = fill::pattern(p.fill, path.bounding_box(), p.stroke_width, p.fill_gap, p.fill_angle, p.seed);
            g.fills.push((region.clone(), FillKind::Tint { alpha: TINT_ALPHA }));
            if !lines.elements().is_empty() {
                g.fills.push((lines, FillKind::Hatch { line_width: p.stroke_width * 0.5, clip: region }));
            }
        }
    }
    let style = p.stroke(p.line);
    for &i in p.passes() {
        g.strokes.push((sketch_pass(path, p.roughness, p.bowing, p.seed, i), style));
    }
    g
}

/// Stroke-only path (open lines).
fn open_stroke(path: &BezPath, p: &Params, dash: LineDash) -> SketchGeometry {
    let style = p.stroke(dash);
    let strokes = p.passes().iter().map(|&i| (sketch_pass(path, p.roughness, p.bowing, p.seed, i), style)).collect();
    SketchGeometry { strokes, fills: Vec::new() }
}

/// Unit direction of the segment `from -> to`, or `fallback` when the segment has no length.
fn direction(from: Point, to: Point, fallback: Vec2) -> Vec2 {
    let d = to - from;
    let len = d.hypot();
    if len > f64::EPSILON { d / len } else { fallback }
}

fn join(paths: &[BezPath]) -> BezPath {
    let mut out = BezPath::new();
    for p in paths {
        for el in p.elements() {
            out.push(*el);
        }
    }
    out
}

fn arrow(pts: &[Point], curve: Curve, start: Arrowhead, end: Arrowhead, size: f64, p: &Params) -> SketchGeometry {
    // The first two and the last two points; fewer than two points is nothing to draw.
    let (Some(&first), Some(&second), Some(&last), Some(&penultimate)) =
        (pts.first(), pts.get(1), pts.last(), pts.iter().rev().nth(1))
    else {
        return SketchGeometry::default();
    };
    if outline::all_equal(pts) {
        return SketchGeometry::default();
    }
    let mut path = outline::through_points(pts, curve, false);

    // Heads are extra subpaths after the shaft (subpath 0), end head first, as in the old app.
    let overall = direction(first, last, Vec2::new(1.0, 0.0));
    let mut spans: Vec<HeadSpan> = Vec::new();
    let mut subpaths = 1;
    let heads =
        [(end, last, direction(penultimate, last, overall)), (start, first, direction(second, first, -overall))];
    for (head, tip, dir) in heads {
        if let Some(span) = arrowhead::append(&mut path, subpaths, head, tip, dir, size) {
            subpaths += span.count;
            spans.push(span);
        }
    }

    let mut g = SketchGeometry::default();
    let shaft_style = p.stroke(p.line);
    let head_style = p.stroke(LineDash::Solid);
    for &pass in p.passes() {
        let sketched = sketch_pass(&path, p.roughness, p.bowing, p.seed, pass);
        let subs = split_subpaths(&sketched);
        if spans.is_empty() || subs.len() != subpaths {
            g.strokes.push((sketched, shaft_style));
            continue;
        }
        match (p.line, subs.split_first()) {
            (LineDash::Solid, _) | (_, None) => g.strokes.push((sketched, shaft_style)),
            (_, Some((shaft, heads))) => {
                g.strokes.push((shaft.clone(), shaft_style));
                g.strokes.push((join(heads), head_style));
            }
        }
        if pass == 0 {
            for s in spans.iter().filter(|s| s.filled) {
                if let Some(head) = subs.get(s.first..s.first + s.count) {
                    g.fills.push((join(head), FillKind::Ink));
                }
            }
        }
    }
    g
}
