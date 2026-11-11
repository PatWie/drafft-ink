//! The transformer: handle geometry for a selection, in the spirit of Konva's `Transformer` node but data only.
//!
//! [`SelectionFrame::of`] looks at the selected elements and decides what to show:
//!
//! | Selection | Frame | Handles |
//! |---|---|---|
//! | one box-like element (rectangle, ellipse, diamond, text, image, frame, freehand) | its rotated box | 4 corners, 4 edge middles (a side too short on screen loses its edge handle), rotation |
//! | one line or arrow | its points | one handle per vertex, one per segment middle (a segment too short on screen loses it) |
//! | two or more elements, or a whole group | the axis-aligned box around all of them | as for a box |
//!
//! The frame is zoom independent and cached by the facade; handle positions and hit areas are computed per call
//! from the viewport, so a handle is always [`TransformerConfig::handle_size_px`] pixels on screen and is hit within
//! a fixed pixel radius whatever the zoom. The rotation handle sits [`TransformerConfig::rotate_offset_px`] pixels
//! beyond the top edge.
//!
//! The transformer only describes and hits handles. It never changes an element: the editor turns a handle drag
//! into a resize, rotate or point-move command. [`SelectionFrame::resize`] and [`SelectionFrame::rotate`] give the
//! result a drag would produce so a host without an editor can apply it itself.

use drafftink_geom::kurbo::Point;
use drafftink_geom::select::{BoxHandle, Corner, Edge, HandleSet, ResizeOpts, ResizeResult, handle_point};
use drafftink_geom::{OrientedBox, Outline, aabb};
use drafftink_scene::{Element, ElementId, ElementKind, Kind, SceneRead};
use serde::{Deserialize, Serialize};

use crate::intent::{Cursor, Handle};
use crate::shape::{element_box, shape_of};
use crate::viewport::Viewport;

/// Sizes and switches of the transformer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransformerConfig {
    /// Drawn size of a handle in logical pixels.
    pub handle_size_px: f64,
    /// Hit radius of a handle for a mouse or pen, in logical pixels.
    pub hit_radius_px: f64,
    /// Hit radius for a finger.
    pub hit_radius_touch_px: f64,
    /// Distance of the rotation handle from the top edge, in logical pixels.
    pub rotate_offset_px: f64,
    /// A side shorter than this on screen gets no edge handle.
    pub min_edge_side_px: f64,
    /// A segment shorter than this on screen gets no midpoint handle.
    pub min_midpoint_segment_px: f64,
    /// Offer the rotation handle.
    pub rotate: bool,
    /// Offer the four edge handles.
    pub edges: bool,
    /// Offer segment midpoints on lines and arrows.
    pub midpoints: bool,
}

impl Default for TransformerConfig {
    fn default() -> Self {
        Self {
            handle_size_px: 16.0,
            hit_radius_px: 12.0,
            hit_radius_touch_px: 24.0,
            rotate_offset_px: 25.0,
            min_edge_side_px: 40.0,
            min_midpoint_segment_px: 40.0,
            rotate: true,
            edges: true,
            midpoints: true,
        }
    }
}

/// A handle and where it is.
#[derive(Debug, Clone, PartialEq)]
pub struct HandlePos {
    /// Which handle.
    pub handle: Handle,
    /// Its position in world space.
    pub world: Point,
}

/// The handles an element kind offers for resizing, before the on-screen size filter.
pub fn handle_set_for(kind: Kind) -> HandleSet {
    match kind {
        Kind::Math => HandleSet::CORNERS_AND_ROTATE,
        _ => HandleSet::ALL,
    }
}

/// What the transformer wraps. Zoom independent.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum SelectionFrame {
    /// Nothing selected (or nothing selectable).
    #[default]
    None,
    /// A box: the rotated box of one element, or the axis-aligned box around several.
    Box {
        /// The box in world space.
        obox: OrientedBox,
        /// The handles it offers.
        set: HandleSet,
        /// More than one element is selected.
        multi: bool,
    },
    /// One line or arrow, handled by its points.
    Linear {
        /// The element.
        element: ElementId,
        /// The vertices in world space.
        points: Vec<Point>,
        /// The last point joins the first.
        closed: bool,
    },
}

impl SelectionFrame {
    /// The frame for a selection. Missing and deleted elements are ignored.
    pub fn of<'a, R: SceneRead>(scene: &R, ids: impl IntoIterator<Item = &'a ElementId>) -> SelectionFrame {
        let elements: Vec<&Element> = ids.into_iter().filter_map(|id| scene.get(id)).filter(|e| !e.deleted).collect();
        match elements.as_slice() {
            [] => SelectionFrame::None,
            [e] => Self::of_one(e),
            many => {
                let union = aabb::union_all(many.iter().filter_map(|e| shape_of(e).map(|s| s.bounds())));
                match union {
                    Some(r) => {
                        SelectionFrame::Box { obox: OrientedBox::from_rect(r), set: HandleSet::ALL, multi: true }
                    }
                    None => SelectionFrame::None,
                }
            }
        }
    }

    fn of_one(e: &Element) -> SelectionFrame {
        if let ElementKind::Line(d) | ElementKind::Arrow(d) = &e.kind {
            if d.points.len() < 2 || shape_of(e).is_none() {
                return SelectionFrame::None;
            }
            let points = d.points.iter().map(|p| e.transform.local_to_world(*p)).collect();
            return SelectionFrame::Linear { element: e.id.clone(), points, closed: d.closed && d.points.len() >= 3 };
        }
        let obox = element_box(e);
        if !obox.is_valid() {
            return SelectionFrame::None;
        }
        SelectionFrame::Box { obox, set: handle_set_for(e.tag()), multi: false }
    }

    /// The box, when the frame is a box.
    pub fn obox(&self) -> Option<OrientedBox> {
        match self {
            SelectionFrame::Box { obox, .. } => Some(*obox),
            _ => None,
        }
    }

    /// The world bounds of the frame (the box's, or the points').
    pub fn bounds(&self) -> Option<drafftink_geom::kurbo::Rect> {
        match self {
            SelectionFrame::None => None,
            SelectionFrame::Box { obox, .. } => Some(obox.aabb()),
            SelectionFrame::Linear { points, .. } => aabb::from_points(points.iter().copied()),
        }
    }

    /// True when there is nothing to show.
    pub fn is_none(&self) -> bool {
        matches!(self, SelectionFrame::None)
    }

    /// The visible handles with their world positions, in hit priority order (corners, rotation, edges; points,
    /// then midpoints).
    pub fn handles(&self, vp: &Viewport, cfg: &TransformerConfig) -> Vec<HandlePos> {
        match self {
            SelectionFrame::None => Vec::new(),
            SelectionFrame::Box { obox, set, .. } => box_handles(obox, *set, vp, cfg)
                .into_iter()
                .map(|(h, world)| HandlePos { handle: Handle::from_box(h), world })
                .collect(),
            SelectionFrame::Linear { element, points, closed } => {
                let mut out: Vec<HandlePos> = points
                    .iter()
                    .enumerate()
                    .map(|(index, p)| HandlePos {
                        handle: Handle::Point { element: element.clone(), index },
                        world: *p,
                    })
                    .collect();
                if cfg.midpoints {
                    for (segment, (a, b)) in segments(points, *closed).enumerate() {
                        if vp.world_to_px((b - a).hypot()) >= cfg.min_midpoint_segment_px {
                            out.push(HandlePos {
                                handle: Handle::Midpoint { element: element.clone(), segment },
                                world: a.midpoint(b),
                            });
                        }
                    }
                }
                out
            }
        }
    }

    /// The handle under a screen point: the nearest one within `radius_px`, corners before the rotation handle
    /// before edges, vertices before midpoints. Does not allocate for a line.
    pub fn hit(&self, vp: &Viewport, screen: Point, radius_px: f64, cfg: &TransformerConfig) -> Option<Handle> {
        if !(radius_px.is_finite() && radius_px >= 0.0 && screen.x.is_finite() && screen.y.is_finite()) {
            return None;
        }
        let r2 = radius_px * radius_px;
        let dist2 = |world: Point| (vp.world_to_screen(world) - screen).hypot2();
        match self {
            SelectionFrame::None => None,
            SelectionFrame::Box { obox, set, .. } => {
                let mut best: Option<(u8, f64, BoxHandle)> = None;
                for (h, world) in box_handles(obox, *set, vp, cfg) {
                    let d = dist2(world);
                    if d > r2 {
                        continue;
                    }
                    let class = match h {
                        BoxHandle::Corner(_) => 0,
                        BoxHandle::Rotate => 1,
                        BoxHandle::Edge(_) => 2,
                    };
                    if best.is_none_or(|(c, bd, _)| (class, d) < (c, bd)) {
                        best = Some((class, d, h));
                    }
                }
                best.map(|(_, _, h)| Handle::from_box(h))
            }
            SelectionFrame::Linear { element, points, closed } => {
                let mut best: Option<(f64, usize)> = None;
                for (i, p) in points.iter().enumerate() {
                    let d = dist2(*p);
                    if d <= r2 && best.is_none_or(|(bd, _)| d < bd) {
                        best = Some((d, i));
                    }
                }
                if let Some((_, index)) = best {
                    return Some(Handle::Point { element: element.clone(), index });
                }
                if !cfg.midpoints {
                    return None;
                }
                let mut mid: Option<(f64, usize)> = None;
                for (segment, (a, b)) in segments(points, *closed).enumerate() {
                    if vp.world_to_px((b - a).hypot()) < cfg.min_midpoint_segment_px {
                        continue;
                    }
                    let d = dist2(a.midpoint(b));
                    if d <= r2 && mid.is_none_or(|(bd, _)| d < bd) {
                        mid = Some((d, segment));
                    }
                }
                mid.map(|(_, segment)| Handle::Midpoint { element: element.clone(), segment })
            }
        }
    }

    /// The cursor for a handle of this frame: resize arrows turned with the box.
    pub fn cursor_for(&self, handle: &Handle) -> Cursor {
        let angle = self.obox().map_or(0.0, |b| b.angle);
        let dir = match handle {
            Handle::TopLeft | Handle::BottomRight => (1.0, 1.0),
            Handle::TopRight | Handle::BottomLeft => (1.0, -1.0),
            Handle::Top | Handle::Bottom => (0.0, 1.0),
            Handle::Left | Handle::Right => (1.0, 0.0),
            Handle::Rotate => return Cursor::Rotate,
            Handle::Point { .. } | Handle::Midpoint { .. } => return Cursor::Crosshair,
        };
        let (s, c) = angle.sin_cos();
        let (x, y) = (dir.0 * c - dir.1 * s, dir.0 * s + dir.1 * c);
        let sector = (y.atan2(x).to_degrees() / 45.0).round().rem_euclid(4.0) as i64;
        match sector {
            0 => Cursor::ResizeEw,
            1 => Cursor::ResizeNwse,
            2 => Cursor::ResizeNs,
            _ => Cursor::ResizeNesw,
        }
    }

    /// The box a drag of `handle` to the world point `pointer` would produce, with the signed scale. `None` for a
    /// frame without a box, for point handles and for the rotation handle.
    pub fn resize(&self, handle: &Handle, pointer: Point, opts: ResizeOpts) -> Option<ResizeResult> {
        let obox = self.obox()?;
        drafftink_geom::select::resize_box(&obox, handle.to_box()?, pointer, opts)
    }

    /// The angle (radians) a drag of the rotation handle to `pointer` would give the box, optionally snapped to a
    /// multiple of `snap_step`.
    pub fn rotate(&self, pointer: Point, snap_step: Option<f64>) -> Option<f64> {
        let obox = self.obox()?;
        drafftink_geom::select::rotation_from_pointer(obox.center, pointer, snap_step)
    }
}

fn segments(points: &[Point], closed: bool) -> impl Iterator<Item = (Point, Point)> + '_ {
    let n = points.len();
    let count = if n < 2 {
        0
    } else if closed {
        n
    } else {
        n - 1
    };
    (0..count).filter_map(move |i| Some((*points.get(i)?, *points.get((i + 1) % n)?)))
}

fn box_handles(obox: &OrientedBox, set: HandleSet, vp: &Viewport, cfg: &TransformerConfig) -> Vec<(BoxHandle, Point)> {
    let mut out = Vec::with_capacity(9);
    if !obox.is_valid() {
        return out;
    }
    let off = vp.px_to_world(cfg.rotate_offset_px);
    let at = |h: BoxHandle| handle_point(obox, h, off);
    if set.corners {
        for c in [Corner::TopLeft, Corner::TopRight, Corner::BottomRight, Corner::BottomLeft] {
            out.push((BoxHandle::Corner(c), at(BoxHandle::Corner(c))));
        }
    }
    if set.rotate && cfg.rotate {
        out.push((BoxHandle::Rotate, at(BoxHandle::Rotate)));
    }
    if set.edges && cfg.edges {
        let (w, h) = (vp.world_to_px(obox.size.width), vp.world_to_px(obox.size.height));
        if w >= cfg.min_edge_side_px {
            out.push((BoxHandle::Edge(Edge::Top), at(BoxHandle::Edge(Edge::Top))));
            out.push((BoxHandle::Edge(Edge::Bottom), at(BoxHandle::Edge(Edge::Bottom))));
        }
        if h >= cfg.min_edge_side_px {
            out.push((BoxHandle::Edge(Edge::Right), at(BoxHandle::Edge(Edge::Right))));
            out.push((BoxHandle::Edge(Edge::Left), at(BoxHandle::Edge(Edge::Left))));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::f64::consts::FRAC_PI_2;

    use drafftink_geom::kurbo::Size;
    use drafftink_scene::{LinearKind, MemoryStore, SceneStore, SceneTx, ZIndex};

    use super::*;

    fn rect(id: &str, x: f64, y: f64, w: f64, h: f64, z: &ZIndex) -> Element {
        Element::new(ElementId::new(id).unwrap(), ElementKind::Rectangle, z.clone()).with_bounds(x, y, w, h)
    }

    fn store_with(elements: Vec<Element>) -> MemoryStore {
        let mut s = MemoryStore::new();
        let mut tx = s.begin();
        for e in elements {
            tx.insert(e).unwrap();
        }
        tx.commit().unwrap();
        s
    }

    fn vp(zoom: f64) -> Viewport {
        let mut v = Viewport::new(Size::new(800.0, 600.0), 1.0);
        v.set_camera(crate::viewport::Camera { scroll: Point::new(400.0, 300.0), zoom });
        v
    }

    #[test]
    fn single_rotated_box_has_nine_handles_that_turn_with_it() {
        let z = ZIndex::first();
        let mut e = rect("a", 100.0, 100.0, 200.0, 100.0, &z);
        e.transform.angle = FRAC_PI_2;
        let s = store_with(vec![e.clone()]);
        let f = SelectionFrame::of(&s, [&e.id]);
        let hs = f.handles(&vp(1.0), &TransformerConfig::default());
        assert_eq!(hs.len(), 9);
        let rot = hs.iter().find(|h| h.handle == Handle::Rotate).unwrap();
        // Box centre (200, 150); turned a quarter clockwise the top points right: 25 px beyond the half height.
        assert!((rot.world - Point::new(275.0, 150.0)).hypot() < 1e-9, "{:?}", rot.world);
    }

    #[test]
    fn hit_areas_stay_constant_in_pixels() {
        let z = ZIndex::first();
        let e = rect("a", 0.0, 0.0, 100.0, 100.0, &z);
        let s = store_with(vec![e.clone()]);
        let f = SelectionFrame::of(&s, [&e.id]);
        let cfg = TransformerConfig::default();
        for zoom in [0.5, 1.0, 4.0] {
            let v = vp(zoom);
            let corner = v.world_to_screen(Point::new(100.0, 100.0));
            assert_eq!(
                f.hit(&v, corner + drafftink_geom::kurbo::Vec2::new(11.0, 0.0), 12.0, &cfg),
                Some(Handle::BottomRight),
                "zoom {zoom}"
            );
            assert_eq!(
                f.hit(&v, corner + drafftink_geom::kurbo::Vec2::new(13.0, 0.0), 12.0, &cfg),
                None,
                "zoom {zoom}"
            );
        }
    }

    #[test]
    fn rotation_handle_keeps_its_pixel_distance() {
        let z = ZIndex::first();
        let e = rect("a", 0.0, 0.0, 100.0, 100.0, &z);
        let s = store_with(vec![e.clone()]);
        let f = SelectionFrame::of(&s, [&e.id]);
        for zoom in [0.25, 1.0, 8.0] {
            let v = vp(zoom);
            let hs = f.handles(&v, &TransformerConfig::default());
            let rot = hs.iter().find(|h| h.handle == Handle::Rotate).unwrap();
            let top = v.world_to_screen(Point::new(50.0, 0.0));
            assert!((v.world_to_screen(rot.world) - top).hypot() - 25.0 < 1e-9);
        }
    }

    #[test]
    fn tiny_boxes_lose_edge_handles() {
        let z = ZIndex::first();
        let e = rect("a", 0.0, 0.0, 100.0, 100.0, &z);
        let s = store_with(vec![e.clone()]);
        let f = SelectionFrame::of(&s, [&e.id]);
        let cfg = TransformerConfig::default();
        assert_eq!(f.handles(&vp(1.0), &cfg).len(), 9);
        assert_eq!(f.handles(&vp(0.1), &cfg).len(), 5, "10 px sides: corners and rotation only");
    }

    #[test]
    fn multi_selection_is_the_aabb_of_rotated_members() {
        let z = ZIndex::first();
        let mut a = rect("a", 0.0, 0.0, 100.0, 20.0, &z);
        a.transform.angle = FRAC_PI_2;
        let b = rect("b", 300.0, 300.0, 10.0, 10.0, &ZIndex::between(Some(&z), None).unwrap());
        let s = store_with(vec![a.clone(), b.clone()]);
        let f = SelectionFrame::of(&s, [&a.id, &b.id]);
        let SelectionFrame::Box { obox, multi, .. } = f else { panic!("expected a box") };
        assert!(multi);
        assert_eq!(obox.angle, 0.0);
        let r = obox.aabb();
        // a, turned upright, spans x 40..60 and y -40..60.
        assert!((r.x0 - 40.0).abs() < 1e-9 && (r.y0 + 40.0).abs() < 1e-9 && (r.x1 - 310.0).abs() < 1e-9);
    }

    #[test]
    fn line_has_point_and_midpoint_handles_and_no_box() {
        let line = Element::linear(
            ElementId::new("l").unwrap(),
            LinearKind::Line,
            ZIndex::first(),
            &[Point::new(0.0, 0.0), Point::new(200.0, 0.0), Point::new(200.0, 200.0)],
        )
        .unwrap();
        let s = store_with(vec![line.clone()]);
        let f = SelectionFrame::of(&s, [&line.id]);
        let cfg = TransformerConfig::default();
        let hs = f.handles(&vp(1.0), &cfg);
        assert_eq!(hs.len(), 5, "3 vertices and 2 midpoints");
        let v = vp(1.0);
        let mid = v.world_to_screen(Point::new(100.0, 0.0));
        assert_eq!(f.hit(&v, mid, 12.0, &cfg), Some(Handle::Midpoint { element: line.id.clone(), segment: 0 }));
        let end = v.world_to_screen(Point::new(200.0, 200.0));
        assert_eq!(f.hit(&v, end, 12.0, &cfg), Some(Handle::Point { element: line.id.clone(), index: 2 }));
        assert_eq!(f.handles(&vp(0.1), &cfg).len(), 3, "20 px segments lose their midpoints");
    }

    #[test]
    fn cursor_turns_with_the_box() {
        let f = SelectionFrame::Box {
            obox: OrientedBox::from_xywh(0.0, 0.0, 10.0, 10.0, 0.0),
            set: HandleSet::ALL,
            multi: false,
        };
        assert_eq!(f.cursor_for(&Handle::Top), Cursor::ResizeNs);
        assert_eq!(f.cursor_for(&Handle::TopLeft), Cursor::ResizeNwse);
        let turned = SelectionFrame::Box {
            obox: OrientedBox::from_xywh(0.0, 0.0, 10.0, 10.0, FRAC_PI_2),
            set: HandleSet::ALL,
            multi: false,
        };
        assert_eq!(turned.cursor_for(&Handle::Top), Cursor::ResizeEw);
        assert_eq!(turned.cursor_for(&Handle::TopLeft), Cursor::ResizeNesw);
    }

    #[test]
    fn resize_wrapper_follows_the_pointer() {
        let z = ZIndex::first();
        let e = rect("a", 0.0, 0.0, 100.0, 50.0, &z);
        let s = store_with(vec![e.clone()]);
        let f = SelectionFrame::of(&s, [&e.id]);
        let r = f.resize(&Handle::BottomRight, Point::new(200.0, 100.0), ResizeOpts::default()).unwrap();
        assert_eq!(r.obox.aabb(), drafftink_geom::kurbo::Rect::new(0.0, 0.0, 200.0, 100.0));
        assert!(f.resize(&Handle::Rotate, Point::ZERO, ResizeOpts::default()).is_none());
        assert!(f.rotate(Point::new(50.0, -100.0), None).unwrap().abs() < 1e-9);
    }
}
