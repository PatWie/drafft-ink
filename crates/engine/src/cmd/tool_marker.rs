//! `tool.set_marker`: the colour of the highlighter tool (docs/commands.md §2.3).
//!
//! The highlighter creates no element. A drag of the tool runs `highlight.apply` (E19) with this colour; `null` is
//! "none", and a drag then takes the highlight off what it touches. The colour is tool state: no undo step, not in
//! the document.

use drafftink_scene::MarkerColor;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use crate::refusal::Refusal;
use crate::session::Session;
use crate::spec::{
    Batch, CommandSpec, DocWrite, Facts, Gate, Mutates, PortSet, Redraw, RingMode, Selection, TextGate, Typed, Undo,
};

/// Arguments of `tool.set_marker`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct SetMarkerArgs {
    /// A marker (`yellow-400`; shades 300, 400, 500) or any colour (`blue-100`, `#ef4444`), mapped to the nearest
    /// marker. `null` is none: the highlighter then removes highlights.
    #[serde(deserialize_with = "marker_of_any_colour")]
    #[schemars(with = "Option<String>")]
    pub color: Option<MarkerColor>,
}

fn marker_of_any_colour<'de, D: Deserializer<'de>>(d: D) -> Result<Option<MarkerColor>, D::Error> {
    let Some(text) = Option::<String>::deserialize(d)? else { return Ok(None) };
    MarkerColor::nearest(&text).map(Some).ok_or_else(|| {
        serde::de::Error::custom(format!(
            "`{}` is not a colour a marker can have. Use a palette colour such as `yellow-400`, a #rrggbb colour, or null for none",
            text.chars().take(40).collect::<String>()
        ))
    })
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
    "Sets the colour of the highlighter tool: a palette family in shade 300, 400 or 500, such as yellow-400. null is none: the tool then removes highlights.",
    &Typed::new(set_marker),
    FACTS,
)
.example(r#"{"color":"green-400"}"#)];

#[cfg(test)]
#[path = "tool_marker_tests.rs"]
mod tests;
