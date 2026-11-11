//! Text bound to a container: labels on rectangles, diamonds, ellipses and arrows.
//!
//! The text element stores its `container_id` (the single authoritative side of the relation). Everything else
//! about a label is derived from the container and the text, so it can always be recomputed:
//!
//! * **Wrapping width** is the inner width of the container: `w - 2P` for a rectangle, `round(w / sqrt 2) - 2P` for an
//!   ellipse, `round(w / 2) - 2P` for a diamond (`P` is [`BOUND_TEXT_PADDING`]). An arrow label wraps at
//!   `max(0.7 * arrow width, 11 * font size)`.
//! * **Growth.** A shape grows when the wrapped text is taller than its inner height (it never shrinks by itself).
//!   It also grows wider when a single character is wider than the inner width.
//! * **Placement** follows the text's `align` and `vertical_align` inside the inner box. The label turns with its
//!   container. An arrow label sits at `label_position` (0 to 1, by arc length) and is never rotated.
//!
//! Text measurement goes through [`TextMeasure`]. The engine adapts the typesetter to it; tests use
//! [`MonospaceMeasure`]. Wrapping is a function of the measure, so the shape that is drawn and the shape that was
//! sized agree as long as the renderer wraps at the same width with the same measure.
//!
//! The sizing rules (inner boxes, the inverse container size, grow-only containers, the wrap algorithm, arrow label
//! width) are adapted from Excalidraw (MIT), `element/src/textElement.ts` and `textWrapping.ts`; see `ATTRIBUTION.md`.
//!
//! Callers run these functions inside one transaction. After a change that moves, resizes, rotates or reroutes a
//! container, call [`refresh_labels`] with the changed ids (after `binding::update_bound_arrows`).

mod container;
mod font;
mod math;
mod measure;
mod runs;

pub use container::{Wrapped, wrap_in_container};
pub use font::{
    FONT_PRESETS, FONT_STEP, FontStep, MAX_FONT_SIZE, MIN_FONT_SIZE, StepReport, TextProps, anchor_top_left,
    apply_text_props, next_font_size, remeasure_text, step_font_size,
};
pub use math::{MathMeasure, set_latex, set_math_size};
pub use measure::{MonospaceMeasure, TextMeasure, measure_block, measure_lines, measure_text, wrap_text};
pub use runs::{FormatReport, carry_runs, format_range, format_texts, replace_runs, style_range};

use std::collections::HashMap;

use drafftink_geom::kurbo::{Point, Rect, Size};
use drafftink_geom::transform::rotate_point;
use drafftink_profile::{Counter, count};

use drafftink_scene::{
    Element, ElementError, ElementId, ElementKind, IdSource, IndexError, Kind, SceneRead, SceneTx, StoreError, Style,
    TextAlign, TextData, TextRun, TextStyle, Transform, VerticalAlign, ZIndex,
};
use drafftink_typeset::MathError;

use crate::binding::{BindingError, update_bound_arrows};
use crate::create::{FreeTextError, TextPlace, TextSpec, text_element};
use crate::frame::FrameError;
use crate::ids::unused_element_id;
use crate::target::TargetError;

/// Space between a container's outline and its label, on every side.
pub const BOUND_TEXT_PADDING: f64 = 5.0;
/// An arrow label wraps at this fraction of the arrow's width, or at [`ARROW_LABEL_MIN_WIDTH_PER_SIZE`] times the
/// font size, whichever is wider.
pub const ARROW_LABEL_WIDTH_FRACTION: f64 = 0.7;
/// See [`ARROW_LABEL_WIDTH_FRACTION`].
pub const ARROW_LABEL_MIN_WIDTH_PER_SIZE: f64 = 11.0;
/// Where an arrow label sits by default: the middle of the arrow.
pub const DEFAULT_LABEL_POSITION: f64 = 0.5;

// ---------------------------------------------------------------------------------------------
// Container geometry
// ---------------------------------------------------------------------------------------------

/// Size of the area a label may use in a container of the given kind and outer size. `None` for a kind that cannot
/// hold a label or for an arrow (an arrow has no inner box).
pub fn inner_size(kind: Kind, outer: Size) -> Option<Size> {
    let p2 = 2.0 * BOUND_TEXT_PADDING;
    let (w, h) = match kind {
        Kind::Rectangle => (outer.width - p2, outer.height - p2),
        Kind::Ellipse => (
            (outer.width / std::f64::consts::SQRT_2).round() - p2,
            (outer.height / std::f64::consts::SQRT_2).round() - p2,
        ),
        Kind::Diamond => ((outer.width / 2.0).round() - p2, (outer.height / 2.0).round() - p2),
        _ => return None,
    };
    Some(Size::new(w.max(0.0), h.max(0.0)))
}

/// The inner box of a container: its inner size, centred in the unrotated outer box.
pub fn inner_box(kind: Kind, outer: Rect) -> Option<Rect> {
    let s = inner_size(kind, outer.size())?;
    let c = outer.center();
    Some(Rect::from_center_size(c, s))
}

/// The outer length (width or height) a container needs so that its inner length is at least `d`.
/// It is the inverse of [`inner_size`]: `inner(container_dim_for_text(d)) >= ceil(d)`.
pub fn container_dim_for_text(d: f64, kind: Kind) -> f64 {
    let d = if d.is_finite() { d.max(0.0).ceil() } else { 0.0 };
    let p2 = 2.0 * BOUND_TEXT_PADDING;
    match kind {
        Kind::Ellipse => ((d + p2) * std::f64::consts::SQRT_2).round(),
        Kind::Diamond => 2.0 * (d + p2),
        _ => d + p2,
    }
}

/// Where the grown container keeps its edge when its height has to grow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GrowAnchor {
    /// The top edge stays, the bottom moves down. For typing into a label.
    #[default]
    Top,
    /// The bottom edge stays. For a resize that dragged the top handle.
    Bottom,
    /// The centre stays.
    Center,
}

/// The computed placement of one label.
#[derive(Debug, Clone, PartialEq)]
pub struct LabelLayout {
    /// The wrapped lines (derived, never stored).
    pub lines: Vec<String>,
    /// Box and angle of the text element.
    pub text: Transform,
    /// The container's unrotated box after growth.
    pub container: Rect,
    /// The container had to grow.
    pub grew: bool,
}

/// Point at fraction `t` (0 to 1, by arc length) of the polyline through the arrow's points, in world space.
/// `None` when the arrow has no points.
pub fn arrow_point_at(arrow: &Element, t: f64) -> Option<Point> {
    let pts = arrow.linear_data().map(|d| &d.points)?;
    let world: Vec<Point> = pts.iter().map(|p| arrow.transform.local_to_world(*p)).collect();
    let first = *world.first()?;
    let total: f64 = world.windows(2).map(|w| (w[1] - w[0]).hypot()).sum();
    if !total.is_finite() || total <= 0.0 {
        return Some(first);
    }
    let mut left = t.clamp(0.0, 1.0) * total;
    for w in world.windows(2) {
        let len = (w[1] - w[0]).hypot();
        if left <= len {
            return Some(if len > 0.0 { w[0].lerp(w[1], left / len) } else { w[0] });
        }
        left -= len;
    }
    world.last().copied()
}

/// Computes the layout of `content` as the label of `container`. Pure: nothing is written.
pub fn layout_label<M: TextMeasure + ?Sized>(
    m: &M,
    container: &Element,
    font: &TextStyle,
    content: &str,
    label_position: Option<f64>,
    anchor: GrowAnchor,
) -> Result<LabelLayout, TextBindError> {
    layout_label_runs(m, container, font, content, &[], label_position, anchor)
}

/// Like [`layout_label`], for a label whose characters have the style `runs` say (bold and italic are wider).
pub fn layout_label_runs<M: TextMeasure + ?Sized>(
    m: &M,
    container: &Element,
    font: &TextStyle,
    content: &str,
    runs: &[TextRun],
    label_position: Option<f64>,
    anchor: GrowAnchor,
) -> Result<LabelLayout, TextBindError> {
    let mut lines = Vec::new();
    let fit = fit_label(container, font, label_position, anchor, &mut |width| {
        lines = m.wrap(content, font, width);
        if runs.is_empty() { measure_lines(m, &lines, font) } else { m.block_size(content, runs, font, Some(width)) }
    })?;
    Ok(LabelLayout { lines, text: fit.text, container: fit.container, grew: fit.grew })
}

/// Where a label goes and whether its container grows. See [`fit_label`].
#[derive(Debug, Clone, PartialEq)]
pub struct LabelFit {
    /// The width the text was last measured at (the wrap width of the label).
    pub wrap_width: f64,
    /// The size of the text at that width.
    pub size: Size,
    /// Box and angle of the text element.
    pub text: Transform,
    /// The container's unrotated box after growth.
    pub container: Rect,
    /// The container had to grow.
    pub grew: bool,
}

/// The placement rules of a label, for any way of measuring text: `size_at(width)` says how large the label's text
/// is when wrapped at `width`. The text editor measures with the layout it draws; [`layout_label`] measures lines
/// with a [`TextMeasure`]. Pure: nothing is written.
pub fn fit_label(
    container: &Element,
    font: &TextStyle,
    label_position: Option<f64>,
    anchor: GrowAnchor,
    size_at: &mut dyn FnMut(f64) -> Size,
) -> Result<LabelFit, TextBindError> {
    let kind = container.tag();
    let mut rect = container.rect();
    if kind == Kind::Arrow {
        let max_w = (ARROW_LABEL_WIDTH_FRACTION * rect.width()).max(font.size * ARROW_LABEL_MIN_WIDTH_PER_SIZE);
        let size = size_at(max_w);
        let at = arrow_point_at(container, label_position.unwrap_or(DEFAULT_LABEL_POSITION)).unwrap_or(rect.center());
        let text = Transform::new(at.x - size.width / 2.0, at.y - size.height / 2.0, size.width, size.height);
        return Ok(LabelFit { wrap_width: max_w, size, text, container: rect, grew: false });
    }
    if !kind.is_label_container() {
        return Err(TextBindError::NotContainer { id: container.id.clone(), kind });
    }

    let mut grew = false;
    let mut size = Size::ZERO;
    let mut wrap_width = 0.0;
    // At most two rounds: growing the width changes the wrapping once.
    for _ in 0..2 {
        let inner = inner_size(kind, rect.size()).unwrap_or(Size::ZERO);
        wrap_width = inner.width;
        size = size_at(inner.width);
        let mut next = rect;
        if size.width > inner.width {
            let w = container_dim_for_text(size.width, kind).max(rect.width());
            let dx = (w - rect.width()) / 2.0;
            next = Rect::new(rect.x0 - dx, rect.y0, rect.x1 + dx, rect.y1);
        }
        if size.height > inner.height {
            let h = container_dim_for_text(size.height, kind).max(rect.height());
            let dy = h - rect.height();
            let (y0, y1) = match anchor {
                GrowAnchor::Top => (next.y0, next.y1 + dy),
                GrowAnchor::Bottom => (next.y0 - dy, next.y1),
                GrowAnchor::Center => (next.y0 - dy / 2.0, next.y1 + dy / 2.0),
            };
            next = Rect::new(next.x0, y0, next.x1, y1);
        }
        if next == rect {
            break;
        }
        grew = true;
        rect = next;
    }

    let text = place_label(kind, rect, container.transform.angle, font, size);
    Ok(LabelFit { wrap_width, size, text, container: rect, grew })
}

/// The box of a label of `size` inside a container of `kind` whose unrotated box is `rect` and which is turned by
/// `angle`: aligned by the font inside the container's text area, then turned about the container's centre. For
/// shapes; an arrow's label is centred on the arrow instead.
pub fn place_label(kind: Kind, rect: Rect, angle: f64, font: &TextStyle, size: Size) -> Transform {
    let inner = inner_box(kind, rect).unwrap_or(rect);
    let y = match font.vertical_align {
        VerticalAlign::Top => inner.y0,
        VerticalAlign::Middle => inner.y0 + (inner.height() - size.height) / 2.0,
        VerticalAlign::Bottom => inner.y1 - size.height,
    };
    let x = match font.align {
        TextAlign::Left => inner.x0,
        TextAlign::Center => inner.x0 + (inner.width() - size.width) / 2.0,
        TextAlign::Right => inner.x1 - size.width,
    };
    let centre = rotate_point(Point::new(x + size.width / 2.0, y + size.height / 2.0), rect.center(), angle);
    let mut text = Transform::new(centre.x - size.width / 2.0, centre.y - size.height / 2.0, size.width, size.height);
    text.angle = angle;
    text
}

// ---------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------

/// Why a text rule refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TextBindError {
    /// An element does not exist, is deleted, or is locked.
    #[error(transparent)]
    Target(#[from] TargetError),
    /// The element is not a text.
    #[error("{id} is {}, not a text element. Pass the id of a text.", drafftink_scene::words::Article::new(.kind.name()))]
    NotText {
        /// The element.
        id: ElementId,
        /// Its kind.
        kind: Kind,
    },
    /// The element is not a math element.
    #[error("{id} is {}, not a math element. Pass the id of a math element.", drafftink_scene::words::Article::new(.kind.name()))]
    NotMath {
        /// The element.
        id: ElementId,
        /// Its kind.
        kind: Kind,
    },
    /// The element cannot hold a label.
    #[error("{id} is {}, which cannot hold a label. Labels go on rectangles, diamonds, ellipses and arrows.", drafftink_scene::words::Article::new(.kind.name()))]
    NotContainer {
        /// The element.
        id: ElementId,
        /// Its kind.
        kind: Kind,
    },
    /// The container already has a label.
    #[error("{container} already has a label ({existing}). Remove it first.")]
    AlreadyLabelled {
        /// The container.
        container: ElementId,
        /// Its current label.
        existing: ElementId,
    },
    /// The text is the label of another container.
    #[error("{text} is already the label of {container}. Unbind it first.")]
    BoundElsewhere {
        /// The text.
        text: ElementId,
        /// Its container.
        container: ElementId,
    },
    /// The text is not a label.
    #[error("{0} is not the label of any element.")]
    NotBound(ElementId),
    /// The text is empty.
    #[error("A text element cannot be empty. Give some text, or remove the label.")]
    EmptyText,
    /// The text is longer than a text element can be.
    #[error("the text has {n} characters, and a text element holds at most {max}")]
    TextTooLong {
        /// Characters given.
        n: usize,
        /// The limit.
        max: usize,
    },
    /// The range does not lie inside the text.
    #[error("The range {start}..{end} is outside the text, which has {len} characters. Use a range within 0..{len}.")]
    BadRange {
        /// First character.
        start: u32,
        /// One past the last character.
        end: u32,
        /// The text length in characters.
        len: usize,
    },
    /// The range is empty or backwards.
    #[error("The range {start}..{end} is empty. Give a start below the end, within 0..{len}.")]
    EmptyRange {
        /// First character.
        start: u32,
        /// One past the last character.
        end: u32,
        /// The text length in characters.
        len: usize,
    },
    /// A range was given for more than one text.
    #[error(
        "A character range belongs to one text, but {n} texts were named. Name one text, or leave the range out to format all of each."
    )]
    RangeNeedsOneText {
        /// How many texts the targets came to.
        n: usize,
    },
    /// The formula is empty.
    #[error("The LaTeX could not be parsed: the formula is empty. Fix the expression. {LATEX_EXAMPLES}")]
    EmptyFormula,
    /// The formula is longer than the model allows.
    #[error(
        "The LaTeX could not be parsed: the formula has more than {max} characters. Fix the expression. {LATEX_EXAMPLES}"
    )]
    FormulaTooLong {
        /// The limit.
        max: usize,
    },
    /// The formula does not parse.
    #[error("The LaTeX could not be parsed: {} (line {line}, column {column}). Fix the expression. {LATEX_EXAMPLES}", .error.message)]
    InvalidLatex {
        /// What the typesetter said.
        error: MathError,
        /// Line of the mistake, counted from 1.
        line: usize,
        /// Column of the mistake, counted from 1.
        column: usize,
    },
    /// A size or position is not a finite number, or is outside the document limits.
    #[error("The result has a size or position that is not a finite number. Use smaller values.")]
    NotFinite,
    /// The new text breaks a rule of the model.
    #[error("the new element {id} is invalid: {error}")]
    Invalid {
        /// The element.
        id: ElementId,
        /// The broken rule.
        error: ElementError,
    },
    /// The scene refused a write.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// No z-order key could be made.
    #[error(transparent)]
    Index(#[from] IndexError),
    /// Re-solving the arrows bound to a grown container failed.
    #[error(transparent)]
    Binding(#[from] BindingError),
    /// Joining a frame failed.
    #[error("joining the frame failed: {0}")]
    Frame(Box<FrameError>),
}

/// Formulas that parse, for the message of a formula that does not.
const LATEX_EXAMPLES: &str = "Examples: `x^2`, `\\frac{a}{b}`, `\\sqrt{x}`, `\\sum_{i=1}^n i`.";

impl From<FreeTextError> for TextBindError {
    fn from(e: FreeTextError) -> Self {
        match e {
            FreeTextError::Empty => TextBindError::EmptyText,
            FreeTextError::TooLong { n, max } => TextBindError::TextTooLong { n, max },
            FreeTextError::NotFinite => TextBindError::NotFinite,
            FreeTextError::Invalid { id, error } => TextBindError::Invalid { id, error },
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Lookups
// ---------------------------------------------------------------------------------------------

fn live<'a, R: SceneRead>(r: &'a R, id: &ElementId) -> Result<&'a Element, TextBindError> {
    r.get(id).filter(|e| !e.deleted).ok_or_else(|| TextBindError::Target(TargetError::NotFound(id.clone())))
}

fn text_of<'a, R: SceneRead>(r: &'a R, id: &ElementId) -> Result<(&'a Element, &'a TextData), TextBindError> {
    let e = live(r, id)?;
    match &e.kind {
        ElementKind::Text(t) => Ok((e, t)),
        _ => Err(TextBindError::NotText { id: id.clone(), kind: e.tag() }),
    }
}

/// The live label of a container (the smallest id when several claim it).
pub fn label_of<R: SceneRead>(r: &R, container: &ElementId) -> Option<ElementId> {
    if let Some(index) = r.derived_index() {
        return index.label_of(container).cloned();
    }
    count(Counter::ElementsScanned, u64::try_from(r.len_all()).unwrap_or(u64::MAX));
    r.iter().filter(|e| e.container_id() == Some(container)).map(|e| e.id.clone()).min()
}

/// The labels of the containers in `ids` (a map from container to label; an id with no label is left out). With the
/// store's derived index each id costs a lookup, so the cost follows `ids` and not the scene; without one, one pass
/// finds every label.
pub fn labels_of<R: SceneRead>(r: &R, ids: &[ElementId]) -> HashMap<ElementId, ElementId> {
    if r.derived_index().is_none() {
        return label_index(r);
    }
    ids.iter().filter_map(|id| label_of(r, id).map(|label| (id.clone(), label))).collect()
}

/// The label of every container in one pass.
pub fn label_index<R: SceneRead>(r: &R) -> HashMap<ElementId, ElementId> {
    count(Counter::ElementsScanned, u64::try_from(r.len_all()).unwrap_or(u64::MAX));
    let mut out: HashMap<ElementId, ElementId> = HashMap::new();
    for e in r.iter() {
        if let Some(c) = e.container_id() {
            match out.get(c) {
                Some(old) if *old <= e.id => {}
                _ => {
                    out.insert(c.clone(), e.id.clone());
                }
            }
        }
    }
    out
}

/// The label of `container`: the smallest id among the texts that claim it.
fn label_for<R: SceneRead>(
    r: &R,
    scanned: Option<&HashMap<ElementId, ElementId>>,
    container: &ElementId,
) -> Option<ElementId> {
    match (r.derived_index(), scanned) {
        (Some(index), _) => index.label_of(container).cloned(),
        (None, Some(map)) => map.get(container).cloned(),
        (None, None) => None,
    }
}

fn z_above_container<R: SceneRead>(r: &R, container: &Element) -> Result<ZIndex, IndexError> {
    let (_, up) = r.z_neighbors(&container.id);
    ZIndex::between(Some(&container.index), up.as_ref())
}

fn same_label_group(text: &mut Element, container: &Element) {
    text.group_ids = container.group_ids.clone();
    text.frame_id = container.frame_id.clone();
}

// ---------------------------------------------------------------------------------------------
// Writing a label
// ---------------------------------------------------------------------------------------------

/// Writes the layout: the container (when it grew) and the text box. Re-solves arrows bound to a grown container.
fn write_layout<T: SceneTx>(
    tx: &mut T,
    container: &ElementId,
    text: &ElementId,
    layout: &LabelLayout,
) -> Result<(), TextBindError> {
    if layout.grew {
        let r = layout.container;
        tx.update(container, |e| {
            e.transform.x = r.x0;
            e.transform.y = r.y0;
            e.transform.w = r.width();
            e.transform.h = r.height();
        })?;
        update_bound_arrows(tx, std::slice::from_ref(container))?;
    }
    let t = layout.text;
    tx.update(text, |e| {
        e.transform.x = t.x;
        e.transform.y = t.y;
        e.transform.w = t.w;
        e.transform.h = t.h;
        e.transform.angle = t.angle;
    })?;
    Ok(())
}

/// The label font of a new label: centred, middle, colour from the container.
pub fn new_label_font(base: Option<&TextStyle>) -> TextStyle {
    let mut f = base.cloned().unwrap_or_default();
    f.align = TextAlign::Center;
    f.vertical_align = VerticalAlign::Middle;
    f
}

/// What a label operation did.
#[derive(Debug, Clone, PartialEq)]
pub struct LabelOutcome {
    /// The label text element.
    pub label: ElementId,
    /// The container grew to fit the label.
    pub container_grew: bool,
}

/// Makes the existing text `text` the label of `container` (`text.bind`). The text is centred, wrapped to the
/// container, the container grows when needed, and the text moves directly above the container in z-order. It
/// takes the container's groups and frame.
pub fn bind_text<T: SceneTx, M: TextMeasure + ?Sized>(
    tx: &mut T,
    m: &M,
    text: &ElementId,
    container: &ElementId,
) -> Result<LabelOutcome, TextBindError> {
    let (te, td) = text_of(tx, text)?;
    let ce = live(tx, container)?;
    if !ce.tag().is_label_container() {
        return Err(TextBindError::NotContainer { id: container.clone(), kind: ce.tag() });
    }
    if te.locked {
        return Err(TextBindError::Target(TargetError::Locked(vec![text.clone()])));
    }
    if ce.locked {
        return Err(TextBindError::Target(TargetError::Locked(vec![container.clone()])));
    }
    if let Some(other) = td.container_id.as_ref().filter(|c| *c != container) {
        return Err(TextBindError::BoundElsewhere { text: text.clone(), container: other.clone() });
    }
    if let Some(existing) = label_of(tx, container).filter(|l| l != text) {
        return Err(TextBindError::AlreadyLabelled { container: container.clone(), existing });
    }
    if td.content.trim().is_empty() {
        return Err(TextBindError::EmptyText);
    }
    let font = new_label_font(Some(&td.font));
    let position = (ce.tag() == Kind::Arrow).then(|| td.label_position.unwrap_or(DEFAULT_LABEL_POSITION));
    let layout = layout_label_runs(m, ce, &font, &td.content, &td.runs, position, GrowAnchor::Top)?;
    let ce = ce.clone();
    let new_index =
        if te.index < ce.index || z_neighbor_is_not(tx, &ce, text) { Some(z_above_container(tx, &ce)?) } else { None };
    tx.update(text, |e| {
        if let ElementKind::Text(t) = &mut e.kind {
            t.container_id = Some(ce.id.clone());
            t.auto_resize = true;
            t.font = font.clone();
            t.label_position = position;
        }
        same_label_group(e, &ce);
        if let Some(i) = new_index {
            e.index = i;
        }
    })?;
    write_layout(tx, container, text, &layout)?;
    Ok(LabelOutcome { label: text.clone(), container_grew: layout.grew })
}

/// True when `text` is not the element directly above `container`.
fn z_neighbor_is_not<R: SceneRead>(r: &R, container: &Element, text: &ElementId) -> bool {
    let (_, up) = r.z_neighbors(&container.id);
    let Some(up) = up else { return true };
    r.get(text).is_none_or(|t| t.index != up)
}

/// Creates a new label for `container` (`element.relabel` with create, `diagram.create`). The label is a text
/// element directly above the container in z-order, centred and middle-aligned, and takes the container's colour,
/// groups and frame. `font` is the base font (default: the text defaults).
pub fn create_label<T: SceneTx, M: TextMeasure + ?Sized>(
    tx: &mut T,
    m: &M,
    ids: &mut dyn IdSource,
    container: &ElementId,
    content: &str,
    font: Option<&TextStyle>,
) -> Result<LabelOutcome, TextBindError> {
    let ce = live(tx, container)?.clone();
    if !ce.tag().is_label_container() {
        return Err(TextBindError::NotContainer { id: container.clone(), kind: ce.tag() });
    }
    if ce.locked {
        return Err(TextBindError::Target(TargetError::Locked(vec![container.clone()])));
    }
    if let Some(existing) = label_of(tx, container) {
        return Err(TextBindError::AlreadyLabelled { container: container.clone(), existing });
    }
    if content.trim().is_empty() {
        return Err(TextBindError::EmptyText);
    }
    let font = new_label_font(font);
    let position = (ce.tag() == Kind::Arrow).then_some(DEFAULT_LABEL_POSITION);
    let layout = layout_label(m, &ce, &font, content, position, GrowAnchor::Top)?;
    let mut data = TextData::new(content);
    data.font = font;
    data.container_id = Some(ce.id.clone());
    data.label_position = position;
    let id = unused_element_id(tx, ids);
    let mut e = Element::new(id.clone(), ElementKind::Text(data), z_above_container(tx, &ce)?);
    e.style.stroke_color = ce.style.stroke_color;
    e.style.opacity = ce.style.opacity;
    e.style.seed = ids.seed();
    e.transform = layout.text;
    same_label_group(&mut e, &ce);
    tx.insert(e)?;
    write_layout(tx, container, &id, &layout)?;
    Ok(LabelOutcome { label: id, container_grew: layout.grew })
}

/// A new free text.
#[derive(Debug, Clone)]
pub struct NewText<'a> {
    /// Top-left corner.
    pub at: Point,
    /// The content (not blank).
    pub content: &'a str,
    /// Font.
    pub font: TextStyle,
    /// Style (colour, opacity). The seed is replaced.
    pub style: Style,
    /// A fixed width: the text wraps and auto-resize is off. `None`: the box follows the content.
    pub width: Option<f64>,
    /// The frame the text joins, if any.
    pub frame: Option<ElementId>,
}

/// Creates a free text on top of the scene, sized by `m`. Used when a text edit of a draft ends.
pub fn create_text<T: SceneTx, M: TextMeasure + ?Sized>(
    tx: &mut T,
    m: &M,
    ids: &mut dyn IdSource,
    spec: &NewText<'_>,
) -> Result<ElementId, TextBindError> {
    let id = unused_element_id(&*tx, ids);
    let z = tx.next_top_index()?;
    let mut style = spec.style.clone();
    style.seed = ids.seed();
    let place = TextSpec { font: spec.font.clone(), place: TextPlace::TopLeft(spec.at), width: spec.width };
    let e = text_element(m, id.clone(), z, spec.content, place, style)?;
    tx.insert(e)?;
    if let Some(f) = &spec.frame {
        crate::frame::add_to_frame(tx, f, std::slice::from_ref(&id)).map_err(|e| TextBindError::Frame(Box::new(e)))?;
    }
    Ok(id)
}

/// Detaches a label (`text.unbind`). The text keeps its place and size as a free text with a fixed width.
pub fn unbind_text<T: SceneTx>(tx: &mut T, text: &ElementId) -> Result<(), TextBindError> {
    let (te, td) = text_of(tx, text)?;
    if td.container_id.is_none() {
        return Err(TextBindError::NotBound(text.clone()));
    }
    if te.locked {
        return Err(TextBindError::Target(TargetError::Locked(vec![text.clone()])));
    }
    tx.update(text, |e| {
        if let ElementKind::Text(t) = &mut e.kind {
            t.container_id = None;
            t.label_position = None;
            t.auto_resize = false;
        }
    })?;
    Ok(())
}

/// Re-wraps and re-places the label of `container` after the container changed (`anchor` says which edge stays when
/// the container has to grow back to fit the text). `None` when the container has no label.
pub fn relayout_label<T: SceneTx, M: TextMeasure + ?Sized>(
    tx: &mut T,
    m: &M,
    container: &ElementId,
    anchor: GrowAnchor,
) -> Result<Option<LabelOutcome>, TextBindError> {
    let Some(label) = label_of(tx, container) else { return Ok(None) };
    relayout_pair(tx, m, container, &label, anchor).map(Some)
}

fn relayout_pair<T: SceneTx, M: TextMeasure + ?Sized>(
    tx: &mut T,
    m: &M,
    container: &ElementId,
    label: &ElementId,
    anchor: GrowAnchor,
) -> Result<LabelOutcome, TextBindError> {
    let (_, td) = text_of(tx, label)?;
    let ce = live(tx, container)?;
    let layout = layout_label_runs(m, ce, &td.font, &td.content, &td.runs, td.label_position, anchor)?;
    write_layout(tx, container, label, &layout)?;
    Ok(LabelOutcome { label: label.clone(), container_grew: layout.grew })
}

/// Re-places the labels that depend on `changed`: the label of every changed container (shape or arrow) and the
/// container of every changed label. Call it after a move, resize, rotate or reroute. Returns the labels written.
pub fn refresh_labels<T: SceneTx, M: TextMeasure + ?Sized>(
    tx: &mut T,
    m: &M,
    changed: &[ElementId],
    anchor: GrowAnchor,
) -> Result<Vec<ElementId>, TextBindError> {
    // The committed index knows every label; without one, a single pass finds them all.
    let scanned = if tx.derived_index().is_some() { None } else { Some(label_index(tx)) };
    let mut pairs: Vec<(ElementId, ElementId)> = Vec::new();
    for id in changed {
        let Some(e) = tx.get(id).filter(|e| !e.deleted) else { continue };
        let pair = match e.container_id() {
            Some(c) => (c.clone(), id.clone()),
            None => match label_for(&*tx, scanned.as_ref(), id) {
                Some(l) => (id.clone(), l),
                None => continue,
            },
        };
        if !pairs.contains(&pair) {
            pairs.push(pair);
        }
    }
    let mut out = Vec::new();
    for (c, l) in pairs {
        if tx.get(&c).is_none_or(|e| e.deleted) {
            continue;
        }
        relayout_pair(tx, m, &c, &l, anchor)?;
        out.push(l);
    }
    Ok(out)
}

/// Where along the arrow the label sits, set from a point (dragging the label): the fraction of the arrow's arc
/// length closest to `p`. The label is re-placed.
pub fn move_arrow_label<T: SceneTx, M: TextMeasure + ?Sized>(
    tx: &mut T,
    m: &M,
    label: &ElementId,
    p: Point,
) -> Result<f64, TextBindError> {
    let (_, td) = text_of(tx, label)?;
    let cid = td.container_id.clone().ok_or_else(|| TextBindError::NotBound(label.clone()))?;
    let arrow = live(tx, &cid)?;
    if arrow.tag() != Kind::Arrow {
        return Err(TextBindError::NotContainer { id: cid, kind: arrow.tag() });
    }
    let t = param_at_point(arrow, p);
    tx.update(label, |e| {
        if let ElementKind::Text(t_) = &mut e.kind {
            t_.label_position = Some(t);
        }
    })?;
    relayout_pair(tx, m, &cid, label, GrowAnchor::Top)?;
    Ok(t)
}

/// The arc-length fraction of the point on the arrow's polyline closest to `p`.
pub fn param_at_point(arrow: &Element, p: Point) -> f64 {
    let Some(pts) = arrow.linear_data().map(|d| &d.points) else { return DEFAULT_LABEL_POSITION };
    let world: Vec<Point> = pts.iter().map(|q| arrow.transform.local_to_world(*q)).collect();
    let total: f64 = world.windows(2).map(|w| (w[1] - w[0]).hypot()).sum();
    if !total.is_finite() || total <= 0.0 {
        return 0.0;
    }
    let (mut best, mut best_d, mut before) = (0.0, f64::INFINITY, 0.0);
    for w in world.windows(2) {
        let seg = w[1] - w[0];
        let len = seg.hypot();
        let s = if len > 0.0 { ((p - w[0]).dot(seg) / (len * len)).clamp(0.0, 1.0) } else { 0.0 };
        let d = (w[0] + seg * s - p).hypot();
        if d < best_d {
            best_d = d;
            best = (before + s * len) / total;
        }
        before += len;
    }
    best.clamp(0.0, 1.0)
}

// ---------------------------------------------------------------------------------------------
// Editing the content
// ---------------------------------------------------------------------------------------------

/// Replaces the content of a text element or label (`text.set`). A label may be emptied: that removes it and the
/// result is `None`. A free text cannot be emptied. The container grows or the label is re-centred as needed.
pub fn set_text_content<T: SceneTx, M: TextMeasure + ?Sized>(
    tx: &mut T,
    m: &M,
    text: &ElementId,
    content: &str,
    keep_runs: bool,
) -> Result<Option<LabelOutcome>, TextBindError> {
    let (te, td) = text_of(tx, text)?;
    if te.locked {
        return Err(TextBindError::Target(TargetError::Locked(vec![text.clone()])));
    }
    let empty = content.trim().is_empty();
    if empty && td.container_id.is_none() {
        return Err(TextBindError::EmptyText);
    }
    let runs = if keep_runs { carry_runs(&td.content, content, &td.runs) } else { Vec::new() };
    let container = td.container_id.clone();
    let (font, auto, width) = (td.font.clone(), td.auto_resize, te.transform.w);
    if empty {
        tx.update(text, |e| e.deleted = true)?;
        return Ok(None);
    }
    let size_runs = runs.clone();
    tx.update(text, |e| {
        if let ElementKind::Text(t) = &mut e.kind {
            t.content = content.to_string();
            t.runs = runs;
        }
    })?;
    match container {
        Some(c) => relayout_pair(tx, m, &c, text, GrowAnchor::Top).map(Some),
        None => {
            // Free text: the box follows the content, or keeps its width and wraps.
            let size = measure_block(m, content, &size_runs, &font, if auto { None } else { Some(width) });
            let w = if auto { size.width } else { width };
            tx.update(text, |e| {
                e.transform.w = w;
                e.transform.h = size.height;
            })?;
            Ok(Some(LabelOutcome { label: text.clone(), container_grew: false }))
        }
    }
}

/// What [`relabel`] did.
#[derive(Debug, Clone, PartialEq)]
pub enum Relabeled {
    /// The text of an existing label or text was replaced.
    Updated(LabelOutcome),
    /// A label was created.
    Created(LabelOutcome),
    /// The label was removed.
    Removed(ElementId),
    /// There was no label and none was asked for.
    Nothing,
}

/// `element.relabel`: sets the text of `id`. On a text element it replaces the content. On a shape or an arrow it
/// replaces the label, creates one when `create` is set, and removes it when `content` is empty. A new label
/// gets `font` (the tool's text defaults), or the scene default when it is `None`.
pub fn relabel<T: SceneTx, M: TextMeasure + ?Sized>(
    tx: &mut T,
    m: &M,
    ids: &mut dyn IdSource,
    id: &ElementId,
    content: &str,
    create: bool,
    font: Option<&TextStyle>,
) -> Result<Relabeled, TextBindError> {
    let e = live(tx, id)?;
    if e.locked {
        return Err(TextBindError::Target(TargetError::Locked(vec![id.clone()])));
    }
    match e.tag() {
        Kind::Text => match set_text_content(tx, m, id, content, true)? {
            Some(o) => Ok(Relabeled::Updated(o)),
            None => Ok(Relabeled::Removed(id.clone())),
        },
        k if k.is_label_container() => {
            let existing = label_of(tx, id);
            match (existing, content.trim().is_empty()) {
                (Some(l), true) => {
                    tx.update(&l, |e| e.deleted = true)?;
                    Ok(Relabeled::Removed(l))
                }
                (Some(l), false) => {
                    let o = set_text_content(tx, m, &l, content, true)?;
                    o.map(Relabeled::Updated).ok_or(TextBindError::EmptyText)
                }
                (None, true) => Ok(Relabeled::Nothing),
                (None, false) if create => create_label(tx, m, ids, id, content, font).map(Relabeled::Created),
                (None, false) => Ok(Relabeled::Nothing),
            }
        }
        k => Err(TextBindError::NotContainer { id: id.clone(), kind: k }),
    }
}

/// `text.set_autoresize`: on, the box follows the content; off, it keeps its width and the text wraps. A label is
/// always sized by its container, so this only changes free text.
pub fn set_auto_resize<T: SceneTx, M: TextMeasure + ?Sized>(
    tx: &mut T,
    m: &M,
    text: &ElementId,
    on: bool,
) -> Result<(), TextBindError> {
    let (te, td) = text_of(tx, text)?;
    if te.locked {
        return Err(TextBindError::Target(TargetError::Locked(vec![text.clone()])));
    }
    if td.container_id.is_some() {
        return Ok(());
    }
    let (content, font, width) = (td.content.clone(), td.font.clone(), te.transform.w);
    let size = measure_block(m, &content, &td.runs, &font, if on { None } else { Some(width) });
    tx.update(text, |e| {
        if let ElementKind::Text(t) = &mut e.kind {
            t.auto_resize = on;
        }
        e.transform.w = if on { size.width } else { width };
        e.transform.h = size.height;
    })?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use drafftink_scene::{LinearKind, MemoryStore, SceneStore, SeqIds, TextRun};
    use proptest::prelude::*;

    fn font() -> TextStyle {
        TextStyle { size: 10.0, ..TextStyle::default() }
    }

    fn m() -> MonospaceMeasure {
        MonospaceMeasure::default()
    }

    fn id(s: &str) -> ElementId {
        ElementId::new(s).unwrap()
    }

    fn shape(store: &mut MemoryStore, s: &str, kind: ElementKind, x: f64, y: f64, w: f64, h: f64) -> ElementId {
        let mut tx = store.begin();
        let z = tx.next_top_index().unwrap();
        tx.insert(Element::new(id(s), kind, z).with_bounds(x, y, w, h)).unwrap();
        tx.commit().unwrap();
        id(s)
    }

    fn free_text(store: &mut MemoryStore, s: &str, content: &str) -> ElementId {
        let mut tx = store.begin();
        let z = tx.next_top_index().unwrap();
        let mut d = TextData::new(content);
        d.font = font();
        tx.insert(Element::new(id(s), ElementKind::Text(d), z).with_bounds(0.0, 0.0, 50.0, 12.5)).unwrap();
        tx.commit().unwrap();
        id(s)
    }

    #[test]
    fn wraps_words_and_breaks_long_words() {
        let f = font(); // 5 per char
        assert_eq!(wrap_text(&m(), "ab cd ef", &f, 25.0), vec!["ab cd", "ef"]);
        assert_eq!(wrap_text(&m(), "abcdefghij", &f, 20.0), vec!["abcd", "efgh", "ij"]);
        assert_eq!(wrap_text(&m(), "a\n\nb", &f, 100.0), vec!["a", "", "b"]);
        // Trailing spaces never force a break.
        assert_eq!(wrap_text(&m(), "ab   cd", &f, 10.0), vec!["ab", "cd"]);
        // Hostile widths keep the hard breaks only.
        assert_eq!(wrap_text(&m(), "ab cd", &f, f64::NAN), vec!["ab cd"]);
        assert_eq!(wrap_text(&m(), "ab cd", &f, -1.0), vec!["ab cd"]);
        // Narrower than one character: one character per line, no endless loop.
        assert_eq!(wrap_text(&m(), "abc", &f, 1.0), vec!["a", "b", "c"]);
    }

    #[test]
    fn inner_boxes_and_inverse() {
        let r = Rect::new(0.0, 0.0, 100.0, 60.0);
        assert_eq!(inner_box(Kind::Rectangle, r), Some(Rect::new(5.0, 5.0, 95.0, 55.0)));
        let e = inner_size(Kind::Ellipse, r.size()).unwrap();
        assert_eq!((e.width, e.height), (61.0, 32.0));
        let d = inner_size(Kind::Diamond, r.size()).unwrap();
        assert_eq!((d.width, d.height), (40.0, 20.0));
        assert!(inner_size(Kind::Arrow, r.size()).is_none());
        assert!(inner_size(Kind::Text, r.size()).is_none());
    }

    proptest! {
        #[test]
        fn container_dim_always_fits(d in 0.0f64..5000.0) {
            for kind in [Kind::Rectangle, Kind::Ellipse, Kind::Diamond] {
                let outer = container_dim_for_text(d, kind);
                let inner = inner_size(kind, Size::new(outer, outer)).unwrap();
                prop_assert!(inner.width >= d.ceil(), "{kind:?} d={d} outer={outer} inner={}", inner.width);
            }
        }
    }

    #[test]
    fn bind_centres_text_and_wraps_to_inner_width() {
        let mut s = MemoryStore::new();
        let c = shape(&mut s, "c", ElementKind::Rectangle, 100.0, 100.0, 60.0, 60.0);
        let t = free_text(&mut s, "t", "hello world");
        let mut tx = s.begin();
        let out = bind_text(&mut tx, &m(), &t, &c).unwrap();
        tx.commit().unwrap();
        assert!(!out.container_grew);
        let te = s.get(&t).unwrap();
        let td = te.text_data().unwrap();
        assert_eq!(td.container_id, Some(c.clone()));
        assert_eq!(td.font.align, TextAlign::Center);
        assert_eq!(td.font.vertical_align, VerticalAlign::Middle);
        // Inner width 50 holds 10 characters: "hello" / "world".
        assert_eq!(te.transform.w, 25.0);
        assert_eq!(te.transform.h, 25.0);
        // Centred in the container.
        assert_eq!(te.transform.center(), Point::new(130.0, 130.0));
        assert!(te.index > s.get(&c).unwrap().index);
        assert_eq!(label_of(&s, &c), Some(t));
    }

    #[test]
    fn container_grows_when_text_is_taller_but_never_shrinks() {
        let mut s = MemoryStore::new();
        let c = shape(&mut s, "c", ElementKind::Rectangle, 0.0, 0.0, 60.0, 30.0);
        let mut ids = SeqIds::new();
        let mut tx = s.begin();
        let out = create_label(&mut tx, &m(), &mut ids, &c, "one two three four five six", Some(&font())).unwrap();
        tx.commit().unwrap();
        assert!(out.container_grew);
        let ce = s.get(&c).unwrap().clone();
        let te = s.get(&out.label).unwrap().clone();
        assert_eq!((ce.transform.x, ce.transform.y, ce.transform.w), (0.0, 0.0, 60.0));
        assert!(ce.transform.h > 30.0);
        let inner = inner_box(Kind::Rectangle, ce.rect()).unwrap();
        assert!(te.rect().y0 >= inner.y0 - 1e-9 && te.rect().y1 <= inner.y1 + 1e-9);
        // Shorter text: the container keeps its height.
        let h = ce.transform.h;
        let mut tx = s.begin();
        set_text_content(&mut tx, &m(), &out.label, "x", true).unwrap();
        tx.commit().unwrap();
        assert_eq!(s.get(&c).unwrap().transform.h, h);
    }

    #[test]
    fn narrow_container_widens_for_one_wide_character() {
        let mut s = MemoryStore::new();
        let c = shape(&mut s, "c", ElementKind::Rectangle, 0.0, 0.0, 12.0, 40.0);
        let mut ids = SeqIds::new();
        let mut tx = s.begin();
        create_label(&mut tx, &m(), &mut ids, &c, "W", Some(&font())).unwrap();
        tx.commit().unwrap();
        let ce = s.get(&c).unwrap();
        assert!(ce.transform.w >= 15.0);
        assert_eq!(ce.rect().center().x, 6.0);
    }

    #[test]
    fn vertical_and_horizontal_alignment() {
        let mut s = MemoryStore::new();
        let c = shape(&mut s, "c", ElementKind::Rectangle, 0.0, 0.0, 100.0, 100.0);
        let t = free_text(&mut s, "t", "ab");
        let mut tx = s.begin();
        bind_text(&mut tx, &m(), &t, &c).unwrap();
        tx.commit().unwrap();
        for (va, ha, expect) in [
            (VerticalAlign::Top, TextAlign::Left, (5.0, 5.0)),
            (VerticalAlign::Bottom, TextAlign::Right, (85.0, 82.5)),
            (VerticalAlign::Middle, TextAlign::Center, (45.0, 43.75)),
        ] {
            let mut tx = s.begin();
            tx.update(&t, |e| {
                if let ElementKind::Text(d) = &mut e.kind {
                    d.font.vertical_align = va;
                    d.font.align = ha;
                }
            })
            .unwrap();
            refresh_labels(&mut tx, &m(), std::slice::from_ref(&c), GrowAnchor::Top).unwrap();
            tx.commit().unwrap();
            let te = s.get(&t).unwrap();
            assert_eq!((te.transform.x, te.transform.y), expect, "{va:?} {ha:?}");
        }
    }

    #[test]
    fn rotated_container_rotates_the_label() {
        let mut s = MemoryStore::new();
        let c = shape(&mut s, "c", ElementKind::Rectangle, 0.0, 0.0, 100.0, 40.0);
        let t = free_text(&mut s, "t", "ab");
        let mut tx = s.begin();
        tx.update(&c, |e| e.transform.angle = std::f64::consts::FRAC_PI_2).unwrap();
        bind_text(&mut tx, &m(), &t, &c).unwrap();
        tx.commit().unwrap();
        let te = s.get(&t).unwrap();
        assert_eq!(te.transform.angle, std::f64::consts::FRAC_PI_2);
        let ctr = te.transform.center();
        assert!((ctr.x - 50.0).abs() < 1e-9 && (ctr.y - 20.0).abs() < 1e-9);
    }

    #[test]
    fn resize_rewraps_and_grows_back() {
        let mut s = MemoryStore::new();
        let c = shape(&mut s, "c", ElementKind::Rectangle, 0.0, 0.0, 200.0, 40.0);
        let mut ids = SeqIds::new();
        let mut tx = s.begin();
        let l = create_label(&mut tx, &m(), &mut ids, &c, "alpha beta gamma delta", Some(&font())).unwrap().label;
        tx.commit().unwrap();
        assert_eq!(s.get(&l).unwrap().transform.h, 12.5);
        // Narrow the container: the text wraps to more lines and the container grows to keep them.
        let mut tx = s.begin();
        tx.update(&c, |e| e.transform.w = 40.0).unwrap();
        let out = relayout_label(&mut tx, &m(), &c, GrowAnchor::Bottom).unwrap().unwrap();
        tx.commit().unwrap();
        assert!(out.container_grew);
        let ce = s.get(&c).unwrap();
        // The bottom edge stayed (it was at 40), the top moved up.
        assert_eq!(ce.rect().y1, 40.0);
        assert!(ce.rect().y0 < 0.0);
        let te = s.get(&l).unwrap();
        assert!(te.transform.h > 12.5);
        assert!(te.transform.w <= 30.0 + 1e-9);
        assert_eq!(te.transform.h, 50.0);
    }

    #[test]
    fn ellipse_and_diamond_use_smaller_inner_boxes() {
        for (kind, k) in [(ElementKind::Ellipse, Kind::Ellipse), (ElementKind::Diamond, Kind::Diamond)] {
            let mut s = MemoryStore::new();
            let c = shape(&mut s, "c", kind, 0.0, 0.0, 120.0, 80.0);
            let mut ids = SeqIds::new();
            let mut tx = s.begin();
            let l = create_label(&mut tx, &m(), &mut ids, &c, "aaaa bbbb cccc dddd", Some(&font())).unwrap().label;
            tx.commit().unwrap();
            let ce = s.get(&c).unwrap();
            let inner = inner_box(k, ce.rect()).unwrap();
            let te = s.get(&l).unwrap();
            assert!(te.rect().x0 >= inner.x0 - 1e-9 && te.rect().x1 <= inner.x1 + 1e-9, "{k:?}");
            assert!(te.rect().y0 >= inner.y0 - 1e-9 && te.rect().y1 <= inner.y1 + 1e-9, "{k:?}");
        }
    }

    #[test]
    fn arrow_label_sits_on_the_path_and_never_grows_the_arrow() {
        let mut s = MemoryStore::new();
        let mut tx = s.begin();
        let z = tx.next_top_index().unwrap();
        let a = Element::linear(
            id("a"),
            LinearKind::Arrow,
            z,
            &[Point::new(0.0, 0.0), Point::new(100.0, 0.0), Point::new(100.0, 100.0)],
        )
        .unwrap();
        tx.insert(a).unwrap();
        let mut ids = SeqIds::new();
        let before = tx.get(&id("a")).unwrap().transform;
        let l = create_label(&mut tx, &m(), &mut ids, &id("a"), "yes", Some(&font())).unwrap().label;
        tx.commit().unwrap();
        let te = s.get(&l).unwrap();
        // Half of 200 along the path is the corner (100, 0).
        assert_eq!(te.transform.center(), Point::new(100.0, 0.0));
        assert_eq!(te.transform.angle, 0.0);
        assert_eq!(te.text_data().unwrap().label_position, Some(0.5));
        assert_eq!(s.get(&id("a")).unwrap().transform, before);
        // Drag the label to the start of the second leg.
        let mut tx = s.begin();
        let t = move_arrow_label(&mut tx, &m(), &l, Point::new(105.0, 75.0)).unwrap();
        tx.commit().unwrap();
        assert!((t - 0.875).abs() < 1e-9);
        assert_eq!(s.get(&l).unwrap().transform.center(), Point::new(100.0, 75.0));
    }

    #[test]
    fn errors_are_typed() {
        let mut s = MemoryStore::new();
        let r = shape(&mut s, "r", ElementKind::Rectangle, 0.0, 0.0, 50.0, 50.0);
        let t1 = free_text(&mut s, "t1", "a");
        let t2 = free_text(&mut s, "t2", "b");
        let t3 = free_text(&mut s, "t3", "   ");
        let mut tx = s.begin();
        assert_eq!(
            bind_text(&mut tx, &m(), &id("nope"), &r),
            Err(TextBindError::Target(TargetError::NotFound(id("nope"))))
        );
        assert!(matches!(bind_text(&mut tx, &m(), &r, &r), Err(TextBindError::NotText { .. })));
        assert!(matches!(bind_text(&mut tx, &m(), &t1, &t2), Err(TextBindError::NotContainer { .. })));
        assert_eq!(bind_text(&mut tx, &m(), &t3, &r), Err(TextBindError::EmptyText));
        bind_text(&mut tx, &m(), &t1, &r).unwrap();
        assert!(matches!(bind_text(&mut tx, &m(), &t2, &r), Err(TextBindError::AlreadyLabelled { .. })));
        let r2 = fresh(&mut tx, "r2");
        assert!(matches!(bind_text(&mut tx, &m(), &t1, &r2), Err(TextBindError::BoundElsewhere { .. })));
        assert_eq!(unbind_text(&mut tx, &t2), Err(TextBindError::NotBound(t2.clone())));
        tx.update(&r, |e| e.locked = true).unwrap();
        assert_eq!(bind_text(&mut tx, &m(), &t2, &r), Err(TextBindError::Target(TargetError::Locked(vec![r.clone()]))));
    }

    fn fresh(tx: &mut impl SceneTx, s: &str) -> ElementId {
        let z = tx.next_top_index().unwrap();
        tx.insert(Element::new(id(s), ElementKind::Rectangle, z).with_bounds(0.0, 0.0, 50.0, 50.0)).unwrap();
        id(s)
    }

    #[test]
    fn unbind_keeps_the_text_in_place() {
        let mut s = MemoryStore::new();
        let c = shape(&mut s, "c", ElementKind::Rectangle, 0.0, 0.0, 80.0, 40.0);
        let t = free_text(&mut s, "t", "hi");
        let mut tx = s.begin();
        bind_text(&mut tx, &m(), &t, &c).unwrap();
        tx.commit().unwrap();
        let placed = s.get(&t).unwrap().transform;
        let mut tx = s.begin();
        unbind_text(&mut tx, &t).unwrap();
        tx.commit().unwrap();
        let te = s.get(&t).unwrap();
        assert_eq!(te.transform, placed);
        assert_eq!(te.container_id(), None);
        assert_eq!(label_of(&s, &c), None);
    }

    #[test]
    fn relabel_creates_updates_and_removes() {
        let mut s = MemoryStore::new();
        let c = shape(&mut s, "c", ElementKind::Rectangle, 0.0, 0.0, 80.0, 40.0);
        let mut ids = SeqIds::new();
        let mut tx = s.begin();
        assert_eq!(relabel(&mut tx, &m(), &mut ids, &c, "x", false, None).unwrap(), Relabeled::Nothing);
        let created = relabel(&mut tx, &m(), &mut ids, &c, "x", true, None).unwrap();
        let Relabeled::Created(o) = created else { panic!("{created:?}") };
        let upd = relabel(&mut tx, &m(), &mut ids, &c, "y", true, None).unwrap();
        assert!(matches!(upd, Relabeled::Updated(_)));
        assert_eq!(tx.get(&o.label).unwrap().text_data().unwrap().content, "y");
        let rem = relabel(&mut tx, &m(), &mut ids, &c, "", true, None).unwrap();
        assert_eq!(rem, Relabeled::Removed(o.label.clone()));
        assert_eq!(label_of(&tx, &c), None);
        tx.commit().unwrap();
    }

    #[test]
    fn set_text_content_on_free_text_resizes_and_keeps_edge_runs() {
        let mut s = MemoryStore::new();
        let t = free_text(&mut s, "t", "abcdef");
        let mut tx = s.begin();
        tx.update(&t, |e| {
            if let ElementKind::Text(d) = &mut e.kind {
                d.runs = vec![
                    TextRun { start: 0, end: 2, color: None, ..Default::default() },
                    TextRun { start: 4, end: 6, color: None, ..Default::default() },
                    TextRun { start: 2, end: 4, color: None, ..Default::default() },
                ];
                d.runs.sort_by_key(|r| r.start);
            }
        })
        .unwrap();
        set_text_content(&mut tx, &m(), &t, "abXXXXef", true).unwrap();
        let d = tx.get(&t).unwrap().text_data().unwrap().clone();
        assert_eq!(d.runs.iter().map(|r| (r.start, r.end)).collect::<Vec<_>>(), vec![(0, 2), (6, 8)]);
        assert_eq!(tx.get(&t).unwrap().transform.w, 8.0 * 5.0);
        assert_eq!(set_text_content(&mut tx, &m(), &t, "  ", true), Err(TextBindError::EmptyText));
        tx.commit().unwrap();
    }

    #[test]
    fn labels_follow_container_groups_and_stay_above() {
        let mut s = MemoryStore::new();
        let c = shape(&mut s, "c", ElementKind::Rectangle, 0.0, 0.0, 80.0, 40.0);
        let _other = shape(&mut s, "o", ElementKind::Rectangle, 200.0, 0.0, 80.0, 40.0);
        let mut tx = s.begin();
        tx.update(&c, |e| e.group_ids.push(drafftink_scene::GroupId::new("g1").unwrap())).unwrap();
        let mut ids = SeqIds::new();
        let l = create_label(&mut tx, &m(), &mut ids, &c, "x", Some(&font())).unwrap().label;
        tx.commit().unwrap();
        let le = s.get(&l).unwrap();
        assert_eq!(le.group_ids, s.get(&c).unwrap().group_ids);
        // Directly above the container, below the other shape.
        let order: Vec<_> = s.iter().map(|e| e.id.as_str().to_string()).collect();
        assert_eq!(order, vec!["c", l.as_str(), "o"]);
    }
}
