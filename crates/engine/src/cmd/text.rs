//! `text`: text, labels and math (docs/commands.md §2.8).
//!
//! `set`, `set_latex`, `style_range`, `step_size`, `bind`, `unbind`, `wrap_in_container` and `set_autoresize` are
//! here. The text edit session (`edit_begin`, `edit_end`) is in `session.rs`; the helpers shared with the diagram,
//! layout and introspection commands are in `support.rs`.
//!
//! The rules live in `drafftink_edit::text_bind` (labels) and `drafftink_edit::text_cmd` (font steps, formulas,
//! colour runs, wrapping, creation). A command validates, delegates, and maps typed errors to refusals.

mod session;
pub(super) mod support;

use drafftink_edit::text_bind::{self, Relabeled};
use drafftink_edit::text_cmd::{self, FontStep};
use drafftink_scene::{Color, ElementId, Kind};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use self::session::{edit_begin, edit_end, editing_now};
use self::support::{RectOut, check_latex_len, check_text_len, ids_out, refuse, select_only, with_doc};
use crate::args;
use crate::refusal::Refusal;
use crate::session::{Ref, Session};
use crate::spec::{
    Batch, Binding, CommandSpec, Facts, Gate, Mutates, Redraw, RingMode, Scope, Selection, TextGate, Typed, Undo,
};

const SET_WRONG_KIND: &str = "`text.set` edits text elements and labels. For math use `text.set_latex`.";
const SET_LATEX_WRONG_KIND: &str = "`text.set_latex` edits math elements. For text use `text.set`.";

fn yes() -> bool {
    true
}

// ---------------------------------------------------------------------------------------------
// text.set
// ---------------------------------------------------------------------------------------------

/// Arguments of `text.set`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetArgs {
    /// The text element, or the shape or arrow whose label to set. An id or `@name`.
    pub id: Ref,
    /// The new content. Line breaks are `\n`.
    pub text: String,
    /// Keep the colours of the unchanged beginning and end of the text.
    #[serde(default = "yes")]
    pub keep_runs: bool,
}

/// Output of `text.set`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SetOut {
    /// The text element that holds the content now (the label for a container). Absent when a label was removed.
    pub id: Option<String>,
    /// The label was removed because `text` was empty.
    pub removed: bool,
    /// The bounds of the text element after the change.
    pub bounds: Option<RectOut>,
}

fn set(s: &mut Session, a: SetArgs) -> Result<SetOut, Refusal> {
    let cmd = s.cmd();
    check_text_len(cmd, "text", &a.text)?;
    let id = s.resolve_ref(&a.id)?;
    let kind = s.element(&id).map(|e| e.tag()).ok_or_else(|| Refusal::not_found(cmd, &a.id.0))?;
    let wrong = || Refusal::custom("wrong_kind", cmd, SET_WRONG_KIND);
    if kind != Kind::Text && !kind.is_label_container() {
        return Err(wrong());
    }
    let text = a.text.as_str();
    let defaults = s.tool.text_defaults.clone();
    let out = with_doc(s, |tx, m, _math, ids| {
        if kind == Kind::Text {
            let r = text_bind::set_text_content(tx, m, &id, text, a.keep_runs)?;
            return Ok(match r {
                Some(_) => (Some(id.clone()), false),
                None => (None, true),
            });
        }
        Ok(match text_bind::relabel(tx, m, ids, &id, text, true, Some(&defaults))? {
            Relabeled::Updated(o) | Relabeled::Created(o) => (Some(o.label), false),
            Relabeled::Removed(_) => (None, true),
            Relabeled::Nothing => (None, false),
        })
    })
    .map_err(|e| refuse(cmd, e))?;
    let (label, removed) = out;
    let bounds = label.as_ref().and_then(|l| s.element(l)).map(|e| RectOut::from(e.rect()));
    Ok(SetOut { id: label.map(|l| l.as_str().to_owned()), removed, bounds })
}

// ---------------------------------------------------------------------------------------------
// text.set_latex
// ---------------------------------------------------------------------------------------------

/// Arguments of `text.set_latex`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetLatexArgs {
    /// The math element. An id or `@name`.
    pub id: Ref,
    /// The formula, for example `\frac{a}{b}`.
    pub latex: String,
}

/// Output of `text.set_latex`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SetLatexOut {
    /// The element.
    pub id: String,
    /// Its bounds after the new layout.
    pub bounds: RectOut,
}

fn set_latex(s: &mut Session, a: SetLatexArgs) -> Result<SetLatexOut, Refusal> {
    let cmd = s.cmd();
    check_latex_len(cmd, "latex", &a.latex)?;
    let id = s.resolve_ref(&a.id)?;
    if s.element(&id).map(|e| e.tag()) != Some(Kind::Math) {
        return Err(Refusal::custom("wrong_kind", cmd, SET_LATEX_WRONG_KIND));
    }
    with_doc(s, |tx, _m, math, _ids| text_cmd::set_latex(tx, math, &id, &a.latex)).map_err(|e| refuse(cmd, e))?;
    let bounds = s.element(&id).map(|e| RectOut::from(e.rect())).unwrap_or(RectOut { x: 0.0, y: 0.0, w: 0.0, h: 0.0 });
    Ok(SetLatexOut { id: id.as_str().to_owned(), bounds })
}

// ---------------------------------------------------------------------------------------------
// text.style_range
// ---------------------------------------------------------------------------------------------

/// Arguments of `text.style_range`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StyleRangeArgs {
    /// The text element. An id or `@name`.
    pub id: Ref,
    /// First character of the range (0-based, in characters).
    pub start: u32,
    /// One past the last character of the range.
    pub end: u32,
    /// The colour of the characters: `#rrggbb`, `#rrggbbaa` or `transparent`. Absent or null clears the colour, so
    /// the characters use the element's colour again.
    #[serde(default)]
    pub color: Option<Color>,
}

/// Output of `text.style_range`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct StyleRangeOut {
    /// The text.
    pub id: String,
    /// The colour runs after the change as `[start, end, colour]`.
    pub runs: Vec<Value>,
}

fn style_range(s: &mut Session, a: StyleRangeArgs) -> Result<StyleRangeOut, Refusal> {
    let cmd = s.cmd();
    let id = s.resolve_ref(&a.id)?;
    with_doc(s, |tx, _m, _math, _ids| text_cmd::style_range(tx, &id, a.start, a.end, a.color)).map_err(|e| refuse(cmd, e))?;
    let runs = s
        .element(&id)
        .and_then(|e| e.text_data())
        .map(|t| t.runs.iter().map(|r| json!([r.start, r.end, r.color])).collect())
        .unwrap_or_default();
    Ok(StyleRangeOut { id: id.as_str().to_owned(), runs })
}

// ---------------------------------------------------------------------------------------------
// text.step_size
// ---------------------------------------------------------------------------------------------

/// Arguments of `text.step_size`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StepSizeArgs {
    /// Larger or smaller. Sizes go 16, 20, 28, 36, then by 8 (8 to 200).
    pub direction: FontStep,
    /// Texts, formulas, or shapes and arrows with a label. Default: the selection.
    #[serde(default)]
    pub ids: Option<Vec<Ref>>,
}

/// Output of `text.step_size`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct StepSizeOut {
    /// Texts and formulas whose size changed.
    pub resized: Vec<String>,
    /// Texts and formulas already at the smallest or largest size.
    pub at_limit: Vec<String>,
    /// Targets that are neither text nor math nor a container with a label.
    pub skipped: Vec<String>,
}

fn step_size(s: &mut Session, a: StepSizeArgs) -> Result<StepSizeOut, Refusal> {
    let cmd = s.cmd();
    let ids = s.targets(a.ids.as_deref())?;
    let report = with_doc(s, |tx, m, math, _ids| text_cmd::step_font_size(tx, m, math, &ids, a.direction))
        .map_err(|e| refuse(cmd, e))?;
    if !report.applicable() {
        return Err(Refusal::not_applicable(cmd, "a font size", "text and math elements, and shapes or arrows that have a label"));
    }
    Ok(StepSizeOut {
        resized: ids_out(&report.changed),
        at_limit: ids_out(&report.unchanged),
        skipped: ids_out(&report.skipped),
    })
}

// ---------------------------------------------------------------------------------------------
// text.bind, text.unbind
// ---------------------------------------------------------------------------------------------

/// Arguments of `text.bind`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BindArgs {
    /// The free text that becomes the label.
    pub text: Ref,
    /// The rectangle, diamond, ellipse or arrow that gets the label.
    pub container: Ref,
}

/// Output of `text.bind`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct BindOut {
    /// The label.
    pub text: String,
    /// Its container.
    pub container: String,
    /// The container grew to fit the text.
    pub container_grew: bool,
}

fn bind(s: &mut Session, a: BindArgs) -> Result<BindOut, Refusal> {
    let cmd = s.cmd();
    let text = s.resolve_ref(&a.text)?;
    let container = s.resolve_ref(&a.container)?;
    let o = with_doc(s, |tx, m, _math, _ids| Ok(text_bind::bind_text(tx, m, &text, &container)?)).map_err(|e| refuse(cmd, e))?;
    Ok(BindOut { text: text.as_str().to_owned(), container: container.as_str().to_owned(), container_grew: o.container_grew })
}

/// Arguments of `text.unbind`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnbindArgs {
    /// The label to detach. An id or `@name`.
    pub text: Ref,
}

/// Output of `text.unbind`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct UnbindOut {
    /// The text, now a free element.
    pub text: String,
}

fn unbind(s: &mut Session, a: UnbindArgs) -> Result<UnbindOut, Refusal> {
    let cmd = s.cmd();
    let text = s.resolve_ref(&a.text)?;
    with_doc(s, |tx, _m, _math, _ids| Ok(text_bind::unbind_text(tx, &text)?)).map_err(|e| refuse(cmd, e))?;
    Ok(UnbindOut { text: text.as_str().to_owned() })
}

// ---------------------------------------------------------------------------------------------
// text.wrap_in_container
// ---------------------------------------------------------------------------------------------

/// The shapes a text can be wrapped in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WrapShape {
    /// A rectangle (the default).
    #[default]
    Rectangle,
    /// An ellipse.
    Ellipse,
    /// A diamond.
    Diamond,
}

/// Arguments of `text.wrap_in_container`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct WrapArgs {
    /// Free text elements. Default: the selection.
    pub ids: Option<Vec<Ref>>,
    /// The shape to create around each text.
    pub shape: WrapShape,
    /// Space between the text and the outline, in world units (0 to 1000).
    pub padding: f64,
}

impl Default for WrapArgs {
    fn default() -> Self {
        Self { ids: None, shape: WrapShape::Rectangle, padding: 16.0 }
    }
}

/// Output of `text.wrap_in_container`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct WrapOut {
    /// The new shapes, one per wrapped text.
    pub containers: Vec<String>,
    /// The texts that were wrapped, in the same order.
    pub texts: Vec<String>,
    /// Targets that were skipped because they are not free text (labels, shapes, lines).
    pub skipped: Vec<String>,
}

fn wrap_in_container(s: &mut Session, a: WrapArgs) -> Result<WrapOut, Refusal> {
    let cmd = s.cmd();
    let padding = args::in_range(cmd, "padding", a.padding, 0.0, 1000.0)?;
    let ids = s.targets(a.ids.as_deref())?;
    let shape = match a.shape {
        WrapShape::Rectangle => Kind::Rectangle,
        WrapShape::Ellipse => Kind::Ellipse,
        WrapShape::Diamond => Kind::Diamond,
    };
    let (wrapped, skipped) =
        with_doc(s, |tx, m, _math, ids_src| text_cmd::wrap_in_container(tx, m, ids_src, &ids, shape, padding))
            .map_err(|e| refuse(cmd, e))?;
    if wrapped.is_empty() {
        return Err(Refusal::not_applicable(cmd, "a container", "free text elements (a text that is already a label is skipped)"));
    }
    let containers: Vec<ElementId> = wrapped.iter().map(|w| w.container.clone()).collect();
    select_only(s, &containers);
    Ok(WrapOut {
        containers: ids_out(&containers),
        texts: ids_out(wrapped.iter().map(|w| &w.text)),
        skipped: ids_out(&skipped),
    })
}

// ---------------------------------------------------------------------------------------------
// text.set_autoresize
// ---------------------------------------------------------------------------------------------

/// Arguments of `text.set_autoresize`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AutoResizeArgs {
    /// Free text elements. Default: the selection.
    #[serde(default)]
    pub ids: Option<Vec<Ref>>,
    /// True: the box follows the content. False: the box keeps its width and the text wraps.
    pub on: bool,
}

/// Output of `text.set_autoresize`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AutoResizeOut {
    /// The texts that were set.
    pub texts: Vec<String>,
    /// Targets that are not text and were skipped.
    pub skipped: Vec<String>,
}

fn set_autoresize(s: &mut Session, a: AutoResizeArgs) -> Result<AutoResizeOut, Refusal> {
    let cmd = s.cmd();
    let ids = s.targets(a.ids.as_deref())?;
    let (texts, skipped): (Vec<ElementId>, Vec<ElementId>) =
        ids.into_iter().partition(|i| s.element(i).is_some_and(|e| e.tag() == Kind::Text));
    if texts.is_empty() {
        return Err(Refusal::not_applicable(cmd, "auto-resize", "text elements"));
    }
    with_doc(s, |tx, m, _math, _ids| {
        for t in &texts {
            text_bind::set_auto_resize(tx, m, t, a.on)?;
        }
        Ok(())
    })
    .map_err(|e| refuse(cmd, e))?;
    Ok(AutoResizeOut { texts: ids_out(&texts), skipped: ids_out(&skipped) })
}
// ---------------------------------------------------------------------------------------------
// registration
// ---------------------------------------------------------------------------------------------

/// A command that edits the document and is also shown to agents.
const DOC: Facts = Facts {
    undo: Undo::Undoable,
    reads_only: false,
    mutates: Mutates::DOC,
    needs_selection: Selection::No,
    ports: crate::spec::PortSet::NONE,
    batch: Batch::Allowed,
    view_only: Gate::Refused,
    while_gesture: Gate::Refused,
    while_text_edit: TextGate::Refused,
    redraw: Redraw::Scene,
    ring: RingMode::Always,
    palette: false,
    agent: true,
};

const STEP_KEYS: &[Binding] = &[
    Binding::new(&["Mod+Shift+."], Scope::Canvas)
        .with_args(r#"{"direction":"increase"}"#)
        .titled("Increase font size"),
    Binding::new(&["Mod+Shift+,"], Scope::Canvas)
        .with_args(r#"{"direction":"decrease"}"#)
        .titled("Decrease font size"),
];
const EDIT_BEGIN_KEYS: &[Binding] = &[Binding::new(&["Enter"], Scope::Canvas)];
const EDIT_END_KEYS: &[Binding] = &[Binding::new(&["Mod+Enter"], Scope::Text).with_args(r#"{"commit":true}"#)];

/// The text family.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "text.set",
        "text",
        "Set text",
        "Replaces the content of a text element, or of the label of a shape or arrow, and re-measures it.",
        &Typed::new(set),
        DOC,
    )
    .refusals(&["wrong_kind", "empty_text"])
    .example(r#"{"id":"@title","text":"Release plan"}"#),
    CommandSpec::new(
        "text.set_latex",
        "text",
        "Set LaTeX",
        "Replaces the formula of a math element and lays it out once.",
        &Typed::new(set_latex),
        DOC,
    )
    .refusals(&["wrong_kind", "invalid_latex"])
    .example(r#"{"id":"@formula","latex":"\\frac{a}{b}"}"#),
    CommandSpec::new(
        "text.edit_begin",
        "text",
        "Start editing text",
        "Starts a text edit session on a text, math, shape or arrow, or a draft text at a point.",
        &Typed::new(edit_begin),
        Facts {
            undo: Undo::NoUndo,
            mutates: Mutates::OBSERVED.with(Mutates::EDIT_SESSION),
            needs_selection: Selection::Soft,
            while_text_edit: TextGate::Commit,
            redraw: Redraw::Overlay,
            agent: false,
            ..DOC
        },
    )
    .refusals(&["wrong_kind"])
    .shortcuts(EDIT_BEGIN_KEYS),
    CommandSpec::new(
        "text.edit_end",
        "text",
        "Finish editing text",
        "Ends the text edit session: applies the editor's final content in one undo step, or discards it.",
        &Typed::new(edit_end),
        Facts {
            mutates: Mutates::DOC.with(Mutates::OBSERVED).with(Mutates::EDIT_SESSION),
            while_text_edit: TextGate::Allowed,
            agent: false,
            ..DOC
        },
    )
    .available(editing_now)
    .refusals(&["not_editing", "invalid_latex"])
    .shortcuts(EDIT_END_KEYS),
    CommandSpec::new(
        "text.style_range",
        "text",
        "Style text range",
        "Sets or clears the colour of a range of characters of a text.",
        &Typed::new(style_range),
        DOC,
    )
    .refusals(&["bad_range"])
    .example(r##"{"id":"@title","start":0,"end":5,"color":"#e03131"}"##),
    CommandSpec::new(
        "text.step_size",
        "text",
        "Step font size",
        "Moves the font size of texts and formulas to the next preset (16, 20, 28, 36, then steps of 8).",
        &Typed::new(step_size),
        Facts { needs_selection: Selection::Required, palette: true, ..DOC },
    )
    .coalesce("text.step_size", 1000)
    .shortcuts(STEP_KEYS)
    .example(r#"{"direction":"increase"}"#),
    CommandSpec::new(
        "text.bind",
        "text",
        "Bind text to container",
        "Makes a free text the label of a rectangle, diamond, ellipse or arrow.",
        &Typed::new(bind),
        DOC,
    )
    .refusals(&["already_bound"])
    .example(r#"{"text":"t1","container":"@box"}"#),
    CommandSpec::new(
        "text.unbind",
        "text",
        "Unbind text",
        "Detaches a label from its container; it stays where it is as a free text.",
        &Typed::new(unbind),
        DOC,
    )
    .refusals(&["nothing_to_do"])
    .example(r#"{"text":"t1"}"#),
    CommandSpec::new(
        "text.wrap_in_container",
        "text",
        "Wrap text in shape",
        "Creates a shape around each free text and binds the text as its label.",
        &Typed::new(wrap_in_container),
        Facts { needs_selection: Selection::Required, palette: true, ..DOC },
    )
    .example(r#"{"shape":"ellipse","padding":24}"#),
    CommandSpec::new(
        "text.set_autoresize",
        "text",
        "Set auto-resize",
        "Makes a text box follow its content, or keep its width so that the text wraps.",
        &Typed::new(set_autoresize),
        Facts { needs_selection: Selection::Required, palette: true, ..DOC },
    )
    .example(r#"{"on":false}"#),
];

#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;
