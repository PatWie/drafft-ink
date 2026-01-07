//! Text shape.

use super::{ShapeId, ShapeStyle, ShapeTrait};
use kurbo::{Affine, BezPath, Point, Rect};
use serde::{Deserialize, Serialize};
use std::sync::RwLock;
use uuid::Uuid;

/// Font family options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FontFamily {
    /// GelPen hand-drawn style font (default).
    #[default]
    GelPen,
    /// Vanilla Extract handwritten font.
    VanillaExtract,
    /// GelPen Serif handwritten font.
    GelPenSerif,
}

impl FontFamily {
    /// Get the font family name as used by the renderer.
    pub fn name(&self) -> &'static str {
        match self {
            FontFamily::GelPen => "GelPen",
            FontFamily::VanillaExtract => "Vanilla Extract",
            FontFamily::GelPenSerif => "GelPen Serif",
        }
    }

    /// Get display name for UI.
    pub fn display_name(&self) -> &'static str {
        match self {
            FontFamily::GelPen => "GelPen",
            FontFamily::VanillaExtract => "Vanilla",
            FontFamily::GelPenSerif => "GelPen Serif",
        }
    }

    /// Get all available font families.
    pub fn all() -> &'static [FontFamily] {
        &[
            FontFamily::GelPen,
            FontFamily::VanillaExtract,
            FontFamily::GelPenSerif,
        ]
    }
}

/// Font weight options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FontWeight {
    /// Light weight.
    Light,
    /// Regular weight (default).
    #[default]
    Regular,
    /// Heavy/Bold weight.
    Heavy,
}

impl FontWeight {
    /// Get display name for UI.
    pub fn display_name(&self) -> &'static str {
        match self {
            FontWeight::Light => "Light",
            FontWeight::Regular => "Regular",
            FontWeight::Heavy => "Heavy",
        }
    }

    /// Get all available font weights.
    pub fn all() -> &'static [FontWeight] {
        &[FontWeight::Light, FontWeight::Regular, FontWeight::Heavy]
    }
}

/// A text shape.
#[derive(Debug, Serialize, Deserialize)]
pub struct Text {
    pub(crate) id: ShapeId,
    /// Position (top-left corner of text bounding box).
    pub position: Point,
    /// The text content.
    pub content: String,
    /// Font size in pixels.
    pub font_size: f64,
    /// Font family.
    pub font_family: FontFamily,
    /// Font weight.
    pub font_weight: FontWeight,
    /// Rotation angle in radians (around center).
    #[serde(default)]
    pub rotation: f64,
    /// Style properties.
    pub style: ShapeStyle,
    /// Cached layout size (width, height) computed by the renderer.
    /// This is set after text layout and provides accurate bounds.
    /// Uses RwLock for thread-safe interior mutability.
    /// If None, approximate bounds are used.
    #[serde(skip)]
    cached_size: RwLock<Option<(f64, f64)>>,
}

impl Clone for Text {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            position: self.position,
            content: self.content.clone(),
            font_size: self.font_size,
            font_family: self.font_family,
            font_weight: self.font_weight,
            rotation: self.rotation,
            style: self.style.clone(),
            // Clone the cached size value, not the lock
            cached_size: RwLock::new(self.cached_size.read().ok().and_then(|guard| *guard)),
        }
    }
}

impl Text {
    /// Default font size (M = Medium).
    pub const DEFAULT_FONT_SIZE: f64 = 20.0;

    /// Create a new text shape.
    pub fn new(position: Point, content: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            position,
            content,
            font_size: Self::DEFAULT_FONT_SIZE,
            font_family: FontFamily::default(),
            font_weight: FontWeight::default(),
            rotation: 0.0,
            style: ShapeStyle::default(),
            cached_size: RwLock::new(None),
        }
    }

    /// Set the cached layout size (computed by the renderer).
    /// Uses interior mutability so this can be called during rendering.
    pub fn set_cached_size(&self, width: f64, height: f64) {
        if let Ok(mut cache) = self.cached_size.write() {
            *cache = Some((width, height));
        }
    }

    /// Clear the cached size (call when text properties change).
    pub fn invalidate_cache(&self) {
        if let Ok(mut cache) = self.cached_size.write() {
            *cache = None;
        }
    }

    /// Reconstruct a text with a specific ID (for CRDT/storage).
    pub(crate) fn reconstruct(
        id: ShapeId,
        position: Point,
        content: String,
        font_size: f64,
        font_family: FontFamily,
        font_weight: FontWeight,
        rotation: f64,
        style: ShapeStyle,
    ) -> Self {
        Self {
            id,
            position,
            content,
            font_size,
            font_family,
            font_weight,
            rotation,
            style,
            cached_size: RwLock::new(None),
        }
    }

    /// Create a new text shape with font size.
    pub fn with_font_size(mut self, size: f64) -> Self {
        self.font_size = size;
        self
    }

    /// Set the font family.
    pub fn with_font_family(mut self, family: FontFamily) -> Self {
        self.font_family = family;
        self
    }

    /// Set the font weight.
    pub fn with_font_weight(mut self, weight: FontWeight) -> Self {
        self.font_weight = weight;
        self
    }

    /// Set the text content.
    pub fn set_content(&mut self, content: String) {
        self.content = content;
        self.invalidate_cache();
    }

    /// Get the text content.
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Approximate width based on character count and font size.
    /// This is a rough estimate; actual width depends on the font.
    fn approximate_width(&self) -> f64 {
        // For multi-line text, find the widest line
        let max_line_len = self
            .content
            .lines()
            .map(|line| line.len())
            .max()
            .unwrap_or(0);

        // Average character width varies by font family and weight
        // These values are empirically determined approximations
        let char_width_factor = match (&self.font_family, &self.font_weight) {
            // GelPen is a handwritten-style font with medium width
            (FontFamily::GelPen, FontWeight::Light) => 0.50,
            (FontFamily::GelPen, FontWeight::Regular) => 0.55,
            (FontFamily::GelPen, FontWeight::Heavy) => 0.60,
            // Vanilla Extract is a handwritten font
            (FontFamily::VanillaExtract, _) => 0.50,
            // GelPen Serif is a handwritten font with wider characters
            (FontFamily::GelPenSerif, FontWeight::Light) => 0.55,
            (FontFamily::GelPenSerif, FontWeight::Regular) => 0.58,
            (FontFamily::GelPenSerif, FontWeight::Heavy) => 0.60,
        };

        max_line_len as f64 * self.font_size * char_width_factor
    }

    /// Approximate height based on font size and number of lines.
    fn approximate_height(&self) -> f64 {
        // Count lines (empty content = 1 line)
        let line_count = self.content.lines().count().max(1);
        // Add 1 if content ends with newline (lines() doesn't count trailing empty line)
        let line_count = if self.content.ends_with('\n') {
            line_count + 1
        } else {
            line_count
        };
        // Line height is typically 1.2 * font_size
        line_count as f64 * self.font_size * 1.2
    }
}

impl ShapeTrait for Text {
    fn id(&self) -> ShapeId {
        self.id
    }

    fn bounds(&self) -> Rect {
        // Use cached size if available, otherwise approximate
        let (width, height) = self
            .cached_size
            .read()
            .ok()
            .and_then(|guard| *guard)
            .map(|(w, h)| (w.max(20.0), h))
            .unwrap_or_else(|| {
                (
                    self.approximate_width().max(20.0),
                    self.approximate_height(),
                )
            });
        // Position is top-left corner
        Rect::new(
            self.position.x,
            self.position.y,
            self.position.x + width,
            self.position.y + height,
        )
    }

    fn hit_test(&self, point: Point, tolerance: f64) -> bool {
        let bounds = self.bounds().inflate(tolerance, tolerance);
        bounds.contains(point)
    }

    fn to_path(&self) -> BezPath {
        // Text doesn't have a simple path representation
        // Return the bounding box as a path for selection purposes
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
        // Scale font size if there's uniform scaling
        let coeffs = affine.as_coeffs();
        let scale = (coeffs[0].abs() + coeffs[3].abs()) / 2.0;
        if (scale - 1.0).abs() > 0.01 {
            self.font_size *= scale;
        }
    }

    fn clone_box(&self) -> Box<dyn ShapeTrait + Send + Sync> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_creation() {
        let text = Text::new(Point::new(100.0, 100.0), "Hello".to_string());
        assert_eq!(text.content(), "Hello");
        assert!((text.font_size - Text::DEFAULT_FONT_SIZE).abs() < f64::EPSILON); // Default is 20.0 (M size)
    }

    #[test]
    fn test_text_with_font_size() {
        let text = Text::new(Point::new(0.0, 0.0), "Test".to_string()).with_font_size(32.0);
        assert!((text.font_size - 32.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_hit_test() {
        let text = Text::new(Point::new(100.0, 100.0), "Hello World".to_string());
        let bounds = text.bounds();
        let center = Point::new((bounds.x0 + bounds.x1) / 2.0, (bounds.y0 + bounds.y1) / 2.0);
        assert!(text.hit_test(center, 0.0));
        assert!(!text.hit_test(Point::new(0.0, 0.0), 0.0));
    }

    #[test]
    fn test_bounds() {
        let text = Text::new(Point::new(100.0, 100.0), "Hi".to_string());
        let bounds = text.bounds();
        assert!(bounds.width() > 0.0);
        assert!(bounds.height() > 0.0);
    }
}
