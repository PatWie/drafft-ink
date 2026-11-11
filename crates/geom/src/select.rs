//! Selection math: handle positions, resizing and rotating a box, and marquee tests.
//!
//! Ported from the old app's `selection.rs` and fixed where it was wrong:
//! - resizing works in the box's own rotated frame, so the handle follows the pointer and the
//!   opposite corner stays put (the old code applied the world delta to the unrotated box);
//! - keeping the aspect ratio keeps the anchor fixed for every corner (the old code let three of
//!   the four corners drift);
//! - edge handles and resizing from the centre exist;
//! - dragging past the anchor reports a flip instead of clamping.
//!
//! The pure result of a resize is a new [`OrientedBox`] plus the signed scale, so a multi-selection
//! can apply the same [`ResizeResult::affine`] to every member.

use std::f64::consts::FRAC_PI_2;

use kurbo::{Affine, Point, Rect, Vec2};

use crate::aabb;
use crate::obox::OrientedBox;
use crate::outline::{Fill, Outline};
use crate::snap::snap_rotation;
use crate::transform::normalize_angle;

/// Visual size of a handle, in screen pixels.
pub const HANDLE_SIZE: f64 = 16.0;

/// Pick radius of a handle, in screen pixels. Divide by the zoom before using it as a world radius.
pub const HANDLE_HIT_TOLERANCE: f64 = 24.0;

/// Distance of the rotate handle from the top edge. The old app used 25 world units, which
/// shrinks on screen when zooming out; callers should pass `25.0 / zoom` for a constant size.
pub const ROTATE_HANDLE_OFFSET: f64 = 25.0;

/// Smallest side a resize produces, as in the old app.
pub const MIN_RESIZE_SIZE: f64 = 1.0;

/// A corner of a box, clockwise from the top left.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Corner {
    /// Top left.
    TopLeft,
    /// Top right.
    TopRight,
    /// Bottom right.
    BottomRight,
    /// Bottom left.
    BottomLeft,
}

/// A side of a box, clockwise from the top.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Edge {
    /// Top side.
    Top,
    /// Right side.
    Right,
    /// Bottom side.
    Bottom,
    /// Left side.
    Left,
}

/// A handle of a selection box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BoxHandle {
    /// A resize handle on a corner.
    Corner(Corner),
    /// A resize handle in the middle of a side.
    Edge(Edge),
    /// The rotate handle above the top edge.
    Rotate,
}

/// Which handles a box offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandleSet {
    /// Offer the four corner handles.
    pub corners: bool,
    /// Offer the four edge handles.
    pub edges: bool,
    /// Offer the rotate handle.
    pub rotate: bool,
}

impl HandleSet {
    /// Every handle.
    pub const ALL: Self = Self { corners: true, edges: true, rotate: true };
    /// What the old app offered for rectangles, ellipses and images.
    pub const CORNERS_AND_ROTATE: Self = Self { corners: true, edges: false, rotate: true };
    /// Only the rotate handle (lines and other shapes that do not resize by box).
    pub const ROTATE_ONLY: Self = Self { corners: false, edges: false, rotate: true };
}

const CORNERS: [Corner; 4] = [Corner::TopLeft, Corner::TopRight, Corner::BottomRight, Corner::BottomLeft];
const EDGES: [Edge; 4] = [Edge::Top, Edge::Right, Edge::Bottom, Edge::Left];

fn corner_sign(c: Corner) -> (f64, f64) {
    match c {
        Corner::TopLeft => (-1.0, -1.0),
        Corner::TopRight => (1.0, -1.0),
        Corner::BottomRight => (1.0, 1.0),
        Corner::BottomLeft => (-1.0, 1.0),
    }
}

/// World position of a handle. `rotate_offset` is the gap between the top edge and the rotate handle.
pub fn handle_point(b: &OrientedBox, handle: BoxHandle, rotate_offset: f64) -> Point {
    let h = b.half();
    let local = match handle {
        BoxHandle::Corner(c) => {
            let (sx, sy) = corner_sign(c);
            Point::new(sx * h.x, sy * h.y)
        }
        BoxHandle::Edge(Edge::Top) => Point::new(0.0, -h.y),
        BoxHandle::Edge(Edge::Right) => Point::new(h.x, 0.0),
        BoxHandle::Edge(Edge::Bottom) => Point::new(0.0, h.y),
        BoxHandle::Edge(Edge::Left) => Point::new(-h.x, 0.0),
        BoxHandle::Rotate => Point::new(0.0, -h.y - rotate_offset),
    };
    b.to_world(local)
}

/// All handles of the set with their world positions: corners, then the rotate handle, then edges.
/// The order is the hit-test priority.
pub fn box_handles(b: &OrientedBox, set: HandleSet, rotate_offset: f64) -> Vec<(BoxHandle, Point)> {
    let mut out = Vec::new();
    if !b.is_valid() {
        return out;
    }
    if set.corners {
        out.extend(
            CORNERS.iter().map(|c| (BoxHandle::Corner(*c), handle_point(b, BoxHandle::Corner(*c), rotate_offset))),
        );
    }
    if set.rotate {
        out.push((BoxHandle::Rotate, handle_point(b, BoxHandle::Rotate, rotate_offset)));
    }
    if set.edges {
        out.extend(EDGES.iter().map(|e| (BoxHandle::Edge(*e), handle_point(b, BoxHandle::Edge(*e), rotate_offset))));
    }
    out
}

/// The handle under `p` (circle of radius `tolerance`), first in priority order.
pub fn hit_test_handles(
    b: &OrientedBox,
    p: Point,
    tolerance: f64,
    set: HandleSet,
    rotate_offset: f64,
) -> Option<BoxHandle> {
    if !crate::point_is_finite(p) || !tolerance.is_finite() || tolerance < 0.0 {
        return None;
    }
    box_handles(b, set, rotate_offset)
        .into_iter()
        .find(|(_, q)| (*q - p).hypot2() <= tolerance * tolerance)
        .map(|(h, _)| h)
}

/// Is `p` on the border ring of the box: inside the box grown by `tolerance` and outside the box
/// shrunk by `min(tolerance, half width, half height)`? Used to grab a text box by its border.
pub fn hit_test_boundary(b: &OrientedBox, p: Point, tolerance: f64) -> bool {
    if !b.is_valid() || !crate::point_is_finite(p) || !tolerance.is_finite() || tolerance < 0.0 {
        return false;
    }
    let l = b.to_local(p);
    let h = b.half();
    let inset = tolerance.min(h.x).min(h.y);
    let in_outer = l.x.abs() <= h.x + tolerance && l.y.abs() <= h.y + tolerance;
    let in_inner = l.x.abs() <= h.x - inset && l.y.abs() <= h.y - inset;
    in_outer && !in_inner
}

/// Options for [`resize_box`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResizeOpts {
    /// Keep the width to height ratio of the original box (Shift).
    pub keep_aspect: bool,
    /// Resize about the centre instead of the opposite handle (Alt).
    pub from_center: bool,
    /// Smallest result side.
    pub min_size: f64,
}

impl Default for ResizeOpts {
    fn default() -> Self {
        Self { keep_aspect: false, from_center: false, min_size: MIN_RESIZE_SIZE }
    }
}

/// The outcome of a resize.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResizeResult {
    /// The resized box. Same angle as the original; the size is never negative.
    pub obox: OrientedBox,
    /// Signed scale along the original box axes: negative when the pointer went past the anchor
    /// and the content is mirrored.
    pub scale: Vec2,
    /// The new centre in the original box's local frame (needed for [`ResizeResult::affine`]).
    local_center: Point,
}

impl ResizeResult {
    /// True when the content is mirrored horizontally.
    pub fn flip_x(&self) -> bool {
        self.scale.x < 0.0
    }

    /// True when the content is mirrored vertically.
    pub fn flip_y(&self) -> bool {
        self.scale.y < 0.0
    }

    /// The world affine that carries the original box (and anything attached to it) onto the new
    /// box, mirroring when `scale` is negative. Apply it to every member of a multi-selection.
    pub fn affine(&self, original: &OrientedBox) -> Affine {
        original.to_world_affine()
            * Affine::translate(self.local_center.to_vec2())
            * Affine::scale_non_uniform(self.scale.x, self.scale.y)
            * original.to_local_affine()
    }
}

fn sgn(v: f64) -> f64 {
    if v < 0.0 { -1.0 } else { 1.0 }
}

/// Resize `original` by dragging `handle` to the world point `pointer`.
///
/// Returns `None` for the rotate handle, an invalid box, or a non-finite pointer.
pub fn resize_box(original: &OrientedBox, handle: BoxHandle, pointer: Point, opts: ResizeOpts) -> Option<ResizeResult> {
    if !original.is_valid() || !crate::point_is_finite(pointer) || !opts.min_size.is_finite() {
        return None;
    }
    let min = opts.min_size.max(0.0);
    let h = original.half();
    let (w0, h0) = (h.x * 2.0, h.y * 2.0);
    let p = original.to_local(pointer);

    // Per-axis result: (new size, direction sign relative to the anchor, new centre, signed scale).
    // `axis` is the moving coordinate of the handle in the original local frame (a half side with sign).
    let axis = |moving: f64, pointer_coord: f64, size0: f64, from_center: bool| -> (f64, f64, f64) {
        // returns (size, centre, signed scale)
        let anchor = if from_center { 0.0 } else { -moving };
        let d = if from_center { pointer_coord } else { pointer_coord - anchor };
        let dir = if d == 0.0 { sgn(moving) } else { sgn(d) };
        let size = if from_center { 2.0 * d.abs() } else { d.abs() };
        let size = size.max(min);
        let centre = if from_center { 0.0 } else { anchor + dir * size * 0.5 };
        let flipped = dir * sgn(moving) < 0.0;
        let k = if size0 > 0.0 { size / size0 } else { 1.0 };
        (size, centre, if flipped { -k } else { k })
    };

    let (sx, sy, cx, cy, kx, ky);
    match handle {
        BoxHandle::Rotate => return None,
        BoxHandle::Corner(c) => {
            let (mx, my) = corner_sign(c);
            let (mx, my) = (mx * h.x, my * h.y);
            let (mut wx, mut cxx, mut kxx) = axis(mx, p.x, w0, opts.from_center);
            let (mut wy, mut cyy, mut kyy) = axis(my, p.y, h0, opts.from_center);
            if opts.keep_aspect && w0 > 0.0 && h0 > 0.0 {
                let s = (wx / w0).max(wy / h0).max(min / w0).max(min / h0);
                // Re-derive both axes with the uniform scale, keeping each axis' direction.
                let dir_x = if kxx < 0.0 { -sgn(mx) } else { sgn(mx) };
                let dir_y = if kyy < 0.0 { -sgn(my) } else { sgn(my) };
                wx = s * w0;
                wy = s * h0;
                cxx = if opts.from_center { 0.0 } else { -mx + dir_x * wx * 0.5 };
                cyy = if opts.from_center { 0.0 } else { -my + dir_y * wy * 0.5 };
                kxx = if kxx < 0.0 { -s } else { s };
                kyy = if kyy < 0.0 { -s } else { s };
            }
            (sx, sy, cx, cy, kx, ky) = (wx, wy, cxx, cyy, kxx, kyy);
        }
        BoxHandle::Edge(e) => {
            let horizontal = matches!(e, Edge::Left | Edge::Right);
            if horizontal {
                let mx = if e == Edge::Right { h.x } else { -h.x };
                let (wx, cxx, kxx) = axis(mx, p.x, w0, opts.from_center);
                let (wy, kyy) =
                    if opts.keep_aspect && w0 > 0.0 { ((wx / w0 * h0).max(0.0), kxx.abs()) } else { (h0, 1.0) };
                (sx, sy, cx, cy, kx, ky) = (wx, wy, cxx, 0.0, kxx, kyy);
            } else {
                let my = if e == Edge::Bottom { h.y } else { -h.y };
                let (wy, cyy, kyy) = axis(my, p.y, h0, opts.from_center);
                let (wx, kxx) =
                    if opts.keep_aspect && h0 > 0.0 { ((wy / h0 * w0).max(0.0), kyy.abs()) } else { (w0, 1.0) };
                (sx, sy, cx, cy, kx, ky) = (wx, wy, 0.0, cyy, kxx, kyy);
            }
        }
    }

    let local_center = Point::new(cx, cy);
    let obox = OrientedBox::new(original.to_world(local_center), kurbo::Size::new(sx, sy), original.angle);
    Some(ResizeResult { obox, scale: Vec2::new(kx, ky), local_center })
}

/// The rotation that points the top of a box at `pointer`, as the rotate handle does:
/// `atan2(pointer - center) + 90 degrees`, in `[0, 2π)`. With `snap_step` (radians) the angle is
/// rounded to a multiple of it. `None` when the pointer is on the centre or a number is not finite.
pub fn rotation_from_pointer(center: Point, pointer: Point, snap_step: Option<f64>) -> Option<f64> {
    if !crate::point_is_finite(center) || !crate::point_is_finite(pointer) {
        return None;
    }
    let v = pointer - center;
    if v.hypot2() == 0.0 {
        return None;
    }
    let a = normalize_angle(v.y.atan2(v.x) + FRAC_PI_2);
    Some(match snap_step {
        Some(step) => snap_rotation(a, step),
        None => a,
    })
}

/// How a marquee selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarqueeMode {
    /// Any shape the marquee touches (the old app's behaviour).
    Touch,
    /// Only shapes whose bounds lie completely inside the marquee.
    Contain,
}

/// Does the marquee `rect` select `shape`? With `Touch`, a shape with a [`Fill::Filled`] fill is selected when
/// the marquee is inside it; a stroke-only shape only when the marquee crosses or contains its outline.
pub fn marquee_selects<O: Outline + ?Sized>(shape: &O, rect: &Rect, mode: MarqueeMode, fill: Fill) -> bool {
    if !shape.is_valid() || !aabb::is_finite(rect) {
        return false;
    }
    match mode {
        MarqueeMode::Contain => aabb::contains_rect(rect, &shape.bounds()),
        MarqueeMode::Touch => shape.intersects_rect(rect, fill),
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::{FRAC_PI_4, PI};

    use kurbo::Size;

    use super::*;
    use crate::shapes::{Polyline, RectShape};

    fn close(a: Point, b: Point) -> bool {
        (a - b).hypot() < 1e-9
    }

    fn bx(x: f64, y: f64, w: f64, h: f64, a: f64) -> OrientedBox {
        OrientedBox::from_xywh(x, y, w, h, a)
    }

    #[test]
    fn constants_match_the_old_app() {
        assert_eq!(HANDLE_SIZE, 16.0);
        assert_eq!(HANDLE_HIT_TOLERANCE, 24.0);
        assert_eq!(ROTATE_HANDLE_OFFSET, 25.0);
    }

    #[test]
    fn handles_of_an_unrotated_box() {
        let b = bx(0.0, 0.0, 100.0, 50.0, 0.0);
        let hs = box_handles(&b, HandleSet::CORNERS_AND_ROTATE, 25.0);
        assert_eq!(hs.len(), 5);
        assert!(close(hs[0].1, Point::new(0.0, 0.0)) && hs[0].0 == BoxHandle::Corner(Corner::TopLeft));
        assert!(close(hs[2].1, Point::new(100.0, 50.0)));
        assert!(close(hs[4].1, Point::new(50.0, -25.0)) && hs[4].0 == BoxHandle::Rotate);
        assert_eq!(box_handles(&b, HandleSet::ALL, 25.0).len(), 9);
        assert_eq!(box_handles(&b, HandleSet::ROTATE_ONLY, 25.0).len(), 1);
    }

    #[test]
    fn rotated_handles_turn_with_the_box() {
        // Same formula as the old rotate handle: centre + R(rot) * (0, -half_h - 25).
        let b = bx(0.0, 0.0, 100.0, 50.0, FRAC_PI_2);
        let rot = handle_point(&b, BoxHandle::Rotate, 25.0);
        // centre (50,25); local (0,-50) rotated 90 degrees clockwise -> (50, 0)
        assert!(close(rot, Point::new(100.0, 25.0)), "{rot:?}");
        let old = Point::new(50.0 - (-50.0_f64) * FRAC_PI_2.sin(), 25.0 + (-50.0_f64) * FRAC_PI_2.cos());
        assert!(close(rot, old), "agrees with the old closed form");
    }

    #[test]
    fn handle_hit_test_priority_and_tolerance() {
        let b = bx(0.0, 0.0, 100.0, 50.0, 0.0);
        let h = |p: Point| hit_test_handles(&b, p, 5.0, HandleSet::ALL, 25.0);
        assert_eq!(h(Point::new(2.0, 2.0)), Some(BoxHandle::Corner(Corner::TopLeft)));
        assert_eq!(h(Point::new(50.0, -26.0)), Some(BoxHandle::Rotate));
        assert_eq!(h(Point::new(50.0, 1.0)), Some(BoxHandle::Edge(Edge::Top)));
        assert_eq!(h(Point::new(50.0, 25.0)), None);
        assert_eq!(hit_test_handles(&b, Point::new(f64::NAN, 0.0), 5.0, HandleSet::ALL, 25.0), None);
    }

    #[test]
    fn boundary_ring() {
        let b = bx(0.0, 0.0, 100.0, 50.0, 0.0);
        assert!(hit_test_boundary(&b, Point::new(50.0, -3.0), 8.0));
        assert!(hit_test_boundary(&b, Point::new(50.0, 5.0), 8.0));
        assert!(!hit_test_boundary(&b, Point::new(50.0, 25.0), 8.0));
        assert!(!hit_test_boundary(&b, Point::new(50.0, -9.0), 8.0));
        // rotated: the ring follows the box
        let r = bx(0.0, 0.0, 100.0, 10.0, FRAC_PI_2);
        assert!(hit_test_boundary(&r, Point::new(50.0, -44.0), 3.0), "the top end of the turned bar");
        assert!(!hit_test_boundary(&r, Point::new(50.0, 5.0), 3.0), "its middle");
        assert!(!hit_test_boundary(&r, Point::new(5.0, 5.0), 3.0), "where the unrotated box used to be");
    }

    #[test]
    fn resize_corner_keeps_the_opposite_corner() {
        let b = bx(10.0, 20.0, 100.0, 50.0, 0.0);
        let r = resize_box(&b, BoxHandle::Corner(Corner::BottomRight), Point::new(210.0, 120.0), ResizeOpts::default())
            .unwrap();
        let bb = r.obox.aabb();
        assert_eq!((bb.x0, bb.y0, bb.x1, bb.y1), (10.0, 20.0, 210.0, 120.0));
        assert_eq!(r.scale, Vec2::new(2.0, 2.0));
        assert!(!r.flip_x() && !r.flip_y());
        let r = resize_box(&b, BoxHandle::Corner(Corner::TopLeft), Point::new(-40.0, -30.0), ResizeOpts::default())
            .unwrap();
        let bb = r.obox.aabb();
        assert_eq!((bb.x0, bb.y0, bb.x1, bb.y1), (-40.0, -30.0, 110.0, 70.0), "bottom-right stays");
    }

    #[test]
    fn resize_aspect_keeps_the_anchor_for_every_corner() {
        // The old app let three corners drift when Shift was held.
        let b = bx(0.0, 0.0, 100.0, 50.0, 0.0);
        let opts = ResizeOpts { keep_aspect: true, ..Default::default() };
        for (corner, anchor, pointer) in [
            (Corner::TopLeft, Point::new(100.0, 50.0), Point::new(-100.0, -20.0)),
            (Corner::TopRight, Point::new(0.0, 50.0), Point::new(300.0, -10.0)),
            (Corner::BottomLeft, Point::new(100.0, 0.0), Point::new(-50.0, 90.0)),
            (Corner::BottomRight, Point::new(0.0, 0.0), Point::new(250.0, 90.0)),
        ] {
            let r = resize_box(&b, BoxHandle::Corner(corner), pointer, opts).unwrap();
            assert!(
                (r.obox.size.width / r.obox.size.height - 2.0).abs() < 1e-9,
                "{corner:?}: aspect {:?}",
                r.obox.size
            );
            let bb = r.obox.aabb();
            let corners = [
                Point::new(bb.x0, bb.y0),
                Point::new(bb.x1, bb.y0),
                Point::new(bb.x1, bb.y1),
                Point::new(bb.x0, bb.y1),
            ];
            assert!(corners.iter().any(|c| close(*c, anchor)), "{corner:?}: anchor {anchor:?} not a corner of {bb:?}");
        }
    }

    #[test]
    fn resize_a_rotated_box_follows_the_pointer() {
        // The old code applied the world delta to the unrotated box.
        let b = bx(0.0, 0.0, 100.0, 50.0, FRAC_PI_4);
        let anchor = handle_point(&b, BoxHandle::Corner(Corner::TopLeft), 0.0);
        let target = b.to_world(Point::new(80.0, 40.0)); // a point on the local diagonal, past the corner
        let r = resize_box(&b, BoxHandle::Corner(Corner::BottomRight), target, ResizeOpts::default()).unwrap();
        assert_eq!(r.obox.angle, FRAC_PI_4);
        assert!(
            close(handle_point(&r.obox, BoxHandle::Corner(Corner::BottomRight), 0.0), target),
            "the handle is under the pointer"
        );
        assert!(
            close(handle_point(&r.obox, BoxHandle::Corner(Corner::TopLeft), 0.0), anchor),
            "the opposite corner did not move"
        );
    }

    #[test]
    fn resize_from_centre() {
        let b = bx(0.0, 0.0, 100.0, 50.0, 0.0);
        let r = resize_box(
            &b,
            BoxHandle::Corner(Corner::BottomRight),
            Point::new(100.0, 75.0),
            ResizeOpts { from_center: true, ..Default::default() },
        )
        .unwrap();
        assert!(close(r.obox.center, Point::new(50.0, 25.0)));
        assert_eq!((r.obox.size.width, r.obox.size.height), (100.0, 100.0));
    }

    #[test]
    fn resize_edge() {
        let b = bx(0.0, 0.0, 100.0, 50.0, 0.0);
        let r = resize_box(&b, BoxHandle::Edge(Edge::Right), Point::new(300.0, 999.0), ResizeOpts::default()).unwrap();
        let bb = r.obox.aabb();
        assert_eq!((bb.x0, bb.x1, bb.y0, bb.y1), (0.0, 300.0, 0.0, 50.0), "only the width changes");
        let r = resize_box(&b, BoxHandle::Edge(Edge::Top), Point::new(999.0, -50.0), ResizeOpts::default()).unwrap();
        let bb = r.obox.aabb();
        assert_eq!((bb.x0, bb.x1, bb.y0, bb.y1), (0.0, 100.0, -50.0, 50.0));
        // with aspect: the other side scales about the centre line
        let r = resize_box(
            &b,
            BoxHandle::Edge(Edge::Right),
            Point::new(200.0, 0.0),
            ResizeOpts { keep_aspect: true, ..Default::default() },
        )
        .unwrap();
        assert_eq!((r.obox.size.width, r.obox.size.height), (200.0, 100.0));
        assert!(close(r.obox.center, Point::new(100.0, 25.0)));
    }

    #[test]
    fn resize_past_the_anchor_flips() {
        let b = bx(0.0, 0.0, 100.0, 50.0, 0.0);
        let r = resize_box(&b, BoxHandle::Edge(Edge::Right), Point::new(-60.0, 25.0), ResizeOpts::default()).unwrap();
        assert!(r.flip_x() && !r.flip_y());
        let bb = r.obox.aabb();
        assert_eq!((bb.x0, bb.x1), (-60.0, 0.0));
        // The affine mirrors content: the original right edge (x = 100) lands on the new left edge.
        let a = r.affine(&b);
        assert!(close(a * Point::new(100.0, 10.0), Point::new(-60.0, 10.0)));
        assert!(close(a * Point::new(0.0, 10.0), Point::new(0.0, 10.0)), "anchor is fixed");
    }

    #[test]
    fn resize_affine_maps_the_box_onto_the_result() {
        let b = bx(10.0, 10.0, 100.0, 50.0, 0.6);
        let r = resize_box(&b, BoxHandle::Corner(Corner::BottomLeft), Point::new(-20.0, 140.0), ResizeOpts::default())
            .unwrap();
        let a = r.affine(&b);
        let mut want = r.obox.corners().to_vec();
        for c in b.corners() {
            let m = a * c;
            let i = want.iter().position(|w| close(*w, m)).expect("corner maps to a corner");
            want.swap_remove(i);
        }
    }

    #[test]
    fn resize_min_size_and_hostile() {
        let b = bx(0.0, 0.0, 100.0, 50.0, 0.0);
        let r = resize_box(&b, BoxHandle::Corner(Corner::BottomRight), Point::new(0.0, 0.0), ResizeOpts::default())
            .unwrap();
        assert_eq!((r.obox.size.width, r.obox.size.height), (1.0, 1.0));
        assert!(resize_box(&b, BoxHandle::Rotate, Point::ZERO, ResizeOpts::default()).is_none());
        assert!(
            resize_box(&b, BoxHandle::Edge(Edge::Left), Point::new(f64::NAN, 0.0), ResizeOpts::default()).is_none()
        );
        let zero = bx(5.0, 5.0, 0.0, 0.0, 0.0);
        let r = resize_box(
            &zero,
            BoxHandle::Corner(Corner::BottomRight),
            Point::new(15.0, 15.0),
            ResizeOpts { keep_aspect: true, ..Default::default() },
        )
        .unwrap();
        assert!(r.obox.is_valid() && r.scale.x.is_finite());
        let bad = OrientedBox::new(Point::ZERO, Size::new(f64::NAN, 1.0), 0.0);
        assert!(resize_box(&bad, BoxHandle::Edge(Edge::Left), Point::ZERO, ResizeOpts::default()).is_none());
    }

    #[test]
    fn rotation_from_pointer_matches_the_old_formula() {
        let c = Point::new(0.0, 0.0);
        // pointer straight above the centre -> 0
        assert!(rotation_from_pointer(c, Point::new(0.0, -50.0), None).unwrap().abs() < 1e-12);
        // pointer to the right -> 90 degrees
        assert!((rotation_from_pointer(c, Point::new(50.0, 0.0), None).unwrap() - FRAC_PI_2).abs() < 1e-12);
        // pointer below -> 180 degrees; left -> 270 degrees (normalised, the old code returned -90)
        assert!((rotation_from_pointer(c, Point::new(0.0, 50.0), None).unwrap() - PI).abs() < 1e-12);
        assert!((rotation_from_pointer(c, Point::new(-50.0, 0.0), None).unwrap() - 3.0 * FRAC_PI_2).abs() < 1e-12);
        // snapping to 15 degrees
        let a = rotation_from_pointer(c, Point::new(50.0, -9.0), Some(crate::snap::ROTATION_SNAP_STEP)).unwrap();
        assert!((a.to_degrees() - 75.0).abs() < 1e-9, "{}", a.to_degrees());
        assert!(rotation_from_pointer(c, c, None).is_none());
        assert!(rotation_from_pointer(c, Point::new(f64::NAN, 1.0), None).is_none());
    }

    #[test]
    fn marquee_modes() {
        let shape = RectShape::from_rect(Rect::new(0.0, 0.0, 100.0, 100.0));
        let big = Rect::new(-10.0, -10.0, 110.0, 110.0);
        let part = Rect::new(50.0, 50.0, 200.0, 200.0);
        let inside = Rect::new(40.0, 40.0, 60.0, 60.0);
        assert!(marquee_selects(&shape, &big, MarqueeMode::Contain, Fill::Filled));
        assert!(!marquee_selects(&shape, &part, MarqueeMode::Contain, Fill::Filled));
        assert!(marquee_selects(&shape, &part, MarqueeMode::Touch, Fill::Filled));
        assert!(marquee_selects(&shape, &inside, MarqueeMode::Touch, Fill::Filled));
        assert!(!marquee_selects(&shape, &inside, MarqueeMode::Touch, Fill::Stroke));
        let line = Polyline::new(vec![Point::new(0.0, 0.0), Point::new(100.0, 100.0)], false);
        assert!(
            !marquee_selects(&line, &Rect::new(60.0, 0.0, 100.0, 30.0), MarqueeMode::Touch, Fill::Stroke),
            "bounds overlap but the line does not touch"
        );
        assert!(marquee_selects(&line, &Rect::new(40.0, 40.0, 60.0, 60.0), MarqueeMode::Touch, Fill::Stroke));
        assert!(!marquee_selects(&line, &Rect::new(f64::NAN, 0.0, 1.0, 1.0), MarqueeMode::Touch, Fill::Stroke));
    }
}
