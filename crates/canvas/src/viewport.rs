//! The camera: world to screen transforms, zoom around a point, pan, fit to bounds.
//!
//! Conventions
//! * World space is the scene's space, y pointing down, in world units.
//! * Screen space is *logical* pixels (CSS pixels in a browser), origin at the top-left of the canvas, y down.
//!   Pointer events and all pixel thresholds are in logical pixels. [`Viewport::dpr`] (device pixels per logical
//!   pixel) only matters for the final GPU transform, [`Viewport::world_to_device`].
//! * The camera is a scroll point (the world point at the centre of the viewport) and a zoom (logical pixels per
//!   world unit). That is the form `view.get` / `view.set` use in the engine.
//!
//! Nothing here animates. A caller that wants a smooth zoom calls [`Viewport::set_camera`] once per frame with
//! interpolated cameras ([`Camera::lerp`] helps).
//!
//! Every method ignores non-finite input and returns `false` (or the input unchanged) instead of panicking.

use drafftink_geom::kurbo::{Affine, Point, Rect, Size, Vec2};
use serde::{Deserialize, Serialize};

/// Smallest zoom.
pub const MIN_ZOOM: f64 = 0.1;
/// Largest zoom.
pub const MAX_ZOOM: f64 = 30.0;

/// Clamps a zoom into [`MIN_ZOOM`], [`MAX_ZOOM`]. A non-finite zoom becomes 1.
pub fn clamp_zoom(zoom: f64) -> f64 {
    if zoom.is_finite() { zoom.clamp(MIN_ZOOM, MAX_ZOOM) } else { 1.0 }
}

/// Where the viewport looks: the world point at the centre of the viewport, and the zoom.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    /// The world point shown at the centre of the viewport.
    pub scroll: Point,
    /// Logical pixels per world unit.
    pub zoom: f64,
}

impl Camera {
    /// The camera at the origin with zoom 1.
    pub const HOME: Camera = Camera { scroll: Point::ZERO, zoom: 1.0 };

    /// Interpolates between two cameras. `t` is clamped to `[0, 1]`. The zoom is interpolated in log space so
    /// that a zoom animation feels even; the scroll point moves linearly in world space.
    pub fn lerp(self, other: Camera, t: f64) -> Camera {
        let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 1.0 };
        let z = (self.zoom.ln() * (1.0 - t) + other.zoom.ln() * t).exp();
        Camera { scroll: self.scroll.lerp(other.scroll, t), zoom: clamp_zoom(z) }
    }

    fn is_finite(&self) -> bool {
        self.scroll.x.is_finite() && self.scroll.y.is_finite() && self.zoom.is_finite() && self.zoom > 0.0
    }
}

impl Default for Camera {
    fn default() -> Self {
        Camera::HOME
    }
}

/// A camera plus the size of the canvas in logical pixels and the device pixel ratio.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    camera: Camera,
    size: Size,
    dpr: f64,
}

impl Viewport {
    /// A viewport of `size` logical pixels at device pixel ratio `dpr`, looking at the origin with zoom 1.
    /// A bad size becomes 1x1 and a bad ratio becomes 1.
    pub fn new(size: Size, dpr: f64) -> Self {
        let mut v = Viewport { camera: Camera::HOME, size: Size::new(1.0, 1.0), dpr: 1.0 };
        v.set_size(size);
        v.set_dpr(dpr);
        v
    }

    /// The camera.
    pub fn camera(&self) -> Camera {
        self.camera
    }

    /// The world point at the centre of the viewport.
    pub fn scroll(&self) -> Point {
        self.camera.scroll
    }

    /// Logical pixels per world unit.
    pub fn zoom(&self) -> f64 {
        self.camera.zoom
    }

    /// The size in logical pixels.
    pub fn size(&self) -> Size {
        self.size
    }

    /// Device pixels per logical pixel.
    pub fn dpr(&self) -> f64 {
        self.dpr
    }

    /// Sets the camera, clamping the zoom. Returns true when the camera changed. A non-finite camera is refused.
    pub fn set_camera(&mut self, camera: Camera) -> bool {
        if !camera.is_finite() {
            return false;
        }
        let next = Camera { scroll: camera.scroll, zoom: clamp_zoom(camera.zoom) };
        let changed = next != self.camera;
        self.camera = next;
        changed
    }

    /// Resizes the canvas, keeping the scroll point at the centre. Returns true when the size changed. A size that
    /// is not finite or smaller than one pixel is refused.
    pub fn set_size(&mut self, size: Size) -> bool {
        if !(size.width.is_finite() && size.height.is_finite() && size.width >= 1.0 && size.height >= 1.0) {
            return false;
        }
        let changed = size != self.size;
        self.size = size;
        changed
    }

    /// Sets the device pixel ratio. Returns true when it changed; a ratio that is not positive and finite is refused.
    pub fn set_dpr(&mut self, dpr: f64) -> bool {
        if !(dpr.is_finite() && dpr > 0.0) {
            return false;
        }
        let changed = dpr != self.dpr;
        self.dpr = dpr;
        changed
    }

    fn half(&self) -> Vec2 {
        Vec2::new(self.size.width * 0.5, self.size.height * 0.5)
    }

    /// World point to logical screen pixels.
    pub fn world_to_screen(&self, p: Point) -> Point {
        let c = self.camera;
        Point::new(
            (p.x - c.scroll.x) * c.zoom + self.size.width * 0.5,
            (p.y - c.scroll.y) * c.zoom + self.size.height * 0.5,
        )
    }

    /// Logical screen pixels to a world point.
    pub fn screen_to_world(&self, p: Point) -> Point {
        let c = self.camera;
        Point::new(
            (p.x - self.size.width * 0.5) / c.zoom + c.scroll.x,
            (p.y - self.size.height * 0.5) / c.zoom + c.scroll.y,
        )
    }

    /// The affine that maps world points to logical screen pixels.
    pub fn world_to_screen_affine(&self) -> Affine {
        Affine::translate(self.half())
            * Affine::scale(self.camera.zoom)
            * Affine::translate(-self.camera.scroll.to_vec2())
    }

    /// The affine that maps logical screen pixels to world points.
    pub fn screen_to_world_affine(&self) -> Affine {
        self.world_to_screen_affine().inverse()
    }

    /// The affine the GPU draws with: world to device pixels (logical pixels times the device pixel ratio).
    pub fn world_to_device(&self) -> Affine {
        Affine::scale(self.dpr) * self.world_to_screen_affine()
    }

    /// A length in logical pixels as world units.
    pub fn px_to_world(&self, px: f64) -> f64 {
        px / self.camera.zoom
    }

    /// A length in world units as logical pixels.
    pub fn world_to_px(&self, len: f64) -> f64 {
        len * self.camera.zoom
    }

    /// The world rectangle on screen.
    pub fn visible_rect(&self) -> Rect {
        let a = self.screen_to_world(Point::ZERO);
        let b = self.screen_to_world(Point::new(self.size.width, self.size.height));
        Rect::new(a.x, a.y, b.x, b.y)
    }

    /// The visible world rectangle grown by `px` logical pixels on every side (for culling with a margin).
    pub fn visible_rect_padded(&self, px: f64) -> Rect {
        let pad = if px.is_finite() { px.max(0.0) / self.camera.zoom } else { 0.0 };
        self.visible_rect().inflate(pad, pad)
    }

    /// True when a world rectangle touches the visible area.
    pub fn is_visible(&self, world: &Rect) -> bool {
        drafftink_geom::aabb::intersects(&self.visible_rect(), world)
    }

    /// Moves the content by `delta` logical pixels (a drag to the right moves the scene to the right).
    pub fn pan_by_screen(&mut self, delta: Vec2) -> bool {
        if !(delta.x.is_finite() && delta.y.is_finite()) || delta == Vec2::ZERO {
            return false;
        }
        let z = self.camera.zoom;
        self.set_camera(Camera { scroll: self.camera.scroll - delta / z, zoom: z })
    }

    /// Moves the camera by `delta` world units.
    pub fn pan_by_world(&mut self, delta: Vec2) -> bool {
        if !(delta.x.is_finite() && delta.y.is_finite()) || delta == Vec2::ZERO {
            return false;
        }
        self.set_camera(Camera { scroll: self.camera.scroll + delta, zoom: self.camera.zoom })
    }

    /// Puts a world point at the centre of the viewport.
    pub fn center_on(&mut self, world: Point) -> bool {
        self.set_camera(Camera { scroll: world, zoom: self.camera.zoom })
    }

    /// Multiplies the zoom by `factor`, keeping the world point under `screen` where it is. The result is clamped
    /// to [`MIN_ZOOM`], [`MAX_ZOOM`]; at the limit the camera does not move. Returns true when the camera changed.
    pub fn zoom_at(&mut self, screen: Point, factor: f64) -> bool {
        if !(factor.is_finite() && factor > 0.0) {
            return false;
        }
        self.set_zoom_at(screen, self.camera.zoom * factor)
    }

    /// Sets the zoom, keeping the world point under `screen` where it is.
    pub fn set_zoom_at(&mut self, screen: Point, zoom: f64) -> bool {
        if !(zoom.is_finite() && zoom > 0.0 && screen.x.is_finite() && screen.y.is_finite()) {
            return false;
        }
        let z = clamp_zoom(zoom);
        let anchor = self.screen_to_world(screen);
        let scroll = Point::new(
            anchor.x - (screen.x - self.size.width * 0.5) / z,
            anchor.y - (screen.y - self.size.height * 0.5) / z,
        );
        self.set_camera(Camera { scroll, zoom: z })
    }

    /// Sets the zoom around the centre of the viewport.
    pub fn set_zoom(&mut self, zoom: f64) -> bool {
        let c = Point::new(self.size.width * 0.5, self.size.height * 0.5);
        self.set_zoom_at(c, zoom)
    }

    /// Fits a world rectangle into the viewport: centres it and picks the largest zoom that shows all of it with
    /// `padding` logical pixels free on every side. `max_zoom` keeps a tiny rectangle from being blown up. A
    /// rectangle with no width or height fits by the other side; a single point only centres. Returns true when the
    /// camera changed.
    pub fn fit_bounds(&mut self, bounds: Rect, padding: f64, max_zoom: Option<f64>) -> bool {
        if !drafftink_geom::aabb::is_finite(&bounds) {
            return false;
        }
        let b = drafftink_geom::aabb::normalize(bounds);
        let pad = if padding.is_finite() { padding.max(0.0) } else { 0.0 };
        let avail_w = (self.size.width - 2.0 * pad).max(1.0);
        let avail_h = (self.size.height - 2.0 * pad).max(1.0);
        let mut zoom = f64::INFINITY;
        if b.width() > 0.0 {
            zoom = zoom.min(avail_w / b.width());
        }
        if b.height() > 0.0 {
            zoom = zoom.min(avail_h / b.height());
        }
        if !zoom.is_finite() {
            zoom = self.camera.zoom;
        }
        let cap = max_zoom.filter(|m| m.is_finite() && *m > 0.0).unwrap_or(MAX_ZOOM);
        let zoom = clamp_zoom(zoom.min(cap));
        self.set_camera(Camera { scroll: b.center(), zoom })
    }

    /// Moves the camera the least it takes to show `bounds` with `padding` free pixels. Does nothing when the
    /// rectangle is already inside. Does not change the zoom, so a rectangle larger than the viewport is aligned
    /// by its top-left corner.
    pub fn scroll_into_view(&mut self, bounds: Rect, padding: f64) -> bool {
        if !drafftink_geom::aabb::is_finite(&bounds) {
            return false;
        }
        let b = drafftink_geom::aabb::normalize(bounds);
        let pad = if padding.is_finite() { padding.max(0.0) } else { 0.0 } / self.camera.zoom;
        let v = self.visible_rect().inflate(-pad, -pad);
        let dx = if b.x0 < v.x0 {
            b.x0 - v.x0
        } else if b.x1 > v.x1 {
            (b.x1 - v.x1).min(b.x0 - v.x0)
        } else {
            0.0
        };
        let dy = if b.y0 < v.y0 {
            b.y0 - v.y0
        } else if b.y1 > v.y1 {
            (b.y1 - v.y1).min(b.y0 - v.y0)
        } else {
            0.0
        };
        self.pan_by_world(Vec2::new(dx, dy))
    }
}

impl Default for Viewport {
    fn default() -> Self {
        Viewport::new(Size::new(800.0, 600.0), 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vp() -> Viewport {
        Viewport::new(Size::new(800.0, 600.0), 2.0)
    }

    fn close(a: Point, b: Point) -> bool {
        (a - b).hypot() < 1e-9
    }

    #[test]
    fn round_trip_and_centre() {
        let mut v = vp();
        v.set_camera(Camera { scroll: Point::new(100.0, -50.0), zoom: 2.5 });
        let w = Point::new(13.0, 77.0);
        assert!(close(v.screen_to_world(v.world_to_screen(w)), w));
        assert!(close(v.world_to_screen(Point::new(100.0, -50.0)), Point::new(400.0, 300.0)));
        assert!(close(v.world_to_screen_affine() * w, v.world_to_screen(w)));
        assert!(close(v.screen_to_world_affine() * v.world_to_screen(w), w));
        assert!(close(v.world_to_device() * w, Point::new(v.world_to_screen(w).x * 2.0, v.world_to_screen(w).y * 2.0)));
    }

    #[test]
    fn zoom_keeps_the_anchor_fixed() {
        let mut v = vp();
        v.pan_by_screen(Vec2::new(33.0, -12.0));
        let at = Point::new(123.0, 456.0);
        let before = v.screen_to_world(at);
        assert!(v.zoom_at(at, 1.7));
        assert!(close(v.screen_to_world(at), before));
        assert!((v.zoom() - 1.7).abs() < 1e-12);
    }

    #[test]
    fn zoom_is_clamped_and_stops_at_the_limit() {
        let mut v = vp();
        assert!(v.zoom_at(Point::new(10.0, 10.0), 1e9));
        assert_eq!(v.zoom(), MAX_ZOOM);
        let cam = v.camera();
        assert!(!v.zoom_at(Point::new(10.0, 10.0), 2.0), "no change at the limit");
        assert_eq!(v.camera(), cam);
        v.zoom_at(Point::new(10.0, 10.0), 1e-9);
        assert_eq!(v.zoom(), MIN_ZOOM);
    }

    #[test]
    fn pan_moves_the_content_with_the_pointer() {
        let mut v = vp();
        v.set_camera(Camera { scroll: Point::ZERO, zoom: 2.0 });
        let w = Point::new(5.0, 5.0);
        let s0 = v.world_to_screen(w);
        v.pan_by_screen(Vec2::new(10.0, -4.0));
        assert!(close(v.world_to_screen(w), s0 + Vec2::new(10.0, -4.0)));
    }

    #[test]
    fn fit_bounds_centres_and_leaves_padding() {
        let mut v = vp();
        v.fit_bounds(Rect::new(0.0, 0.0, 1000.0, 100.0), 50.0, None);
        assert!((v.zoom() - 0.7).abs() < 1e-12, "(800 - 100) / 1000");
        assert!(close(v.scroll(), Point::new(500.0, 50.0)));
        let r = v.visible_rect();
        assert!(r.x0 < 0.0 && r.x1 > 1000.0);
        v.fit_bounds(Rect::new(0.0, 0.0, 1.0, 1.0), 0.0, Some(3.0));
        assert_eq!(v.zoom(), 3.0, "max_zoom caps a tiny rect");
        v.fit_bounds(Rect::new(5.0, 5.0, 5.0, 5.0), 10.0, None);
        assert!(close(v.scroll(), Point::new(5.0, 5.0)));
        assert!(v.zoom() <= MAX_ZOOM);
    }

    #[test]
    fn scroll_into_view_moves_the_least() {
        let mut v = vp();
        assert!(!v.scroll_into_view(Rect::new(-10.0, -10.0, 10.0, 10.0), 0.0));
        assert!(v.scroll_into_view(Rect::new(500.0, 0.0, 600.0, 10.0), 0.0));
        assert!((v.visible_rect().x1 - 600.0).abs() < 1e-9);
    }

    #[test]
    fn hostile_input_is_refused() {
        let mut v = vp();
        let cam = v.camera();
        assert!(!v.zoom_at(Point::new(f64::NAN, 0.0), 2.0));
        assert!(!v.zoom_at(Point::ZERO, f64::NAN));
        assert!(!v.zoom_at(Point::ZERO, -1.0));
        assert!(!v.pan_by_screen(Vec2::new(f64::INFINITY, 0.0)));
        assert!(!v.set_camera(Camera { scroll: Point::new(f64::NAN, 0.0), zoom: 1.0 }));
        assert!(!v.fit_bounds(Rect::new(f64::NAN, 0.0, 1.0, 1.0), 0.0, None));
        assert!(!v.set_size(Size::new(0.0, 100.0)));
        assert!(!v.set_dpr(0.0));
        assert_eq!(v.camera(), cam);
        assert_eq!(clamp_zoom(f64::NAN), 1.0);
    }

    #[test]
    fn camera_lerp_is_even_in_zoom() {
        let a = Camera { scroll: Point::ZERO, zoom: 1.0 };
        let b = Camera { scroll: Point::new(10.0, 0.0), zoom: 4.0 };
        let m = a.lerp(b, 0.5);
        assert!((m.zoom - 2.0).abs() < 1e-9);
        assert!((m.scroll.x - 5.0).abs() < 1e-9);
        assert_eq!(a.lerp(b, 2.0), b);
    }
}
