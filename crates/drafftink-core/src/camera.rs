//! Camera module for pan/zoom transforms.

use kurbo::{Affine, Point, Vec2};
use serde::{Deserialize, Serialize};

/// Base zoom level that corresponds to "100%" in the UI.
/// This makes fonts and strokes appear at a comfortable default size.
pub const BASE_ZOOM: f64 = 1.68;

/// Camera manages the view transform for the canvas.
///
/// It handles panning (translation) and zooming (scaling) operations,
/// converting between screen coordinates and world coordinates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Camera {
    /// Current translation offset (pan)
    pub offset: Vec2,
    /// Current zoom level (BASE_ZOOM = 100% in UI)
    pub zoom: f64,
    /// Minimum allowed zoom level
    pub min_zoom: f64,
    /// Maximum allowed zoom level
    pub max_zoom: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            offset: Vec2::ZERO,
            zoom: BASE_ZOOM,
            min_zoom: 0.1,
            max_zoom: 10.0,
        }
    }
}

impl Camera {
    /// Create a new camera with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the affine transform for rendering.
    ///
    /// This transform converts world coordinates to screen coordinates.
    pub fn transform(&self) -> Affine {
        Affine::translate(self.offset) * Affine::scale(self.zoom)
    }

    /// Get the inverse transform for input handling.
    ///
    /// This transform converts screen coordinates to world coordinates.
    pub fn inverse_transform(&self) -> Affine {
        Affine::scale(1.0 / self.zoom) * Affine::translate(-self.offset)
    }

    /// Convert a screen point to world coordinates.
    pub fn screen_to_world(&self, screen_point: Point) -> Point {
        self.inverse_transform() * screen_point
    }

    /// Convert a world point to screen coordinates.
    pub fn world_to_screen(&self, world_point: Point) -> Point {
        self.transform() * world_point
    }

    /// Pan the camera by a delta in screen coordinates.
    pub fn pan(&mut self, delta: Vec2) {
        self.offset += delta;
    }

    /// Zoom the camera, keeping the given screen point fixed.
    pub fn zoom_at(&mut self, screen_point: Point, factor: f64) {
        let new_zoom = (self.zoom * factor).clamp(self.min_zoom, self.max_zoom);
        if (new_zoom - self.zoom).abs() < f64::EPSILON {
            return;
        }

        // Convert screen point to world before zoom
        let world_point = self.screen_to_world(screen_point);

        // Apply new zoom
        self.zoom = new_zoom;

        // Adjust offset so world_point stays at screen_point
        let new_screen = self.world_to_screen(world_point);
        let correction = Vec2::new(screen_point.x - new_screen.x, screen_point.y - new_screen.y);
        self.offset += correction;
    }

    /// Reset camera to default position and zoom.
    pub fn reset(&mut self) {
        self.offset = Vec2::ZERO;
        self.zoom = BASE_ZOOM;
    }

    /// Fit the camera to show the given bounding box.
    pub fn fit_to_bounds(&mut self, bounds: kurbo::Rect, viewport: kurbo::Size, padding: f64) {
        if bounds.is_zero_area() {
            self.reset();
            return;
        }

        let padded_viewport = kurbo::Size::new(
            (viewport.width - padding * 2.0).max(1.0),
            (viewport.height - padding * 2.0).max(1.0),
        );

        let scale_x = padded_viewport.width / bounds.width();
        let scale_y = padded_viewport.height / bounds.height();
        self.zoom = scale_x.min(scale_y).clamp(self.min_zoom, self.max_zoom);

        // Center the bounds in the viewport
        let bounds_center = bounds.center();
        let viewport_center = Point::new(viewport.width / 2.0, viewport.height / 2.0);

        self.offset = Vec2::new(
            viewport_center.x - bounds_center.x * self.zoom,
            viewport_center.y - bounds_center.y * self.zoom,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_camera() {
        let camera = Camera::new();
        assert_eq!(camera.offset, Vec2::ZERO);
        assert!((camera.zoom - BASE_ZOOM).abs() < f64::EPSILON);
    }

    #[test]
    fn test_screen_to_world_identity() {
        let camera = Camera::new();
        let screen = Point::new(100.0, 200.0);
        let world = camera.screen_to_world(screen);
        // With BASE_ZOOM, screen coords are divided by zoom
        let expected_x = screen.x / BASE_ZOOM;
        let expected_y = screen.y / BASE_ZOOM;
        assert!((world.x - expected_x).abs() < f64::EPSILON);
        assert!((world.y - expected_y).abs() < f64::EPSILON);
    }

    #[test]
    fn test_screen_to_world_with_offset() {
        let mut camera = Camera::new();
        camera.offset = Vec2::new(50.0, 100.0);
        let screen = Point::new(100.0, 200.0);
        let world = camera.screen_to_world(screen);
        // screen_to_world: (screen - offset) / zoom
        let expected_x = (screen.x - 50.0) / BASE_ZOOM;
        let expected_y = (screen.y - 100.0) / BASE_ZOOM;
        assert!((world.x - expected_x).abs() < f64::EPSILON);
        assert!((world.y - expected_y).abs() < f64::EPSILON);
    }

    #[test]
    fn test_screen_to_world_with_zoom() {
        let mut camera = Camera::new();
        camera.zoom = 2.0;
        let screen = Point::new(100.0, 200.0);
        let world = camera.screen_to_world(screen);
        assert!((world.x - 50.0).abs() < f64::EPSILON);
        assert!((world.y - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_roundtrip_conversion() {
        let mut camera = Camera::new();
        camera.offset = Vec2::new(30.0, -20.0);
        camera.zoom = 1.5;

        let original = Point::new(123.0, 456.0);
        let world = camera.screen_to_world(original);
        let back = camera.world_to_screen(world);

        assert!((back.x - original.x).abs() < 1e-10);
        assert!((back.y - original.y).abs() < 1e-10);
    }

    #[test]
    fn test_zoom_clamp() {
        let mut camera = Camera::new();
        camera.zoom_at(Point::ZERO, 0.001); // Try to zoom way out
        assert!((camera.zoom - camera.min_zoom).abs() < f64::EPSILON);

        camera.zoom = 1.0;
        camera.zoom_at(Point::ZERO, 1000.0); // Try to zoom way in
        assert!((camera.zoom - camera.max_zoom).abs() < f64::EPSILON);
    }

    #[test]
    fn test_pan() {
        let mut camera = Camera::new();
        camera.pan(Vec2::new(10.0, 20.0));
        assert!((camera.offset.x - 10.0).abs() < f64::EPSILON);
        assert!((camera.offset.y - 20.0).abs() < f64::EPSILON);
    }
}
