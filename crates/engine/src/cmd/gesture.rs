//! `gesture`: the registry face of the gesture protocol (docs/commands.md §1.10, §2.20).
//!
//! The four commands wrap the machinery in `crate::gesture`. They manage document state themselves
//! (`own_transaction`): `gesture.begin` and `gesture.update` run live frames, `gesture.end` commits one
//! undo step through the ordinary pipeline. Shells use the typed methods on `Session` instead.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::refusal::Refusal;
use crate::session::Session;
use crate::spec::{Host, HostEffect};
use crate::spec::{Batch, CommandSpec, Facts, Gate, Mutates, Redraw, RingMode, TextGate, Typed, Undo};

/// Arguments of `gesture.begin`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BeginArgs {
    /// The command to run as a gesture, for example `element.move`.
    pub command: String,
    /// The command's arguments for the first frame.
    #[serde(default)]
    pub args: Value,
}

/// Arguments of `gesture.update`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateArgs {
    /// The gesture id returned by `gesture.begin`.
    pub gesture: String,
    /// The full arguments of this frame, relative to the state at `begin` (absolute, never an increment).
    #[serde(default)]
    pub args: Value,
}

/// Arguments of `gesture.end`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct EndArgs {
    /// The gesture to end. Default: the running one.
    pub gesture: Option<String>,
    /// The final arguments. Default: the last frame's.
    pub args: Option<Value>,
}

/// Arguments of `gesture.cancel`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct CancelArgs {
    /// The gesture to cancel. Default: the running one, if any.
    pub gesture: Option<String>,
}

fn begin(s: &mut Session, a: BeginArgs) -> Result<Value, Refusal> {
    s.gesture_begin_json(&a.command, &a.args)
}

fn update(s: &mut Session, a: UpdateArgs) -> Result<Value, Refusal> {
    s.gesture_update_json(&a.gesture, &a.args)
}

fn end(s: &mut Session, a: EndArgs) -> Result<Value, Refusal> {
    s.gesture_end_json(a.gesture.as_deref(), a.args.as_ref())
}

fn cancel(s: &mut Session, a: CancelArgs) -> Result<Value, Refusal> {
    s.gesture_cancel_json(a.gesture.as_deref())
}

/// Not in batches, not in palettes, not for agents; they change the document through their frames.
const GESTURE: Facts = Facts {
    undo: Undo::NoUndo,
    reads_only: false,
    mutates: Mutates::DOC.with(Mutates::EDIT_SESSION),
    needs_selection: crate::spec::Selection::No,
    ports: crate::spec::PortSet::NONE,
    batch: Batch::Never,
    // A gesture cannot start in view-only mode (the gestured command refuses), and entering view-only
    // mode cancels a running one, so none of the four ever runs there on its own.
    view_only: Gate::Allowed,
    while_gesture: Gate::Allowed,
    while_text_edit: TextGate::Refused,
    redraw: Redraw::Scene,
    ring: RingMode::Always,
    palette: false,
    agent: false,
    doc_write: crate::spec::DocWrite::Managed,
};

/// The gesture family.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "gesture.begin",
        "gesture",
        "Begin gesture",
        "Starts a drag-like action: captures the base state and runs the first live frame of the command.",
        &Typed::new(begin),
        Facts { while_gesture: Gate::Refused, ..GESTURE },
    )
    .host(Host::effect(HostEffect::BeginsGesture))
    .refusals(&["not_gesturable"])
    .own_transaction()
    .example(r#"{"command":"element.move","args":{"dx":10,"dy":0}}"#),
    CommandSpec::new(
        "gesture.update",
        "gesture",
        "Update gesture",
        "Sends one live frame with absolute arguments. A refusal keeps the last good frame.",
        &Typed::new(update),
        Facts { ring: RingMode::Quiet, while_text_edit: TextGate::Allowed, ..GESTURE },
    )
    .host(Host::effect(HostEffect::EndsGestureOnRefusal).silent_refusals())
    .refusals(&["no_gesture"])
    .own_transaction()
    .example(r#"{"gesture":"g1","args":{"dx":40,"dy":5}}"#),
    CommandSpec::new(
        "gesture.end",
        "gesture",
        "End gesture",
        "Ends the gesture: restores the base and runs the command once with the final arguments, as one undo step.",
        &Typed::new(end),
        Facts { undo: Undo::Undoable, mutates: Mutates::DOC.with(Mutates::OBSERVED).with(Mutates::EDIT_SESSION), while_text_edit: TextGate::Allowed, ..GESTURE },
    )
    .host(Host::effect(HostEffect::EndsGesture).silent_refusals())
    .refusals(&["no_gesture"])
    .own_transaction()
    .example(r#"{"args":{"dx":40,"dy":5}}"#),
    CommandSpec::new(
        "gesture.cancel",
        "gesture",
        "Cancel gesture",
        "Cancels the running gesture and restores the state from before it began. With none running it does nothing.",
        &Typed::new(cancel),
        Facts { while_text_edit: TextGate::Allowed, ..GESTURE },
    )
    .host(Host::effect(HostEffect::EndsGesture).silent_refusals())
    .own_transaction()
    .allowed_when_blocked()
    .example(r#"{}"#),
];

#[cfg(test)]
#[path = "gesture_tests.rs"]
mod tests;
