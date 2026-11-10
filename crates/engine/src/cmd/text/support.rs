//! Helpers shared by the text, diagram, layout and introspection commands: one document transaction with the
//! measurers, the mapping from typed text errors to refusals, and a few small output and argument helpers.

use drafftink_edit::text_bind::TextMeasure;
use drafftink_edit::text_cmd::{MathMeasure, TextCmdError};
use drafftink_geom::kurbo::{Rect, Size};
use drafftink_scene::limits;
use drafftink_scene::{ElementId, IdSource};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::refusal::Refusal;
use crate::services::Services;
use crate::session::Session;
use crate::store::DocTx;

// ---------------------------------------------------------------------------------------------
// Shared with the other families of this task: measuring, one document transaction, refusals
// ---------------------------------------------------------------------------------------------

/// Lays out formulas through the engine services (the typesetter and its cache).
struct ServicesMath<'a>(&'a Services);

impl MathMeasure for ServicesMath<'_> {
    fn math_box(&self, latex: &str, size: f64) -> Result<Size, String> {
        match self.0.math_layout(latex, size) {
            Ok(l) => Ok(l.size()),
            Err(e) => {
                let (line, col) = e.line_col(latex);
                Err(format!("{} (line {}, column {})", e.message, line + 1, col + 1))
            }
        }
    }
}

/// Runs `f` on one document transaction of the active tab, together with the text measurer, the formula measurer
/// (both from `Session::services`) and the session's id source. An `Err` leaves the document unchanged.
pub(crate) fn with_doc<R, E: From<drafftink_scene::StoreError>>(
    s: &mut Session,
    f: impl FnOnce(&mut DocTx<'_, '_>, &dyn TextMeasure, &dyn MathMeasure, &mut dyn IdSource) -> Result<R, E>,
) -> Result<R, E> {
    let idx = s.active_index();
    let math = ServicesMath(&s.services);
    let measure = s.services.edit();
    let ids: &mut dyn IdSource = &mut *s.ports.ids;
    match s.tabs.get_mut(idx) {
        Some(tab) => tab.doc.edit(|tx| f(tx, measure, &math, ids)).map(|(r, _)| r),
        None => Err(drafftink_scene::StoreError::Backend("there is no active tab".to_owned()).into()),
    }
}

/// Selects exactly these elements (no groups).
pub(crate) fn select_only(s: &mut Session, ids: &[ElementId]) {
    let o = &mut s.tab_mut().observed;
    o.selection = ids.iter().cloned().collect();
    o.selected_groups.clear();
    o.linear_editor = None;
}

/// Maps a typed error of the text rules to the refusal of the catalog.
pub(crate) fn refuse(cmd: &str, e: TextCmdError) -> Refusal {
    use drafftink_edit::text_bind::TextBindError as B;
    match e {
        TextCmdError::Missing(id) => Refusal::not_found(cmd, id.as_str()),
        TextCmdError::Locked(id) => Refusal::locked(cmd, id.as_str()),
        TextCmdError::NotMath { kind, .. } => Refusal::wrong_kind(cmd, kind, "math elements"),
        TextCmdError::NotText { kind, .. } => Refusal::wrong_kind(cmd, kind, "text elements"),
        e @ (TextCmdError::BadRange { .. } | TextCmdError::EmptyRange { .. }) => {
            Refusal::custom("bad_range", cmd, e.to_string())
        }
        e @ TextCmdError::InvalidLatex(_) => Refusal::custom("invalid_latex", cmd, e.to_string()),
        TextCmdError::NotFinite => Refusal::bad_args(cmd, "a size or position came out as a number that is not finite"),
        TextCmdError::Text(t) => match t {
            B::Missing(id) => Refusal::not_found(cmd, id.as_str()),
            B::NotText { kind, .. } => Refusal::wrong_kind(cmd, kind, "text elements"),
            B::NotContainer { kind, .. } => {
                Refusal::wrong_kind(cmd, kind, "rectangles, diamonds, ellipses and arrows (they can hold a label)")
            }
            B::AlreadyLabelled { container, .. } => Refusal::custom(
                "already_bound",
                cmd,
                format!("`{container}` already has a label. Remove it first with `text.unbind`."),
            )
            .suggesting("text.unbind", None),
            B::BoundElsewhere { text, container } => Refusal::custom(
                "already_bound",
                cmd,
                format!("`{text}` is already the label of `{container}`. Unbind it first with `text.unbind`."),
            )
            .suggesting("text.unbind", Some(json!({ "text": text.as_str() }))),
            B::NotBound(id) => {
                Refusal::custom("nothing_to_do", cmd, format!("`{id}` is not a label of any element."))
            }
            B::Locked(id) => Refusal::locked(cmd, id.as_str()),
            B::EmptyText => Refusal::custom(
                "empty_text",
                cmd,
                "A text element cannot be empty. Delete it with `element.delete`.",
            )
            .suggesting("element.delete", None),
            other => Refusal::internal(cmd, other.to_string()),
        },
        TextCmdError::Store(e) => Refusal::internal(cmd, e.to_string()),
        TextCmdError::Frame(e) => Refusal::internal(cmd, e),
        TextCmdError::Index(e) => Refusal::internal(cmd, e.to_string()),
    }
}

/// A rectangle in command output.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, JsonSchema)]
pub struct RectOut {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
}

impl From<Rect> for RectOut {
    fn from(r: Rect) -> Self {
        Self { x: round2(r.x0), y: round2(r.y0), w: round2(r.width()), h: round2(r.height()) }
    }
}

pub(crate) fn round2(v: f64) -> f64 {
    if v.is_finite() { (v * 100.0).round() / 100.0 } else { 0.0 }
}

pub(crate) fn ids_out<'a>(ids: impl IntoIterator<Item = &'a ElementId>) -> Vec<String> {
    ids.into_iter().map(|i| i.as_str().to_owned()).collect()
}

pub(crate) fn check_text_len(cmd: &str, key: &str, text: &str) -> Result<(), Refusal> {
    let n = text.chars().count();
    if n > limits::MAX_TEXT_CHARS {
        return Err(Refusal::bad_arg(cmd, key, format!("has {n} characters, and the limit is {}", limits::MAX_TEXT_CHARS)));
    }
    Ok(())
}

pub(crate) fn check_latex_len(cmd: &str, key: &str, latex: &str) -> Result<(), Refusal> {
    let n = latex.chars().count();
    if n > limits::MAX_LATEX_CHARS {
        return Err(Refusal::bad_arg(cmd, key, format!("has {n} characters, and the limit is {}", limits::MAX_LATEX_CHARS)));
    }
    Ok(())
}
