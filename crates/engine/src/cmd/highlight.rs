//! `highlight`: the marker band behind elements and behind characters of a text (docs/commands.md §2.7a).
//!
//! `highlight.apply` is the one call for "highlight these elements and these character ranges": the highlight
//! control, the highlighter tool and agents use it. A call is one transaction, so it is one undo step and one sync
//! change however many elements and ranges it names. The rules are in `drafftink_edit::highlight`.
//!
//! The whole element is also a style property (`style.set` with `{"props": {"highlight": "yellow-400"}}`); that is the
//! way to set it together with other properties and for the defaults. `highlight.apply` is the way to take it
//! to ranges of characters, and to many targets with their ranges at once.

use drafftink_edit::highlight::{self, CharRange, HighlightError};
use drafftink_edit::style::{Prop, StyleError};
use drafftink_scene::{ElementId, MarkerColor};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::text::support::{ids_out, refuse, with_doc};
use crate::cmd::element_common::target_refusal;
use crate::refusal::Refusal;
use crate::session::{Ref, Session};
use crate::spec::{
    Batch, CommandSpec, Facts, Gate, Host, Mutates, PortSet, Redraw, RingMode, Selection, TextGate, Typed, Undo,
};

/// A range of characters of one text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RangeArg {
    /// The text element, or a label (which is a text element). An id or `@name`.
    pub id: Ref,
    /// First character, counted in Unicode characters (not bytes, not UTF-16 units) from 0.
    pub start: u32,
    /// One past the last character. Above `start`, and at most the number of characters of the text.
    pub end: u32,
}

/// Arguments of `highlight.apply`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyArgs {
    /// The marker colour: `<family>-<shade>` with a palette family and shade 300, 400 or 500 (`yellow-400`); the old
    /// names `yellow`, `green`, `pink`, `blue` and `orange` mean the 400 shade. `null` (or absent) takes the highlight
    /// off.
    #[serde(default)]
    pub color: Option<MarkerColor>,
    /// Elements to highlight as a whole. A text named here is highlighted all over. Default: the selection when no
    /// `ranges` are given, and none when they are.
    #[serde(default)]
    pub ids: Option<Vec<Ref>>,
    /// Character ranges of texts to highlight. Several ranges, of one text or of many, are one step.
    #[serde(default)]
    pub ranges: Vec<RangeArg>,
}

/// Output of `highlight.apply`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ApplyOut {
    /// False when everything already looked as asked.
    pub changed: bool,
    /// Elements and texts whose highlight changed.
    pub highlighted: Vec<String>,
    /// Elements and texts that already looked as asked.
    pub unchanged: Vec<String>,
    /// Elements that cannot be highlighted (frames).
    pub skipped: Vec<String>,
}

fn apply(s: &mut Session, a: ApplyArgs) -> Result<ApplyOut, Refusal> {
    let cmd = s.cmd();
    let elements: Vec<ElementId> = match (a.ids.as_deref(), a.ranges.is_empty()) {
        (Some(refs), _) => s.targets(Some(refs))?,
        (None, true) => s.targets(None)?,
        (None, false) => Vec::new(),
    };
    let ranges = a
        .ranges
        .iter()
        .map(|r| Ok(CharRange { id: s.resolve_ref(&r.id)?, start: r.start, end: r.end }))
        .collect::<Result<Vec<_>, Refusal>>()?;
    let report = with_doc(s, |tx, _m, _math, _ids| highlight::apply(tx, a.color, &elements, &ranges))
        .map_err(|e| refusal(cmd, e))?;
    Ok(ApplyOut {
        changed: !report.changed.is_empty(),
        highlighted: ids_out(&report.changed),
        unchanged: ids_out(&report.unchanged),
        skipped: ids_out(&report.skipped),
    })
}

fn refusal(cmd: &str, e: HighlightError) -> Refusal {
    match e {
        HighlightError::Nothing => {
            Refusal::bad_args(cmd, "name elements in `ids`, character ranges in `ranges`, or select something")
        }
        HighlightError::Style(StyleError::Target(t)) => target_refusal(cmd, t),
        HighlightError::Style(StyleError::NotApplicable { .. }) => {
            Refusal::not_applicable(cmd, "`highlight`", Prop::Highlight.applies_to())
        }
        HighlightError::Style(other) => Refusal::internal(cmd, other.to_string()),
        HighlightError::Text(e) => refuse(cmd, e),
        HighlightError::Store(e) => Refusal::internal(cmd, e.to_string()),
    }
}

const DOC: Facts = Facts {
    undo: Undo::Undoable,
    reads_only: false,
    mutates: Mutates::DOC,
    needs_selection: Selection::Soft,
    ports: PortSet::NONE,
    batch: Batch::Allowed,
    view_only: Gate::Refused,
    while_gesture: Gate::Refused,
    while_text_edit: TextGate::Refused,
    redraw: Redraw::Scene,
    ring: RingMode::Always,
    palette: false,
    agent: true,
    doc_write: crate::spec::DocWrite::Edit,
};

/// The highlight family.
pub const COMMANDS: &[CommandSpec] = &[CommandSpec::new(
    "highlight.apply",
    "highlight",
    "Highlight",
    "Puts a marker band behind elements, or behind ranges of characters of texts, or takes it off. Elements and ranges in one call are one undo step.",
    &Typed::new(apply),
    DOC,
)
.host(Host::text_editor_takes_style())
.refusals(&["bad_range", "not_applicable"])
.example(r#"{"color":"yellow-400","ids":["@box"],"ranges":[{"id":"@title","start":0,"end":5}]}"#)];

#[cfg(test)]
#[path = "highlight_tests.rs"]
mod tests;
