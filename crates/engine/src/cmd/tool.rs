//! `tool`: the active tool, tool lock, temporary tools and the Esc ladder (docs/commands.md §2.3).
//!
//! Tool state is not in the document and not in undo history. `tool.set` is the one place that decides
//! what a tool change implies. The style defaults that new elements start with are `Session::tool
//! .style_defaults`; they are built from the `style.defaults` setting (see `cmd/settings/store.rs`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::refusal::Refusal;
use crate::session::Session;
use crate::spec::{Host};
use crate::spec::{Batch, Binding, CommandSpec, Facts, Gate, Mutates, NoArgs, PortSet, Redraw, RingMode, Scope, Selection, TextGate, Typed, Undo};
use crate::state::Tool;

/// The tools that work while the document is view-only.
fn works_in_view_only(t: Tool) -> bool {
    matches!(t, Tool::Select | Tool::Lasso | Tool::Hand | Tool::Laser)
}

fn check_view_only(s: &Session, id: &str, t: Tool) -> Result<(), Refusal> {
    if s.view_only && !works_in_view_only(t) {
        return Err(Refusal::custom(
            "view_only_tool",
            id,
            "In view-only mode only the select, hand and laser tools are available. Turn view-only off with `view.set_view_only` (on=false).",
        )
        .suggesting("view.set_view_only", Some(json!({ "on": false }))));
    }
    Ok(())
}

/// The tool state after a tool command.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ToolOut {
    /// False when the state was already as requested.
    pub changed: bool,
    /// The tool in effect: the held one, else the active one.
    pub tool: Tool,
    /// The tool that returns when a held tool is released.
    pub active: Tool,
    /// The tool held temporarily, if any.
    pub held: Option<Tool>,
    /// Whether the tool stays after one element is drawn.
    pub tool_lock: bool,
}

fn out(s: &Session, changed: bool) -> ToolOut {
    ToolOut {
        changed,
        tool: s.tool.effective(),
        active: s.tool.active,
        held: s.tool.temporary,
        tool_lock: s.tool.lock,
    }
}

// ---- tool.set ----

/// Arguments of `tool.set`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetArgs {
    /// The tool to use.
    pub tool: Tool,
    /// Keep the current selection. Default: true for `select` and `hand`, false for the drawing tools.
    #[serde(default)]
    pub keep_selection: Option<bool>,
}

fn set(s: &mut Session, a: SetArgs) -> Result<ToolOut, Refusal> {
    let id = s.cmd();
    check_view_only(s, id, a.tool)?;
    let keep = a.keep_selection.unwrap_or(matches!(a.tool, Tool::Select | Tool::Lasso | Tool::Hand));
    let tool_before = s.tool.clone();
    let observed_before = s.tab().observed.clone();
    // A running text edit was committed by the gate before this handler. Leaving the linear point editor
    // belongs to every tool change.
    let obs = &mut s.tab_mut().observed;
    obs.close_editors();
    if !keep {
        obs.selection.clear();
        obs.selected_groups.clear();
        obs.editing_group = None;
    }
    s.tool.active = a.tool;
    let changed = s.tool != tool_before || s.tab().observed != observed_before;
    Ok(out(s, changed))
}

// ---- tool.lock ----

/// Arguments of `tool.lock`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct LockArgs {
    /// Turn the lock on or off. Omit it to toggle.
    pub on: Option<bool>,
}

fn lock(s: &mut Session, a: LockArgs) -> Result<ToolOut, Refusal> {
    let want = a.on.unwrap_or(!s.tool.lock);
    let changed = s.tool.lock != want;
    s.tool.lock = want;
    Ok(out(s, changed))
}

// ---- tool.hold / tool.release ----

fn hand() -> Tool {
    Tool::Hand
}

/// Arguments of `tool.hold`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HoldArgs {
    /// The tool to use until `tool.release`. Default: the hand tool.
    #[serde(default = "hand")]
    pub tool: Tool,
}

fn hold(s: &mut Session, a: HoldArgs) -> Result<ToolOut, Refusal> {
    check_view_only(s, s.cmd(), a.tool)?;
    // Key repeat sends the same hold again: that is not a change.
    let changed = s.tool.temporary != Some(a.tool);
    s.tool.temporary = Some(a.tool);
    Ok(out(s, changed))
}

fn release(s: &mut Session, _: NoArgs) -> Result<ToolOut, Refusal> {
    let changed = s.tool.temporary.take().is_some();
    Ok(out(s, changed))
}

// ---- tool.cancel ----

/// Arguments of `tool.cancel`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct CancelArgs {
    /// false (default): one press undoes one thing, the first rung that applies. true: every rung runs,
    /// as the old app's single Esc did.
    pub all: bool,
}

/// Output of `tool.cancel`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct CancelOut {
    /// False when no rung applied (a stray Esc).
    pub changed: bool,
    /// The rungs that ran, in order: `gesture`, `text_edit`, `crop_editor`, `point_editor`, `group`, `selection`, `tool`.
    pub rungs: Vec<&'static str>,
}

/// Runs another command when its family is registered, else the fallback. The fallback is the whole
/// effect of the command on session state, for sessions built from fewer families (tests).
fn via(s: &mut Session, id: &str, args: serde_json::Value, fallback: impl FnOnce(&mut Session)) -> Result<(), Refusal> {
    if s.registry().find(id).is_some() {
        s.run_nested(id, args)?;
    } else {
        fallback(s);
    }
    Ok(())
}

fn cancel(s: &mut Session, a: CancelArgs) -> Result<CancelOut, Refusal> {
    let mut rungs: Vec<&'static str> = Vec::new();
    let finish = |rungs: Vec<&'static str>| CancelOut { changed: !rungs.is_empty(), rungs };

    // 1. The running gesture goes back to where it began.
    if s.gesture_running() {
        s.gesture_cancel(None)?;
        // The gesture machinery marked its own ring entry as the entry of this call. This call is
        // `tool.cancel`, and it gets its own entry.
        s.run.ring_handled = false;
        rungs.push("gesture");
        if !a.all {
            return Ok(finish(rungs));
        }
    }
    // 2. A text edit ends and is kept (one undo step, as `text.edit_end`).
    if s.text_edit.is_some() {
        let args = s.staged_text_edit().unwrap_or_else(|| json!({ "commit": true }));
        via(s, "text.edit_end", args, |_| {})?;
        s.text_edit = None;
        rungs.push("text_edit");
        if !a.all {
            return Ok(finish(rungs));
        }
    }
    // 3. The crop editor closes. Whatever was dragged in it and not committed is dropped: the interaction layer
    //    commits it on Esc itself (`interact::crop`), so a `tool.cancel` that reaches here is one without a pointer.
    if s.tab().observed.crop_editor.is_some() {
        s.tab_mut().observed.crop_editor = None;
        rungs.push("crop_editor");
        if !a.all {
            return Ok(finish(rungs));
        }
    }
    // 3b. The linear point editor closes.
    if s.tab().observed.linear_editor.is_some() {
        s.tab_mut().observed.linear_editor = None;
        rungs.push("point_editor");
        if !a.all {
            return Ok(finish(rungs));
        }
    }
    // 4. The editing group is left.
    if s.tab().observed.editing_group.is_some() {
        via(s, "group.exit", json!({}), |s| s.tab_mut().observed.editing_group = None)?;
        rungs.push("group");
        if !a.all {
            return Ok(finish(rungs));
        }
    }
    // 5. The selection is cleared.
    if !s.tab().observed.selection.is_empty() || !s.tab().observed.selected_groups.is_empty() {
        via(s, "selection.clear", json!({}), |s| {
            let o = &mut s.tab_mut().observed;
            o.selection.clear();
            o.selected_groups.clear();
            o.editing_group = None;
            o.close_editors();
        })?;
        rungs.push("selection");
        if !a.all {
            return Ok(finish(rungs));
        }
    }
    // 6. Back to the select tool, with the tool lock off.
    if s.tool.active != Tool::Select || s.tool.lock {
        s.tool.active = Tool::Select;
        s.tool.lock = false;
        rungs.push("tool");
    }
    Ok(finish(rungs))
}

// ---- registration ----

const TOOL: Facts = Facts {
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

const SET_KEYS: &[Binding] = &[
    Binding::new(&["V", "1"], Scope::Canvas).with_args(r#"{"tool":"select"}"#).titled("Select tool"),
    Binding::new(&["H"], Scope::Canvas).with_args(r#"{"tool":"hand"}"#).titled("Hand tool"),
    Binding::new(&["R", "2"], Scope::Canvas).with_args(r#"{"tool":"rectangle"}"#).titled("Rectangle tool"),
    Binding::new(&["D", "3"], Scope::Canvas).with_args(r#"{"tool":"diamond"}"#).titled("Diamond tool"),
    Binding::new(&["O", "4"], Scope::Canvas).with_args(r#"{"tool":"ellipse"}"#).titled("Ellipse tool"),
    Binding::new(&["A", "5"], Scope::Canvas).with_args(r#"{"tool":"arrow"}"#).titled("Arrow tool"),
    Binding::new(&["L", "6"], Scope::Canvas).with_args(r#"{"tool":"line"}"#).titled("Line tool"),
    Binding::new(&["P", "7", "X"], Scope::Canvas).with_args(r#"{"tool":"freehand"}"#).titled("Freehand tool"),
    Binding::new(&["T", "8"], Scope::Canvas).with_args(r#"{"tool":"text"}"#).titled("Text tool"),
    Binding::new(&["M", "9"], Scope::Canvas).with_args(r#"{"tool":"math"}"#).titled("Math tool"),
    Binding::new(&["F"], Scope::Canvas).with_args(r#"{"tool":"frame"}"#).titled("Frame tool"),
    Binding::new(&["E", "0"], Scope::Canvas).with_args(r#"{"tool":"eraser"}"#).titled("Eraser tool"),
    Binding::new(&["Z", "K"], Scope::Canvas).with_args(r#"{"tool":"laser"}"#).titled("Laser tool"),
];
const LOCK_KEYS: &[Binding] = &[Binding::new(&["Q"], Scope::Canvas)];
const HOLD_KEYS: &[Binding] = &[Binding::new(&["Space"], Scope::Canvas).with_args(r#"{"tool":"hand"}"#)];
const CANCEL_KEYS: &[Binding] = &[Binding::new(&["Esc"], Scope::Always)];

/// The tool family.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "tool.set",
        "tool",
        "Set tool",
        "Switches to a tool: ends a text edit, leaves the point editor and, for a drawing tool, clears the selection.",
        &Typed::new(set),
        Facts { mutates: Mutates::TOOL.with(Mutates::OBSERVED), while_text_edit: TextGate::Commit, ..TOOL },
    )
    .refusals(&["view_only_tool"])
    .shortcuts(SET_KEYS)
    .example(r#"{"tool":"rectangle"}"#),
    CommandSpec::new(
        "tool.lock",
        "tool",
        "Keep tool active",
        "Keeps the drawing tool after an element is created instead of returning to select (omitted toggles).",
        &Typed::new(lock),
        TOOL,
    )
    .shortcuts(LOCK_KEYS)
    .example(r#"{"on":true}"#),
    CommandSpec::new(
        "tool.hold",
        "tool",
        "Hold tool temporarily",
        "Activates a tool until `tool.release`, for Space-drag panning. A repeat of the same hold changes nothing.",
        &Typed::new(hold),
        Facts { palette: false, agent: false, ..TOOL },
    )
    .host(Host::first_press_only())
    .refusals(&["view_only_tool"])
    .shortcuts(HOLD_KEYS),
    CommandSpec::new(
        "tool.release",
        "tool",
        "Release held tool",
        "Returns to the tool that was active before `tool.hold`. Without a hold it changes nothing.",
        &Typed::new(release),
        Facts { palette: false, agent: false, while_gesture: Gate::Allowed, ..TOOL },
    ),
    CommandSpec::new(
        "tool.cancel",
        "tool",
        "Cancel",
        "Esc: cancels a drag, ends a text edit, leaves the point editor and the group, clears the selection, then returns to select.",
        &Typed::new(cancel),
        Facts {
            undo: Undo::Undoable,
            mutates: Mutates::DOC.with(Mutates::OBSERVED).with(Mutates::TOOL).with(Mutates::EDIT_SESSION),
            while_gesture: Gate::Allowed,
            doc_write: crate::spec::DocWrite::Managed,
            ..TOOL
        },
    )
    .shortcuts(CANCEL_KEYS)
    .example(r#"{"all":false}"#),
];

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tests;
