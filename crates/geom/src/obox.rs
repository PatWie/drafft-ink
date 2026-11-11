//! The oriented box: a rectangle with a centre, a size and a rotation about its centre.
//!
//! This is the frame of every box-like element (rectangle, ellipse, diamond, text, image).
//! Excalidraw stores `x, y, width, height, angle` and rotates about the centre of the unrotated
//! box; [`OrientedBox::from_xywh`] reads that form.

use kurbo::{Affine, Point, Rect, Size, Vec2};

use crate::aabb;

/// A rotated rectangle. `angle` is in radians, clockwise on screen, about `center`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrientedBox {
    /// Centre of the box in world space.
    pub center: Point,
    /// Unrotated width and height.
    pub size: Size,
    /// Radians, clockwise on screen, about `center`.
    pub angle: f64,
}

impl OrientedBox {
    /// A box from its centre, size and angle.
    pub fn new(center: Point, size: Size, angle: f64) -> Self {
        Self { center, size, angle }
    }

    /// An unrotated box covering `rect`.
    pub fn from_rect(rect: Rect) -> Self {
        Self::new(aabb::center(&rect), Size::new(rect.width().abs(), rect.height().abs()), 0.0)
    }

    /// The Excalidraw form: top-left `(x, y)` of the unrotated box, size, and angle about the centre.
    pub fn from_xywh(x: f64, y: f64, w: f64, h: f64, angle: f64) -> Self {
        Self::new(Point::new(x + w * 0.5, y + h * 0.5), Size::new(w, h), angle)
    }

    /// True when every number is usable (finite, at most `MAX_COORD`) and the size is not negative.
    pub fn is_valid(&self) -> bool {
        crate::point_is_finite(self.center)
            && crate::num_is_sane(self.size.width)
            && crate::num_is_sane(self.size.height)
            && self.size.width >= 0.0
            && self.size.height >= 0.0
            && self.angle.is_finite()
    }

    /// Half of the size as a vector.
    #[inline]
    pub fn half(&self) -> Vec2 {
        Vec2::new(self.size.width * 0.5, self.size.height * 0.5)
    }

    /// The box before rotation (the `x, y, width, height` rect of the element).
    pub fn unrotated_rect(&self) -> Rect {
        let h = self.half();
        Rect::new(self.center.x - h.x, self.center.y - h.y, self.center.x + h.x, self.center.y + h.y)
    }

    /// The box in its own frame: centred on the origin, unrotated.
    pub fn local_rect(&self) -> Rect {
        let h = self.half();
        Rect::new(-h.x, -h.y, h.x, h.y)
    }

    /// Local frame (origin at the centre, axes along the box) to world.
    pub fn to_world_affine(&self) -> Affine {
        Affine::translate(self.center.to_vec2()) * Affine::rotate(self.angle)
    }

    /// World to the local frame.
    pub fn to_local_affine(&self) -> Affine {
        Affine::rotate(-self.angle) * Affine::translate(-self.center.to_vec2())
    }

    /// World point to the local frame (origin at the centre).
    #[inline]
    pub fn to_local(&self, p: Point) -> Point {
        let v = p - self.center;
        let (s, c) = (-self.angle).sin_cos();
        Point::new(v.x * c - v.y * s, v.x * s + v.y * c)
    }

    /// Local point to world.
    #[inline]
    pub fn to_world(&self, p: Point) -> Point {
        let (s, c) = self.angle.sin_cos();
        Point::new(self.center.x + p.x * c - p.y * s, self.center.y + p.x * s + p.y * c)
    }

    /// World direction to the local frame (no translation).
    #[inline]
    pub fn dir_to_local(&self, v: Vec2) -> Vec2 {
        let (s, c) = (-self.angle).sin_cos();
        Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
    }

    /// Local direction to world.
    #[inline]
    pub fn dir_to_world(&self, v: Vec2) -> Vec2 {
        let (s, c) = self.angle.sin_cos();
        Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
    }

    /// World corners: top-left, top-right, bottom-right, bottom-left (of the unrotated box).
    pub fn corners(&self) -> [Point; 4] {
        let h = self.half();
        [(-h.x, -h.y), (h.x, -h.y), (h.x, h.y), (-h.x, h.y)].map(|(x, y)| self.to_world(Point::new(x, y)))
    }

    /// World midpoints of the sides: top, right, bottom, left (of the unrotated box).
    pub fn side_midpoints(&self) -> [Point; 4] {
        let h = self.half();
        [(0.0, -h.y), (h.x, 0.0), (0.0, h.y), (-h.x, 0.0)].map(|(x, y)| self.to_world(Point::new(x, y)))
    }

    /// The axis-aligned bounds of the rotated box.
    pub fn aabb(&self) -> Rect {
        let h = self.half();
        let (s, c) = self.angle.sin_cos();
        let ex = h.x * c.abs() + h.y * s.abs();
        let ey = h.x * s.abs() + h.y * c.abs();
        Rect::new(self.center.x - ex, self.center.y - ey, self.center.x + ex, self.center.y + ey)
    }

    /// Closed containment. False for invalid boxes and non-finite points.
    pub fn contains(&self, p: Point) -> bool {
        if !self.is_valid() || !crate::point_is_finite(p) {
            return false;
        }
        let l = self.to_local(p);
        let h = self.half();
        l.x.abs() <= h.x && l.y.abs() <= h.y
    }

    /// Distance from the point to the filled box (0 inside). Infinity for invalid input.
    pub fn distance_to_point(&self, p: Point) -> f64 {
        if !self.is_valid() || !crate::point_is_finite(p) {
            return f64::INFINITY;
        }
        let l = self.to_local(p);
        let h = self.half();
        (l.x.abs() - h.x).max(0.0).hypot((l.y.abs() - h.y).max(0.0))
    }

    /// The same box turned by `delta` radians about `pivot`.
    pub fn rotated_about(&self, pivot: Point, delta: f64) -> Self {
        let c = Affine::rotate_about(delta, pivot) * self.center;
        Self::new(c, self.size, self.angle + delta)
    }

    /// The same box moved by `v`.
    pub fn translated(&self, v: Vec2) -> Self {
        Self::new(self.center + v, self.size, self.angle)
    }

    /// The box with its angle brought into `[0, 2π)`.
    pub fn with_normalized_angle(&self) -> Self {
        Self::new(self.center, self.size, crate::transform::normalize_angle(self.angle))
    }

    /// Does the rotated box touch the axis-aligned rect? Separating axis test.
    pub fn intersects_aabb(&self, r: &Rect) -> bool {
        if !self.is_valid() || !aabb::is_finite(r) {
            return false;
        }
        self.intersects(&OrientedBox::from_rect(aabb::normalize(*r)))
    }

    /// Do two oriented boxes touch? Separating axis test over the four edge normals.
    pub fn intersects(&self, other: &OrientedBox) -> bool {
        if !self.is_valid() || !other.is_valid() {
            return false;
        }
        let axes = [
            Vec2::new(self.angle.cos(), self.angle.sin()),
            Vec2::new(-self.angle.sin(), self.angle.cos()),
            Vec2::new(other.angle.cos(), other.angle.sin()),
            Vec2::new(-other.angle.sin(), other.angle.cos()),
        ];
        let (ca, cb) = (self.corners(), other.corners());
        for axis in axes {
            let project = |cs: &[Point; 4]| {
                cs.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), c| {
                    let d = c.to_vec2().dot(axis);
                    (lo.min(d), hi.max(d))
                })
            };
            let (a0, a1) = project(&ca);
            let (b0, b1) = project(&cb);
            if a1 < b0 - 1e-12 || b1 < a0 - 1e-12 {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::{FRAC_PI_2, FRAC_PI_4};

    use super::*;

    fn close(a: Point, b: Point) -> bool {
        (a - b).hypot() < 1e-9
    }

    #[test]
    fn xywh_rotates_about_centre() {
        let b = OrientedBox::from_xywh(0.0, 0.0, 10.0, 4.0, FRAC_PI_2);
        assert!(close(b.center, Point::new(5.0, 2.0)));
        let c = b.corners();
        // top-left (-5,-2) rotated a quarter turn clockwise about the centre -> (2,-5)
        assert!(close(c[0], Point::new(7.0, -3.0)));
        let bb = b.aabb();
        assert!((bb.width() - 4.0).abs() < 1e-9 && (bb.height() - 10.0).abs() < 1e-9);
    }

    #[test]
    fn local_world_round_trip() {
        let b = OrientedBox::new(Point::new(3.0, -4.0), Size::new(8.0, 2.0), 0.7);
        let p = Point::new(11.0, 5.0);
        assert!(close(b.to_world(b.to_local(p)), p));
        assert!(close(b.to_world_affine() * b.to_local(p), p));
        assert!(close(b.to_local_affine() * p, b.to_local(p)));
    }

    #[test]
    fn containment_follows_rotation() {
        let b = OrientedBox::new(Point::ZERO, Size::new(20.0, 2.0), FRAC_PI_4);
        assert!(b.contains(Point::new(5.0, 5.0)));
        assert!(!b.contains(Point::new(5.0, -5.0)));
        assert!(!b.contains(Point::new(f64::NAN, 0.0)));
    }

    #[test]
    fn distance_is_zero_inside_and_euclidean_outside() {
        let b = OrientedBox::new(Point::ZERO, Size::new(10.0, 10.0), 0.0);
        assert_eq!(b.distance_to_point(Point::new(1.0, 1.0)), 0.0);
        assert!((b.distance_to_point(Point::new(8.0, 9.0)) - 5.0).abs() < 1e-12);
    }

    #[test]
    fn sat_intersection() {
        let a = OrientedBox::new(Point::ZERO, Size::new(20.0, 2.0), FRAC_PI_4);
        // A diagonal sliver: its AABB covers (8,0) but the sliver does not.
        let far = Rect::new(7.0, -1.0, 9.0, 1.0);
        assert!(aabb::intersects(&a.aabb(), &far));
        assert!(!a.intersects_aabb(&far));
        assert!(a.intersects_aabb(&Rect::new(2.0, 2.0, 3.0, 3.0)));
        assert!(!OrientedBox::new(Point::ZERO, Size::new(f64::NAN, 1.0), 0.0).intersects(&a));
    }

    #[test]
    fn rotate_about_pivot() {
        let b = OrientedBox::new(Point::new(10.0, 0.0), Size::new(2.0, 2.0), 0.0).rotated_about(Point::ZERO, FRAC_PI_2);
        assert!(close(b.center, Point::new(0.0, 10.0)));
        assert!((b.angle - FRAC_PI_2).abs() < 1e-12);
    }

    #[test]
    fn invalid_boxes() {
        assert!(!OrientedBox::new(Point::ZERO, Size::new(-1.0, 1.0), 0.0).is_valid());
        assert!(!OrientedBox::new(Point::new(f64::INFINITY, 0.0), Size::new(1.0, 1.0), 0.0).is_valid());
        assert!(OrientedBox::new(Point::ZERO, Size::new(0.0, 0.0), 0.0).is_valid());
    }
}
