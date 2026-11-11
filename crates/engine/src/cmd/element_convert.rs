//! `element.convert`: change the type of shapes, lines and arrows in place (docs/commands.md §2.4).
//!
//! The rules (families, cycle order, labels, bindings, refusals) are in `drafftink_edit::convert`. The command
//! reads the arguments, picks the target and maps the typed errors to catalog refusals.

use drafftink_edit::convert::{self, ConvertError, Target};
use drafftink_scene::words::Article;
use drafftink_scene::{ElementId, PathType};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::element_common::{edit_with_measure, id_strings, target_refusal};
use crate::refusal::Refusal;
use crate::session::{Ref, Session};
use crate::spec::{Host, HostEffect};
use crate::spec::{
    Batch, Binding, CommandSpec, Facts, Gate, Mutates, PortSet, Redraw, RingMode, Scope, Selection, TextGate, Typed, Undo,
};

/// The type to convert to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConvertTo {
    /// A rectangle.
    Rectangle,
    /// A diamond.
    Diamond,
    /// An ellipse.
    Ellipse,
    /// A line.
    Line,
    /// An arrow; `arrow_type` says which kind.
    Arrow,
}

/// Arguments of `element.convert`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ConvertArgs {
    /// Rectangles, diamonds, ellipses, or lines and arrows (not both). Default: the selection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ids: Option<Vec<Ref>>,
    /// The type to convert to. Absent: the next type of the family (rectangle, diamond, ellipse; or line, sharp
    /// arrow, round arrow, elbow arrow), as Tab does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<ConvertTo>,
    /// With `to` = arrow: sharp (default), round or elbow.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arrow_type: Option<PathType>,
    /// Without `to`: take the previous type instead of the next (Shift+Tab).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub reverse: bool,
}

/// What `element.convert` did.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ConvertOut {
    /// The type now, in words: `rectangle`, `diamond`, `ellipse`, `line`, `sharp arrow`, `round arrow` or
    /// `elbow arrow`.
    pub to: String,
    /// The elements whose type changed.
    pub converted: Vec<String>,
    /// Elements that already had the type.
    pub unchanged: Vec<String>,
    /// Arrows bound to a converted shape that were put back on its new outline.
    pub followers: Vec<String>,
}

fn target(cmd: &str, a: &ConvertArgs) -> Result<Option<Target>, Refusal> {
    if a.arrow_type.is_some() && a.to != Some(ConvertTo::Arrow) {
        return Err(Refusal::bad_arg(cmd, "arrow_type", "applies only with `to` set to `arrow`"));
    }
    if a.reverse && a.to.is_some() {
        return Err(Refusal::bad_arg(cmd, "reverse", "applies only without `to`: it picks the previous type"));
    }
    Ok(a.to.map(|to| match to {
        ConvertTo::Rectangle => Target::Rectangle,
        ConvertTo::Diamond => Target::Diamond,
        ConvertTo::Ellipse => Target::Ellipse,
        ConvertTo::Line => Target::Line,
        ConvertTo::Arrow => Target::Arrow(a.arrow_type.unwrap_or(PathType::Sharp)),
    }))
}

const CONVERTIBLE: &str = "rectangles, diamonds, ellipses, lines and arrows";

fn ids_of(id: &ElementId) -> Value {
    json!({ "ids": [id.as_str()] })
}

fn refuse(cmd: &str, e: ConvertError) -> Refusal {
    match e {
        ConvertError::NoElements => Refusal::no_target(cmd),
        ConvertError::Target(t) => target_refusal(cmd, t),
        ConvertError::NotConvertible { id, kind } => {
            Refusal::wrong_kind(cmd, kind.name(), CONVERTIBLE).with_details(ids_of(&id))
        }
        ConvertError::Mixed => Refusal::custom(
            "mixed_kinds",
            cmd,
            "Shapes (rectangle, diamond, ellipse) and lines or arrows cannot be converted together. Select only shapes, or only lines and arrows.",
        ),
        ConvertError::WrongFamily { id, from, to } => Refusal::custom(
            "not_convertible",
            cmd,
            format!(
                "`{id}` is {} and cannot become {}. Shapes become a rectangle, diamond or ellipse; lines and arrows become a line or an arrow.",
                Article::new(from),
                Article::new(to.words())
            ),
        )
        .with_details(ids_of(&id)),
        ConvertError::BoundArrow(id) => Refusal::custom(
            "not_convertible",
            cmd,
            format!("The arrow `{id}` is attached to a shape, and a line cannot be attached. Detach it first with `arrow.unbind`, or convert it to another arrow type."),
        )
        .suggesting("arrow.unbind", Some(json!({ "ids": [id.as_str()] })))
        .with_details(ids_of(&id)),
        ConvertError::LabelledArrow(id) => Refusal::custom(
            "not_convertible",
            cmd,
            format!("The arrow `{id}` has a label, and a line cannot hold one. Remove the label first with `element.relabel` (empty text), or convert it to another arrow type."),
        )
        .with_details(ids_of(&id)),
        ConvertError::ClosedLine(id) => Refusal::custom(
            "not_convertible",
            cmd,
            format!("The line `{id}` is closed, so it is an outline and not a path. Only open lines become arrows."),
        )
        .with_details(ids_of(&id)),
        ConvertError::AlreadyThere { to } => {
            Refusal::nothing_to_do(cmd, "convert", &format!("every element is already {}", Article::new(to.words())))
        }
        other => Refusal::internal(cmd, other.to_string()),
    }
}

fn convert_elements(s: &mut Session, a: ConvertArgs) -> Result<ConvertOut, Refusal> {
    let cmd = s.cmd();
    let to = target(cmd, &a)?;
    let ids = s.targets(a.ids.as_deref())?;
    let reverse = a.reverse;
    let done = edit_with_measure::<_, ConvertError>(s, |tx, measure, _| convert::convert(tx, measure, &ids, to, reverse))
        .map_err(|e| refuse(cmd, e))?;
    Ok(ConvertOut {
        to: done.to.words().to_owned(),
        converted: id_strings(done.changed.iter()),
        unchanged: id_strings(done.unchanged.iter()),
        followers: id_strings(done.followers.iter()),
    })
}

const FACTS: Facts = Facts {
    undo: Undo::Undoable,
    reads_only: false,
    mutates: Mutates::DOC.with(Mutates::OBSERVED),
    needs_selection: Selection::Required,
    ports: PortSet::NONE,
    batch: Batch::Allowed,
    view_only: Gate::Refused,
    while_gesture: Gate::Refused,
    while_text_edit: TextGate::Commit,
    redraw: Redraw::Scene,
    ring: RingMode::Always,
    palette: true,
    agent: true,
    doc_write: crate::spec::DocWrite::Edit,
};

const KEYS: &[Binding] = &[
    Binding::new(&["Tab"], Scope::Canvas).with_args(r#"{"reverse":false}"#).titled("Next element type"),
    Binding::new(&["Shift+Tab"], Scope::Canvas).with_args(r#"{"reverse":true}"#).titled("Previous element type"),
];

/// Commands of this file.
pub const COMMANDS: &[CommandSpec] = &[CommandSpec::new(
    "element.convert",
    "element",
    "Convert element type",
    "Changes rectangles, diamonds and ellipses into one another, or lines and arrows into one another, keeping text, style and attached arrows; without `to` it takes the next type.",
    &Typed::new(convert_elements),
    FACTS,
)
    .host(Host::effect(HostEffect::ShowsConvertPopup))
.shortcuts(KEYS)
.refusals(&["mixed_kinds", "not_convertible"])
.example(r#"{"ids":["e1"],"to":"diamond"}"#)];

#[cfg(test)]
#[path = "element_convert_tests.rs"]
mod tests;
