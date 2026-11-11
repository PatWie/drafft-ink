//! The overlay pass: selection, handles, hover, binding highlight, guides, marquee, remote cursors.
//!
//! These change on every pointer move while the document does not, so they live in a scene of their own. The
//! canvas keeps the world scene from [`crate::SceneComposer::compose`] and rebuilds only this one, then calls
//! [`combine`] (a memcpy of the encodings) to get the scene Vello renders. Everything is drawn in device space
//! with sizes in logical pixels, so handles keep their on-screen size at any zoom.
//!
//! The description is plain data. The caller works out *what* to show (which elements are selected, which handle
//! set a box offers, where the guides are); this module only decides how it looks.
//!
//! Parts with text (the names of collaborators, the distances on spacing guides) need the typesetter, so
//! [`paint_overlay`] takes one. The drawing of each family of parts is in a module of its own: `overlay_peers`
//! (cursors with names, selections, laser trails), `overlay_marks` (link icons), `overlay_snap` (the snap
//! crosshair, angle rays, rotation guides, eraser ring), `overlay_text` (labels, the text editor's caret and
//! selection).

use drafftink_geom::OrientedBox;
use drafftink_geom::guides::{SmartGuide, SmartGuideKind};
use drafftink_geom::select::{BoxHandle, HandleSet, box_handles};
use drafftink_typeset::Typesetter;
use kurbo::{Affine, BezPath, Circle, Point, Rect, Stroke};
use peniko::{Color, Fill};
use vello::Scene;

use crate::overlay_marks::{LinkMark, paint_link_marks};
use crate::overlay_peers::{LaserTrail, RemoteSelection, paint_cursors, paint_selections, paint_trails};
use crate::overlay_snap::{
    AngleRays, EraserRing, RotationGuide, paint_angle_rays, paint_eraser, paint_rotation, paint_snap_point,
};
use crate::overlay_text::{TextEdit, paint_guide_badges, paint_text_edit};
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
    /// Font size of the labels (names of collaborators, distances on guides).
    pub label_size: f64,
    /// A point of a laser trail older than this is gone; the trail fades towards it, in milliseconds. The
    /// collaboration layer keeps peers' points for 1.5 s (`drafftink_collab::TRAIL_TTL_MS`).
    pub laser_fade_ms: f64,
}

impl Default for OverlayStyle {
    fn default() -> Self {
        Self {
            accent: Color::from_rgba8(59, 130, 246, 255),
            guide: Color::from_rgba8(236, 72, 153, 180),
            handle: 16.0,
            rotate_offset: 25.0,
            dash: 4.0,
            label_size: 12.0,
            laser_fade_ms: 1500.0,
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
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteCursor {
    /// World position.
    pub at: Point,
    /// Their colour.
    pub color: Color,
    /// Their name, drawn in a label in their colour beside the pointer. `None` or empty: no label.
    pub name: Option<String>,
}

impl RemoteCursor {
    /// A cursor without a name.
    pub fn new(at: Point, color: Color) -> Self {
        Self { at, color, name: None }
    }

    /// The same cursor with a name.
    #[must_use]
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
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
    /// The lasso being drawn, in world space. The painter closes it. Empty: none.
    pub lasso: Vec<Point>,
    /// Alignment and spacing guides.
    pub guides: Vec<SmartGuide>,
    /// The point a drag snapped to: a crosshair across the view with a ring on the point.
    pub snap_point: Option<Point>,
    /// The polar rays of an angle lock.
    pub angle_rays: Option<AngleRays>,
    /// The guides of a rotation.
    pub rotation: Option<RotationGuide>,
    /// The eraser's radius ring.
    pub eraser: Option<EraserRing>,
    /// Other people's pointers, with their names.
    pub cursors: Vec<RemoteCursor>,
    /// What other people have selected, as outlines in their colours.
    pub remote_selections: Vec<RemoteSelection>,
    /// Other people's laser trails.
    pub trails: Vec<LaserTrail>,
    /// Link icons of elements that have a link and are not selected.
    pub link_icons: Vec<LinkMark>,
    /// The caret and selection of the text being edited on the canvas.
    pub text_edit: Option<TextEdit>,
}

impl Overlay {
    /// True when nothing is shown.
    pub fn is_empty(&self) -> bool {
        self.selection.is_empty()
            && self.point_handles.is_empty()
            && self.hover.is_empty()
            && self.binding.is_empty()
            && self.marquee.is_none()
            && self.lasso.is_empty()
            && self.guides.is_empty()
            && self.snap_point.is_none()
            && self.angle_rays.is_none()
            && self.rotation.is_none()
            && self.eraser.is_none()
            && self.cursors.is_empty()
            && self.remote_selections.is_empty()
            && self.trails.is_empty()
            && self.link_icons.is_empty()
            && self.text_edit.is_none()
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

/// What every overlay item needs to be drawn: the world to device map, one logical pixel in device pixels, the
/// colours and the zoom.
pub(crate) struct Pen<'a> {
    pub(crate) to_dev: Affine,
    /// One logical pixel in device pixels.
    pub(crate) px: f64,
    pub(crate) style: &'a OverlayStyle,
    pub(crate) zoom: f64,
    pub(crate) scale: f64,
    /// For the parts that have text.
    pub(crate) typesetter: &'a mut Typesetter,
}

impl Pen<'_> {
    pub(crate) fn dev(&self, p: Point) -> Point {
        self.to_dev * p
    }
}

/// Draws the overlay into `scene` (reset first). `typesetter` lays out the labels; the same one the composer uses
/// keeps the layouts cached.
pub fn paint_overlay(
    scene: &mut Scene,
    overlay: &Overlay,
    view: &View,
    style: &OverlayStyle,
    typesetter: &mut Typesetter,
) -> OverlayReport {
    scene.reset();
    let mut report = OverlayReport::default();
    if overlay.is_empty() || !view.is_valid() {
        return report;
    }
    let mut pen = Pen {
        to_dev: view.world_to_device(),
        px: view.logical_to_device().as_coeffs()[0],
        style,
        zoom: view.zoom,
        scale: view.scale(),
        typesetter,
    };
    report.items += paint_marquee(scene, &pen, overlay.marquee);
    report.items += paint_lasso(scene, &pen, &overlay.lasso);
    report.items += paint_text_edit(scene, &pen, overlay.text_edit.as_ref());
    report.items += paint_hover(scene, &pen, &overlay.hover);
    report.items += paint_binding(scene, &pen, &overlay.binding);
    report.items += paint_selections(scene, &pen, &overlay.remote_selections);
    report.items += paint_selection(scene, &pen, &overlay.selection);
    report.items += paint_point_handles(scene, &pen, &overlay.point_handles);
    report.items += paint_link_marks(scene, &pen, &overlay.link_icons);
    for g in &overlay.guides {
        paint_guide(scene, g, &pen.to_dev, style, pen.px);
        report.items += 1;
    }
    report.items += paint_guide_badges(scene, &mut pen, &overlay.guides);
    let view_px = view.device_size();
    report.items += paint_snap_point(scene, &pen, view_px, overlay.snap_point);
    report.items += paint_angle_rays(scene, &mut pen, view_px, overlay.angle_rays);
    report.items += paint_rotation(scene, &pen, overlay.rotation);
    report.items += paint_eraser(scene, &pen, overlay.eraser);
    report.items += paint_trails(scene, &pen, &overlay.trails);
    report.items += paint_cursors(scene, &mut pen, &overlay.cursors);
    report
}

fn paint_marquee(scene: &mut Scene, pen: &Pen, marquee: Option<Rect>) -> usize {
    let Some(r) = marquee else { return 0 };
    let path = polygon(&[
        pen.dev(Point::new(r.x0, r.y0)),
        pen.dev(Point::new(r.x1, r.y0)),
        pen.dev(Point::new(r.x1, r.y1)),
        pen.dev(Point::new(r.x0, r.y1)),
    ]);
    scene.fill(Fill::NonZero, Affine::IDENTITY, pen.style.accent.with_alpha(0.1), None, &path);
    scene.stroke(&Stroke::new(pen.px), Affine::IDENTITY, pen.style.accent, None, &path);
    1
}

fn paint_lasso(scene: &mut Scene, pen: &Pen, lasso: &[Point]) -> usize {
    if lasso.len() < 2 {
        return 0;
    }
    let path = polygon(&lasso.iter().copied().map(|p| pen.dev(p)).collect::<Vec<_>>());
    scene.fill(Fill::NonZero, Affine::IDENTITY, pen.style.accent.with_alpha(0.05), None, &path);
    scene.stroke(&Stroke::new(pen.px), Affine::IDENTITY, pen.style.accent, None, &path);
    1
}

fn paint_hover(scene: &mut Scene, pen: &Pen, hover: &[OrientedBox]) -> usize {
    let mut items = 0;
    for b in hover.iter().filter(|b| b.is_valid()) {
        let path = polygon(&b.corners().map(|p| pen.dev(p)));
        scene.stroke(&Stroke::new(1.5 * pen.px), Affine::IDENTITY, pen.style.accent.with_alpha(0.6), None, &path);
        items += 1;
    }
    items
}

fn paint_binding(scene: &mut Scene, pen: &Pen, binding: &[BindingHighlight]) -> usize {
    for h in binding {
        let path = pen.to_dev * &h.outline;
        scene.stroke(&Stroke::new(5.0 * pen.px), Affine::IDENTITY, pen.style.accent.with_alpha(0.35), None, &path);
        if let Some(p) = h.at {
            let c = Circle::new(pen.dev(p), 4.0 * pen.px);
            scene.fill(Fill::NonZero, Affine::IDENTITY, pen.style.accent, None, &c);
        }
    }
    binding.len()
}

fn paint_selection(scene: &mut Scene, pen: &Pen, selection: &[SelectionBox]) -> usize {
    let (px, style) = (pen.px, pen.style);
    let mut items = 0;
    for s in selection.iter().filter(|s| s.obox.is_valid()) {
        let path = polygon(&s.obox.corners().map(|p| pen.dev(p)));
        let mut stroke = Stroke::new(px);
        // Dashes are expanded into path segments, so a box far larger than the screen would cost a lot: draw it
        // solid once it would need more than MAX_DASHES.
        let perimeter = 2.0 * (s.obox.size.width + s.obox.size.height) * pen.scale;
        if s.dashed && perimeter / (2.0 * style.dash * px) <= MAX_DASHES {
            stroke = stroke.with_dashes(0.0, [style.dash * px, style.dash * px]);
        }
        scene.stroke(&stroke, Affine::IDENTITY, style.accent, None, &path);
        // The rotate offset is in logical pixels, so convert it to world units for the geometry.
        let offset = style.rotate_offset / pen.zoom.max(1e-9);
        for (kind, at) in box_handles(&s.obox, s.handles, offset) {
            let c = pen.dev(at);
            match kind {
                BoxHandle::Rotate => paint_rotate_handle(scene, c, style, px),
                BoxHandle::Corner(_) | BoxHandle::Edge(_) => {
                    let half = style.handle * px / 2.0;
                    let r = Rect::new(c.x - half, c.y - half, c.x + half, c.y + half);
                    scene.fill(Fill::NonZero, Affine::IDENTITY, Color::WHITE, None, &r);
                    scene.stroke(&Stroke::new(1.5 * px), Affine::IDENTITY, style.accent, None, &r);
                }
            }
        }
        items += 1;
    }
    items
}

fn paint_point_handles(scene: &mut Scene, pen: &Pen, handles: &[PointHandle]) -> usize {
    let (px, style) = (pen.px, pen.style);
    for h in handles {
        let c = pen.dev(h.at);
        let (radius, fill, width) = match h.kind {
            PointKind::End | PointKind::Vertex => (style.handle * px / 2.0, Color::WHITE, 2.0 * px),
            PointKind::Midpoint => (style.handle * px / 3.0, Color::from_rgba8(200, 220, 255, 200), 1.5 * px),
        };
        let circle = Circle::new(c, radius);
        scene.fill(Fill::NonZero, Affine::IDENTITY, fill, None, &circle);
        scene.stroke(&Stroke::new(width), Affine::IDENTITY, style.accent, None, &circle);
    }
    handles.len()
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
