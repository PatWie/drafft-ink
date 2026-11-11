//! The overlay pass: selection, handles, hover, binding highlight, guides, marquee, remote cursors.
//!
//! These change on every pointer move while the document does not, so they live in a scene of their own. The
//! canvas keeps the world scene from [`crate::SceneComposer::compose`] and rebuilds only this one, then calls
//! [`combine`] (a memcpy of the encodings) to get the scene Vello renders. Everything is drawn in device space
//! with sizes in logical pixels, so handles keep their on-screen size at any zoom.
//!
//! The description is plain data. The caller works out *what* to show (which elements are selected, which handle
//! set a box offers, where the guides are); this module only decides how it looks.

use drafftink_geom::OrientedBox;
use drafftink_geom::guides::{SmartGuide, SmartGuideKind};
use drafftink_geom::select::{BoxHandle, HandleSet, box_handles};
use kurbo::{Affine, BezPath, Circle, Point, Rect, Stroke, Vec2};
use peniko::{Color, Fill};
use vello::Scene;

use crate::view::View;

/// Colours and sizes of the overlay, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayStyle {
    /// Selection blue of the old app.
    pub accent: Color,
    /// Smart guide pink of the old app.
    pub guide: Color,
    /// Side of a square handle.
    pub handle: f64,
    /// Gap between the top edge and the rotate handle.
    pub rotate_offset: f64,
    /// Dash length of the selection box.
    pub dash: f64,
}

impl Default for OverlayStyle {
    fn default() -> Self {
        Self {
            accent: Color::from_rgba8(59, 130, 246, 255),
            guide: Color::from_rgba8(236, 72, 153, 180),
            handle: 16.0,
            rotate_offset: 25.0,
            dash: 4.0,
        }
    }
}

/// A dashed outline with more dashes than this is drawn solid.
const MAX_DASHES: f64 = 2_000.0;

/// A box drawn around a selection (one element, or a multi-selection).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectionBox {
    /// The box in world space.
    pub obox: OrientedBox,
    /// Which handles it shows.
    pub handles: HandleSet,
    /// Dashed outline (the old look); false draws a solid one.
    pub dashed: bool,
}

/// What a point handle of a line or arrow stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointKind {
    /// First or last point: a white circle with a blue border.
    End,
    /// A point between the ends.
    Vertex,
    /// The middle of a segment, which a drag turns into a new point: small and pale.
    Midpoint,
}

/// A handle on a point of a line or arrow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointHandle {
    /// World position.
    pub at: Point,
    /// Kind.
    pub kind: PointKind,
}

/// An element that a dragged arrow end would bind to.
#[derive(Clone, Debug)]
pub struct BindingHighlight {
    /// The outline of the target, in world space.
    pub outline: BezPath,
    /// Where the end would attach, in world space.
    pub at: Option<Point>,
}

/// Another person's pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RemoteCursor {
    /// World position.
    pub at: Point,
    /// Their colour.
    pub color: Color,
}

/// Everything the overlay shows. All fields default to nothing.
#[derive(Clone, Debug, Default)]
pub struct Overlay {
    /// Selection boxes.
    pub selection: Vec<SelectionBox>,
    /// Handles on the points of the selected line or arrow.
    pub point_handles: Vec<PointHandle>,
    /// Outlines of the hovered element (a solid thin line).
    pub hover: Vec<OrientedBox>,
    /// Binding targets to highlight.
    pub binding: Vec<BindingHighlight>,
    /// The rubber band.
    pub marquee: Option<Rect>,
    /// Alignment and spacing guides.
    pub guides: Vec<SmartGuide>,
    /// The point a drag snapped to.
    pub snap_point: Option<Point>,
    /// Other people's pointers.
    pub cursors: Vec<RemoteCursor>,
}

impl Overlay {
    /// True when nothing is shown.
    pub fn is_empty(&self) -> bool {
        self.selection.is_empty()
            && self.point_handles.is_empty()
            && self.hover.is_empty()
            && self.binding.is_empty()
            && self.marquee.is_none()
            && self.guides.is_empty()
            && self.snap_point.is_none()
            && self.cursors.is_empty()
    }
}

/// What an overlay pass drew.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OverlayReport {
    /// Items drawn (boxes, handles, guides, ...).
    pub items: usize,
}

/// Appends `overlay` on top of `world` into `out` (reset first). Both inputs are left as they are.
pub fn combine(world: &Scene, overlay: &Scene, out: &mut Scene) {
    out.reset();
    out.append(world, None);
    out.append(overlay, None);
}

fn polygon(points: &[Point]) -> BezPath {
    let mut p = BezPath::new();
    if let Some((first, rest)) = points.split_first() {
        p.move_to(*first);
        for q in rest {
            p.line_to(*q);
        }
    }
    p.close_path();
    p
}

/// Draws the overlay into `scene` (reset first).
pub fn paint_overlay(scene: &mut Scene, overlay: &Overlay, view: &View, style: &OverlayStyle) -> OverlayReport {
    scene.reset();
    let mut report = OverlayReport::default();
    if overlay.is_empty() || !view.is_valid() {
        return report;
    }
    let to_dev = view.world_to_device();
    let px = view.logical_to_device().as_coeffs()[0];
    let id = Affine::IDENTITY;
    let dev = |p: Point| to_dev * p;

    if let Some(r) = overlay.marquee {
        let path = polygon(&[
            dev(Point::new(r.x0, r.y0)),
            dev(Point::new(r.x1, r.y0)),
            dev(Point::new(r.x1, r.y1)),
            dev(Point::new(r.x0, r.y1)),
        ]);
        scene.fill(Fill::NonZero, id, style.accent.with_alpha(0.1), None, &path);
        scene.stroke(&Stroke::new(px), id, style.accent, None, &path);
        report.items += 1;
    }

    for b in &overlay.hover {
        if b.is_valid() {
            let path = polygon(&b.corners().map(dev));
            scene.stroke(&Stroke::new(1.5 * px), id, style.accent.with_alpha(0.6), None, &path);
            report.items += 1;
        }
    }

    for h in &overlay.binding {
        let path = to_dev * &h.outline;
        scene.stroke(&Stroke::new(5.0 * px), id, style.accent.with_alpha(0.35), None, &path);
        if let Some(p) = h.at {
            let c = Circle::new(dev(p), 4.0 * px);
            scene.fill(Fill::NonZero, id, style.accent, None, &c);
        }
        report.items += 1;
    }

    for s in &overlay.selection {
        if !s.obox.is_valid() {
            continue;
        }
        let path = polygon(&s.obox.corners().map(dev));
        let mut stroke = Stroke::new(px);
        // Dashes are expanded into path segments, so a box far larger than the screen would cost a lot: draw it
        // solid once it would need more than MAX_DASHES.
        let perimeter = 2.0 * (s.obox.size.width + s.obox.size.height) * view.scale();
        if s.dashed && perimeter / (2.0 * style.dash * px) <= MAX_DASHES {
            stroke = stroke.with_dashes(0.0, [style.dash * px, style.dash * px]);
        }
        scene.stroke(&stroke, id, style.accent, None, &path);
        // The rotate offset is in logical pixels, so convert it to world units for the geometry.
        let offset = style.rotate_offset / view.zoom.max(1e-9);
        for (kind, at) in box_handles(&s.obox, s.handles, offset) {
            let c = dev(at);
            match kind {
                BoxHandle::Rotate => paint_rotate_handle(scene, c, style, px),
                BoxHandle::Corner(_) | BoxHandle::Edge(_) => {
                    let half = style.handle * px / 2.0;
                    let r = Rect::new(c.x - half, c.y - half, c.x + half, c.y + half);
                    scene.fill(Fill::NonZero, id, Color::WHITE, None, &r);
                    scene.stroke(&Stroke::new(1.5 * px), id, style.accent, None, &r);
                }
            }
        }
        report.items += 1;
    }

    for h in &overlay.point_handles {
        let c = dev(h.at);
        let (radius, fill, width) = match h.kind {
            PointKind::End | PointKind::Vertex => (style.handle * px / 2.0, Color::WHITE, 2.0 * px),
            PointKind::Midpoint => (style.handle * px / 3.0, Color::from_rgba8(200, 220, 255, 200), 1.5 * px),
        };
        let circle = Circle::new(c, radius);
        scene.fill(Fill::NonZero, id, fill, None, &circle);
        scene.stroke(&Stroke::new(width), id, style.accent, None, &circle);
        report.items += 1;
    }

    for g in &overlay.guides {
        paint_guide(scene, g, &to_dev, style, px);
        report.items += 1;
    }

    if let Some(p) = overlay.snap_point {
        let c = dev(p);
        let a = 6.0 * px;
        let mut path = BezPath::new();
        path.move_to(Point::new(c.x - a, c.y));
        path.line_to(Point::new(c.x + a, c.y));
        path.move_to(Point::new(c.x, c.y - a));
        path.line_to(Point::new(c.x, c.y + a));
        scene.stroke(&Stroke::new(px), id, style.guide, None, &path);
        report.items += 1;
    }

    for cur in &overlay.cursors {
        let c = dev(cur.at);
        let mut path = BezPath::new();
        path.move_to(c);
        path.line_to(c + Vec2::new(0.0, 18.0 * px));
        path.line_to(c + Vec2::new(12.0 * px, 13.0 * px));
        path.close_path();
        scene.fill(Fill::NonZero, id, cur.color, None, &path);
        scene.stroke(&Stroke::new(1.5 * px), id, Color::WHITE, None, &path);
        report.items += 1;
    }
    report
}

fn paint_rotate_handle(scene: &mut Scene, c: Point, style: &OverlayStyle, px: f64) {
    let id = Affine::IDENTITY;
    let radius = style.handle * px / 2.0;
    let circle = Circle::new(c, radius);
    scene.fill(Fill::NonZero, id, Color::WHITE, None, &circle);
    scene.stroke(&Stroke::new(2.0 * px), id, style.accent, None, &circle);
    // A three-quarter arc inside the circle.
    let arc_r = radius * 0.5;
    let start = -std::f64::consts::FRAC_PI_4;
    let mut arc = BezPath::new();
    for i in 0..=12 {
        let a = start + f64::from(i) / 12.0 * std::f64::consts::PI * 1.5;
        let p = Point::new(c.x + arc_r * a.cos(), c.y + arc_r * a.sin());
        if i == 0 {
            arc.move_to(p);
        } else {
            arc.line_to(p);
        }
    }
    scene.stroke(&Stroke::new(1.5 * px), id, style.accent, None, &arc);
}

fn paint_guide(scene: &mut Scene, g: &SmartGuide, to_dev: &Affine, style: &OverlayStyle, px: f64) {
    let vertical = matches!(g.kind, SmartGuideKind::Vertical | SmartGuideKind::EqualSpacingV);
    let at = |along: f64| -> Point {
        let w = if vertical { Point::new(g.position, along) } else { Point::new(along, g.position) };
        *to_dev * w
    };
    let mut path = BezPath::new();
    path.move_to(at(g.start));
    path.line_to(at(g.end));
    // A small cross at each marked point.
    let tick = 4.0 * px;
    for &m in &g.snap_points {
        let p = at(m);
        path.move_to(Point::new(p.x - tick, p.y - tick));
        path.line_to(Point::new(p.x + tick, p.y + tick));
        path.move_to(Point::new(p.x - tick, p.y + tick));
        path.line_to(Point::new(p.x + tick, p.y - tick));
    }
    scene.stroke(&Stroke::new(px), Affine::IDENTITY, style.guide, None, &path);
}
