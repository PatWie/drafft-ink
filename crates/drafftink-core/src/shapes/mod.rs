//! Shape definitions for the whiteboard.

mod arrow;
mod ellipse;
mod freehand;
mod group;
mod image;
mod line;
mod rectangle;
mod text;

pub use arrow::Arrow;
pub use ellipse::Ellipse;
pub use freehand::Freehand;
pub use group::Group;
pub use image::{Image, ImageFormat};
pub use line::Line;
pub use rectangle::Rectangle;
pub use text::{FontFamily, FontWeight, Text};

use kurbo::{Affine, BezPath, Point, Rect};
use peniko::Color;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Serializable color representation (RGBA8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SerializableColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl SerializableColor {
    pub fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub fn black() -> Self {
        Self::new(0, 0, 0, 255)
    }

    pub fn white() -> Self {
        Self::new(255, 255, 255, 255)
    }

    pub fn transparent() -> Self {
        Self::new(0, 0, 0, 0)
    }
}

impl From<Color> for SerializableColor {
    fn from(color: Color) -> Self {
        let rgba = color.to_rgba8();
        Self {
            r: rgba.r,
            g: rgba.g,
            b: rgba.b,
            a: rgba.a,
        }
    }
}

impl From<SerializableColor> for Color {
    fn from(color: SerializableColor) -> Self {
        Color::from_rgba8(color.r, color.g, color.b, color.a)
    }
}

/// Sloppiness level for hand-drawn effect.
/// Based on Excalidraw's roughness algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Sloppiness {
    /// No roughness - clean, precise lines (roughness = 0)
    Architect = 0,
    /// Medium roughness - slight hand-drawn feel (roughness = 1)
    #[default]
    Artist = 1,
    /// High roughness - very sketchy, cartoon-like (roughness = 2)
    Cartoonist = 2,
}

impl Sloppiness {
    /// Get the roughness value for this sloppiness level.
    pub fn roughness(&self) -> f64 {
        match self {
            Sloppiness::Architect => 0.0,
            Sloppiness::Artist => 1.0,
            Sloppiness::Cartoonist => 2.0,
        }
    }

    /// Cycle to the next sloppiness level.
    pub fn next(self) -> Self {
        match self {
            Sloppiness::Architect => Sloppiness::Artist,
            Sloppiness::Artist => Sloppiness::Cartoonist,
            Sloppiness::Cartoonist => Sloppiness::Architect,
        }
    }
}

/// Style properties for shapes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShapeStyle {
    /// Stroke color.
    pub stroke_color: SerializableColor,
    /// Stroke width.
    pub stroke_width: f64,
    /// Fill color (None = no fill).
    pub fill_color: Option<SerializableColor>,
    /// Sloppiness level for hand-drawn effect.
    pub sloppiness: Sloppiness,
    /// Random seed for hand-drawn effect (ensures consistent rendering across transforms).
    #[serde(default = "generate_seed")]
    pub seed: u32,
}

/// Generate a random seed for new shapes.
/// Uses a simple counter + hash approach that works on all platforms including WASM.
fn generate_seed() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};

    // Global counter for seed generation - ensures uniqueness even without time
    static SEED_COUNTER: AtomicU32 = AtomicU32::new(1);

    // Get next counter value
    let counter = SEED_COUNTER.fetch_add(1, Ordering::Relaxed);

    // Mix the counter with some constants for better distribution
    // Using a simple hash function (similar to splitmix32)
    let mut x = counter.wrapping_mul(0x9E3779B9);
    x ^= x >> 16;
    x = x.wrapping_mul(0x85EBCA6B);
    x ^= x >> 13;
    x = x.wrapping_mul(0xC2B2AE35);
    x ^= x >> 16;
    x
}

impl ShapeStyle {
    /// Get the stroke color as a peniko Color.
    pub fn stroke(&self) -> Color {
        self.stroke_color.into()
    }

    /// Get the fill color as a peniko Color.
    pub fn fill(&self) -> Option<Color> {
        self.fill_color.map(|c| c.into())
    }

    /// Set the stroke color from a peniko Color.
    pub fn set_stroke(&mut self, color: Color) {
        self.stroke_color = color.into();
    }

    /// Set the fill color from a peniko Color.
    pub fn set_fill(&mut self, color: Option<Color>) {
        self.fill_color = color.map(|c| c.into());
    }
}

impl Default for ShapeStyle {
    fn default() -> Self {
        Self {
            stroke_color: SerializableColor::black(),
            stroke_width: 2.0,
            fill_color: None,
            sloppiness: Sloppiness::default(),
            seed: generate_seed(),
        }
    }
}

/// Unique identifier for shapes.
pub type ShapeId = Uuid;

/// Common trait for all shapes.
pub trait ShapeTrait {
    /// Get the unique identifier.
    fn id(&self) -> ShapeId;

    /// Get the bounding box in world coordinates.
    fn bounds(&self) -> Rect;

    /// Check if a point (in world coordinates) hits this shape.
    fn hit_test(&self, point: Point, tolerance: f64) -> bool;

    /// Get the path representation for rendering.
    fn to_path(&self) -> BezPath;

    /// Get the style.
    fn style(&self) -> &ShapeStyle;

    /// Get mutable style.
    fn style_mut(&mut self) -> &mut ShapeStyle;

    /// Apply a transform to this shape.
    fn transform(&mut self, affine: Affine);

    /// Clone this shape into a boxed trait object.
    fn clone_box(&self) -> Box<dyn ShapeTrait + Send + Sync>;
}

/// Enum wrapper for all shape types (for serialization).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Shape {
    Rectangle(Rectangle),
    Ellipse(Ellipse),
    Line(Line),
    Arrow(Arrow),
    Freehand(Freehand),
    Text(Text),
    Group(Group),
    Image(Image),
}

impl Shape {
    pub fn id(&self) -> ShapeId {
        match self {
            Shape::Rectangle(s) => s.id(),
            Shape::Ellipse(s) => s.id(),
            Shape::Line(s) => s.id(),
            Shape::Arrow(s) => s.id(),
            Shape::Freehand(s) => s.id(),
            Shape::Text(s) => s.id(),
            Shape::Group(s) => s.id(),
            Shape::Image(s) => s.id(),
        }
    }

    pub fn bounds(&self) -> Rect {
        match self {
            Shape::Rectangle(s) => s.bounds(),
            Shape::Ellipse(s) => s.bounds(),
            Shape::Line(s) => s.bounds(),
            Shape::Arrow(s) => s.bounds(),
            Shape::Freehand(s) => s.bounds(),
            Shape::Text(s) => s.bounds(),
            Shape::Group(s) => s.bounds(),
            Shape::Image(s) => s.bounds(),
        }
    }

    pub fn hit_test(&self, point: Point, tolerance: f64) -> bool {
        match self {
            Shape::Rectangle(s) => s.hit_test(point, tolerance),
            Shape::Ellipse(s) => s.hit_test(point, tolerance),
            Shape::Line(s) => s.hit_test(point, tolerance),
            Shape::Arrow(s) => s.hit_test(point, tolerance),
            Shape::Freehand(s) => s.hit_test(point, tolerance),
            Shape::Text(s) => s.hit_test(point, tolerance),
            Shape::Group(s) => s.hit_test(point, tolerance),
            Shape::Image(s) => s.hit_test(point, tolerance),
        }
    }

    pub fn to_path(&self) -> BezPath {
        match self {
            Shape::Rectangle(s) => s.to_path(),
            Shape::Ellipse(s) => s.to_path(),
            Shape::Line(s) => s.to_path(),
            Shape::Arrow(s) => s.to_path(),
            Shape::Freehand(s) => s.to_path(),
            Shape::Text(s) => s.to_path(),
            Shape::Group(s) => s.to_path(),
            Shape::Image(s) => s.to_path(),
        }
    }

    pub fn style(&self) -> &ShapeStyle {
        match self {
            Shape::Rectangle(s) => s.style(),
            Shape::Ellipse(s) => s.style(),
            Shape::Line(s) => s.style(),
            Shape::Arrow(s) => s.style(),
            Shape::Freehand(s) => s.style(),
            Shape::Text(s) => s.style(),
            Shape::Group(s) => s.style(),
            Shape::Image(s) => s.style(),
        }
    }

    pub fn style_mut(&mut self) -> &mut ShapeStyle {
        match self {
            Shape::Rectangle(s) => s.style_mut(),
            Shape::Ellipse(s) => s.style_mut(),
            Shape::Line(s) => s.style_mut(),
            Shape::Arrow(s) => s.style_mut(),
            Shape::Freehand(s) => s.style_mut(),
            Shape::Text(s) => s.style_mut(),
            Shape::Group(s) => s.style_mut(),
            Shape::Image(s) => s.style_mut(),
        }
    }

    pub fn transform(&mut self, affine: Affine) {
        match self {
            Shape::Rectangle(s) => s.transform(affine),
            Shape::Ellipse(s) => s.transform(affine),
            Shape::Line(s) => s.transform(affine),
            Shape::Arrow(s) => s.transform(affine),
            Shape::Freehand(s) => s.transform(affine),
            Shape::Text(s) => s.transform(affine),
            Shape::Group(s) => s.transform(affine),
            Shape::Image(s) => s.transform(affine),
        }
    }

    /// Check if this shape is a group.
    pub fn is_group(&self) -> bool {
        matches!(self, Shape::Group(_))
    }

    /// Get the group if this shape is a group.
    pub fn as_group(&self) -> Option<&Group> {
        match self {
            Shape::Group(g) => Some(g),
            _ => None,
        }
    }

    /// Get the mutable group if this shape is a group.
    pub fn as_group_mut(&mut self) -> Option<&mut Group> {
        match self {
            Shape::Group(g) => Some(g),
            _ => None,
        }
    }

    /// Regenerate the shape's ID with a new unique identifier.
    /// This is used when duplicating or pasting shapes to ensure they have unique IDs.
    pub fn regenerate_id(&mut self) {
        let new_id = Uuid::new_v4();
        match self {
            Shape::Rectangle(s) => s.id = new_id,
            Shape::Ellipse(s) => s.id = new_id,
            Shape::Line(s) => s.id = new_id,
            Shape::Arrow(s) => s.id = new_id,
            Shape::Freehand(s) => s.id = new_id,
            Shape::Text(s) => s.id = new_id,
            Shape::Group(s) => s.id = new_id,
            Shape::Image(s) => s.id = new_id,
        }
    }

    /// Check if this shape is an image.
    pub fn is_image(&self) -> bool {
        matches!(self, Shape::Image(_))
    }

    /// Get the image if this shape is an image.
    pub fn as_image(&self) -> Option<&Image> {
        match self {
            Shape::Image(img) => Some(img),
            _ => None,
        }
    }
}
