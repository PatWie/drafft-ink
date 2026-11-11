//! Rules of the `text` command family that `text_bind` does not cover: font-size steps, formulas, colour
//! runs, wrapping free text in a shape, creating a free text, and re-measuring after a font change.
//!
//! `text_bind` owns labels (create, bind, relabel, wrap, grow). This module builds on it. Math cannot be laid out
//! here (the typesetter sits above this crate), so the caller passes a [`MathMeasure`] and this module only
//! writes the result.

use kurbo::{Point, Size, Vec2};

use drafftink_scene::{
    Color, Element, ElementId, ElementKind, FontFamily, FontWeight, IdSource, IndexError, Kind, SceneRead, SceneTx,
    StoreError, Style, TextAlign, TextData, TextRun, TextStyle, Transform, VerticalAlign, ZIndex,
};

use crate::style::{Prop, StyleProps};
use crate::text_bind::{
    BOUND_TEXT_PADDING, GrowAnchor, TextBindError, TextMeasure, bind_text, container_dim_for_text, fresh_element_id,
    measure_text, relayout_label,
};

/// The font sizes the step commands move between, smallest first.
pub const FONT_PRESETS: [f64; 4] = [16.0, 20.0, 28.0, 36.0];
/// Above the last preset, and below the first, a step is this many units.
pub const FONT_STEP: f64 = 8.0;
/// The smallest size a step reaches.
pub const MIN_FONT_SIZE: f64 = 8.0;
/// The largest size a step reaches.
pub const MAX_FONT_SIZE: f64 = 200.0;

/// Which way a font size steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FontStep {
    /// The next larger size.
    Increase,
    /// The next smaller size.
    Decrease,
}

/// The size after one step: the next preset in 16, 20, 28, 36, then 8 more above 36 or 8 less below 16 (never
/// under [`MIN_FONT_SIZE`] or over [`MAX_FONT_SIZE`]). A size that is not a number counts as 20.
pub fn next_font_size(size: f64, step: FontStep) -> f64 {
    let increase = step == FontStep::Increase;
    let s = if size.is_finite() { size } else { 20.0 };
    let top = FONT_PRESETS[FONT_PRESETS.len() - 1];
    let bottom = FONT_PRESETS[0];
    let next = if increase {
        if s < top { FONT_PRESETS.iter().copied().find(|p| *p > s + 1e-9).unwrap_or(top) } else { s + FONT_STEP }
    } else if s > top {
        (s - FONT_STEP).max(top)
    } else if s > bottom {
        FONT_PRESETS.iter().rev().copied().find(|p| *p < s - 1e-9).unwrap_or(bottom)
    } else {
        s - FONT_STEP
    };
    next.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
}

/// Lays out a formula. The engine implements it over the typesetter.
pub trait MathMeasure {
    /// The size of the formula at a font size, or the parser's plain-words message when it does not parse.
    fn math_box(&self, latex: &str, size: f64) -> Result<Size, String>;
}

/// Why a text rule refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TextCmdError {
    /// The element does not exist.
    #[error("element {0} does not exist. Look up ids with element.query or doc.outline.")]
    Missing(ElementId),
    /// The element is locked.
    #[error("{0} is locked. Unlock it first.")]
    Locked(ElementId),
    /// The element is not a math element.
    #[error("{id} is {}, not a math element. Pass the id of a math element.", drafftink_scene::words::Article::new(.kind))]
    NotMath {
        /// The element.
        id: ElementId,
        /// Its kind.
        kind: &'static str,
    },
    /// The element is not a text.
    #[error("{id} is {}, not a text element. Pass the id of a text.", drafftink_scene::words::Article::new(.kind))]
    NotText {
        /// The element.
        id: ElementId,
        /// Its kind.
        kind: &'static str,
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
    /// The formula does not parse.
    #[error(
        "The LaTeX could not be parsed: {0}. Fix the expression. Examples: `x^2`, `\\frac{{a}}{{b}}`, `\\sqrt{{x}}`, `\\sum_{{i=1}}^n i`."
    )]
    InvalidLatex(String),
    /// A number is not finite or out of range.
    #[error("The result has a size or position that is not a finite number. Use smaller values.")]
    NotFinite,
    /// A label rule refused.
    #[error(transparent)]
    Text(#[from] TextBindError),
    /// The scene refused a write.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// No z-order key could be made.
    #[error(transparent)]
    Index(#[from] IndexError),
    /// Joining a frame failed.
    #[error("joining the frame failed: {0}")]
    Frame(String),
}

impl TextCmdError {
    /// The refusal code of the command catalog.
    pub fn code(&self) -> &'static str {
        match self {
            TextCmdError::Missing(_) => "not_found",
            TextCmdError::Locked(_) => "locked",
            TextCmdError::NotMath { .. } | TextCmdError::NotText { .. } => "wrong_kind",
            TextCmdError::BadRange { .. } | TextCmdError::EmptyRange { .. } => "bad_range",
            TextCmdError::InvalidLatex(_) => "invalid_latex",
            TextCmdError::NotFinite => "bad_args",
            TextCmdError::Text(e) => e.code(),
            TextCmdError::Store(_) | TextCmdError::Index(_) | TextCmdError::Frame(_) => "internal",
        }
    }
}

fn live<'a, R: SceneRead>(r: &'a R, id: &ElementId) -> Result<&'a Element, TextCmdError> {
    r.get(id).filter(|e| !e.deleted).ok_or_else(|| TextCmdError::Missing(id.clone()))
}

/// The new top-left `(x, y)` of a box of size `(w, h)` that keeps the rotated top-left corner of `old` where it
/// is. For an unrotated element this is `(old.x, old.y)`. The old app kept this anchor when a text edit ended
/// (`exit_text_edit`).
pub fn anchor_top_left(old: &Transform, w: f64, h: f64) -> (f64, f64) {
    if old.angle == 0.0 {
        return (old.x, old.y);
    }
    let rot = |v: Vec2| {
        let (s, c) = old.angle.sin_cos();
        Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
    };
    let corner = old.center() + rot(Vec2::new(-old.w / 2.0, -old.h / 2.0));
    let centre = corner - rot(Vec2::new(-w / 2.0, -h / 2.0));
    (centre.x - w / 2.0, centre.y - h / 2.0)
}

/// Re-measures a text after its content or font changed. A label is laid out again in its container (which grows
/// when needed). A free text takes the size of its content, or keeps its width and wraps when auto-resize is off.
/// The rotated top-left corner stays.
pub fn remeasure_text<T: SceneTx, M: TextMeasure + ?Sized>(
    tx: &mut T,
    m: &M,
    id: &ElementId,
) -> Result<(), TextCmdError> {
    let e = live(tx, id)?;
    let Some(td) = e.text_data() else {
        return Err(TextCmdError::NotText { id: id.clone(), kind: e.tag().name() });
    };
    if let Some(c) = td.container_id.clone() {
        relayout_label(tx, m, &c, GrowAnchor::Top)?;
        return Ok(());
    }
    let size = if td.auto_resize {
        measure_text(m, &td.content, &td.font, None)
    } else {
        measure_text(m, &td.content, &td.font, Some(e.transform.w))
    };
    let w = if td.auto_resize { size.width } else { e.transform.w };
    if !(w.is_finite() && size.height.is_finite()) {
        return Err(TextCmdError::NotFinite);
    }
    let (x, y) = anchor_top_left(&e.transform, w, size.height);
    tx.update(id, |e| {
        e.transform.x = x;
        e.transform.y = y;
        e.transform.w = w;
        e.transform.h = size.height;
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// text.step_size
// ---------------------------------------------------------------------------------------------

/// What [`step_font_size`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StepReport {
    /// Texts and formulas whose size changed.
    pub changed: Vec<ElementId>,
    /// Texts and formulas already at the end of the range.
    pub unchanged: Vec<ElementId>,
    /// Targets that are no text, no formula and no container with a label.
    pub skipped: Vec<ElementId>,
}

impl StepReport {
    /// True when at least one target was a text or a formula.
    pub fn applicable(&self) -> bool {
        !self.changed.is_empty() || !self.unchanged.is_empty()
    }
}

/// Moves the font size of every text and formula in `ids` one step. A container with a label steps its label.
/// Locked targets refuse the whole step before anything is written.
pub fn step_font_size<T: SceneTx, M: TextMeasure + ?Sized>(
    tx: &mut T,
    m: &M,
    math: &dyn MathMeasure,
    ids: &[ElementId],
    step: FontStep,
) -> Result<StepReport, TextCmdError> {
    let mut report = StepReport::default();
    let mut targets: Vec<ElementId> = Vec::new();
    for id in ids {
        let e = live(tx, id)?;
        let target = match e.tag() {
            Kind::Text | Kind::Math => Some(id.clone()),
            k if k.is_label_container() => crate::text_bind::label_of(tx, id),
            _ => None,
        };
        match target {
            Some(t) if !targets.contains(&t) => {
                if e.locked || live(tx, &t)?.locked {
                    return Err(TextCmdError::Locked(id.clone()));
                }
                targets.push(t);
            }
            Some(_) => {}
            None => report.skipped.push(id.clone()),
        }
    }
    for id in targets {
        let e = live(tx, &id)?;
        match &e.kind {
            ElementKind::Text(td) => {
                let next = next_font_size(td.font.size, step);
                if (next - td.font.size).abs() < 1e-9 {
                    report.unchanged.push(id);
                    continue;
                }
                tx.update(&id, |e| {
                    if let ElementKind::Text(t) = &mut e.kind {
                        t.font.size = next;
                    }
                })?;
                remeasure_text(tx, m, &id)?;
                report.changed.push(id);
            }
            ElementKind::Math(md) => {
                let next = next_font_size(md.font_size, step);
                if (next - md.font_size).abs() < 1e-9 {
                    report.unchanged.push(id);
                    continue;
                }
                let latex = md.latex.clone();
                set_math_size(tx, math, &id, &latex, next)?;
                report.changed.push(id);
            }
            _ => report.skipped.push(id),
        }
    }
    Ok(report)
}

// ---------------------------------------------------------------------------------------------
// text.set_latex
// ---------------------------------------------------------------------------------------------

/// Lays out `latex` at `size` and writes formula, size and box. The rotated top-left corner stays.
pub fn set_math_size<T: SceneTx>(
    tx: &mut T,
    math: &dyn MathMeasure,
    id: &ElementId,
    latex: &str,
    size: f64,
) -> Result<(), TextCmdError> {
    if latex.trim().is_empty() {
        return Err(TextCmdError::InvalidLatex("the formula is empty".to_owned()));
    }
    if latex.chars().count() > drafftink_scene::limits::MAX_LATEX_CHARS {
        return Err(TextCmdError::InvalidLatex(format!(
            "the formula has more than {} characters",
            drafftink_scene::limits::MAX_LATEX_CHARS
        )));
    }
    let b = math.math_box(latex, size).map_err(TextCmdError::InvalidLatex)?;
    let (w, h) = (b.width.max(1.0), b.height.max(1.0));
    if !(w.is_finite() && h.is_finite()) {
        return Err(TextCmdError::NotFinite);
    }
    let old = live(tx, id)?.transform;
    let (x, y) = anchor_top_left(&old, w, h);
    let latex = latex.to_owned();
    tx.update(id, |e| {
        if let ElementKind::Math(d) = &mut e.kind {
            d.latex = latex;
            d.font_size = size;
        }
        e.transform.x = x;
        e.transform.y = y;
        e.transform.w = w;
        e.transform.h = h;
    })?;
    Ok(())
}

/// Replaces the formula of a math element (`text.set_latex`). On a parse error nothing is written. Returns false
/// when the formula was already as given.
pub fn set_latex<T: SceneTx>(
    tx: &mut T,
    math: &dyn MathMeasure,
    id: &ElementId,
    latex: &str,
) -> Result<bool, TextCmdError> {
    let e = live(tx, id)?;
    let ElementKind::Math(d) = &e.kind else {
        return Err(TextCmdError::NotMath { id: id.clone(), kind: e.tag().name() });
    };
    if e.locked {
        return Err(TextCmdError::Locked(id.clone()));
    }
    if d.latex == latex {
        return Ok(false);
    }
    let size = d.font_size;
    set_math_size(tx, math, id, latex, size)?;
    Ok(true)
}

// ---------------------------------------------------------------------------------------------
// Colour runs
// ---------------------------------------------------------------------------------------------

/// A colour range from the editor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RunSpec {
    /// First character.
    pub start: u32,
    /// One past the last character.
    pub end: u32,
    /// The colour.
    pub color: Color,
}

fn check_range(start: u32, end: u32, len: usize) -> Result<(), TextCmdError> {
    if end as usize > len || start as usize > len {
        return Err(TextCmdError::BadRange { start, end, len });
    }
    if start >= end {
        return Err(TextCmdError::EmptyRange { start, end, len });
    }
    Ok(())
}

/// Sets, or with `None` clears, the colour of the characters `start..end` of a text (`text.style_range`).
/// Returns false when nothing changed.
pub fn style_range<T: SceneTx>(
    tx: &mut T,
    id: &ElementId,
    start: u32,
    end: u32,
    color: Option<Color>,
) -> Result<bool, TextCmdError> {
    let e = live(tx, id)?;
    let ElementKind::Text(td) = &e.kind else {
        return Err(TextCmdError::NotText { id: id.clone(), kind: e.tag().name() });
    };
    if e.locked {
        return Err(TextCmdError::Locked(id.clone()));
    }
    let len = td.char_count();
    check_range(start, end, len)?;
    let mut colors = td.char_colors();
    if let Some(slice) = colors.get_mut(start as usize..end as usize) {
        slice.fill(color);
    }
    let runs = TextData::runs_from_char_colors(&colors, len);
    if runs == td.runs {
        return Ok(false);
    }
    tx.update(id, |e| {
        if let ElementKind::Text(t) = &mut e.kind {
            t.runs = runs;
        }
    })?;
    Ok(true)
}

/// Replaces all colour runs of a text with the editor's final ranges (`text.edit_end`). Later ranges win where they
/// overlap. Returns false when the runs are unchanged.
pub fn replace_runs<T: SceneTx>(tx: &mut T, id: &ElementId, specs: &[RunSpec]) -> Result<bool, TextCmdError> {
    let e = live(tx, id)?;
    let ElementKind::Text(td) = &e.kind else {
        return Err(TextCmdError::NotText { id: id.clone(), kind: e.tag().name() });
    };
    let len = td.char_count();
    let mut colors: Vec<Option<Color>> = vec![None; len];
    for r in specs {
        check_range(r.start, r.end, len)?;
        if let Some(slice) = colors.get_mut(r.start as usize..r.end as usize) {
            slice.fill(Some(r.color));
        }
    }
    let runs: Vec<TextRun> = TextData::runs_from_char_colors(&colors, len);
    if runs == td.runs {
        return Ok(false);
    }
    tx.update(id, |e| {
        if let ElementKind::Text(t) = &mut e.kind {
            t.runs = runs;
        }
    })?;
    Ok(true)
}

// ---------------------------------------------------------------------------------------------
// Element-level text properties chosen during an edit
// ---------------------------------------------------------------------------------------------

/// Text properties that can be changed together with the content (`text.edit_end` `style`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TextProps {
    /// Text colour.
    pub stroke_color: Option<Color>,
    /// Opacity, 0 to 1.
    pub opacity: Option<f64>,
    /// Font family.
    pub font_family: Option<FontFamily>,
    /// Font weight.
    pub font_weight: Option<FontWeight>,
    /// Font size.
    pub font_size: Option<f64>,
    /// Horizontal alignment.
    pub text_align: Option<TextAlign>,
    /// Vertical alignment.
    pub vertical_align: Option<VerticalAlign>,
    /// Line height as a multiple of the font size.
    pub line_height: Option<f64>,
}

impl TextProps {
    /// The text properties of a style change (`text.edit_end` `style`): colour, opacity and the font
    /// properties. Any other property is an error that lists them, because text cannot take it.
    pub fn from_style(props: &StyleProps) -> Result<Self, Vec<Prop>> {
        let unsupported: Vec<Prop> = props
            .set_props()
            .into_iter()
            .filter(|p| !(p.is_text() || matches!(p, Prop::StrokeColor | Prop::Opacity)))
            .collect();
        if !unsupported.is_empty() {
            return Err(unsupported);
        }
        Ok(Self {
            stroke_color: props.stroke_color.map(|c| c.0),
            opacity: props.opacity,
            font_family: props.font_family,
            font_weight: props.font_weight,
            font_size: props.font_size,
            text_align: props.text_align,
            vertical_align: props.vertical_align,
            line_height: props.line_height,
        })
    }

    /// True when nothing is set.
    pub fn is_empty(&self) -> bool {
        *self == TextProps::default()
    }

    /// Applies the font properties to a font.
    pub fn apply_font(&self, f: &mut TextStyle) {
        if let Some(v) = self.font_family {
            f.family = v;
        }
        if let Some(v) = self.font_weight {
            f.weight = v;
        }
        if let Some(v) = self.font_size {
            f.size = v;
        }
        if let Some(v) = self.text_align {
            f.align = v;
        }
        if let Some(v) = self.vertical_align {
            f.vertical_align = v;
        }
        if let Some(v) = self.line_height {
            f.line_height = v;
        }
    }
}

/// Applies `props` to a text element and re-measures it. Returns false when nothing changed.
pub fn apply_text_props<T: SceneTx, M: TextMeasure + ?Sized>(
    tx: &mut T,
    m: &M,
    id: &ElementId,
    props: &TextProps,
) -> Result<bool, TextCmdError> {
    let e = live(tx, id)?;
    let Some(td) = e.text_data() else {
        return Err(TextCmdError::NotText { id: id.clone(), kind: e.tag().name() });
    };
    let mut font = td.font.clone();
    props.apply_font(&mut font);
    let (color, opacity) =
        (props.stroke_color.unwrap_or(e.style.stroke_color), props.opacity.unwrap_or(e.style.opacity));
    if font == td.font && color == e.style.stroke_color && opacity == e.style.opacity {
        return Ok(false);
    }
    let font_changed = font != td.font;
    tx.update(id, |e| {
        if let ElementKind::Text(t) = &mut e.kind {
            t.font = font;
        }
        e.style.stroke_color = color;
        e.style.opacity = opacity;
    })?;
    if font_changed {
        remeasure_text(tx, m, id)?;
    }
    Ok(true)
}

// ---------------------------------------------------------------------------------------------
// Creating free text
// ---------------------------------------------------------------------------------------------

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
) -> Result<ElementId, TextCmdError> {
    if spec.content.trim().is_empty() {
        return Err(TextBindError::EmptyText.into());
    }
    if !(spec.at.x.is_finite() && spec.at.y.is_finite()) {
        return Err(TextCmdError::NotFinite);
    }
    let mut data = TextData::new(spec.content);
    data.font = spec.font.clone();
    data.auto_resize = spec.width.is_none();
    let size = measure_text(m, spec.content, &data.font, spec.width);
    let w = spec.width.unwrap_or(size.width);
    let id = fresh_element_id(tx, ids);
    let mut style = spec.style.clone();
    style.seed = ids.seed();
    let z = tx.next_top_index()?;
    let e = Element::new(id.clone(), ElementKind::Text(data), z)
        .with_bounds(spec.at.x, spec.at.y, w, size.height)
        .with_style(style);
    tx.insert(e)?;
    if let Some(f) = &spec.frame {
        crate::frame::add_to_frame(tx, f, std::slice::from_ref(&id)).map_err(|e| TextCmdError::Frame(e.to_string()))?;
    }
    Ok(id)
}

// ---------------------------------------------------------------------------------------------
// text.wrap_in_container
// ---------------------------------------------------------------------------------------------

/// The result of wrapping one text.
#[derive(Debug, Clone, PartialEq)]
pub struct Wrapped {
    /// The text, now a label.
    pub text: ElementId,
    /// The new shape.
    pub container: ElementId,
}

/// Wraps each free text in `ids` in a new shape of kind `shape` (rectangle, ellipse or diamond) with `padding`
/// around the text, and binds the text as its label. The shape sits directly below the text and takes its groups,
/// frame, colour and opacity. Texts that are already labels, and other kinds, are skipped; the second list holds them.
pub fn wrap_in_container<T: SceneTx, M: TextMeasure + ?Sized>(
    tx: &mut T,
    m: &M,
    ids_src: &mut dyn IdSource,
    ids: &[ElementId],
    shape: Kind,
    padding: f64,
) -> Result<(Vec<Wrapped>, Vec<ElementId>), TextCmdError> {
    let mut todo: Vec<ElementId> = Vec::new();
    let mut skipped = Vec::new();
    for id in ids {
        let e = live(tx, id)?;
        match e.text_data() {
            Some(td) if td.container_id.is_none() => {
                if e.locked {
                    return Err(TextCmdError::Locked(id.clone()));
                }
                if !todo.contains(id) {
                    todo.push(id.clone());
                }
            }
            _ => skipped.push(id.clone()),
        }
    }
    let pad = (padding - BOUND_TEXT_PADDING).max(0.0);
    let mut out = Vec::new();
    for tid in todo {
        let t = live(tx, &tid)?.clone();
        let (w, h) = (
            container_dim_for_text(t.transform.w + 2.0 * pad, shape),
            container_dim_for_text(t.transform.h + 2.0 * pad, shape),
        );
        let c = t.transform.center();
        let kind = match shape {
            Kind::Ellipse => ElementKind::Ellipse,
            Kind::Diamond => ElementKind::Diamond,
            _ => ElementKind::Rectangle,
        };
        let (below, _) = tx.z_neighbors(&tid);
        let z = ZIndex::between(below.as_ref(), Some(&t.index))?;
        let cid = fresh_element_id(tx, ids_src);
        let mut style = Style::default();
        style.stroke_color = t.style.stroke_color;
        style.opacity = t.style.opacity;
        style.seed = ids_src.seed();
        let mut ce =
            Element::new(cid.clone(), kind, z).with_bounds(c.x - w / 2.0, c.y - h / 2.0, w, h).with_style(style);
        ce.transform.angle = t.transform.angle;
        ce.group_ids = t.group_ids.clone();
        ce.frame_id = t.frame_id.clone();
        tx.insert(ce)?;
        bind_text(tx, m, &tid, &cid)?;
        out.push(Wrapped { text: tid, container: cid });
    }
    Ok((out, skipped))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::text_bind::{MonospaceMeasure, label_of};
    use drafftink_scene::{MathData, MemoryStore, SceneStore, SeqIds};

    fn id(s: &str) -> ElementId {
        ElementId::new(s).unwrap()
    }

    struct FakeMath;
    impl MathMeasure for FakeMath {
        fn math_box(&self, latex: &str, size: f64) -> Result<Size, String> {
            if latex.contains("\\bad") {
                return Err("unknown command `\\bad` (bytes 0..4)".to_owned());
            }
            Ok(Size::new(latex.len() as f64 * size * 0.5, size * 1.5))
        }
    }

    fn text(store: &mut MemoryStore, s: &str, content: &str, size: f64) -> ElementId {
        let mut tx = store.begin();
        let z = tx.next_top_index().unwrap();
        let mut d = TextData::new(content);
        d.font.size = size;
        let b = measure_text(&MonospaceMeasure::default(), content, &d.font, None);
        tx.insert(Element::new(id(s), ElementKind::Text(d), z).with_bounds(10.0, 20.0, b.width, b.height)).unwrap();
        tx.commit().unwrap();
        id(s)
    }

    #[test]
    fn steps_follow_the_presets_then_eight() {
        let up = |s| next_font_size(s, FontStep::Increase);
        let down = |s| next_font_size(s, FontStep::Decrease);
        assert_eq!([up(12.0), up(16.0), up(20.0), up(28.0), up(36.0), up(44.0)], [16.0, 20.0, 28.0, 36.0, 44.0, 52.0]);
        assert_eq!(
            [down(52.0), down(44.0), down(40.0), down(36.0), down(28.0), down(20.0)],
            [44.0, 36.0, 36.0, 28.0, 20.0, 16.0]
        );
        assert_eq!([down(16.0), down(12.0), down(8.0)], [8.0, 8.0, 8.0]);
        assert_eq!([up(199.0), up(200.0)], [200.0, 200.0]);
        assert_eq!(up(f64::NAN), 28.0);
        assert_eq!(down(f64::INFINITY), 16.0);
    }

    #[test]
    fn stepping_a_text_remeasures_it_and_keeps_the_top_left() {
        let mut store = MemoryStore::new();
        let t = text(&mut store, "t", "abcd", 20.0);
        let mut tx = store.begin();
        let r = step_font_size(
            &mut tx,
            &MonospaceMeasure::default(),
            &FakeMath,
            std::slice::from_ref(&t),
            FontStep::Increase,
        )
        .unwrap();
        assert_eq!(r.changed, vec![t.clone()]);
        let e = tx.get(&t).unwrap();
        assert_eq!(e.text_data().unwrap().font.size, 28.0);
        assert_eq!((e.transform.x, e.transform.y, e.transform.w), (10.0, 20.0, 4.0 * 14.0));
    }

    #[test]
    fn stepping_at_the_limit_changes_nothing_and_a_rectangle_without_label_is_skipped() {
        let mut store = MemoryStore::new();
        let t = text(&mut store, "t", "ab", 200.0);
        let mut tx = store.begin();
        let z = tx.next_top_index().unwrap();
        tx.insert(Element::new(id("r"), ElementKind::Rectangle, z).with_bounds(0.0, 0.0, 50.0, 50.0)).unwrap();
        let r =
            step_font_size(&mut tx, &MonospaceMeasure::default(), &FakeMath, &[t.clone(), id("r")], FontStep::Increase)
                .unwrap();
        assert_eq!((r.changed.len(), r.unchanged, r.skipped), (0, vec![t], vec![id("r")]));
    }

    #[test]
    fn a_container_steps_its_label() {
        let mut store = MemoryStore::new();
        let mut tx = store.begin();
        let z = tx.next_top_index().unwrap();
        tx.insert(Element::new(id("r"), ElementKind::Rectangle, z).with_bounds(0.0, 0.0, 200.0, 100.0)).unwrap();
        let mut ids = SeqIds::default();
        let l = crate::text_bind::create_label(&mut tx, &MonospaceMeasure::default(), &mut ids, &id("r"), "hi", None)
            .unwrap()
            .label;
        let r =
            step_font_size(&mut tx, &MonospaceMeasure::default(), &FakeMath, &[id("r")], FontStep::Increase).unwrap();
        assert_eq!(r.changed, vec![l.clone()]);
        assert_eq!(label_of(&tx, &id("r")), Some(l.clone()));
        assert_eq!(tx.get(&l).unwrap().text_data().unwrap().font.size, 28.0);
    }

    #[test]
    fn set_latex_resizes_and_a_parse_error_leaves_the_element_alone() {
        let mut store = MemoryStore::new();
        let mut tx = store.begin();
        let z = tx.next_top_index().unwrap();
        tx.insert(Element::new(id("m"), ElementKind::Math(MathData::new("x")), z).with_bounds(5.0, 6.0, 10.0, 30.0))
            .unwrap();
        assert!(set_latex(&mut tx, &FakeMath, &id("m"), "x^2").unwrap());
        let e = tx.get(&id("m")).unwrap();
        assert_eq!((e.transform.x, e.transform.y, e.transform.w, e.transform.h), (5.0, 6.0, 30.0, 30.0));
        let err = set_latex(&mut tx, &FakeMath, &id("m"), "\\bad").unwrap_err();
        assert_eq!(err.code(), "invalid_latex");
        assert!(err.to_string().contains("unknown command"));
        assert_eq!(tx.get(&id("m")).unwrap().transform.w, 30.0);
        assert!(!set_latex(&mut tx, &FakeMath, &id("m"), "x^2").unwrap());
        assert_eq!(set_latex(&mut tx, &FakeMath, &id("m"), "  ").unwrap_err().code(), "invalid_latex");
    }

    #[test]
    fn style_range_sets_clears_and_checks_the_range() {
        let mut store = MemoryStore::new();
        let t = text(&mut store, "t", "hello", 20.0);
        let mut tx = store.begin();
        let red = Color::parse("#ff0000").unwrap();
        assert!(style_range(&mut tx, &t, 1, 3, Some(red)).unwrap());
        assert_eq!(tx.get(&t).unwrap().text_data().unwrap().runs, vec![TextRun { start: 1, end: 3, color: Some(red) }]);
        assert!(!style_range(&mut tx, &t, 1, 3, Some(red)).unwrap());
        assert!(style_range(&mut tx, &t, 1, 3, None).unwrap());
        assert!(tx.get(&t).unwrap().text_data().unwrap().runs.is_empty());
        let e = style_range(&mut tx, &t, 2, 9, Some(red)).unwrap_err();
        assert_eq!(e.code(), "bad_range");
        assert!(e.to_string().contains("5 characters"));
        assert_eq!(style_range(&mut tx, &t, 3, 3, Some(red)).unwrap_err().code(), "bad_range");
    }

    #[test]
    fn wrapping_makes_a_shape_that_holds_the_text() {
        let mut store = MemoryStore::new();
        let t = text(&mut store, "t", "hello", 20.0);
        let mut tx = store.begin();
        let mut ids = SeqIds::default();
        let (w, skipped) = wrap_in_container(
            &mut tx,
            &MonospaceMeasure::default(),
            &mut ids,
            std::slice::from_ref(&t),
            Kind::Rectangle,
            16.0,
        )
        .unwrap();
        assert!(skipped.is_empty());
        let c = tx.get(&w[0].container).unwrap();
        assert_eq!(c.tag(), Kind::Rectangle);
        assert_eq!(c.transform.w, 50.0 + 32.0);
        assert_eq!(tx.get(&t).unwrap().text_data().unwrap().container_id, Some(w[0].container.clone()));
        assert!(c.index < tx.get(&t).unwrap().index);
        // A second wrap skips the label.
        let (w2, skipped2) = wrap_in_container(
            &mut tx,
            &MonospaceMeasure::default(),
            &mut ids,
            std::slice::from_ref(&t),
            Kind::Rectangle,
            16.0,
        )
        .unwrap();
        assert!((w2.len(), skipped2.len()) == (0, 1));
    }

    #[test]
    fn rotated_text_keeps_its_top_left_corner() {
        let mut t = Transform::new(0.0, 0.0, 100.0, 20.0);
        t.angle = std::f64::consts::FRAC_PI_2;
        let before = t.local_to_world(Point::ZERO);
        let (x, y) = anchor_top_left(&t, 60.0, 20.0);
        let mut n = Transform::new(x, y, 60.0, 20.0);
        n.angle = t.angle;
        let after = n.local_to_world(Point::ZERO);
        assert!((before - after).hypot() < 1e-9);
    }
}
