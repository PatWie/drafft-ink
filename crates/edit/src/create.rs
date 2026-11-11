//! Creating elements: the rules behind `element.create`, `element.connect` and `element.set_name`.
//!
//! * **Sizes.** A rectangle, diamond, ellipse or frame must be at least [`MIN_SIZE`] wide and high; a line or
//!   arrow must reach [`MIN_SIZE`] in one direction. (The old app added a shape when width or height was above 1;
//!   the catalog turned that into a refusal.)
//! * **Defaults of the old tools.** An arrow has no start head and an `arrow` end head. A highlighter stroke is at
//!   least [`MIN_HIGHLIGHTER_WIDTH`] wide with alpha [`HIGHLIGHTER_ALPHA`], unless the caller's style says
//!   otherwise. A stroke is simplified with Ramer-Douglas-Peucker at [`DEFAULT_SIMPLIFY`] (`Freehand::simplify(2.0)`).
//!   The math placeholder is [`DEFAULT_MATH`].
//! * **Names** ([`validate_name`], [`ensure_name_free`]): 1 to [`MAX_NAME_CHARS`] characters, no leading `@` or `$`,
//!   no control characters, unique among live elements.
//! * **New elements**: the builders ([`shape_element`], [`linear_element`], [`freehand_element`], [`text_element`],
//!   [`math_element`]) make one element of each kind from plain values; [`insert_on_top`] puts it on the top of the
//!   z-order.
//! * **Binding of new arrow ends** ([`bind_new_ends`]): explicit targets use [`crate::arrow::bind_end`]; ends that
//!   are left to chance bind like a dragged end ([`crate::binding::drag_end`]): inside a shape binds `Inside`, near
//!   its outline binds `Orbit`.
//! * **Placement** ([`join_frame`], [`place_above`]): frame membership by the frame under the element's centre or a
//!   named frame, and "directly above that element" without splitting a group, a frame's children or a label pair.
//! * **Connecting** ([`connect`]): an arrow bound to two shapes at the sides that face each other.
//!
//! Callers run these inside one transaction.

use std::collections::BTreeSet;

use drafftink_scene::{
    BindMode, Brush, Element, ElementError, ElementId, ElementKind, FrameData, FreehandData, Head, IdSource,
    IndexError, Kind, LinearData, MathData, PathType, PressureMode, SceneRead, SceneTx, StoreError, Style, TextData,
    TextStyle, Transform, ZIndex, limits, validate_element,
};
use kurbo::{Point, Size};

use crate::arrow::{ArrowError, BindRequest, bind_end};
use crate::binding::{self, BindOpts, BindingError, End};
use crate::elbow::ElbowEditError;
use crate::frame::{self, FrameError};
use crate::group::units::{Relations, TargetError};
use crate::linear::{self, LinearError, world_points};
use crate::text_bind::{self, TextBindError, TextMeasure};
use crate::zorder::{self, Place, ZOrderError};

/// Smallest width or height of a new shape.
pub const MIN_SIZE: f64 = 1.0;
/// Narrowest highlighter stroke when the style does not give a width.
pub const MIN_HIGHLIGHTER_WIDTH: f64 = 12.0;
/// Alpha of a highlighter stroke when the style does not give a colour (50%).
pub const HIGHLIGHTER_ALPHA: u8 = 128;
/// Default Ramer-Douglas-Peucker tolerance of a stroke, in world units.
pub const DEFAULT_SIMPLIFY: f64 = 2.0;
/// The formula a new math element gets when none is given.
pub const DEFAULT_MATH: &str = "x^2 + y^2 = r^2";
/// Longest element name, in characters.
pub const MAX_NAME_CHARS: usize = 64;
/// The head at the end of a new arrow (the old app's open V). Nothing at the start, and nothing on a line.
pub const DEFAULT_ARROW_END_HEAD: Head = Head::Arrow;

/// Why creating an element was refused. Each variant is one catalog code; `Internal` is the rest.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CreateError {
    /// A shape is under [`MIN_SIZE`].
    #[error("the {kind} is smaller than {MIN_SIZE} unit in {}", dims(*width, *height))]
    TooSmall {
        /// Kind name.
        kind: &'static str,
        /// The width is under the minimum.
        width: bool,
        /// The height is under the minimum.
        height: bool,
    },
    /// A line, arrow or stroke has too few points.
    #[error("a {kind} needs at least {min} points, and {have} were given")]
    TooFewPoints {
        /// Kind name.
        kind: &'static str,
        /// Fewest points.
        min: usize,
        /// Points given.
        have: usize,
    },
    /// The id is taken (live or deleted).
    #[error("the id {0} is already used by another element")]
    IdInUse(ElementId),
    /// The name breaks the name rules.
    #[error(
        "a name must be 1 to {MAX_NAME_CHARS} characters, must not start with `@` or `$`, and must not contain control characters"
    )]
    BadName,
    /// Another live element has the name.
    #[error("the name {name} is already used by element {owner}")]
    NameTaken {
        /// The name.
        name: String,
        /// Its owner.
        owner: ElementId,
    },
    /// No live element has this id.
    #[error("element {0} does not exist or is deleted")]
    NotFound(ElementId),
    /// Elements are locked.
    #[error("{} element(s) are locked", .0.len())]
    Locked(Vec<ElementId>),
    /// An arrow end cannot attach to this element.
    #[error("{kind} elements cannot be arrow targets")]
    NotBindable {
        /// The element.
        id: ElementId,
        /// Its kind.
        kind: &'static str,
        /// True for text that labels another element.
        label: bool,
    },
    /// An element cannot be connected to itself.
    #[error("an element cannot be connected to itself")]
    SameElement,
    /// The element cannot hold a label.
    #[error("{id} is a {kind}, which cannot hold a label")]
    NotLabelContainer {
        /// The element.
        id: ElementId,
        /// Its kind.
        kind: &'static str,
    },
    /// The container has a label already.
    #[error("{container} already has a label ({existing})")]
    AlreadyLabelled {
        /// The container.
        container: ElementId,
        /// The label.
        existing: ElementId,
    },
    /// A text element cannot be empty.
    #[error("a text element cannot be empty")]
    EmptyText,
    /// The text is longer than a text element can be.
    #[error("the text has {n} characters, and a text element holds at most {max}")]
    TextTooLong {
        /// Characters given.
        n: usize,
        /// The limit.
        max: usize,
    },
    /// A position or size is not a usable number.
    #[error("the position or size is not a finite number within the document limits")]
    NotFinite,
    /// The new element breaks a rule of the model.
    #[error("the new element {id} is invalid: {error}")]
    Invalid {
        /// The element.
        id: ElementId,
        /// The broken rule.
        error: ElementError,
    },
    /// The element named as a frame is not a frame.
    #[error("{id} is a {kind}, not a frame")]
    NotAFrame {
        /// The element.
        id: ElementId,
        /// Its kind.
        kind: &'static str,
    },
    /// A frame cannot be inside a frame.
    #[error("frames cannot be nested")]
    NestedFrame,
    /// Anything else: the store or a lower rule failed.
    #[error("{0}")]
    Internal(String),
}

fn dims(width: bool, height: bool) -> &'static str {
    match (width, height) {
        (true, true) => "width and height",
        (true, false) => "width",
        _ => "height",
    }
}

impl From<StoreError> for CreateError {
    fn from(e: StoreError) -> Self {
        CreateError::Internal(e.to_string())
    }
}
impl From<IndexError> for CreateError {
    fn from(e: IndexError) -> Self {
        CreateError::Internal(e.to_string())
    }
}
impl From<BindingError> for CreateError {
    fn from(e: BindingError) -> Self {
        match e {
            BindingError::Missing(id) => CreateError::NotFound(id),
            other => CreateError::Internal(other.to_string()),
        }
    }
}
impl From<ElbowEditError> for CreateError {
    fn from(e: ElbowEditError) -> Self {
        CreateError::Internal(e.to_string())
    }
}
impl From<LinearError> for CreateError {
    fn from(e: LinearError) -> Self {
        CreateError::Internal(e.to_string())
    }
}
impl From<ArrowError> for CreateError {
    fn from(e: ArrowError) -> Self {
        match e {
            ArrowError::Missing(id) => CreateError::NotFound(id),
            ArrowError::NotBindable { id, kind, label } => CreateError::NotBindable { id, kind, label },
            other => CreateError::Internal(other.to_string()),
        }
    }
}
impl From<FrameError> for CreateError {
    fn from(e: FrameError) -> Self {
        match e {
            FrameError::NotFound(id) => CreateError::NotFound(id),
            FrameError::Locked(ids) => CreateError::Locked(ids),
            FrameError::NotAFrame { id, kind } => CreateError::NotAFrame { id, kind },
            FrameError::NestedFrame(_) => CreateError::NestedFrame,
            other => CreateError::Internal(other.to_string()),
        }
    }
}
impl From<ZOrderError> for CreateError {
    fn from(e: ZOrderError) -> Self {
        match e {
            ZOrderError::NotFound(id) => CreateError::NotFound(id),
            ZOrderError::Locked(ids) => CreateError::Locked(ids),
            other => CreateError::Internal(other.to_string()),
        }
    }
}
impl From<TargetError> for CreateError {
    fn from(e: TargetError) -> Self {
        match e {
            TargetError::NotFound(id) => CreateError::NotFound(id),
            TargetError::Locked(ids) => CreateError::Locked(ids),
        }
    }
}
impl From<TextBindError> for CreateError {
    fn from(e: TextBindError) -> Self {
        match e {
            TextBindError::Missing(id) => CreateError::NotFound(id),
            TextBindError::NotContainer { id, kind } => CreateError::NotLabelContainer { id, kind },
            TextBindError::AlreadyLabelled { container, existing } => {
                CreateError::AlreadyLabelled { container, existing }
            }
            TextBindError::Locked(id) => CreateError::Locked(vec![id]),
            TextBindError::EmptyText => CreateError::EmptyText,
            other => CreateError::Internal(other.to_string()),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Sizes, ids, names
// ---------------------------------------------------------------------------------------------

/// A box from a corner and a size that may be negative: the same box with a positive size.
pub fn normalized_box(x: f64, y: f64, w: f64, h: f64) -> (f64, f64, f64, f64) {
    let (x0, x1) = if w < 0.0 { (x + w, x) } else { (x, x + w) };
    let (y0, y1) = if h < 0.0 { (y + h, y) } else { (y, y + h) };
    (x0, y0, x1 - x0, y1 - y0)
}

/// A rectangle, diamond, ellipse or frame of this size is allowed: both sides at least [`MIN_SIZE`].
pub fn check_box_size(kind: Kind, w: f64, h: f64) -> Result<(), CreateError> {
    let (bad_w, bad_h) = (w < MIN_SIZE, h < MIN_SIZE);
    if bad_w || bad_h {
        return Err(CreateError::TooSmall { kind: kind.name(), width: bad_w, height: bad_h });
    }
    Ok(())
}

/// A line or arrow with points spanning `w` by `h` is allowed: it reaches [`MIN_SIZE`] in one direction (the old
/// rule: add when width or height is above 1). A horizontal line has height 0 and is fine.
pub fn check_extent(kind: Kind, w: f64, h: f64) -> Result<(), CreateError> {
    if w < MIN_SIZE && h < MIN_SIZE {
        return Err(CreateError::TooSmall { kind: kind.name(), width: true, height: true });
    }
    Ok(())
}

/// The element id must be free (a deleted element still owns its id).
pub fn ensure_id_free<R: SceneRead>(scene: &R, id: &ElementId) -> Result<(), CreateError> {
    if scene.contains(id) { Err(CreateError::IdInUse(id.clone())) } else { Ok(()) }
}

/// The name rules of `element.set_name`.
pub fn validate_name(name: &str) -> Result<(), CreateError> {
    let n = name.chars().count();
    if n == 0
        || n > MAX_NAME_CHARS
        || name.starts_with('@')
        || name.starts_with('$')
        || name.chars().any(char::is_control)
    {
        return Err(CreateError::BadName);
    }
    Ok(())
}

/// No live element other than `except` has this name.
pub fn ensure_name_free<R: SceneRead>(scene: &R, name: &str, except: Option<&ElementId>) -> Result<(), CreateError> {
    match scene.iter().find(|e| e.name.as_deref() == Some(name) && Some(&e.id) != except) {
        Some(owner) => Err(CreateError::NameTaken { name: name.to_owned(), owner: owner.id.clone() }),
        None => Ok(()),
    }
}

/// Sets or clears the name of a live element after checking the rules. Returns whether it changed.
pub fn set_name<T: SceneTx>(tx: &mut T, id: &ElementId, name: Option<&str>) -> Result<bool, CreateError> {
    let current = tx.get(id).filter(|e| !e.deleted).ok_or_else(|| CreateError::NotFound(id.clone()))?.name.clone();
    if let Some(n) = name {
        validate_name(n)?;
        ensure_name_free(&*tx, n, Some(id))?;
    }
    if current.as_deref() == name {
        return Ok(false);
    }
    let next = name.map(str::to_owned);
    tx.update(id, |e| e.name = next)?;
    Ok(true)
}

// ---------------------------------------------------------------------------------------------
// Styles and strokes
// ---------------------------------------------------------------------------------------------

/// The style of a highlighter stroke: `base` with the width raised to [`MIN_HIGHLIGHTER_WIDTH`] unless the caller
/// gave a width, and the alpha of the colour set to [`HIGHLIGHTER_ALPHA`] unless the caller gave a colour.
pub fn highlighter_style(mut base: Style, width_given: bool, color_given: bool) -> Style {
    if !width_given {
        base.stroke_width = base.stroke_width.max(MIN_HIGHLIGHTER_WIDTH);
    }
    if !color_given {
        base.stroke_color.a = HIGHLIGHTER_ALPHA;
    }
    base
}

/// Ramer-Douglas-Peucker simplification of a stroke, keeping the pressure of every kept point. Fewer than three
/// points, or a tolerance that is not above zero, keep the stroke as it is. `pressures` is empty or one per point.
pub fn simplify_stroke(points: &[Point], pressures: &[f64], tolerance: f64) -> (Vec<Point>, Vec<f64>) {
    let with_pressure = pressures.len() == points.len() && !pressures.is_empty();
    if points.len() < 3 || !tolerance.is_finite() || tolerance <= 0.0 {
        return (points.to_vec(), if with_pressure { pressures.to_vec() } else { Vec::new() });
    }
    let mut keep = vec![false; points.len()];
    if let Some(k) = keep.first_mut() {
        *k = true;
    }
    if let Some(k) = keep.last_mut() {
        *k = true;
    }
    // Iterative to bound the stack on long strokes.
    let mut stack = vec![(0usize, points.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (pa, pb) = (points.get(a).copied().unwrap_or(Point::ZERO), points.get(b).copied().unwrap_or(Point::ZERO));
        let mut far = (0.0_f64, a);
        for (i, p) in points.iter().enumerate().take(b).skip(a + 1) {
            let d = distance_to_segment(*p, pa, pb);
            if d > far.0 {
                far = (d, i);
            }
        }
        if far.0 > tolerance {
            if let Some(k) = keep.get_mut(far.1) {
                *k = true;
            }
            stack.push((a, far.1));
            stack.push((far.1, b));
        }
    }
    let mut pts = Vec::new();
    let mut prs = Vec::new();
    for (i, k) in keep.iter().enumerate() {
        if *k {
            if let Some(p) = points.get(i) {
                pts.push(*p);
            }
            if with_pressure && let Some(p) = pressures.get(i) {
                prs.push(*p);
            }
        }
    }
    (pts, prs)
}

fn distance_to_segment(p: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let len2 = ab.hypot2();
    if len2 <= f64::EPSILON {
        return (p - a).hypot();
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    (p - (a + ab * t)).hypot()
}

// ---------------------------------------------------------------------------------------------
// Builders
// ---------------------------------------------------------------------------------------------

/// A rectangle, diamond, ellipse or frame (`kind` must be one of them) in the box `(x, y, w, h)`.
pub fn shape_element(
    id: ElementId,
    kind: Kind,
    index: ZIndex,
    (x, y, w, h): (f64, f64, f64, f64),
    style: Style,
) -> Result<Element, CreateError> {
    let ek = match kind {
        Kind::Rectangle => ElementKind::Rectangle,
        Kind::Diamond => ElementKind::Diamond,
        Kind::Ellipse => ElementKind::Ellipse,
        Kind::Frame => ElementKind::Frame(FrameData::default()),
        other => return Err(CreateError::Internal(format!("{} is not a box shape", other.name()))),
    };
    let mut e = Element::new(id, ek, index);
    e.transform = Transform::new(x, y, w, h);
    e.style = style;
    Ok(e)
}

/// How a new line or arrow is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinearSpec {
    /// Line or arrow.
    pub kind: Kind,
    /// Sharp, round or elbow. Round and elbow are applied after the ends are bound.
    pub path: PathType,
    /// Marker at the first point.
    pub start_head: Head,
    /// Marker at the last point.
    pub end_head: Head,
    /// The last point connects back to the first (lines only).
    pub closed: bool,
}

impl LinearSpec {
    /// The defaults of a kind: an arrow has an `arrow` end head, a line has none.
    pub fn defaults(kind: Kind) -> Self {
        Self {
            kind,
            path: PathType::Sharp,
            start_head: Head::None,
            end_head: if kind == Kind::Arrow { DEFAULT_ARROW_END_HEAD } else { Head::None },
            closed: false,
        }
    }
}

/// A line or arrow through `world` (at least two points). The path is created `Sharp`; see [`finish_path`].
pub fn linear_element(
    id: ElementId,
    index: ZIndex,
    world: &[Point],
    spec: &LinearSpec,
    style: Style,
) -> Result<Element, CreateError> {
    let min = if spec.closed { 3 } else { 2 };
    if world.len() < min {
        return Err(CreateError::TooFewPoints { kind: spec.kind.name(), min, have: world.len() });
    }
    let mut e = Element::linear(id, spec.kind, index, world)
        .ok_or_else(|| CreateError::Internal("the points are not finite".to_owned()))?;
    if let ElementKind::Line(d) | ElementKind::Arrow(d) = &mut e.kind {
        d.start_head = spec.start_head;
        d.end_head = spec.end_head;
        d.closed = spec.closed;
    }
    e.style = style;
    Ok(e)
}

/// How a freehand stroke is drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct StrokeSpec {
    /// Pen pressure per point, 0 to 1: empty or one per point.
    pub pressures: Vec<f64>,
    /// How pressure shapes the stroke.
    pub pressure_mode: PressureMode,
    /// Brush shape.
    pub brush: Brush,
    /// A marker stroke drawn under the other elements' ink.
    pub highlighter: bool,
}

/// A freehand stroke through `world` (at least one point; one point is a dot).
pub fn freehand_element(
    id: ElementId,
    index: ZIndex,
    world: &[Point],
    spec: StrokeSpec,
    style: Style,
) -> Result<Element, CreateError> {
    if world.is_empty() {
        return Err(CreateError::TooFewPoints { kind: "freehand", min: 1, have: 0 });
    }
    let mut d = FreehandData::new(world.to_vec());
    d.pressures = spec.pressures;
    d.pressure_mode = spec.pressure_mode;
    d.brush = spec.brush;
    d.highlighter = spec.highlighter;
    let mut e = Element::new(id, ElementKind::Freehand(d), index);
    e.style = style;
    if !e.refit_points() {
        return Err(CreateError::Internal("the points are not finite".to_owned()));
    }
    Ok(e)
}

/// The point a free text is placed by.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextPlace {
    /// The top-left corner of the box.
    TopLeft(Point),
    /// The middle of the box (a pasted text).
    Center(Point),
}

/// Where and how a free text sits.
#[derive(Debug, Clone, PartialEq)]
pub struct TextSpec {
    /// Font settings.
    pub font: TextStyle,
    /// Where the box is.
    pub place: TextPlace,
    /// With a width the box has that width and the text wraps (auto-resize off); without, the box follows the
    /// content.
    pub width: Option<f64>,
}

/// A free text: the one builder for typed, created and pasted text. Blank text, text over the length limit,
/// a box that is not a finite number within the document limits, and a text that breaks a model rule are
/// refused.
pub fn text_element<M: TextMeasure + ?Sized>(
    m: &M,
    id: ElementId,
    index: ZIndex,
    content: &str,
    spec: TextSpec,
    style: Style,
) -> Result<Element, CreateError> {
    if content.trim().is_empty() {
        return Err(CreateError::EmptyText);
    }
    let n = content.chars().count();
    if n > limits::MAX_TEXT_CHARS {
        return Err(CreateError::TextTooLong { n, max: limits::MAX_TEXT_CHARS });
    }
    let size = text_bind::measure_text(m, content, &spec.font, spec.width);
    let (w, h) = (spec.width.unwrap_or(size.width), size.height);
    let (x, y) = match spec.place {
        TextPlace::TopLeft(p) => (p.x, p.y),
        TextPlace::Center(p) => (p.x - w / 2.0, p.y - h / 2.0),
    };
    let mut data = TextData::new(content);
    data.font = spec.font;
    data.auto_resize = spec.width.is_none();
    let mut e = Element::new(id, ElementKind::Text(data), index);
    e.transform = Transform::new(x, y, w, h);
    e.style = style;
    if ![x, y, w, h].iter().all(|v| v.is_finite() && v.abs() <= limits::MAX_COORD) {
        return Err(CreateError::NotFinite);
    }
    validate_element(&e).map_err(|error| CreateError::Invalid { id: e.id.clone(), error })?;
    Ok(e)
}

/// A formula whose box (top-left `at`) has the measured `size`.
pub fn math_element(
    id: ElementId,
    index: ZIndex,
    latex: &str,
    font_size: f64,
    at: Point,
    size: Size,
    style: Style,
) -> Element {
    let mut data = MathData::new(latex);
    data.font_size = font_size;
    let mut e = Element::new(id, ElementKind::Math(data), index);
    e.transform = Transform::new(at.x, at.y, size.width, size.height);
    e.style = style;
    e
}

/// Inserts the element `make` builds with the next key above everything. Returns its id.
pub fn insert_on_top<T: SceneTx>(
    tx: &mut T,
    make: impl FnOnce(ZIndex) -> Result<Element, CreateError>,
) -> Result<ElementId, CreateError> {
    let z = tx.next_top_index()?;
    let e = make(z)?;
    let id = e.id.clone();
    tx.insert(e)?;
    Ok(id)
}

/// An id source that hands out `first` once and then asks `inner`. A label made by
/// [`crate::text_bind::create_label`] draws its own id; this lets the caller choose it.
pub struct PresetId<'a> {
    first: Option<ElementId>,
    inner: &'a mut dyn IdSource,
}

impl<'a> PresetId<'a> {
    /// `first` is the next element id; `inner` supplies everything else.
    pub fn new(first: Option<ElementId>, inner: &'a mut dyn IdSource) -> Self {
        Self { first, inner }
    }
}

impl IdSource for PresetId<'_> {
    fn element_id(&mut self) -> ElementId {
        match self.first.take() {
            Some(id) => id,
            None => self.inner.element_id(),
        }
    }
    fn group_id(&mut self) -> drafftink_scene::GroupId {
        self.inner.group_id()
    }
    fn seed(&mut self) -> u32 {
        self.inner.seed()
    }
}

// ---------------------------------------------------------------------------------------------
// Binding the ends of a new arrow
// ---------------------------------------------------------------------------------------------

/// How an end of a new arrow gets its binding.
#[derive(Debug, Clone, PartialEq)]
pub enum EndBinding {
    /// Left free.
    Free,
    /// Bound like a dragged end: to the shape under the point, if any.
    Auto,
    /// Bound to this target.
    Target {
        /// The target.
        target: ElementId,
        /// Inside or orbit.
        mode: BindMode,
        /// Where on the target; `None` picks the outline point facing the arrow.
        fixed_point: Option<[f64; 2]>,
    },
}

/// What happens to the ends that are [`EndBinding::Auto`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoBind {
    /// Bind to a shape under the end: `Inside` in the shape, `Orbit` in the margin.
    On,
    /// Do not bind (Ctrl held, or the setting `arrow.binding` is off).
    Off,
    /// Bind to a shape under the end, always `Inside` (Alt held).
    Inside,
}

/// Where the ends of a new arrow ended up bound.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BoundEnds {
    /// Target of the start binding.
    pub start: Option<ElementId>,
    /// Target of the end binding.
    pub end: Option<ElementId>,
}

/// Binds the ends of the freshly inserted line or arrow `arrow` and solves it. Explicit targets go first, so an
/// automatic end sees them. Then the path is set ([`finish_path`]) and the bindings are solved once more.
#[allow(clippy::too_many_arguments)]
pub fn bind_new_ends<T: SceneTx>(
    tx: &mut T,
    arrow: &ElementId,
    start: &EndBinding,
    end: &EndBinding,
    auto: AutoBind,
    path: PathType,
    zoom: f64,
    midpoint_snap: bool,
) -> Result<BoundEnds, CreateError> {
    let el = tx.get(arrow).filter(|e| !e.deleted).ok_or_else(|| CreateError::NotFound(arrow.clone()))?.clone();
    let world = world_points(&el).ok_or_else(|| CreateError::Internal(format!("{arrow} is not a line or arrow")))?;
    let (Some(first), Some(last)) = (world.first().copied(), world.last().copied()) else {
        return Err(CreateError::TooFewPoints { kind: el.tag().name(), min: 2, have: world.len() });
    };
    for (which, spec) in [(End::Start, start), (End::End, end)] {
        if let EndBinding::Target { target, mode, fixed_point } = spec {
            let req = BindRequest { target, mode: *mode, fixed_point: *fixed_point, zoom, midpoint_snap };
            bind_end(tx, arrow, which, &req)?;
        }
    }
    let opts = BindOpts {
        zoom,
        force_inside: auto == AutoBind::Inside,
        disabled: auto == AutoBind::Off,
        midpoint_snap,
        angle_locked: false,
    };
    if auto != AutoBind::Off {
        for (which, spec, at) in [(End::Start, start, first), (End::End, end, last)] {
            if *spec == EndBinding::Auto {
                binding::drag_end(tx, arrow, which, at, &opts)?;
            }
        }
    }
    finish_path(tx, arrow, path)?;
    binding::update_bound_arrows(tx, std::slice::from_ref(arrow))?;
    let bound = tx.get(arrow).map(Element::binding_targets).map_or((None, None), |[a, b]| (a.cloned(), b.cloned()));
    Ok(BoundEnds { start: bound.0, end: bound.1 })
}

/// Sets the path of a new arrow after its ends are bound (round keeps the points; elbow keeps the two ends and
/// routes between them).
pub fn finish_path<T: SceneTx>(tx: &mut T, arrow: &ElementId, path: PathType) -> Result<(), CreateError> {
    if path != PathType::Sharp {
        linear::set_path(tx, arrow, path)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Placement
// ---------------------------------------------------------------------------------------------

/// Which frame a new element joins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameChoice {
    /// The frame under the element's centre, if any.
    Auto,
    /// No frame.
    None,
    /// This frame.
    Frame(ElementId),
}

/// What [`join_frame`] did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Joined {
    /// The frame the element joined.
    pub frame: Option<ElementId>,
    /// The named frame refused the element (an arrow with no end on a member): it is outside every frame. Never set
    /// for `Auto`, where a frame that does not take the element is simply not used.
    pub left_out: Option<ElementId>,
}

/// Puts the new element `id` into a frame by `choice`. A frame never joins a frame. With `Auto`, the topmost
/// unlocked frame under the element's centre is used; an element the frame does not take stays out silently.
pub fn join_frame<T: SceneTx>(tx: &mut T, id: &ElementId, choice: &FrameChoice) -> Result<Joined, CreateError> {
    let e = tx.get(id).filter(|e| !e.deleted).ok_or_else(|| CreateError::NotFound(id.clone()))?.clone();
    let target = match choice {
        FrameChoice::None => return Ok(Joined::default()),
        FrameChoice::Auto if e.tag() == Kind::Frame => return Ok(Joined::default()),
        FrameChoice::Frame(f) if e.tag() == Kind::Frame => {
            let _ = f;
            return Err(CreateError::NestedFrame);
        }
        FrameChoice::Auto => {
            let centre = e.transform.center();
            match frame::frame_at(&*tx, centre, &BTreeSet::from([id.clone()])) {
                Some(f) => f,
                None => return Ok(Joined::default()),
            }
        }
        FrameChoice::Frame(f) => f.clone(),
    };
    let report = frame::add_to_frame(tx, &target, std::slice::from_ref(id))?;
    if report.added.contains(id) {
        Ok(Joined { frame: Some(target), left_out: None })
    } else if matches!(choice, FrameChoice::Frame(_)) {
        Ok(Joined { frame: None, left_out: Some(target) })
    } else {
        Ok(Joined::default())
    }
}

/// Moves `id` directly above `after` in the z-order. The new element does not split a group, the children of a
/// frame, or a container and its label: it goes above the whole enclosure (above the container's label when it
/// has one). Does nothing when `after` is `id`.
pub fn place_above<T: SceneTx>(tx: &mut T, id: &ElementId, after: &ElementId) -> Result<(), CreateError> {
    if id == after {
        return Ok(());
    }
    let start = tx.get(after).filter(|e| !e.deleted).ok_or_else(|| CreateError::NotFound(after.clone()))?.clone();
    let joined_frame = tx.get(id).and_then(|e| e.frame_id.clone());
    let rel = Relations::build(&*tx);
    let anchor = zorder::lift_above_enclosures(&*tx, &rel, &start, &|f| joined_frame.as_ref() == Some(f), &|_| false);
    // A container's label sits directly above it; the new element goes above the pair.
    let top = rel
        .labels(&anchor.id)
        .iter()
        .filter_map(|l| tx.get(l))
        .max_by(|a, b| a.index.cmp(&b.index))
        .map(|l| l.id.clone());
    let anchor_id = top.filter(|l| l != id).unwrap_or(anchor.id);
    if &anchor_id == id {
        return Ok(());
    }
    zorder::move_block(tx, std::slice::from_ref(id), &Place::Above(anchor_id))?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Connecting two elements
// ---------------------------------------------------------------------------------------------

/// How [`connect`] draws the arrow.
#[derive(Debug, Clone, PartialEq)]
pub struct ConnectSpec {
    /// Arrow or line.
    pub linear: LinearSpec,
    /// Inside or orbit at both ends.
    pub mode: BindMode,
    /// The style of the new arrow (its seed is replaced by one from the id source).
    pub style: Style,
    /// Displayed zoom (1 is 100%).
    pub zoom: f64,
    /// Snap to side midpoints near them.
    pub midpoint_snap: bool,
}

impl ConnectSpec {
    /// An orbit arrow with the old defaults, at zoom 1.
    pub fn arrow(style: Style) -> Self {
        Self {
            linear: LinearSpec::defaults(Kind::Arrow),
            mode: BindMode::Orbit,
            style,
            zoom: 1.0,
            midpoint_snap: false,
        }
    }
}

/// Creates an arrow (or line) from `from` to `to`, bound to both at the sides that face each other, solved and
/// routed, on top of the z-order. When both shapes are children of the same frame the arrow joins it.
pub fn connect<T: SceneTx>(
    tx: &mut T,
    ids: &mut dyn IdSource,
    from: &ElementId,
    to: &ElementId,
    spec: &ConnectSpec,
) -> Result<ElementId, CreateError> {
    if from == to {
        return Err(CreateError::SameElement);
    }
    let a = tx.get(from).filter(|e| !e.deleted).ok_or_else(|| CreateError::NotFound(from.clone()))?.clone();
    let b = tx.get(to).filter(|e| !e.deleted).ok_or_else(|| CreateError::NotFound(to.clone()))?.clone();
    for e in [&a, &b] {
        if binding::resolve(&*tx, &e.id).is_none() {
            let label = matches!(&e.kind, ElementKind::Text(t) if t.container_id.is_some());
            return Err(CreateError::NotBindable { id: e.id.clone(), kind: e.tag().name(), label });
        }
    }
    let locked: Vec<ElementId> = [&a, &b].into_iter().filter(|e| e.locked).map(|e| e.id.clone()).collect();
    if !locked.is_empty() {
        return Err(CreateError::Locked(locked));
    }
    let (ca, mut cb) = (a.transform.center(), b.transform.center());
    if (ca - cb).hypot() < 1e-6 {
        cb.x += 1.0;
    }
    let id = text_bind::fresh_element_id(&*tx, ids);
    let mut style = spec.style.clone();
    style.seed = ids.seed();
    let arrow = insert_on_top(tx, |z| linear_element(id, z, &[ca, cb], &spec.linear, style))?;
    let end = |target: &ElementId| EndBinding::Target { target: target.clone(), mode: spec.mode, fixed_point: None };
    bind_new_ends(tx, &arrow, &end(from), &end(to), AutoBind::Off, spec.linear.path, spec.zoom, spec.midpoint_snap)?;
    if let (Some(fa), Some(fb)) = (&a.frame_id, &b.frame_id)
        && fa == fb
    {
        frame::add_to_frame(tx, fa, std::slice::from_ref(&arrow))?;
    }
    Ok(arrow)
}

/// The linear data of the element, for callers that need to read heads and bindings back.
pub fn linear_of<R: SceneRead>(scene: &R, id: &ElementId) -> Option<LinearData> {
    scene.get(id).and_then(|e| e.linear_data().cloned())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use drafftink_scene::{MemoryStore, SceneStore, SeqIds};

    fn pt(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    #[test]
    fn negative_boxes_are_normalised() {
        assert_eq!(normalized_box(10.0, 10.0, -4.0, -6.0), (6.0, 4.0, 4.0, 6.0));
        assert_eq!(normalized_box(1.0, 2.0, 3.0, 4.0), (1.0, 2.0, 3.0, 4.0));
    }

    #[test]
    fn box_sizes_need_one_unit_each_way_and_lines_one_in_either() {
        assert!(check_box_size(Kind::Rectangle, 1.0, 1.0).is_ok());
        assert_eq!(
            check_box_size(Kind::Rectangle, 0.5, 40.0),
            Err(CreateError::TooSmall { kind: "rectangle", width: true, height: false })
        );
        assert!(check_extent(Kind::Line, 100.0, 0.0).is_ok());
        assert!(check_extent(Kind::Arrow, 0.2, 0.9).is_err());
    }

    #[test]
    fn names_follow_the_rules() {
        assert!(validate_name("box").is_ok());
        for bad in ["", "@x", "$x", "a\nb", &"x".repeat(65)] {
            assert_eq!(validate_name(bad), Err(CreateError::BadName), "{bad:?}");
        }
        assert!(validate_name(&"x".repeat(64)).is_ok());
    }

    #[test]
    fn rdp_keeps_the_ends_and_the_corners_with_their_pressures() {
        let pts = [pt(0.0, 0.0), pt(5.0, 0.1), pt(10.0, 0.0), pt(10.0, 10.0)];
        let pr = [0.1, 0.2, 0.3, 0.4];
        let (p, q) = simplify_stroke(&pts, &pr, 2.0);
        assert_eq!(p, vec![pt(0.0, 0.0), pt(10.0, 0.0), pt(10.0, 10.0)]);
        assert_eq!(q, vec![0.1, 0.3, 0.4]);
        let (p, q) = simplify_stroke(&pts, &[], 0.0);
        assert_eq!((p.len(), q.len()), (4, 0));
    }

    #[test]
    fn highlighter_raises_width_and_sets_alpha_unless_told() {
        let base = Style::default();
        let h = highlighter_style(base.clone(), false, false);
        assert_eq!((h.stroke_width, h.stroke_color.a), (12.0, 128));
        let kept = highlighter_style(base, true, true);
        assert_eq!((kept.stroke_width, kept.stroke_color.a), (2.0, 255));
    }

    #[test]
    fn preset_id_is_used_once() {
        let mut inner = SeqIds::new();
        let want = ElementId::new("mine").unwrap();
        let mut p = PresetId::new(Some(want.clone()), &mut inner);
        assert_eq!(p.element_id(), want);
        assert_ne!(p.element_id(), want);
    }

    #[test]
    fn connect_binds_both_ends_and_refuses_the_same_element() {
        let mut store = MemoryStore::new();
        let mut ids = SeqIds::new();
        let mut tx = store.begin();
        let a = insert_on_top(&mut tx, |z| {
            shape_element(ElementId::new("a").unwrap(), Kind::Rectangle, z, (0.0, 0.0, 100.0, 60.0), Style::default())
        })
        .unwrap();
        let b = insert_on_top(&mut tx, |z| {
            shape_element(ElementId::new("b").unwrap(), Kind::Rectangle, z, (300.0, 0.0, 100.0, 60.0), Style::default())
        })
        .unwrap();
        let spec = ConnectSpec::arrow(Style::default());
        assert_eq!(connect(&mut tx, &mut ids, &a, &a, &spec), Err(CreateError::SameElement));
        let arrow = connect(&mut tx, &mut ids, &a, &b, &spec).unwrap();
        let e = tx.get(&arrow).unwrap();
        let d = e.linear_data().unwrap();
        assert_eq!(d.start_binding.as_ref().map(|x| &x.target), Some(&a));
        assert_eq!(d.end_binding.as_ref().map(|x| &x.target), Some(&b));
        assert_eq!((d.start_head, d.end_head), (Head::None, Head::Arrow));
        assert!(binding::stale_arrows(&tx).is_empty());
    }

    fn rect(tx: &mut impl SceneTx, id: &str, x: f64, y: f64, w: f64, h: f64) -> ElementId {
        insert_on_top(tx, |z| {
            shape_element(ElementId::new(id).unwrap(), Kind::Rectangle, z, (x, y, w, h), Style::default())
        })
        .unwrap()
    }

    fn frame_el(tx: &mut impl SceneTx, id: &str, x: f64, y: f64, w: f64, h: f64) -> ElementId {
        insert_on_top(tx, |z| {
            shape_element(ElementId::new(id).unwrap(), Kind::Frame, z, (x, y, w, h), Style::default())
        })
        .unwrap()
    }

    #[test]
    fn set_name_reports_change_and_refuses_taken_and_missing() {
        let mut store = MemoryStore::new();
        let mut tx = store.begin();
        let a = rect(&mut tx, "a", 0.0, 0.0, 10.0, 10.0);
        let b = rect(&mut tx, "b", 20.0, 0.0, 10.0, 10.0);
        assert_eq!(set_name(&mut tx, &a, Some("box")), Ok(true));
        assert_eq!(set_name(&mut tx, &a, Some("box")), Ok(false));
        assert_eq!(
            set_name(&mut tx, &b, Some("box")),
            Err(CreateError::NameTaken { name: "box".into(), owner: a.clone() })
        );
        assert_eq!(set_name(&mut tx, &a, None), Ok(true));
        assert_eq!(set_name(&mut tx, &b, Some("box")), Ok(true));
        assert_eq!(
            set_name(&mut tx, &ElementId::new("zz").unwrap(), Some("x")),
            Err(CreateError::NotFound(ElementId::new("zz").unwrap()))
        );
    }

    #[test]
    fn join_frame_follows_the_choice() {
        let mut store = MemoryStore::new();
        let mut tx = store.begin();
        let f = frame_el(&mut tx, "f", 0.0, 0.0, 200.0, 200.0);
        let inside = rect(&mut tx, "in", 50.0, 50.0, 20.0, 20.0);
        let outside = rect(&mut tx, "out", 500.0, 50.0, 20.0, 20.0);
        assert_eq!(join_frame(&mut tx, &inside, &FrameChoice::Auto).unwrap().frame, Some(f.clone()));
        assert_eq!(join_frame(&mut tx, &outside, &FrameChoice::Auto).unwrap(), Joined::default());
        assert_eq!(join_frame(&mut tx, &outside, &FrameChoice::None).unwrap(), Joined::default());
        assert_eq!(join_frame(&mut tx, &outside, &FrameChoice::Frame(f.clone())).unwrap().frame, Some(f.clone()));
        assert_eq!(
            join_frame(&mut tx, &f, &FrameChoice::Auto).unwrap(),
            Joined::default(),
            "a frame never joins a frame"
        );
        assert_eq!(join_frame(&mut tx, &f, &FrameChoice::Frame(f.clone())), Err(CreateError::NestedFrame));
        let other = rect(&mut tx, "o2", 0.0, 0.0, 5.0, 5.0);
        assert!(matches!(join_frame(&mut tx, &inside, &FrameChoice::Frame(other)), Err(CreateError::NotAFrame { .. })));
    }

    #[test]
    fn place_above_goes_directly_above_the_anchor() {
        let mut store = MemoryStore::new();
        let mut tx = store.begin();
        let a = rect(&mut tx, "a", 0.0, 0.0, 10.0, 10.0);
        let _b = rect(&mut tx, "b", 0.0, 0.0, 10.0, 10.0);
        let _c = rect(&mut tx, "c", 0.0, 0.0, 10.0, 10.0);
        let n = rect(&mut tx, "n", 0.0, 0.0, 10.0, 10.0);
        place_above(&mut tx, &n, &a).unwrap();
        let order: Vec<String> = tx.iter().map(|e| e.id.as_str().to_owned()).collect();
        assert_eq!(order, ["a", "n", "b", "c"]);
        assert_eq!(
            place_above(&mut tx, &n, &ElementId::new("zz").unwrap()),
            Err(CreateError::NotFound(ElementId::new("zz").unwrap()))
        );
    }

    #[test]
    fn new_ends_bind_by_the_auto_policy() {
        for (policy, point, mode) in [
            (AutoBind::On, 50.0, Some(BindMode::Inside)),
            (AutoBind::On, 104.0, Some(BindMode::Orbit)),
            (AutoBind::Inside, 104.0, Some(BindMode::Inside)),
            (AutoBind::Off, 50.0, None),
        ] {
            let mut store = MemoryStore::new();
            let mut tx = store.begin();
            let a = rect(&mut tx, "a", 0.0, 0.0, 100.0, 60.0);
            let id = insert_on_top(&mut tx, |z| {
                linear_element(
                    ElementId::new("ar").unwrap(),
                    z,
                    &[pt(point, 30.0), pt(400.0, 200.0)],
                    &LinearSpec::defaults(Kind::Arrow),
                    Style::default(),
                )
            })
            .unwrap();
            let bound =
                bind_new_ends(&mut tx, &id, &EndBinding::Auto, &EndBinding::Auto, policy, PathType::Sharp, 1.0, false)
                    .unwrap();
            let got = tx.get(&id).unwrap().linear_data().unwrap().start_binding.as_ref().map(|b| b.mode);
            assert_eq!((got, bound.start.is_some()), (mode, mode.is_some()), "{policy:?} at {point}");
            if mode.is_some() {
                assert_eq!(bound.start, Some(a));
            }
            assert_eq!(bound.end, None);
        }
    }
}
