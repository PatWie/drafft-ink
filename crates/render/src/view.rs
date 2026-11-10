//! The view: what part of the world is on screen and how it maps to device pixels.

use kurbo::{Affine, Rect, Size, Vec2};

/// Camera and viewport.
///
/// A world point `p` is at `p * zoom + offset` in logical pixels, and the device draws `dpi` device pixels per
/// logical pixel. This is the old app's camera (`offset`, `zoom`) plus the device pixel ratio.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// Scale from world units to logical pixels.
    pub zoom: f64,
    /// Where the world origin is, in logical pixels from the top-left of the viewport.
    pub offset: Vec2,
    /// Viewport size in logical pixels.
    pub viewport: Size,
    /// Device pixels per logical pixel.
    pub dpi: f64,
}

/// Zoom below this is treated as this, so the maths stays finite for hostile input.
const MIN_ZOOM: f64 = 1.0e-9;

impl View {
    /// A view with a device pixel ratio of 1.
    pub fn new(zoom: f64, offset: Vec2, viewport: Size) -> Self {
        Self { zoom, offset, viewport, dpi: 1.0 }
    }

    /// The same view with another device pixel ratio.
    #[must_use]
    pub fn with_dpi(mut self, dpi: f64) -> Self {
        self.dpi = dpi;
        self
    }

    /// A view that shows `world` filling the viewport (centred, aspect kept) with `padding` logical pixels around.
    pub fn fit(world: Rect, viewport: Size, padding: f64) -> Self {
        let avail_w = (viewport.width - 2.0 * padding).max(1.0);
        let avail_h = (viewport.height - 2.0 * padding).max(1.0);
        let zoom = (avail_w / world.width().max(1e-9)).min(avail_h / world.height().max(1e-9));
        let zoom = if zoom.is_finite() && zoom > 0.0 { zoom } else { 1.0 };
        let c = world.center();
        let offset = Vec2::new(viewport.width / 2.0 - c.x * zoom, viewport.height / 2.0 - c.y * zoom);
        Self { zoom, offset, viewport, dpi: 1.0 }
    }

    fn z(&self) -> f64 {
        if self.zoom.is_finite() { self.zoom.max(MIN_ZOOM) } else { 1.0 }
    }

    fn d(&self) -> f64 {
        if self.dpi.is_finite() && self.dpi > 0.0 { self.dpi } else { 1.0 }
    }

    /// Device pixels per world unit.
    pub fn scale(&self) -> f64 {
        self.z() * self.d()
    }

    /// World space to device pixels.
    pub fn world_to_device(&self) -> Affine {
        Affine::scale(self.d()) * Affine::translate(self.offset) * Affine::scale(self.z())
    }

    /// Logical pixels to device pixels.
    pub fn logical_to_device(&self) -> Affine {
        Affine::scale(self.d())
    }

    /// The viewport in device pixels.
    pub fn device_size(&self) -> Size {
        Size::new(self.viewport.width * self.d(), self.viewport.height * self.d())
    }

    /// The visible part of the world.
    pub fn world_rect(&self) -> Rect {
        let z = self.z();
        Rect::new(
            -self.offset.x / z,
            -self.offset.y / z,
            (self.viewport.width - self.offset.x) / z,
            (self.viewport.height - self.offset.y) / z,
        )
    }

    /// True when every number is finite and the viewport has an area.
    pub fn is_valid(&self) -> bool {
        self.zoom.is_finite()
            && self.zoom > 0.0
            && self.offset.x.is_finite()
            && self.offset.y.is_finite()
            && self.viewport.width.is_finite()
            && self.viewport.height.is_finite()
            && self.viewport.width > 0.0
            && self.viewport.height > 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Point;

    #[test]
    fn world_rect_matches_the_transform() {
        let v = View::new(2.0, Vec2::new(30.0, -10.0), Size::new(800.0, 600.0)).with_dpi(2.0);
        let r = v.world_rect();
        let inv = v.world_to_device().inverse();
        let tl = inv * Point::new(0.0, 0.0);
        let br = inv * Point::new(1600.0, 1200.0);
        assert!((tl.x - r.x0).abs() < 1e-9 && (tl.y - r.y0).abs() < 1e-9);
        assert!((br.x - r.x1).abs() < 1e-9 && (br.y - r.y1).abs() < 1e-9);
    }

    #[test]
    fn fit_shows_the_whole_rect() {
        let world = Rect::new(-100.0, -50.0, 300.0, 250.0);
        let v = View::fit(world, Size::new(800.0, 600.0), 20.0);
        let vis = v.world_rect();
        assert!(vis.contains_rect(world));
    }
}
