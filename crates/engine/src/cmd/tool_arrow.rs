//! `tool.set_arrow`: the path type and the heads that the arrow tool gives a new arrow
//! (docs/commands.md §2.3).
//!
//! The values are tool defaults, like the style defaults: `Session::tool.arrow_default` in memory and the
//! settings `arrow.type`, `arrow.start_head` and `arrow.end_head` on disk. This command writes the settings
//! (`settings::set_internal`), which rebuild the in-memory copy, so there is one rule for both. The arrow tool
//! reads `Session::tool.arrow_default` and passes it to `element.create` as plain arguments. `element.create`
//! itself ignores the defaults, so a script that omits `arrow_type` still gets a sharp arrow.
//!
//! Arrows that already exist are changed with `arrow.set_type` and `arrow.set_head`.

use drafftink_edit::arrow_defaults;
use drafftink_scene::{Head, PathType};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::settings;
use crate::refusal::Refusal;
use crate::session::Session;
use crate::spec::{
    Batch, CommandSpec, Facts, Gate, Mutates, PortSet, Redraw, RingMode, Selection, TextGate, Typed, Undo,
};

fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// Arguments of `tool.set_arrow`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct SetArrowArgs {
    /// How new arrows are drawn: `sharp` (straight segments), `round` (a curve) or `elbow` (right angles).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arrow_type: Option<PathType>,
    /// The head at the start of new arrows. `null` or `"none"` means no head.
    #[serde(deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<Head>")]
    pub start_head: Option<Option<Head>>,
    /// The head at the end of new arrows. `null` or `"none"` means no head.
    #[serde(deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<Head>")]
    pub end_head: Option<Option<Head>>,
}

/// Output of `tool.set_arrow`: the defaults in effect.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct SetArrowOut {
    /// False when the defaults were already as requested.
    pub changed: bool,
    /// The path type of new arrows.
    pub arrow_type: PathType,
    /// The head at the start of new arrows.
    pub start_head: Head,
    /// The head at the end of new arrows.
    pub end_head: Head,
}

fn head_value(h: Option<Option<Head>>) -> Option<Head> {
    h.map(Option::unwrap_or_default)
}

fn set_arrow(s: &mut Session, a: SetArrowArgs) -> Result<SetArrowOut, Refusal> {
    let cmd = s.cmd();
    let (path, start, end) = (a.arrow_type, head_value(a.start_head), head_value(a.end_head));
    if path.is_none() && start.is_none() && end.is_none() {
        return Err(Refusal::bad_args(
            cmd,
            "give at least one of `arrow_type`, `start_head` and `end_head`, for example {\"arrow_type\": \"elbow\"}",
        ));
    }
    let writes = [
        ("arrow.type", path.map(arrow_defaults::path_name)),
        ("arrow.start_head", start.map(arrow_defaults::name_of)),
        ("arrow.end_head", end.map(arrow_defaults::name_of)),
    ];
    let mut changed = false;
    for (key, word) in writes {
        if let Some(word) = word {
            changed |= settings::set_internal(s, key, &json!(word))?;
        }
    }
    let d = s.tool.arrow_default;
    Ok(SetArrowOut { changed, arrow_type: d.path, start_head: d.start_head, end_head: d.end_head })
}

/// The tool command that sets the defaults of new arrows.
pub const COMMANDS: &[CommandSpec] = &[CommandSpec::new(
    "tool.set_arrow",
    "tool",
    "Set new arrow style",
    "Sets the path type (sharp, round, elbow) and the heads that the arrow tool gives new arrows. Existing arrows are not changed.",
    &Typed::new(set_arrow),
    Facts {
        undo: Undo::NoUndo,
        reads_only: false,
        mutates: Mutates::TOOL.with(Mutates::SETTINGS),
        needs_selection: Selection::No,
        ports: PortSet::STORAGE,
        batch: Batch::Allowed,
        view_only: Gate::Allowed,
        while_gesture: Gate::Refused,
        while_text_edit: TextGate::Allowed,
        redraw: Redraw::Overlay,
        ring: RingMode::Always,
        palette: true,
        agent: true,
    },
)
.example(r#"{"arrow_type":"elbow","end_head":"triangle"}"#)];

#[cfg(test)]
#[path = "tool_arrow_tests.rs"]
mod tests;
