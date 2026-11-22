//! The highlighter tool's commands (docs/commands.md §2.3, §2.4): `tool.set_marker` and `highlight.apply`.
//!
//! The highlighter tool creates no element. A drag collects the elements the path touches and, for a text, the
//! characters it crosses; at the release the tool runs one `highlight.apply` with all of them, so the whole drag
//! is one undo step and one change for collaborators. The marker colour is tool state (`tool.set_marker`);
//! `null` is "none": dragging then takes the highlight off.
//!
//! The rules (what may be highlighted, the check before the first write) are in `drafftink_edit::highlight`.
//! Where a highlight is stored is the one [`Writer`] below.

use drafftink_edit::highlight::{self, HighlightError, MarkerColor};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::element_common::target_refusal;
use crate::refusal::Refusal;
use crate::session::{Ref, Session};
use crate::spec::{Batch, CommandSpec, Facts, Gate, Mutates, PortSet, Redraw, RingMode, Selection, TextGate, Typed, Undo};

/// The refusal code for a highlight that cannot be written.
const NOT_HIGHLIGHTABLE: &str = "not_highlightable";
/// The refusal code for a build that has no highlight to write yet.
const HIGHLIGHT_UNAVAILABLE: &str = "highlight_unavailable";

/// The writer of highlights.
///
/// **Not wired yet.** The `highlight` style property is built on another branch. Until it lands the writer
/// refuses with plain words, so the tool does nothing rather than store something that nothing draws. To wire it:
/// replace this type with one that implements [`highlight::HighlightWriter`] on the property (the tests keep a
/// stand-in that stores in `custom`, see `cmd/highlight_tests.rs`).
#[cfg(not(test))]
type Writer = Unwired;
#[cfg(test)]
type Writer = tests::CustomWriter;

/// The writer of a build without the `highlight` property.
#[cfg(not(test))]
#[derive(Default)]
struct Unwired;

#[cfg(not(test))]
impl highlight::HighlightWriter for Unwired {
    fn set_element<T: drafftink_scene::SceneTx>(
        &self,
        _tx: &mut T,
        _id: &drafftink_scene::ElementId,
        _color: Option<MarkerColor>,
    ) -> Result<bool, HighlightError> {
        Err(HighlightError::Unavailable)
    }

    fn set_chars<T: drafftink_scene::SceneTx>(
        &self,
        _tx: &mut T,
        _id: &drafftink_scene::ElementId,
        _chars: std::ops::Range<usize>,
        _color: Option<MarkerColor>,
    ) -> Result<bool, HighlightError> {
        Err(HighlightError::Unavailable)
    }
}

fn refusal(cmd: &str, e: HighlightError) -> Refusal {
    match e {
        HighlightError::Target(t) => target_refusal(cmd, t),
        HighlightError::Store(m) => Refusal::internal(cmd, m.to_string()),
        HighlightError::Unavailable => Refusal::custom(HIGHLIGHT_UNAVAILABLE, cmd, e.to_string()),
        other => Refusal::custom(NOT_HIGHLIGHTABLE, cmd, other.to_string()),
    }
}

// ---- tool.set_marker ----------------------------------------------------------------------------------------

/// Arguments of `tool.set_marker`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct SetMarkerArgs {
    /// The marker colour: `yellow`, `green`, `pink`, `blue` or `orange`. `null` is none: the highlighter then
    /// removes highlights.
    pub color: Option<MarkerColor>,
}

/// Result of `tool.set_marker`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct SetMarkerOut {
    /// False when the marker already had this colour.
    pub changed: bool,
    /// The colour now, `null` for none.
    pub color: Option<MarkerColor>,
}

fn set_marker(s: &mut Session, a: SetMarkerArgs) -> Result<SetMarkerOut, Refusal> {
    let changed = s.tool.marker != a.color;
    s.tool.marker = a.color;
    Ok(SetMarkerOut { changed, color: s.tool.marker })
}

// ---- highlight.apply ----------------------------------------------------------------------------------------

/// One element for `highlight.apply`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetArg {
    /// The element: an id or `@name`.
    pub id: Ref,
    /// For a text: the characters to highlight, as `[start, end)` pairs of character indices (not bytes). Absent
    /// is the whole element.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chars: Option<Vec<[usize; 2]>>,
}

/// Arguments of `highlight.apply`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ApplyArgs {
    /// What to highlight: elements, and for a text optionally the characters.
    pub targets: Vec<TargetArg>,
    /// The colour, or `null` to take the highlight off.
    pub color: Option<MarkerColor>,
}

/// Result of `highlight.apply`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ApplyOut {
    /// The elements whose highlight changed.
    pub elements: Vec<String>,
}

fn apply(s: &mut Session, a: ApplyArgs) -> Result<ApplyOut, Refusal> {
    let cmd = s.cmd();
    if a.targets.is_empty() {
        return Err(Refusal::bad_arg(cmd, "targets", "needs at least one element"));
    }
    let targets = a
        .targets
        .iter()
        .map(|t| {
            let id = s.resolve_ref(&t.id)?;
            let chars = t.chars.as_ref().map(|r| r.iter().map(|[start, end]| *start..*end).collect());
            Ok(highlight::Target { id, chars })
        })
        .collect::<Result<Vec<_>, Refusal>>()?;
    let (applied, _) = s
        .tab_mut()
        .doc
        .edit::<highlight::Applied, HighlightError>(|tx| highlight::apply(tx, &Writer::default(), &targets, a.color))
        .map_err(|e| refusal(cmd, e))?;
    Ok(ApplyOut { elements: applied.changed.iter().map(|i| i.as_str().to_owned()).collect() })
}

// ---- registration -------------------------------------------------------------------------------------------

const SET_MARKER: Facts = Facts {
    undo: Undo::NoUndo,
    reads_only: false,
    mutates: Mutates::TOOL,
    needs_selection: Selection::No,
    ports: PortSet::NONE,
    batch: Batch::Allowed,
    view_only: Gate::Allowed,
    while_gesture: Gate::Refused,
    while_text_edit: TextGate::Allowed,
    redraw: Redraw::Overlay,
    ring: RingMode::Always,
    palette: true,
    agent: true,
    doc_write: crate::spec::DocWrite::NotDocument,
};

const APPLY: Facts = Facts {
    undo: Undo::Undoable,
    mutates: Mutates::DOC,
    view_only: Gate::Refused,
    while_text_edit: TextGate::Commit,
    redraw: Redraw::Scene,
    doc_write: crate::spec::DocWrite::Edit,
    ..SET_MARKER
};

/// The commands of the highlighter tool.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "tool.set_marker",
        "tool",
        "Set marker colour",
        "Sets the colour of the highlighter tool: yellow, green, pink, blue or orange. null is none: the tool then removes highlights.",
        &Typed::new(set_marker),
        SET_MARKER,
    )
    .example(r#"{"color":"green"}"#),
    CommandSpec::new(
        "highlight.apply",
        "highlight",
        "Highlight",
        "Turns the highlight of elements on with a colour, or off with null; for a text, only of the given characters. All of it is one undo step.",
        &Typed::new(apply),
        APPLY,
    )
    .refusals(&[NOT_HIGHLIGHTABLE, HIGHLIGHT_UNAVAILABLE])
    .example(r#"{"targets":[{"id":"e1"},{"id":"e2","chars":[[0,5]]}],"color":"yellow"}"#),
];

#[cfg(test)]
#[path = "highlight_tests.rs"]
mod tests;
