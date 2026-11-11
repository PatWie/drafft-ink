//! The text edit session: `text.edit_begin` opens it, the editor widget owns typing, caret and IME
//! (docs/exceptions.md E16), and `text.edit_end` hands the editor's result to the engine.
//!
//! Creating the element, changing its content and its colour runs, growing the container and selecting the
//! result are one undo step. A draft (a text started with `at`) leaves no trace in the document until
//! `text.edit_end` commits it.

use drafftink_edit::frame;
use drafftink_edit::style::{StyleError, StyleProps};
use drafftink_edit::text_bind::{self, MathMeasure, NewText, Relabeled, RunSpec, TextBindError, TextProps};
use drafftink_geom::kurbo::Point;
use drafftink_scene::{
    Color, ElementId, ElementKind, InlineMath, Kind, SceneTx,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::support::{RectOut, check_latex_len, check_text_len, refuse, round2, select_only, with_doc};
use crate::args;
use crate::refusal::Refusal;
use crate::session::{Ref, Session};
use crate::state::TextEdit;

// ---------------------------------------------------------------------------------------------
// The text edit session: text.edit_begin and text.edit_end
// ---------------------------------------------------------------------------------------------

/// The part of a draft that `TextEdit` does not hold (it lives in the session extensions; `TextEdit` is the
/// rolled-back part).
#[derive(Debug, Clone, Default)]
struct Draft {
    at: Point,
    container: Option<ElementId>,
    /// A fixed width for the new text (it wraps); `None` lets the box follow the content.
    width: Option<f64>,
}

/// Where the caret starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CaretWord {
    /// After the last character.
    End,
    /// Before the first character.
    Start,
    /// The whole text is selected.
    All,
}

/// The initial caret: a word or a character index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Caret {
    /// `"end"`, `"start"` or `"all"`.
    Word(CaretWord),
    /// A character index, 0 to the text length.
    Index(u32),
}

impl Default for Caret {
    fn default() -> Self {
        Caret::Word(CaretWord::End)
    }
}

/// Arguments of `text.edit_begin`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct BeginArgs {
    /// The text, math, shape or arrow to edit (a shape or arrow edits its label). An id or `@name`.
    pub id: Option<Ref>,
    /// Start a new text here (top-left, world units). Nothing is created until `text.edit_end`.
    pub at: Option<[f64; 2]>,
    /// With `at`: the shape or arrow that the new text becomes the label of.
    pub container: Option<Ref>,
    /// With `at`: a fixed width for the new text, in world units. The text wraps at it and the box does not follow
    /// the content (a drag with the text tool). Absent: the box follows the content.
    pub width: Option<f64>,
    /// Where the caret starts: `end`, `start`, `all`, or a character index.
    pub caret: Caret,
}

/// Output of `text.edit_begin`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct BeginOut {
    /// What is edited: `text`, `math`, or the kind of the container whose label is edited.
    pub kind: Kind,
    /// The element being edited, absent for a draft. For a container: the container.
    pub id: Option<String>,
    /// True when nothing exists in the document yet.
    pub draft: bool,
    /// The current content (empty for a draft or a shape without a label).
    pub text: String,
    /// The formula, for a math element.
    pub latex: Option<String>,
    /// Bounds of the text being edited (the container's for a shape without a label; zero size for a draft).
    pub bounds: RectOut,
    /// The caret as a character range: equal ends are a caret, different ends a selection.
    pub caret: [u32; 2],
}


/// The checks and conversions of `text.edit_end` that need no session.
struct Final<'a> {
    text: Option<&'a str>,
    latex: Option<&'a str>,
    runs: Option<Vec<RunSpec>>,
    props: TextProps,
}

impl<'a> Final<'a> {
    fn new(cmd: &str, a: &'a EndArgs) -> Result<Self, Refusal> {
        if let Some(t) = &a.text {
            check_text_len(cmd, "text", t)?;
        }
        if let Some(l) = &a.latex {
            check_latex_len(cmd, "latex", l)?;
        }
        Ok(Self {
            text: a.text.as_deref(),
            latex: a.latex.as_deref(),
            runs: a.runs.as_ref().map(|r| r.iter().map(|r| RunSpec { start: r.start, end: r.end, color: r.color }).collect()),
            props: TextProps { inline_math: a.inline_math, ..text_props(cmd, a.style.as_ref())? },
        })
    }

    /// Colour runs and element-level properties of a text element.
    fn restyle<T: SceneTx>(
        &self,
        tx: &mut T,
        m: &dyn text_bind::TextMeasure,
        id: &ElementId,
    ) -> Result<(), TextBindError> {
        if let Some(runs) = &self.runs {
            text_bind::replace_runs(tx, id, runs)?;
        }
        text_bind::apply_text_props(tx, m, id, &self.props)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// text.edit_begin
// ---------------------------------------------------------------------------------------------

const EDIT_KINDS: &str = "text, math, rectangle, diamond, ellipse and arrow elements";

/// The character range for a caret request in a text of `len` characters.
fn resolve_caret(cmd: &str, caret: Caret, len: usize) -> Result<[u32; 2], Refusal> {
    let n = u32::try_from(len).unwrap_or(u32::MAX);
    match caret {
        Caret::Word(CaretWord::End) => Ok([n, n]),
        Caret::Word(CaretWord::Start) => Ok([0, 0]),
        Caret::Word(CaretWord::All) => Ok([0, n]),
        Caret::Index(i) if i <= n => Ok([i, i]),
        Caret::Index(i) => Err(Refusal::bad_arg(
            cmd,
            "caret",
            format!("is {i}, but the text has {n} characters. Use 0 to {n}, or \"end\""),
        )),
    }
}

pub(super) fn edit_begin(s: &mut Session, a: BeginArgs) -> Result<BeginOut, Refusal> {
    let cmd = s.cmd();
    if a.width.is_some() && a.at.is_none() {
        return Err(Refusal::custom(
            "bad_args",
            cmd,
            "`width` only works together with `at`. To change the width of an existing text, use `element.set_bounds`.",
        ));
    }
    match (a.id.as_ref(), a.at, a.container.as_ref()) {
        (Some(_), Some(_), _) => Err(Refusal::custom(
            "bad_args",
            cmd,
            "Give `id`, or `at`, or neither (then exactly one editable element must be selected).",
        )),
        (_, None, Some(_)) => Err(Refusal::custom(
            "bad_args",
            cmd,
            "`container` only works together with `at`. To edit the label of a shape, pass the shape as `id`.",
        )),
        (_, Some(at), container) => begin_draft(s, cmd, at, container, a.width, a.caret),
        (id, None, None) => begin_existing(s, cmd, id, a.caret),
    }
}

/// Starts a draft at `at`, optionally as the label of `container`.
fn begin_draft(
    s: &mut Session,
    cmd: &str,
    at: [f64; 2],
    container: Option<&Ref>,
    width: Option<f64>,
    caret: Caret,
) -> Result<BeginOut, Refusal> {
    let (x, y) = (args::world(cmd, "at[0]", at[0])?, args::world(cmd, "at[1]", at[1])?);
    let width = width.map(|w| args::positive(cmd, "width", w, args::MAX_WORLD)).transpose()?;
    if width.is_some() && container.is_some() {
        return Err(Refusal::bad_arg(cmd, "width", "does not apply to a label: a label takes the width of its shape"));
    }
    let caret = resolve_caret(cmd, caret, 0)?;
    let container = container.map(|c| draft_container(s, cmd, c)).transpose()?;
    s.text_edit = Some(TextEdit { element: None, draft: true });
    s.clear_staged_text_edit();
    let draft = Draft { at: Point::new(x, y), container: container.clone(), width };
    s.ext.with::<Draft, _>(|d| *d = draft);
    select_only(s, &[]);
    Ok(BeginOut {
        kind: Kind::Text,
        id: container.map(|c| c.as_str().to_owned()),
        draft: true,
        text: String::new(),
        latex: None,
        bounds: RectOut { x: round2(x), y: round2(y), w: width.map_or(0.0, round2), h: 0.0 },
        caret,
    })
}

/// The container of a draft label: it must hold a label, be unlocked and have none yet.
fn draft_container(s: &Session, cmd: &str, c: &Ref) -> Result<ElementId, Refusal> {
    let id = s.resolve_ref(c)?;
    let e = s.element(&id).ok_or_else(|| Refusal::not_found(cmd, &c.0))?;
    if !e.tag().is_label_container() {
        return Err(Refusal::wrong_kind(
            cmd,
            e.tag().name(),
            "rectangle, diamond, ellipse and arrow elements (they can hold a label)",
        ));
    }
    if e.locked {
        return Err(Refusal::locked(cmd, id.as_str()));
    }
    if let Some(existing) = text_bind::label_of(s.store(), &id) {
        return Err(Refusal::custom(
            "bad_args",
            cmd,
            format!(
                "`{id}` already has a label (`{existing}`). Edit it with `text.edit_begin` and `id` = `{id}`, or remove it with `text.unbind` first."
            ),
        ));
    }
    Ok(id)
}

/// The element to edit: the given one, else the single selected element.
fn edit_target(s: &Session, cmd: &str, id: Option<&Ref>) -> Result<ElementId, Refusal> {
    if let Some(r) = id {
        return s.resolve_ref(r);
    }
    let live: Vec<&ElementId> = s.selection().iter().filter(|i| s.element(i).is_some()).collect();
    match live.as_slice() {
        [one] => Ok((*one).clone()),
        other => Err(Refusal::needs_single(cmd, "text, math, shape or arrow", other.len())),
    }
}

/// Starts a session on an existing text, math, shape or arrow.
fn begin_existing(s: &mut Session, cmd: &str, id: Option<&Ref>, caret: Caret) -> Result<BeginOut, Refusal> {
    let id = edit_target(s, cmd, id)?;
    let e = s.element(&id).ok_or_else(|| Refusal::not_found(cmd, id.as_str()))?;
    let kind = e.tag();
    if !(matches!(kind, Kind::Text | Kind::Math) || kind.is_label_container()) {
        return Err(Refusal::wrong_kind(cmd, kind.name(), EDIT_KINDS));
    }
    let label_container = label_container_of(s, &id);
    for locked in std::iter::once(Some(&id)).chain([label_container.as_ref()]).flatten() {
        if s.element(locked).is_some_and(|x| x.locked) {
            return Err(Refusal::locked(cmd, locked.as_str()));
        }
    }
    let (text, latex, bounds) = match &e.kind {
        ElementKind::Text(t) => (t.content.as_str(), None, e.rect()),
        ElementKind::Math(m) => (m.latex.as_str(), Some(m.latex.clone()), e.rect()),
        _ => match text_bind::label_of(s.store(), &id).and_then(|l| s.element(&l)) {
            Some(l) => (l.text_data().map_or("", |t| t.content.as_str()), None, l.rect()),
            None => ("", None, e.rect()),
        },
    };
    let caret = resolve_caret(cmd, caret, text.chars().count())?;
    let out = BeginOut {
        kind: if label_container.is_some() { Kind::Text } else { kind },
        id: Some(id.as_str().to_owned()),
        draft: false,
        text: text.to_owned(),
        latex,
        bounds: bounds.into(),
        caret,
    };
    let selected = label_container.unwrap_or_else(|| id.clone());
    s.text_edit = Some(TextEdit { element: Some(id), draft: false });
    s.clear_staged_text_edit();
    select_only(s, std::slice::from_ref(&selected));
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// text.edit_end
// ---------------------------------------------------------------------------------------------

/// One colour range chosen in the editor.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunArg {
    /// First character.
    pub start: u32,
    /// One past the last character.
    pub end: u32,
    /// The colour of the range.
    pub color: Color,
}

/// The text properties of the `style` argument: validated like `style.set`, and only those text can take.
fn text_props(cmd: &str, style: Option<&StyleProps>) -> Result<TextProps, Refusal> {
    let Some(st) = style.filter(|st| !st.is_empty()) else { return Ok(TextProps::default()) };
    st.validate().map_err(|e| match e {
        StyleError::OutOfRange { prop, expected } => Refusal::bad_arg(cmd, &format!("style.{}", prop.name()), expected),
        other => Refusal::bad_arg(cmd, "style", other.to_string()),
    })?;
    TextProps::from_style(st).map_err(|props| {
        let named = props.iter().map(|p| format!("`{}`", p.name())).collect::<Vec<_>>().join(", ");
        Refusal::not_applicable(cmd, &named, "text, labels and formulas (colour, opacity and the font properties). Use `style.set` for other properties")
    })
}

/// Arguments of `text.edit_end`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct EndArgs {
    /// True: apply the editor's content. False: discard the session and leave the document as it was.
    pub commit: bool,
    /// The final content of the text or label. Absent keeps the current content (and discards a draft).
    pub text: Option<String>,
    /// The final formula, when a math element is edited.
    pub latex: Option<String>,
    /// Colour ranges of the text. Absent keeps the current ones.
    pub runs: Option<Vec<RunArg>>,
    /// Text properties chosen during the edit, applied in the same step: the colour, opacity and font
    /// properties of `style.set` (`stroke_color`, `opacity`, `font_family`, `font_weight`, `font_size`,
    /// `text_align`, `vertical_align`, `line_height`). Other style properties are refused.
    pub style: Option<StyleProps>,
    /// Whether `$...$` in the text is typeset as math (`auto` or `off`, see `text.set`). Absent keeps the setting.
    pub inline_math: Option<InlineMath>,
}

impl Default for EndArgs {
    fn default() -> Self {
        Self { commit: true, text: None, latex: None, runs: None, style: None, inline_math: None }
    }
}

/// Output of `text.edit_end`.
#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
pub struct EndOut {
    /// True when the editor's result was applied (even if it changed nothing).
    pub committed: bool,
    /// The element that was created or edited. For a label: the label text. Absent when nothing is left.
    pub id: Option<String>,
    /// The element is selected now (the container, for a label).
    pub selected: Option<String>,
    /// A blank result discarded a draft, or deleted the text or label.
    pub discarded: bool,
    /// The text element or label was deleted.
    pub deleted: bool,
}


pub(super) fn not_editing(cmd: &str) -> Refusal {
    Refusal::custom("not_editing", cmd, "No text is being edited. Start with `text.edit_begin`.")
        .suggesting("text.edit_begin", None)
}

pub(super) fn editing_now(s: &Session) -> Result<(), Refusal> {
    if s.text_edit.is_some() { Ok(()) } else { Err(not_editing("text.edit_end")) }
}

pub(super) fn edit_end(s: &mut Session, a: EndArgs) -> Result<EndOut, Refusal> {
    let cmd = s.cmd();
    let Some(te) = s.text_edit.take() else {
        return Err(not_editing(cmd));
    };
    s.clear_staged_text_edit();
    if !a.commit {
        return Ok(EndOut { discarded: true, ..EndOut::default() });
    }
    if s.tab().doc.blocked_by_schema().is_some() {
        return Err(Refusal::newer_schema(cmd));
    }
    let fin = Final::new(cmd, &a)?;
    match te.element {
        None => end_draft(s, cmd, &fin),
        Some(id) => end_existing(s, cmd, &id, &fin),
    }
}

fn label_container_of(s: &Session, id: &ElementId) -> Option<ElementId> {
    s.element(id).and_then(|e| e.text_data()).and_then(|t| t.container_id.clone())
}

/// What a finished edit leaves behind: the element, and what is selected.
fn done(id: Option<&ElementId>, selected: Option<&ElementId>) -> EndOut {
    EndOut {
        committed: true,
        id: id.map(|i| i.as_str().to_owned()),
        selected: selected.map(|i| i.as_str().to_owned()),
        ..EndOut::default()
    }
}

/// A blank result: the draft or the text is gone.
fn gone(deleted: bool) -> EndOut {
    EndOut { committed: true, discarded: true, deleted, ..EndOut::default() }
}

fn end_draft(s: &mut Session, cmd: &str, fin: &Final<'_>) -> Result<EndOut, Refusal> {
    if fin.latex.is_some() {
        return Err(Refusal::bad_arg(cmd, "latex", "is for a math element. A new text is created with `text`"));
    }
    let Draft { at, container, width } = s.ext.with::<Draft, _>(|d| d.clone());
    let Some(content) = fin.text.filter(|t| !t.trim().is_empty()) else {
        return Ok(gone(false));
    };
    let defaults = s.tool.text_defaults.clone();
    let mut font = defaults.clone();
    fin.props.apply_font(&mut font);
    let mut style = s.tool.style_defaults.clone();
    style.stroke_color = fin.props.stroke_color.unwrap_or(style.stroke_color);
    style.opacity = fin.props.opacity.unwrap_or(style.opacity);
    let id = with_doc(s, |tx, m, _math, ids| {
        if let Some(c) = &container {
            let label = match text_bind::relabel(tx, m, ids, c, content, true, Some(&defaults))? {
                Relabeled::Created(o) | Relabeled::Updated(o) => o.label,
                Relabeled::Removed(id) => id,
                Relabeled::Nothing => c.clone(),
            };
            fin.restyle(tx, m, &label)?;
            return Ok(label);
        }
        let frame = frame::frame_at(&*tx, at, &Default::default());
        let id = text_bind::create_text(tx, m, ids, &NewText { at, content, font, style, width, frame })?;
        fin.restyle(tx, m, &id)?;
        Ok(id)
    })
    .map_err(|e| refuse(cmd, e))?;
    let selected = label_container_of(s, &id).unwrap_or_else(|| id.clone());
    select_only(s, std::slice::from_ref(&selected));
    Ok(done(Some(&id), Some(&selected)))
}

fn end_existing(s: &mut Session, cmd: &str, id: &ElementId, fin: &Final<'_>) -> Result<EndOut, Refusal> {
    let Some(e) = s.element(id) else {
        s.warn(format!("The edited element `{id}` no longer exists, so nothing was saved."));
        return Ok(gone(false));
    };
    if e.locked {
        return Err(Refusal::locked(cmd, id.as_str()));
    }
    match e.tag() {
        Kind::Math => end_math(s, cmd, id, fin),
        Kind::Text => end_text(s, cmd, id, fin),
        k if k.is_label_container() => end_container(s, cmd, id, fin),
        _ => Ok(gone(false)),
    }
}

fn end_math(s: &mut Session, cmd: &str, id: &ElementId, fin: &Final<'_>) -> Result<EndOut, Refusal> {
    if fin.text.is_some() {
        return Err(Refusal::bad_arg(cmd, "text", "is for text. A math element is edited with `latex`"));
    }
    if fin.runs.is_some() {
        return Err(Refusal::bad_arg(
            cmd,
            "runs",
            "colour ranges belong to text. A math element has one colour: use `style.stroke_color`",
        ));
    }
    with_doc(s, |tx, _m, math, _ids| {
        if let Some(l) = fin.latex {
            text_bind::set_latex(tx, math, id, l)?;
        }
        apply_math_props(tx, math, id, &fin.props)
    })
    .map_err(|e| refuse(cmd, e))?;
    select_only(s, std::slice::from_ref(id));
    Ok(done(Some(id), Some(id)))
}

fn end_text(s: &mut Session, cmd: &str, id: &ElementId, fin: &Final<'_>) -> Result<EndOut, Refusal> {
    if fin.latex.is_some() {
        return Err(Refusal::bad_arg(cmd, "latex", "is for a math element. A text is edited with `text`"));
    }
    let container = label_container_of(s, id);
    let deleted = with_doc(s, |tx, m, _math, _ids| {
        if let Some(c) = fin.text {
            // The old `exit_text_edit`: a blank free text is removed.
            if c.trim().is_empty() && container.is_none() {
                tx.update(id, |e| e.deleted = true)?;
                return Ok(true);
            }
            if text_bind::set_text_content(tx, m, id, c, fin.runs.is_none())?.is_none() {
                return Ok(true);
            }
        }
        fin.restyle(tx, m, id)?;
        Ok(false)
    })
    .map_err(|e| refuse(cmd, e))?;
    if deleted {
        s.tab_mut().observed.selection.remove(id);
        if let Some(c) = &container {
            select_only(s, std::slice::from_ref(c));
        }
        return Ok(EndOut { selected: container.map(|c| c.as_str().to_owned()), ..gone(true) });
    }
    let selected = container.as_ref().unwrap_or(id);
    select_only(s, std::slice::from_ref(selected));
    Ok(done(Some(id), Some(selected)))
}

fn end_container(s: &mut Session, cmd: &str, id: &ElementId, fin: &Final<'_>) -> Result<EndOut, Refusal> {
    if fin.latex.is_some() {
        return Err(Refusal::bad_arg(cmd, "latex", "is for a math element. A label is edited with `text`"));
    }
    let defaults = s.tool.text_defaults.clone();
    let label = with_doc(s, |tx, m, _math, ids| {
        let mut label = text_bind::label_of(&*tx, id);
        if let Some(c) = fin.text {
            label = match text_bind::relabel(tx, m, ids, id, c, true, Some(&defaults))? {
                Relabeled::Created(o) | Relabeled::Updated(o) => Some(o.label),
                Relabeled::Removed(_) | Relabeled::Nothing => None,
            };
        }
        if let Some(l) = &label {
            fin.restyle(tx, m, l)?;
        }
        Ok(label)
    })
    .map_err(|e| refuse(cmd, e))?;
    select_only(s, std::slice::from_ref(id));
    let removed = label.is_none() && fin.text.is_some_and(|t| t.trim().is_empty());
    Ok(EndOut { deleted: removed, ..done(label.as_ref(), Some(id)) })
}

/// Colour, opacity and size of a formula chosen during the edit.
fn apply_math_props<T: SceneTx>(
    tx: &mut T,
    math: &dyn MathMeasure,
    id: &ElementId,
    props: &TextProps,
) -> Result<(), TextBindError> {
    if props.stroke_color.is_some() || props.opacity.is_some() {
        tx.update(id, |e| {
            e.style.stroke_color = props.stroke_color.unwrap_or(e.style.stroke_color);
            e.style.opacity = props.opacity.unwrap_or(e.style.opacity);
        })?;
    }
    if let (Some(size), Some(ElementKind::Math(m))) = (props.font_size, tx.get(id).map(|e| &e.kind)) {
        let latex = m.latex.clone();
        text_bind::set_math_size(tx, math, id, &latex, size)?;
    }
    Ok(())
}
