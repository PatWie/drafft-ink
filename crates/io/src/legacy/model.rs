//! The old drafft-ink shape model, reduced to what migration needs.
//!
//! Both readers (JSON, Loro) produce an [`OldDoc`]; one builder turns it into a native `Document`. Keeping the
//! middle form typed means the old defaults and quirks are written once.

use drafftink_scene::{Color, FillPattern, FontFamily, FontWeight, PathType, Sloppiness, StrokeStyle};
use kurbo::Point;

/// An old document: a name and the top-level shapes, back to front.
#[derive(Debug, Clone, Default)]
pub(crate) struct OldDoc {
    pub name: Option<String>,
    pub shapes: Vec<OShape>,
    /// Shapes that were stored but not listed in the z-order, so the old app never drew them.
    pub undrawn: usize,
}

/// The old `ShapeStyle`.
#[derive(Debug, Clone)]
pub(crate) struct OStyle {
    pub stroke: Color,
    pub stroke_width: f64,
    pub fill: Option<Color>,
    pub fill_pattern: FillPattern,
    pub sloppiness: Sloppiness,
    /// `None` when the file did not carry a seed (the old app made a random one per load).
    pub seed: Option<u32>,
    pub opacity: f64,
}

impl Default for OStyle {
    fn default() -> Self {
        Self {
            stroke: Color::BLACK,
            stroke_width: 2.0,
            fill: None,
            fill_pattern: FillPattern::Solid,
            sloppiness: Sloppiness::Artist,
            seed: None,
            opacity: 1.0,
        }
    }
}

/// One old shape.
#[derive(Debug, Clone)]
pub(crate) struct OShape {
    /// The shape's own id, as stored (any string; validated by the builder).
    pub id: Option<String>,
    pub style: OStyle,
    pub body: Body,
}

/// The per-type data of an old shape.
#[derive(Debug, Clone)]
pub(crate) enum Body {
    Rect { pos: Point, w: f64, h: f64, radius: f64, rotation: f64 },
    Ellipse { center: Point, rx: f64, ry: f64, rotation: f64 },
    Line(OLinear),
    Arrow(OLinear, f64),
    Freehand { points: Vec<Point>, pressures: Vec<f64>, closed: bool },
    Text(OText),
    Math { pos: Point, latex: String, size: f64, rotation: f64 },
    Image(OImage),
    Group { children: Vec<OShape>, rotation: f64 },
}

/// Line and arrow data.
#[derive(Debug, Clone)]
pub(crate) struct OLinear {
    /// Start, intermediate points, end.
    pub points: Vec<Point>,
    pub path: PathType,
    pub stroke_style: StrokeStyle,
    pub closed: bool,
}

/// Text data.
#[derive(Debug, Clone)]
pub(crate) struct OText {
    pub pos: Point,
    pub content: String,
    pub size: f64,
    pub family: FontFamily,
    pub weight: FontWeight,
    pub rotation: f64,
    /// One entry per Unicode scalar value, any length.
    pub char_colors: Vec<Option<Color>>,
}

/// Image data.
#[derive(Debug, Clone)]
pub(crate) struct OImage {
    pub pos: Point,
    pub w: f64,
    pub h: f64,
    pub source_w: u32,
    pub source_h: u32,
    /// The stored `format` name, which the old app did not verify.
    pub format: Option<ImageFormat>,
    pub data: Vec<u8>,
    pub rotation: f64,
}

/// The old `ImageFormat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageFormat {
    Png,
    Jpeg,
    WebP,
}

/// Number of non-group shapes inside `shape` (1 for a leaf).
pub(crate) fn leaf_count(shape: &OShape) -> usize {
    match &shape.body {
        Body::Group { children, .. } => children.iter().map(leaf_count).sum(),
        _ => 1,
    }
}
