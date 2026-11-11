//! The guides of snapping and rotating: the crosshair at a snapped point, the polar rays of an angle lock, the
//! rotation guides, and the eraser's radius ring.
//!
//! All sizes are logical pixels, so they keep their on-screen size at every zoom. Angles are in degrees, measured
//! clockwise on the screen (y points down), as the interaction layer reports them.

use std::f64::consts::FRAC_PI_2;

use kurbo::{Affine, BezPath, Circle, Point, Stroke, Vec2};
use peniko::{Color, Fill};
use vello::Scene;

use crate::overlay::Pen;
use crate::overlay_text::Label;

/// The ring of the eraser is never drawn larger than this, in logical pixels.
const MAX_ERASER_RADIUS: f64 = 1_000.0;
/// The ring of a snapped point, in logical pixels.
const SNAP_RING_RADIUS: f64 = 4.0;
/// Radius of the arc that shows an angle lock, in logical pixels.
const ANGLE_ARC_RADIUS: f64 = 30.0;
/// Gap between the arc and its label, in logical pixels.
const ANGLE_LABEL_GAP: f64 = 6.0;
/// Rays are drawn for steps from this to a half turn; anything else is taken as the default.
const MIN_STEP: f64 = 1.0;
const DEFAULT_STEP: f64 = 15.0;
/// An arc is cut into segments of at most this many degrees.
const ARC_SEGMENT_DEGREES: f64 = 5.0;
/// Half the length of the cross at the centre of the rotation guides, in logical pixels.
const ROTATION_CROSS: f64 = 10.0;
/// Length of the indicator line and the reference line of the rotation guides, in logical pixels.
const ROTATION_LINE: f64 = 50.0;
/// Radius of the arc of the rotation guides, in logical pixels.
const ROTATION_ARC_RADIUS: f64 = 25.0;

/// The polar rays of an angle lock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AngleRays {
    /// The point the locked end turns about (world).
    pub origin: Point,
    /// A faint ray is drawn at every multiple of this many degrees.
    pub step_degrees: f64,
    /// The direction the end is locked to, from the +x axis, clockwise on the screen. Drawn strongly, with an arc
    /// and a label.
    pub locked_degrees: f64,
}

/// The guides while a selection is turned: a cross at the centre, a line at the current angle, a dashed line at 0
/// degrees (straight up) and the arc between them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RotationGuide {
    /// What the selection turns about (world).
    pub center: Point,
    /// The angle, clockwise from straight up.
    pub degrees: f64,
    /// The angle is rounded to a step: the guides turn green.
    pub snapped: bool,
}

/// A ring that shows the radius of the eraser.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EraserRing {
    /// The pointer (world).
    pub at: Point,
    /// The radius in logical pixels.
    pub radius_px: f64,
}

/// The faint colour of the rays that are not locked.
fn ray_color() -> Color {
    Color::from_rgba8(100, 100, 100, 60)
}

/// The colour of the rotation guides once the angle is rounded to a step.
fn snapped_color() -> Color {
    Color::from_rgba8(34, 197, 94, 220)
}

/// `degrees` as a signed angle in `(-180, 180]`, so that an arc takes the short way round.
fn signed(degrees: f64) -> f64 {
    let d = degrees.rem_euclid(360.0);
    if d > 180.0 { d - 360.0 } else { d }
}

/// The unit vector of a screen angle (clockwise from +x, y down).
fn direction(radians: f64) -> Vec2 {
    Vec2::new(radians.cos(), radians.sin())
}

/// An arc about `center` from `from` to `to` (radians, screen angles) as a polyline.
fn arc(center: Point, radius: f64, from: f64, to: f64) -> BezPath {
    let segments = ((to - from).abs().to_degrees() / ARC_SEGMENT_DEGREES).ceil().clamp(2.0, 72.0) as u32;
    let mut path = BezPath::new();
    for i in 0..=segments {
        let a = from + (to - from) * f64::from(i) / f64::from(segments);
        let p = center + direction(a) * radius;
        if i == 0 {
            path.move_to(p);
        } else {
            path.line_to(p);
        }
    }
    path
}

/// A crosshair at the point a drag snapped to: a line across the whole view in each direction and a small ring on
/// the point. Returns 1 when something was drawn.
pub(crate) fn paint_snap_point(scene: &mut Scene, pen: &Pen, view: kurbo::Size, snap_point: Option<Point>) -> usize {
    let Some(p) = snap_point.map(|p| pen.dev(p)).filter(|p| p.x.is_finite() && p.y.is_finite()) else { return 0 };
    let mut lines = BezPath::new();
    lines.move_to(Point::new(0.0, p.y));
    lines.line_to(Point::new(view.width, p.y));
    lines.move_to(Point::new(p.x, 0.0));
    lines.line_to(Point::new(p.x, view.height));
    scene.stroke(&Stroke::new(pen.px), Affine::IDENTITY, pen.style.guide, None, &lines);
    let ring = Circle::new(p, SNAP_RING_RADIUS * pen.px);
    scene.stroke(&Stroke::new(2.0 * pen.px), Affine::IDENTITY, pen.style.guide, None, &ring);
    1
}

/// The rays of an angle lock: a faint one at every multiple of the step, the locked one strongly, the arc from the
/// horizontal to it, and the angle in a chip beside the arc. Returns 1 when something was drawn.
pub(crate) fn paint_angle_rays(scene: &mut Scene, pen: &mut Pen, view: kurbo::Size, rays: Option<AngleRays>) -> usize {
    let Some(rays) = rays.filter(|r| r.locked_degrees.is_finite()) else { return 0 };
    let origin = pen.dev(rays.origin);
    if !origin.x.is_finite() || !origin.y.is_finite() {
        return 0;
    }
    let step = if rays.step_degrees.is_finite() && (MIN_STEP..=180.0).contains(&rays.step_degrees) {
        rays.step_degrees
    } else {
        DEFAULT_STEP
    };
    // Long enough to leave the view from anywhere inside it.
    let length = view.width.hypot(view.height) + origin.x.abs().max(origin.y.abs());
    let count = (360.0 / step).round() as u32;
    let mut faint = BezPath::new();
    for k in 0..count {
        faint.move_to(origin);
        faint.line_to(origin + direction((f64::from(k) * step).to_radians()) * length);
    }
    scene.stroke(&Stroke::new(0.5 * pen.px), Affine::IDENTITY, ray_color(), None, &faint);

    let locked = signed(rays.locked_degrees).to_radians();
    let mut strong = BezPath::new();
    strong.move_to(origin);
    strong.line_to(origin + direction(locked) * length);
    let hot = pen.style.guide.with_alpha(0.8);
    scene.stroke(&Stroke::new(1.5 * pen.px), Affine::IDENTITY, hot, None, &strong);

    let radius = ANGLE_ARC_RADIUS * pen.px;
    scene.stroke(&Stroke::new(1.5 * pen.px), Affine::IDENTITY, hot, None, &arc(origin, radius, 0.0, locked));

    let text = format!("{}\u{b0}", rays.locked_degrees.rem_euclid(360.0).round() as i64 % 360);
    if let Some(label) = Label::new(pen, &text) {
        let at = origin + direction(locked / 2.0) * (radius + ANGLE_LABEL_GAP * pen.px);
        let size = label.chip_size(pen.px);
        // The chip sits outside the arc: its near corner touches the label point.
        let x = if at.x >= origin.x { at.x } else { at.x - size.width };
        let y = if at.y >= origin.y { at.y } else { at.y - size.height };
        label.draw_chip(scene, pen.px, Point::new(x, y), pen.style.guide.with_alpha(1.0));
    }
    1
}

/// The guides of a rotation. Returns 1 when something was drawn.
pub(crate) fn paint_rotation(scene: &mut Scene, pen: &Pen, guide: Option<RotationGuide>) -> usize {
    let Some(g) = guide.filter(|g| g.degrees.is_finite()) else { return 0 };
    let c = pen.dev(g.center);
    if !c.x.is_finite() || !c.y.is_finite() {
        return 0;
    }
    let color = if g.snapped { snapped_color() } else { pen.style.guide.with_alpha(0.78) };
    let px = pen.px;
    let stroke = Stroke::new(1.5 * px);

    let cross_len = ROTATION_CROSS * px;
    let mut cross = BezPath::new();
    cross.move_to(Point::new(c.x - cross_len, c.y));
    cross.line_to(Point::new(c.x + cross_len, c.y));
    cross.move_to(Point::new(c.x, c.y - cross_len));
    cross.line_to(Point::new(c.x, c.y + cross_len));
    scene.stroke(&stroke, Affine::IDENTITY, color, None, &cross);

    // Straight up is the reference; the angle turns clockwise from it.
    let up = -FRAC_PI_2;
    let now = up + signed(g.degrees).to_radians();
    let line_len = ROTATION_LINE * px;
    let mut indicator = BezPath::new();
    indicator.move_to(c);
    indicator.line_to(c + direction(now) * line_len);
    scene.stroke(&Stroke::new(2.25 * px), Affine::IDENTITY, color, None, &indicator);

    let mut reference = BezPath::new();
    reference.move_to(c);
    reference.line_to(c + direction(up) * line_len);
    let dashed = Stroke::new(1.5 * px).with_dashes(0.0, [4.0 * px, 4.0 * px]);
    scene.stroke(&dashed, Affine::IDENTITY, Color::from_rgba8(150, 150, 150, 150), None, &reference);

    if signed(g.degrees).abs() > 0.01_f64 {
        scene.stroke(&stroke, Affine::IDENTITY, color, None, &arc(c, ROTATION_ARC_RADIUS * px, up, now));
    }
    1
}

/// The ring of the eraser. Returns 1 when something was drawn.
pub(crate) fn paint_eraser(scene: &mut Scene, pen: &Pen, ring: Option<EraserRing>) -> usize {
    let Some(r) = ring.filter(|r| r.radius_px.is_finite() && r.radius_px > 0.0) else { return 0 };
    let c = pen.dev(r.at);
    if !c.x.is_finite() || !c.y.is_finite() {
        return 0;
    }
    let circle = Circle::new(c, r.radius_px.min(MAX_ERASER_RADIUS) * pen.px);
    scene.fill(Fill::NonZero, Affine::IDENTITY, Color::from_rgba8(255, 100, 100, 50), None, &circle);
    scene.stroke(&Stroke::new(2.0 * pen.px), Affine::IDENTITY, Color::from_rgba8(200, 50, 50, 200), None, &circle);
    1
}
