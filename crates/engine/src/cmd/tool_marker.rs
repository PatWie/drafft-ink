//! `tool.set_marker`: the colour of the highlighter tool (docs/commands.md §2.3).
//!
//! The highlighter creates no element. A drag of the tool runs `highlight.apply` (E19) with this colour; `null` is
//! "none", and a drag then takes the highlight off what it touches. The colour is tool state: no undo step, not in
//! the document.

use drafftink_scene::MarkerColor;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::refusal::Refusal;
use crate::session::Session;
use crate::spec::{
    Batch, CommandSpec, DocWrite, Facts, Gate, Mutates, PortSet, Redraw, RingMode, Selection, TextGate, Typed, Undo,
};

/// Arguments of `tool.set_marker`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct SetMarkerArgs {
    /// The marker colour, taken exactly as given: `<family>-<shade>` with any palette shade (`yellow-400`,
    /// `blue-100`) or `#rrggbb`. `null` is none: the highlighter then removes highlights.
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

const FACTS: Facts = Facts {
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
    doc_write: DocWrite::NotDocument,
    secret_output: false,
};

/// The tool command that sets the colour of the highlighter.
pub const COMMANDS: &[CommandSpec] = &[CommandSpec::new(
    "tool.set_marker",
    "tool",
    "Set marker colour",
    "Sets the colour of the highlighter tool: a palette colour such as yellow-400 (any shade) or a #rrggbb colour, used exactly as given. null is none: the tool then removes highlights.",
    &Typed::new(set_marker),
    FACTS,
)
.example(r#"{"color":"green-400"}"#)];

#[cfg(test)]
#[path = "tool_marker_tests.rs"]
mod tests;
