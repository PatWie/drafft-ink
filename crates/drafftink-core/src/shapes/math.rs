//! Math shape for LaTeX equations.

use super::{ShapeId, ShapeStyle, ShapeTrait};
use kurbo::{Affine, BezPath, Point, Rect};
use serde::{Deserialize, Serialize};
use std::sync::RwLock;
use uuid::Uuid;

/// A math equation shape (LaTeX).
#[derive(Debug, Serialize, Deserialize)]
pub struct Math {
    pub(crate) id: ShapeId,
    /// Position (baseline origin).
    pub position: Point,
    /// LaTeX source.
    pub latex: String,
    /// Font size in pixels.
    pub font_size: f64,
    /// Rotation angle in radians (around center).
    #[serde(default)]
    pub rotation: f64,
    /// Style properties.
    pub style: ShapeStyle,
    /// Cached layout size (width, height, depth) from renderer.
    #[serde(skip)]
    cached_size: RwLock<Option<(f64, f64, f64)>>,
}

impl Clone for Math {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            position: self.position,
            latex: self.latex.clone(),
            font_size: self.font_size,
            rotation: self.rotation,
            style: self.style.clone(),
            cached_size: RwLock::new(self.cached_size.read().ok().and_then(|g| *g)),
        }
    }
}

impl Math {
    pub const DEFAULT_FONT_SIZE: f64 = 20.0;

    pub fn new(position: Point, latex: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            position,
            latex,
            font_size: Self::DEFAULT_FONT_SIZE,
            rotation: 0.0,
            style: ShapeStyle::default(),
            cached_size: RwLock::new(None),
        }
    }

    pub(crate) fn reconstruct(
        id: ShapeId,
        position: Point,
        latex: String,
        font_size: f64,
        rotation: f64,
        style: ShapeStyle,
    ) -> Self {
        Self {
            id,
            position,
            latex,
            font_size,
            rotation,
            style,
            cached_size: RwLock::new(None),
        }
    }

    pub fn set_cached_size(&self, width: f64, height: f64, depth: f64) {
        if let Ok(mut cache) = self.cached_size.write() {
            *cache = Some((width, height, depth));
        }
    }

    pub fn cached_size(&self) -> Option<(f64, f64, f64)> {
        self.cached_size.read().ok().and_then(|g| *g)
    }

    pub fn invalidate_cache(&self) {
        if let Ok(mut cache) = self.cached_size.write() {
            *cache = None;
        }
    }

    pub fn set_latex(&mut self, latex: String) {
        self.latex = latex;
        self.invalidate_cache();
    }
}

impl ShapeTrait for Math {
    fn id(&self) -> ShapeId {
        self.id
    }

    fn bounds(&self) -> Rect {
        let (width, height, depth) = self.cached_size.read().ok().and_then(|g| *g).unwrap_or((
            self.latex.len() as f64 * self.font_size * 0.5,
            self.font_size,
            self.font_size * 0.3,
        ));

        // height is positive (above baseline), depth is negative (below baseline)
        let unrotated = Rect::new(
            self.position.x,
            self.position.y - height,
            self.position.x + width,
            self.position.y - depth,
        );

        if self.rotation.abs() < 0.001 {
            return unrotated;
        }

        // Rotate around center and compute axis-aligned bounding box
        let center = Point::new(unrotated.center().x, unrotated.center().y);
        let corners = [
            Point::new(unrotated.x0, unrotated.y0),
            Point::new(unrotated.x1, unrotated.y0),
            Point::new(unrotated.x1, unrotated.y1),
            Point::new(unrotated.x0, unrotated.y1),
        ];
        let rot = Affine::rotate_about(self.rotation, center);
        let rotated: Vec<Point> = corners.iter().map(|&p| rot * p).collect();

        let min_x = rotated.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
        let max_x = rotated
            .iter()
            .map(|p| p.x)
            .fold(f64::NEG_INFINITY, f64::max);
        let min_y = rotated.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
        let max_y = rotated
            .iter()
            .map(|p| p.y)
            .fold(f64::NEG_INFINITY, f64::max);

        Rect::new(min_x, min_y, max_x, max_y)
    }

    fn hit_test(&self, point: Point, tolerance: f64) -> bool {
        self.bounds().inflate(tolerance, tolerance).contains(point)
    }

    fn to_path(&self) -> BezPath {
        let bounds = self.bounds();
        let mut path = BezPath::new();
        path.move_to(Point::new(bounds.x0, bounds.y0));
        path.line_to(Point::new(bounds.x1, bounds.y0));
        path.line_to(Point::new(bounds.x1, bounds.y1));
        path.line_to(Point::new(bounds.x0, bounds.y1));
        path.close_path();
        path
    }

    fn style(&self) -> &ShapeStyle {
        &self.style
    }

    fn style_mut(&mut self) -> &mut ShapeStyle {
        &mut self.style
    }

    fn transform(&mut self, affine: Affine) {
        self.position = affine * self.position;
        let coeffs = affine.as_coeffs();
        let scale = (coeffs[0].abs() + coeffs[3].abs()) / 2.0;
        if (scale - 1.0).abs() > 0.01 {
            self.font_size *= scale;
            self.invalidate_cache();
        }
        // Extract rotation from affine
        let rotation = coeffs[1].atan2(coeffs[0]);
        if rotation.abs() > 0.001 {
            self.rotation += rotation;
        }
    }

    fn clone_box(&self) -> Box<dyn ShapeTrait + Send + Sync> {
        Box::new(self.clone())
    }
}
