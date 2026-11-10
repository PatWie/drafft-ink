//! Input descriptions. Everything is in local coordinates: the shape's own origin is (0, 0) and
//! the caller applies translation and rotation with a transform, so moving or rotating a shape
//! never changes the description (and never misses the cache).

use kurbo::Point;

/// Roughness levels of the old app (`Sloppiness::roughness`). Any value from 0 to
/// [`MAX_ROUGHNESS`](crate::MAX_ROUGHNESS) is accepted.
pub mod roughness {
    /// Clean, precise geometry. No noise, one stroke.
    pub const ARCHITECT: f64 = 0.0;
    /// The default of the old app.
    pub const ARTIST: f64 = 1.0;
    pub const CARTOONIST: f64 = 2.0;
    pub const DRUNK: f64 = 3.5;
}

/// How the outline is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineDash {
    #[default]
    Solid,
    /// Dash `4w`, gap `2w` for stroke width `w` (the old app).
    Dashed,
    /// Dot `w`, gap `2w`.
    Dotted,
}

/// Interior fill. All but `None` and `Solid` are sketched by roughr and clipped to the shape by the
/// renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FillStyle {
    #[default]
    None,
    Solid,
    /// Parallel lines.
    Hachure,
    /// Two sets of hachure lines at right angles.
    CrossHatch,
    /// Hachure lines joined into one zigzag.
    ZigZag,
    /// Dots along hachure lines.
    Dots,
    /// Hachure lines broken into dashes.
    Dashed,
    /// Hachure lines drawn as small zigzags.
    ZigZagLine,
}

/// Corner rounding of rectangles and diamonds.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Roundness {
    #[default]
    Sharp,
    /// A fixed radius in local units (the old app's `corner_radius`).
    Radius(f64),
    /// A fraction of the shorter side. Excalidraw's "proportional" is `0.25`.
    Proportional(f64),
    /// Excalidraw's "adaptive" rounding: 25 % of the shorter side for small shapes, and the given
    /// maximum radius (Excalidraw uses 32) for large ones.
    Adaptive(f64),
}

impl Roundness {
    /// The corner radius for a shape of the given size, never more than half the shorter side.
    pub fn radius(self, width: f64, height: f64) -> f64 {
        let short = width.min(height);
        let r = match self {
            Roundness::Sharp => 0.0,
            Roundness::Radius(r) => r,
            Roundness::Proportional(f) => short * f,
            Roundness::Adaptive(max) => {
                let cutoff = max / 0.25;
                if short <= cutoff { short * 0.25 } else { max }
            }
        };
        if r.is_nan() { 0.0 } else { r.clamp(0.0, (short / 2.0).max(0.0)) }
    }
}

/// The path through the points of a line or arrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Curve {
    /// Straight segments, sharp corners. Also used for elbow ("angular") connectors: the caller
    /// routes them and passes the corner points.
    #[default]
    Straight,
    /// A Catmull-Rom spline through the points, as cubic Beziers (the old "flowing" style).
    Smooth,
}

/// The shape of one arrow end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HeadKind {
    #[default]
    None,
    /// Two strokes from the tip (the old app's head). `filled` has no effect.
    Arrow,
    /// A triangle at the tip.
    Triangle,
    /// A circle ending at the tip.
    Circle,
    /// A short bar across the line at the tip. `filled` has no effect.
    Bar,
    /// A diamond ending at the tip.
    Diamond,
}

/// One arrow end: a kind, and whether closed kinds (triangle, circle, diamond) are filled with the
/// stroke colour. Heads are always drawn solid, even when the shaft is dashed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Arrowhead {
    pub kind: HeadKind,
    pub filled: bool,
}

impl Arrowhead {
    pub const NONE: Arrowhead = Arrowhead { kind: HeadKind::None, filled: false };
    /// The old app's arrowhead.
    pub const ARROW: Arrowhead = Arrowhead { kind: HeadKind::Arrow, filled: false };
    pub const TRIANGLE: Arrowhead = Arrowhead { kind: HeadKind::Triangle, filled: true };
    pub const TRIANGLE_OUTLINE: Arrowhead = Arrowhead { kind: HeadKind::Triangle, filled: false };
    pub const CIRCLE: Arrowhead = Arrowhead { kind: HeadKind::Circle, filled: true };
    pub const CIRCLE_OUTLINE: Arrowhead = Arrowhead { kind: HeadKind::Circle, filled: false };
    pub const BAR: Arrowhead = Arrowhead { kind: HeadKind::Bar, filled: false };
    pub const DIAMOND: Arrowhead = Arrowhead { kind: HeadKind::Diamond, filled: true };
    pub const DIAMOND_OUTLINE: Arrowhead = Arrowhead { kind: HeadKind::Diamond, filled: false };
}

/// Pressure outline settings of the old app (`render_freehand_with_pressure`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PressureProfile {
    /// Outline width at full pressure, as a multiple of the stroke width. Old: 2.3.
    pub size_scale: f64,
    /// How much low pressure thins the line, 0 to 1. Old: 0.6.
    pub thinning: f64,
}

impl Default for PressureProfile {
    fn default() -> Self {
        Self { size_scale: 2.3, thinning: 0.6 }
    }
}

/// The geometry to sketch.
#[derive(Debug, Clone, PartialEq)]
pub enum SketchShape {
    /// Corner at (0, 0), size `width` x `height`.
    Rect { width: f64, height: f64, roundness: Roundness },
    /// Inscribed in the box (0, 0) to (`width`, `height`).
    Ellipse { width: f64, height: f64 },
    /// Corners at the middles of the sides of the box (0, 0) to (`width`, `height`).
    Diamond { width: f64, height: f64, roundness: Roundness },
    /// A line through the points. Closed lines are filled when `SketchParams::fill` asks for it.
    Polyline { points: Vec<Point>, curve: Curve, closed: bool },
    /// A line through the points with a head at either end. `head_size` is the head length.
    /// The old app's arrow is `end: Arrowhead::ARROW, head_size: 15.0`.
    Arrow { points: Vec<Point>, curve: Curve, start: Arrowhead, end: Arrowhead, head_size: f64 },
    /// A drawn stroke. With `pressures` (one per point, 0 to 1; missing entries count as 1) and not
    /// closed, the stroke is a variable-width outline, drawn as a fill. Without pressure it is a
    /// sketchy line like a polyline. Closed strokes are polygons with an optional fill.
    Freehand { points: Vec<Point>, pressures: Vec<f64>, closed: bool, profile: PressureProfile },
}

/// How the shape looks.
#[derive(Debug, Clone, PartialEq)]
pub struct SketchParams {
    /// 0 is clean geometry, 1 the old default, up to [`MAX_ROUGHNESS`](crate::MAX_ROUGHNESS).
    pub roughness: f64,
    /// Multiplier for how far straight lines bow. 1.0 is the old app.
    pub bowing: f64,
    /// Seed of all randomness. The same seed gives the same drawing.
    pub seed: u32,
    pub stroke_width: f64,
    pub line: LineDash,
    pub fill: FillStyle,
    /// Distance between fill lines. `None`: four times the stroke width (the old app).
    pub fill_gap: Option<f64>,
    /// Angle of fill lines in degrees. `None`: -41, the roughr default (the old app).
    pub fill_angle: Option<f64>,
}

impl Default for SketchParams {
    fn default() -> Self {
        Self {
            roughness: roughness::ARTIST,
            bowing: 1.0,
            seed: 1,
            stroke_width: 2.0,
            line: LineDash::Solid,
            fill: FillStyle::None,
            fill_gap: None,
            fill_angle: None,
        }
    }
}

/// A shape and how it looks. This is what [`sketch`](crate::sketch) takes and what
/// [`SketchKey`](crate::SketchKey) identifies.
#[derive(Debug, Clone, PartialEq)]
pub struct SketchInput {
    pub shape: SketchShape,
    pub params: SketchParams,
}

impl SketchInput {
    pub fn new(shape: SketchShape, params: SketchParams) -> Self {
        Self { shape, params }
    }
}
