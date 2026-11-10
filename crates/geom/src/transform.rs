//! Element transforms: local-to-world affines with rotation and flip, angle helpers, and
//! mapping a box through an arbitrary affine.
//!
//! The old app moved a box through an affine by mapping only its top-left corner and scaling the
//! width by `|a|`. That put a flipped rectangle on the wrong side of the mirror axis. The functions
//! here map the centre and the two axes, so a flip about any line is exact.

use std::f64::consts::{PI, TAU};

use kurbo::{Affine, Point, Size, Vec2};

use crate::obox::OrientedBox;

/// Mirror flags of an element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Flip {
    pub x: bool,
    pub y: bool,
}

/// The frame of a box-like element: where its unrotated box sits, how it is turned and mirrored.
///
/// Local space is the element's own space with the origin at the top-left of the unrotated box,
/// so `(0, 0)` is the top-left and `(width, height)` the bottom-right. Points stored inside an
/// element (a line's vertices, a path) live in this space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ElementFrame {
    /// Top-left of the unrotated box in world space.
    pub origin: Point,
    pub size: Size,
    /// Radians, clockwise, about the centre of the box.
    pub angle: f64,
    pub flip: Flip,
}

impl ElementFrame {
    pub fn new(origin: Point, size: Size, angle: f64) -> Self {
        Self { origin, size, angle, flip: Flip::default() }
    }

    pub fn with_flip(mut self, x: bool, y: bool) -> Self {
        self.flip = Flip { x, y };
        self
    }

    /// The oriented box of the frame. Flip does not change it: a box is its own mirror image.
    pub fn obox(&self) -> OrientedBox {
        OrientedBox::from_xywh(self.origin.x, self.origin.y, self.size.width, self.size.height, self.angle)
    }

    /// Local space to world: mirror about the box centre, rotate about the centre, translate.
    pub fn to_world(&self) -> Affine {
        let (w, h) = (self.size.width, self.size.height);
        let c = Vec2::new(w * 0.5, h * 0.5);
        let sx = if self.flip.x { -1.0 } else { 1.0 };
        let sy = if self.flip.y { -1.0 } else { 1.0 };
        let world_centre = self.origin.to_vec2() + c;
        Affine::translate(world_centre)
            * Affine::rotate(self.angle)
            * Affine::scale_non_uniform(sx, sy)
            * Affine::translate(-c)
    }

    /// World to local space. Falls back to the identity when the frame is not invertible
    /// (non-finite numbers).
    pub fn to_local(&self) -> Affine {
        let a = self.to_world();
        if a.as_coeffs().iter().all(|v| v.is_finite()) && a.determinant().is_finite() && a.determinant() != 0.0 {
            a.inverse()
        } else {
            Affine::IDENTITY
        }
    }
}

/// Rotate `p` by `angle` about `pivot`.
#[inline]
pub fn rotate_point(p: Point, pivot: Point, angle: f64) -> Point {
    let (s, c) = angle.sin_cos();
    let v = p - pivot;
    Point::new(pivot.x + v.x * c - v.y * s, pivot.y + v.x * s + v.y * c)
}

/// Bring an angle into `[0, 2π)`. Non-finite input gives 0.
pub fn normalize_angle(a: f64) -> f64 {
    if !a.is_finite() {
        return 0.0;
    }
    let r = a.rem_euclid(TAU);
    if r >= TAU { 0.0 } else { r }
}

/// Bring an angle into `(-π, π]`. Non-finite input gives 0.
pub fn normalize_angle_signed(a: f64) -> f64 {
    if !a.is_finite() {
        return 0.0;
    }
    let r = normalize_angle(a);
    if r > PI { r - TAU } else { r }
}

/// The smallest signed turn from `from` to `to`, in `(-π, π]`.
pub fn angle_delta(from: f64, to: f64) -> f64 {
    normalize_angle_signed(to - from)
}

/// An affine applied about a pivot: `translate(pivot) * affine * translate(-pivot)`.
pub fn about(pivot: Point, affine: Affine) -> Affine {
    Affine::translate(pivot.to_vec2()) * affine * Affine::translate(-pivot.to_vec2())
}

/// Mirror about the vertical line `x = cx`.
pub fn flip_x_about(cx: f64) -> Affine {
    Affine::new([-1.0, 0.0, 0.0, 1.0, 2.0 * cx, 0.0])
}

/// Mirror about the horizontal line `y = cy`.
pub fn flip_y_about(cy: f64) -> Affine {
    Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, 2.0 * cy])
}

/// An affine split into translation, rotation and scale. `scale.x` is negative when the transform
/// mirrors (the determinant is negative); the mirror is attributed to the x axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trs {
    pub translation: Vec2,
    pub rotation: f64,
    pub scale: Vec2,
}

/// Split an affine into translation, rotation and scale. Shear is dropped. `None` for non-finite
/// or singular transforms.
pub fn decompose(a: Affine) -> Option<Trs> {
    let [m00, m10, m01, m11, tx, ty] = a.as_coeffs();
    let det = m00 * m11 - m01 * m10;
    if !det.is_finite() || det == 0.0 || !tx.is_finite() || !ty.is_finite() {
        return None;
    }
    let sx = m00.hypot(m10) * det.signum();
    let sy = m01.hypot(m11);
    // The first column is the image of the x axis. With a mirror in x it points the other way.
    let rotation = (m10 * det.signum()).atan2(m00 * det.signum());
    Some(Trs { translation: Vec2::new(tx, ty), rotation, scale: Vec2::new(sx, sy) })
}

/// Map a box through an affine made of a translation, rotation, uniform or non-uniform scale and
/// optional mirror. Returns the new box and whether the transform mirrors.
///
/// The centre is mapped as a point; the two box axes are mapped as directions. A mirror turns the
/// angle around (a mirrored box is the same rectangle at the mirrored angle). Shear is ignored.
/// `None` when the box or the affine is not finite.
pub fn map_box(b: &OrientedBox, a: Affine) -> Option<(OrientedBox, bool)> {
    if !b.is_valid() {
        return None;
    }
    let det = a.determinant();
    if !det.is_finite() || a.as_coeffs().iter().any(|v| !v.is_finite()) {
        return None;
    }
    let center = a * b.center;
    let ux = a * Point::new(b.center.x + b.angle.cos(), b.center.y + b.angle.sin()) - center;
    let uy = a * Point::new(b.center.x - b.angle.sin(), b.center.y + b.angle.cos()) - center;
    let size = Size::new(b.size.width * ux.hypot(), b.size.height * uy.hypot());
    let angle = ux.y.atan2(ux.x);
    Some((OrientedBox::new(center, size, angle), det < 0.0))
}

#[cfg(test)]
mod tests {
    use std::f64::consts::FRAC_PI_2;

    use super::*;

    fn close(a: Point, b: Point) -> bool {
        (a - b).hypot() < 1e-9
    }

    #[test]
    fn frame_maps_local_corners() {
        let f = ElementFrame::new(Point::new(10.0, 20.0), Size::new(30.0, 10.0), 0.0);
        assert!(close(f.to_world() * Point::ZERO, Point::new(10.0, 20.0)));
        assert!(close(f.to_world() * Point::new(30.0, 10.0), Point::new(40.0, 30.0)));
    }

    #[test]
    fn frame_rotation_is_about_the_centre() {
        let f = ElementFrame::new(Point::new(0.0, 0.0), Size::new(10.0, 4.0), FRAC_PI_2);
        // centre (5,2) stays; local top-left (-5,-2 from centre) -> (2,-5) -> world (7,-3)
        assert!(close(f.to_world() * Point::new(5.0, 2.0), Point::new(5.0, 2.0)));
        assert!(close(f.to_world() * Point::ZERO, Point::new(7.0, -3.0)));
    }

    #[test]
    fn frame_flip_mirrors_local_points_in_place() {
        let f = ElementFrame::new(Point::new(10.0, 0.0), Size::new(20.0, 10.0), 0.0).with_flip(true, false);
        // local (0,0) is the top-left; mirrored it lands at the top-right
        assert!(close(f.to_world() * Point::ZERO, Point::new(30.0, 0.0)));
        assert!(close(f.to_world() * Point::new(20.0, 10.0), Point::new(10.0, 10.0)));
        // box itself is unchanged
        assert_eq!(f.obox().aabb(), kurbo::Rect::new(10.0, 0.0, 30.0, 10.0));
    }

    #[test]
    fn frame_inverse_round_trips() {
        let f = ElementFrame::new(Point::new(3.0, 4.0), Size::new(7.0, 9.0), 0.4).with_flip(true, true);
        let p = Point::new(2.0, 3.0);
        assert!(close(f.to_local() * (f.to_world() * p), p));
        let bad = ElementFrame::new(Point::new(f64::NAN, 0.0), Size::new(1.0, 1.0), 0.0);
        assert_eq!(bad.to_local(), Affine::IDENTITY);
    }

    #[test]
    fn angle_helpers() {
        assert!((normalize_angle(-0.5) - (TAU - 0.5)).abs() < 1e-12);
        assert_eq!(normalize_angle(f64::NAN), 0.0);
        assert!((normalize_angle_signed(3.0 * PI / 2.0) + PI / 2.0).abs() < 1e-12);
        assert!((angle_delta(0.1, TAU - 0.1) + 0.2).abs() < 1e-12);
    }

    #[test]
    fn flip_about_line() {
        let a = flip_x_about(5.0);
        assert!(close(a * Point::new(2.0, 7.0), Point::new(8.0, 7.0)));
        let a = flip_y_about(5.0);
        assert!(close(a * Point::new(2.0, 7.0), Point::new(2.0, 3.0)));
    }

    #[test]
    fn decompose_rotation_scale_and_mirror() {
        let a = Affine::translate((5.0, 6.0)) * Affine::rotate(0.5) * Affine::scale_non_uniform(2.0, 3.0);
        let t = decompose(a).unwrap();
        assert!((t.rotation - 0.5).abs() < 1e-12);
        assert!((t.scale.x - 2.0).abs() < 1e-12 && (t.scale.y - 3.0).abs() < 1e-12);
        assert_eq!(t.translation, Vec2::new(5.0, 6.0));
        let m = decompose(Affine::scale_non_uniform(-2.0, 3.0)).unwrap();
        assert!(m.scale.x < 0.0);
        assert!(decompose(Affine::scale(0.0)).is_none());
        assert!(decompose(Affine::translate((f64::NAN, 0.0))).is_none());
    }

    #[test]
    fn map_box_flip_lands_on_the_mirrored_side() {
        // The old app's flip bug: a box at x 0..20 flipped about x = 50 must land at 80..100.
        let b = OrientedBox::from_xywh(0.0, 0.0, 20.0, 10.0, 0.0);
        let (m, mirrored) = map_box(&b, flip_x_about(50.0)).unwrap();
        assert!(mirrored);
        let r = m.aabb();
        assert!((r.x0 - 80.0).abs() < 1e-9 && (r.x1 - 100.0).abs() < 1e-9, "{r:?}");
        assert!((r.y0).abs() < 1e-9 && (r.y1 - 10.0).abs() < 1e-9);
    }

    #[test]
    fn map_box_flip_of_a_rotated_box_negates_its_angle() {
        let b = OrientedBox::from_xywh(0.0, 0.0, 20.0, 10.0, 0.3);
        let (m, _) = map_box(&b, flip_x_about(0.0)).unwrap();
        let expect = normalize_angle_signed(PI - 0.3);
        assert!(angle_delta(m.angle, expect).abs() < 1e-9, "{} vs {expect}", m.angle);
        // corners of the mirrored box are the mirrored corners
        let mut want: Vec<_> = b.corners().iter().map(|c| Point::new(-c.x, c.y)).collect();
        for c in m.corners() {
            let i = want.iter().position(|w| close(*w, c)).expect("corner");
            want.swap_remove(i);
        }
    }

    #[test]
    fn map_box_scales() {
        let b = OrientedBox::from_xywh(0.0, 0.0, 10.0, 10.0, 0.0);
        let (m, mirrored) = map_box(&b, Affine::scale_non_uniform(2.0, 3.0)).unwrap();
        assert!(!mirrored);
        assert!((m.size.width - 20.0).abs() < 1e-9 && (m.size.height - 30.0).abs() < 1e-9);
        assert!(map_box(&b, Affine::translate((f64::NAN, 0.0))).is_none());
    }
}
