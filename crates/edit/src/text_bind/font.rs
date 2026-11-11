//! Font size steps, text properties chosen during an edit, and re-measuring after a font change.

use drafftink_geom::kurbo::Vec2;
use drafftink_scene::{
    Color, ElementId, ElementKind, FontFamily, FontWeight, InlineMath, Kind, SceneTx, TextAlign, TextStyle, Transform,
    VerticalAlign,
};

use super::math::{MathMeasure, set_math_size};
use super::{GrowAnchor, TextBindError, TextMeasure, label_of, live, measure_block, relayout_label};
use crate::style::{Prop, StyleProps};
use crate::target::TargetError;

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
) -> Result<(), TextBindError> {
    let e = live(tx, id)?;
    let Some(td) = e.text_data() else {
        return Err(TextBindError::NotText { id: id.clone(), kind: e.tag() });
    };
    if let Some(c) = td.container_id.clone() {
        relayout_label(tx, m, &c, GrowAnchor::Top)?;
        return Ok(());
    }
    let size = measure_block(m, &td.content, &td.runs, &td.font, (!td.auto_resize).then_some(e.transform.w));
    let w = if td.auto_resize { size.width } else { e.transform.w };
    if !(w.is_finite() && size.height.is_finite()) {
        return Err(TextBindError::NotFinite);
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
) -> Result<StepReport, TextBindError> {
    let mut report = StepReport::default();
    let mut targets: Vec<ElementId> = Vec::new();
    for id in ids {
        let e = live(tx, id)?;
        let target = match e.tag() {
            Kind::Text | Kind::Math => Some(id.clone()),
            k if k.is_label_container() => label_of(tx, id),
            _ => None,
        };
        match target {
            Some(t) if !targets.contains(&t) => {
                if e.locked || live(tx, &t)?.locked {
                    return Err(TextBindError::Target(TargetError::Locked(vec![id.clone()])));
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
    /// Whether `$...$` is typeset as math.
    pub inline_math: Option<InlineMath>,
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
            inline_math: None,
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
        if let Some(v) = self.inline_math {
            f.inline_math = v;
        }
    }
}

/// Applies `props` to a text element and re-measures it. Returns false when nothing changed.
pub fn apply_text_props<T: SceneTx, M: TextMeasure + ?Sized>(
    tx: &mut T,
    m: &M,
    id: &ElementId,
    props: &TextProps,
) -> Result<bool, TextBindError> {
    let e = live(tx, id)?;
    let Some(td) = e.text_data() else {
        return Err(TextBindError::NotText { id: id.clone(), kind: e.tag() });
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
