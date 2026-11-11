//! The element: one flat record per thing on the canvas.
//!
//! Elements are plain data with no behaviour beyond small lookups. Relations (groups, frames, bound text, arrow
//! bindings) are ids on the element that owns the relation; the reverse direction is derived by
//! [`crate::derived::DerivedIndex`] and never stored.

use std::collections::BTreeMap;

use kurbo::{Point, Rect, Vec2};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::geom::{points_bounds, pt, pts};
use crate::id::{ElementId, FileId, GroupId};
use crate::style::{Style, TextStyle};
use crate::{color::Color, zindex::ZIndex};

/// Position, size and rotation of an element. All in world units and radians.
///
/// `(x, y, w, h)` is the unrotated box; the element is rotated by `angle` about the box centre. For line, arrow and
/// freehand the box is the bounding box of their points, so that every kind rotates the same way.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Transform {
    /// Left edge of the unrotated box.
    pub x: f64,
    /// Top edge of the unrotated box.
    pub y: f64,
    /// Width, 0 or more.
    pub w: f64,
    /// Height, 0 or more.
    pub h: f64,
    /// Clockwise rotation in radians (y points down) about the box centre.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub angle: f64,
    /// The content is mirrored left to right. Only images keep a mirror flag; other kinds mirror their geometry.
    #[serde(default, skip_serializing_if = "is_false")]
    pub flip_x: bool,
    /// The content is mirrored top to bottom.
    #[serde(default, skip_serializing_if = "is_false")]
    pub flip_y: bool,
}

impl Transform {
    /// A box with no rotation and no flip.
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h, angle: 0.0, flip_x: false, flip_y: false }
    }

    /// The unrotated box.
    pub fn rect(&self) -> Rect {
        Rect::new(self.x, self.y, self.x + self.w, self.y + self.h)
    }

    /// The centre of the box, which is also the rotation pivot.
    pub fn center(&self) -> Point {
        Point::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    /// Maps a point in the element's local frame (origin at the box top-left, unrotated) to world space.
    pub fn local_to_world(&self, p: Point) -> Point {
        let c = self.center();
        let rel = Vec2::new(self.x + p.x - c.x, self.y + p.y - c.y);
        c + rotate(rel, self.angle)
    }
}

fn rotate(v: Vec2, angle: f64) -> Vec2 {
    let (s, c) = angle.sin_cos();
    Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}
fn is_false(v: &bool) -> bool {
    !*v
}
fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}
fn default_true() -> bool {
    true
}
fn is_true(v: &bool) -> bool {
    *v
}

/// How the points of a line or arrow are joined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PathType {
    /// Straight segments (old `Direct`).
    #[default]
    Sharp,
    /// A smooth curve through the points (old `Flowing`).
    Round,
    /// Orthogonal segments routed around the bound shapes (old `Angular`; the old app drew it but never stored it).
    Elbow,
}

/// What is drawn at an end of a line or arrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Head {
    /// Nothing.
    #[default]
    None,
    /// Open V.
    Arrow,
    /// A bar across the line.
    Bar,
    /// Filled circle.
    Circle,
    /// Hollow circle.
    CircleOutline,
    /// Filled triangle.
    Triangle,
    /// Hollow triangle.
    TriangleOutline,
    /// Filled diamond.
    Diamond,
    /// Hollow diamond.
    DiamondOutline,
    /// Entity-relationship marker: one.
    CardinalityOne,
    /// Entity-relationship marker: many (crow's foot).
    CardinalityMany,
    /// Entity-relationship marker: one or many.
    CardinalityOneOrMany,
    /// Entity-relationship marker: exactly one.
    CardinalityExactlyOne,
    /// Entity-relationship marker: zero or one.
    CardinalityZeroOrOne,
    /// Entity-relationship marker: zero or many.
    CardinalityZeroOrMany,
}

/// Where a bound arrow end sits relative to its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BindMode {
    /// The end is inside the target's outline and keeps its ratio position there.
    Inside,
    /// The end sits on the target's outline plus a small gap, aimed at the fixed point.
    Orbit,
}

/// An arrow end attached to another element. Stored on the arrow only; the reverse index is derived.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    /// The element the end is attached to.
    pub target: ElementId,
    /// Inside or orbit.
    pub mode: BindMode,
    /// Where on the target, as ratios `[rx, ry]` of the target's unrotated box (0 to 1 is inside the box).
    pub fixed_point: [f64; 2],
}

/// An elbow segment the user pinned in place.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FixedSegment {
    /// Index in `points` of the segment's end point (segment `i` joins `points[i-1]` and `points[i]`).
    pub index: u32,
    /// Start of the pinned segment, in the arrow's local frame.
    #[serde(with = "pt")]
    #[schemars(with = "[f64; 2]")]
    pub start: Point,
    /// End of the pinned segment, in the arrow's local frame.
    #[serde(with = "pt")]
    #[schemars(with = "[f64; 2]")]
    pub end: Point,
}

/// Data of a line or an arrow.
///
/// `points` are in the element's local frame: the origin is the top-left of the points' bounding box, so the
/// smallest x and y are 0 and the largest are `transform.w` and `transform.h`. A line has at least 2 points.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinearData {
    /// The vertices, local frame. Written as `[x, y]` pairs.
    #[serde(with = "pts")]
    #[schemars(with = "Vec<[f64; 2]>")]
    pub points: Vec<Point>,
    /// How the points are joined.
    #[serde(default)]
    pub path: PathType,
    /// Attachment of the first point.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_binding: Option<Binding>,
    /// Attachment of the last point.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_binding: Option<Binding>,
    /// Marker at the first point.
    #[serde(default, skip_serializing_if = "is_head_none")]
    pub start_head: Head,
    /// Marker at the last point.
    #[serde(default, skip_serializing_if = "is_head_none")]
    pub end_head: Head,
    /// Marker size in world units. `None` derives it from the stroke width. The old app used 15.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_size: Option<f64>,
    /// Pinned elbow segments, sorted by index. Only for `path = elbow`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixed_segments: Vec<FixedSegment>,
    /// Elbow: the first leg is a hidden stub leaving the bound shape.
    #[serde(default, skip_serializing_if = "is_false")]
    pub start_is_special: bool,
    /// Elbow: the last leg is a hidden stub entering the bound shape.
    #[serde(default, skip_serializing_if = "is_false")]
    pub end_is_special: bool,
    /// The last point connects back to the first, and the shape can be filled. Lines only.
    #[serde(default, skip_serializing_if = "is_false")]
    pub closed: bool,
}

fn is_head_none(h: &Head) -> bool {
    *h == Head::None
}

impl LinearData {
    /// A plain polyline with no heads and no bindings. `points` are local (see the type docs).
    pub fn new(points: Vec<Point>) -> Self {
        Self {
            points,
            path: PathType::Sharp,
            start_binding: None,
            end_binding: None,
            start_head: Head::None,
            end_head: Head::None,
            head_size: None,
            fixed_segments: Vec::new(),
            start_is_special: false,
            end_is_special: false,
            closed: false,
        }
    }
}

/// How pen pressure shapes a freehand stroke.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PressureMode {
    /// Constant width.
    #[default]
    Uniform,
    /// Width follows the drawing speed.
    Simulated,
    /// Width follows the recorded `pressures` of a pen.
    Device,
}

/// Brush of a freehand stroke.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Brush {
    /// Round brush.
    #[default]
    Normal,
    /// Angled nib.
    Calligraphy,
}

/// Data of a freehand stroke. `points` use the same local frame as [`LinearData`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FreehandData {
    /// Sampled positions, local frame. At least 1 (a single point is a dot).
    #[serde(with = "pts")]
    #[schemars(with = "Vec<[f64; 2]>")]
    pub points: Vec<Point>,
    /// Pen pressure per point, 0 to 1. Empty, or exactly one per point.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pressures: Vec<f64>,
    /// How pressure shapes the stroke.
    #[serde(default)]
    pub pressure_mode: PressureMode,
    /// Brush shape.
    #[serde(default)]
    pub brush: Brush,
    /// A wide, translucent marker stroke drawn under the other elements' ink.
    #[serde(default, skip_serializing_if = "is_false")]
    pub highlighter: bool,
    /// The stroke is a closed shape and can be filled.
    #[serde(default, skip_serializing_if = "is_false")]
    pub closed: bool,
}

impl FreehandData {
    /// A stroke with uniform width. `points` are local.
    pub fn new(points: Vec<Point>) -> Self {
        Self {
            points,
            pressures: Vec::new(),
            pressure_mode: PressureMode::Uniform,
            brush: Brush::Normal,
            highlighter: false,
            closed: false,
        }
    }
}

/// A colour override for a range of characters.
///
/// Ranges are in Unicode scalar values (`char`s), not bytes or UTF-16 units, exactly like the old `char_colors`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TextRun {
    /// First character of the range.
    pub start: u32,
    /// One past the last character of the range.
    pub end: u32,
    /// Text colour for the range. `None` uses the element's stroke colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
}

/// Data of a text element.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TextData {
    /// The text, with `\n` for line breaks. This is the source of truth; wrapping is derived from it.
    pub content: String,
    /// Per-character colour ranges, sorted and not overlapping.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<TextRun>,
    /// Font settings.
    #[serde(default)]
    pub font: TextStyle,
    /// True: the box follows the content. False: the box keeps its width and the text wraps.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub auto_resize: bool,
    /// The element this text is the label of. The single authoritative side of the relation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_id: Option<ElementId>,
    /// For an arrow label: where along the arrow, 0 (start) to 1 (end), by arc length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_position: Option<f64>,
}

impl TextData {
    /// A free text with default fonts.
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            runs: Vec::new(),
            font: TextStyle::default(),
            auto_resize: true,
            container_id: None,
            label_position: None,
        }
    }

    /// Number of characters (Unicode scalar values).
    pub fn char_count(&self) -> usize {
        self.content.chars().count()
    }

    /// One entry per character: the run colour, or `None` for the element colour. Shape of the old `char_colors`.
    pub fn char_colors(&self) -> Vec<Option<Color>> {
        let mut out = vec![None; self.char_count()];
        for run in &self.runs {
            let (s, e) = (run.start as usize, (run.end as usize).min(out.len()));
            if let Some(slice) = out.get_mut(s..e) {
                slice.fill(run.color);
            }
        }
        out
    }

    /// Builds runs from per-character colours. Missing entries count as `None`, extra entries are ignored, and equal
    /// neighbours merge, so any old `char_colors` array loads.
    pub fn runs_from_char_colors(colors: &[Option<Color>], char_count: usize) -> Vec<TextRun> {
        let mut runs: Vec<TextRun> = Vec::new();
        for (i, color) in colors.iter().take(char_count).enumerate() {
            let Some(color) = color else { continue };
            let i = u32::try_from(i).unwrap_or(u32::MAX);
            match runs.last_mut() {
                Some(last) if last.end == i && last.color == Some(*color) => last.end = i.saturating_add(1),
                _ => runs.push(TextRun { start: i, end: i.saturating_add(1), color: Some(*color) }),
            }
        }
        runs
    }
}

/// Data of a math element.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MathData {
    /// LaTeX source.
    pub latex: String,
    /// Font size in world units.
    #[serde(default = "default_math_size")]
    pub font_size: f64,
}

fn default_math_size() -> f64 {
    20.0
}

impl MathData {
    /// A formula at the default size.
    pub fn new(latex: impl Into<String>) -> Self {
        Self { latex: latex.into(), font_size: default_math_size() }
    }
}

/// The visible part of an image, in pixels of the source picture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Crop {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}

/// Data of an image element.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImageData {
    /// Hash of the encoded picture in the document's files table. `None` while the picture is not there yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<FileId>,
    /// Width of the encoded picture in pixels.
    pub natural_width: u32,
    /// Height of the encoded picture in pixels.
    pub natural_height: u32,
    /// Visible part of the picture. `None` shows all of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop: Option<Crop>,
}

/// Data of a frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FrameData {
    /// Label. `None` shows the default "Frame".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Children are clipped to the frame when drawn.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub clip: bool,
}

impl Default for FrameData {
    fn default() -> Self {
        Self { name: None, clip: true }
    }
}

/// The kind of an element and the data only that kind has.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ElementKind {
    /// Rectangle (rounded when `style.corner_radius > 0`).
    Rectangle,
    /// Diamond.
    Diamond,
    /// Ellipse.
    Ellipse,
    /// Polyline or curve with optional bindings and markers.
    Line(LinearData),
    /// Like a line; arrows are the usual choice for markers and bindings.
    Arrow(LinearData),
    /// Pen stroke.
    Freehand(FreehandData),
    /// Rich text.
    Text(TextData),
    /// LaTeX formula.
    Math(MathData),
    /// Picture.
    Image(ImageData),
    /// Clipping container.
    Frame(FrameData),
}

/// The kind tag without data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// See [`ElementKind::Rectangle`].
    Rectangle,
    /// See [`ElementKind::Diamond`].
    Diamond,
    /// See [`ElementKind::Ellipse`].
    Ellipse,
    /// See [`ElementKind::Line`].
    Line,
    /// See [`ElementKind::Arrow`].
    Arrow,
    /// See [`ElementKind::Freehand`].
    Freehand,
    /// See [`ElementKind::Text`].
    Text,
    /// See [`ElementKind::Math`].
    Math,
    /// See [`ElementKind::Image`].
    Image,
    /// See [`ElementKind::Frame`].
    Frame,
}

impl Kind {
    /// Lowercase name, as used in JSON and in command arguments.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Rectangle => "rectangle",
            Kind::Diamond => "diamond",
            Kind::Ellipse => "ellipse",
            Kind::Line => "line",
            Kind::Arrow => "arrow",
            Kind::Freehand => "freehand",
            Kind::Text => "text",
            Kind::Math => "math",
            Kind::Image => "image",
            Kind::Frame => "frame",
        }
    }

    /// An arrow end can attach to this kind.
    pub fn is_bindable(self) -> bool {
        matches!(self, Kind::Rectangle | Kind::Diamond | Kind::Ellipse | Kind::Text | Kind::Image | Kind::Frame)
    }

    /// A text can be bound to this kind as its label.
    pub fn is_label_container(self) -> bool {
        matches!(self, Kind::Rectangle | Kind::Diamond | Kind::Ellipse | Kind::Arrow)
    }

    /// Line or arrow.
    pub fn is_linear(self) -> bool {
        matches!(self, Kind::Line | Kind::Arrow)
    }
}

impl ElementKind {
    /// The tag of this kind.
    pub fn kind(&self) -> Kind {
        match self {
            ElementKind::Rectangle => Kind::Rectangle,
            ElementKind::Diamond => Kind::Diamond,
            ElementKind::Ellipse => Kind::Ellipse,
            ElementKind::Line(_) => Kind::Line,
            ElementKind::Arrow(_) => Kind::Arrow,
            ElementKind::Freehand(_) => Kind::Freehand,
            ElementKind::Text(_) => Kind::Text,
            ElementKind::Math(_) => Kind::Math,
            ElementKind::Image(_) => Kind::Image,
            ElementKind::Frame(_) => Kind::Frame,
        }
    }
}

/// An element of the scene.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Element {
    /// Stable id.
    pub id: ElementId,
    /// Kind and kind-specific data.
    pub kind: ElementKind,
    /// Position, size, rotation.
    pub transform: Transform,
    /// Visual style.
    #[serde(default)]
    pub style: Style,
    /// Z-order key; smaller is further back. Ties between elements are broken by id.
    pub index: ZIndex,
    /// Groups this element belongs to, innermost first and outermost last.
    #[serde(default, skip_serializing_if = "SmallVec::is_empty")]
    #[schemars(with = "Vec<GroupId>")]
    pub group_ids: SmallVec<[GroupId; 2]>,
    /// The frame this element is a child of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_id: Option<ElementId>,
    /// Locked elements cannot be selected or edited by commands.
    #[serde(default, skip_serializing_if = "is_false")]
    pub locked: bool,
    /// Tombstone. A deleted element stays in the scene so that undo and sync work; it is invisible and ignored by
    /// the derived indexes.
    #[serde(default, skip_serializing_if = "is_false")]
    pub deleted: bool,
    /// Label an agent can address the element by (`@name`). Not drawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Hyperlink.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// Free data for the host application.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<BTreeMap<String, serde_json::Value>>,
    /// Local change counter, increased by the store on every change. Renderers key caches by `(id, version)`. It is
    /// not a synchronisation clock and two stores may number the same history differently.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub version: u64,
}

impl Element {
    /// A new element with default style and an empty box.
    pub fn new(id: ElementId, kind: ElementKind, index: ZIndex) -> Self {
        Self {
            id,
            kind,
            transform: Transform::new(0.0, 0.0, 0.0, 0.0),
            style: Style::default(),
            index,
            group_ids: SmallVec::new(),
            frame_id: None,
            locked: false,
            deleted: false,
            name: None,
            link: None,
            custom: None,
            version: 0,
        }
    }

    /// Sets the box.
    #[must_use]
    pub fn with_bounds(mut self, x: f64, y: f64, w: f64, h: f64) -> Self {
        self.transform = Transform::new(x, y, w, h);
        self
    }

    /// Sets the style.
    #[must_use]
    pub fn with_style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    /// A line or arrow through `world_points`. The box and local points are derived; see [`LinearData`].
    pub fn linear(id: ElementId, kind: Kind, index: ZIndex, world_points: &[Point]) -> Option<Self> {
        let data = LinearData::new(world_points.to_vec());
        let ek = if kind == Kind::Arrow {
            ElementKind::Arrow(data)
        } else if kind == Kind::Line {
            ElementKind::Line(data)
        } else {
            return None;
        };
        let mut e = Element::new(id, ek, index);
        e.refit_points().then_some(e)
    }

    /// The tag of this element's kind.
    pub fn tag(&self) -> Kind {
        self.kind.kind()
    }

    /// The unrotated box.
    pub fn rect(&self) -> Rect {
        self.transform.rect()
    }

    /// The points of a line, arrow or freehand.
    pub fn points(&self) -> Option<&[Point]> {
        match &self.kind {
            ElementKind::Line(d) | ElementKind::Arrow(d) => Some(&d.points),
            ElementKind::Freehand(d) => Some(&d.points),
            _ => None,
        }
    }

    /// Linear data of a line or arrow.
    pub fn linear_data(&self) -> Option<&LinearData> {
        match &self.kind {
            ElementKind::Line(d) | ElementKind::Arrow(d) => Some(d),
            _ => None,
        }
    }

    /// The text data of a text element.
    pub fn text_data(&self) -> Option<&TextData> {
        match &self.kind {
            ElementKind::Text(d) => Some(d),
            _ => None,
        }
    }

    /// The text this element is the label of, if it is bound text.
    pub fn container_id(&self) -> Option<&ElementId> {
        self.text_data().and_then(|t| t.container_id.as_ref())
    }

    /// The elements this arrow's ends are bound to, start first.
    pub fn binding_targets(&self) -> [Option<&ElementId>; 2] {
        match self.linear_data() {
            Some(d) => [d.start_binding.as_ref().map(|b| &b.target), d.end_binding.as_ref().map(|b| &b.target)],
            None => [None, None],
        }
    }

    /// The outermost group, which is the unit a click selects.
    pub fn outermost_group(&self) -> Option<&GroupId> {
        self.group_ids.last()
    }

    fn points_mut(&mut self) -> Option<&mut Vec<Point>> {
        match &mut self.kind {
            ElementKind::Line(d) | ElementKind::Arrow(d) => Some(&mut d.points),
            ElementKind::Freehand(d) => Some(&mut d.points),
            _ => None,
        }
    }

    /// Re-establishes the point-box invariant after `points` were edited or replaced.
    ///
    /// The points are read in the element's current local frame (they may be anywhere, even negative). Afterwards
    /// their smallest coordinates are 0, `transform.w/h` is their extent, and the world position of every point is
    /// unchanged, also when the element is rotated. Returns false (and changes nothing) for a kind without points,
    /// no points, or non-finite coordinates.
    pub fn refit_points(&mut self) -> bool {
        let old = self.transform;
        let Some(points) = self.points() else { return false };
        if points.iter().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
            return false;
        }
        let Some((min_x, min_y, max_x, max_y)) = points_bounds(points) else { return false };
        let (w, h) = (max_x - min_x, max_y - min_y);
        let shift = Vec2::new(min_x, min_y);
        // World position of the first point under the old transform, which must stay put.
        let first = points.first().copied().unwrap_or(Point::ZERO);
        let world0 = old.local_to_world(first);
        // New centre: world0 = c' + R(q0 - half) with q0 the shifted first point.
        let half = Vec2::new(w / 2.0, h / 2.0);
        let q0 = first - shift;
        let rel = Vec2::new(q0.x - half.x, q0.y - half.y);
        let c = world0 - rotate(rel, old.angle);
        if let Some(pts) = self.points_mut() {
            for p in pts.iter_mut() {
                *p -= shift;
            }
        }
        // The shift also applies to fixed segments, which live in the same local frame.
        if let ElementKind::Line(d) | ElementKind::Arrow(d) = &mut self.kind {
            for s in &mut d.fixed_segments {
                s.start -= shift;
                s.end -= shift;
            }
        }
        self.transform.x = c.x - half.x;
        self.transform.y = c.y - half.y;
        self.transform.w = w;
        self.transform.h = h;
        true
    }
}
