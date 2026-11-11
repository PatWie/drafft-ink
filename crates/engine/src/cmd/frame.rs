//! `frame`: wrap, unwrap, rename, add, remove and select the contents of frames (docs/commands.md §2.11).
//!
//! Frames clip their children and cannot nest; children sit directly below their frame in z-order. The
//! rules are in `drafftink_edit::frame`; a command resolves its targets and maps the typed errors to the
//! refusals of the catalog. Creating a frame with the frame tool is `element.create` with kind=frame, and
//! deleting a frame with or without its children is `element.delete` (`frame_children`).

use drafftink_edit::frame::{self, FrameError, SkipReason, Skipped, WrapOpts};
use drafftink_edit::group::selection::select_ids;
use drafftink_edit::lifecycle::LifecycleError;
use drafftink_scene::words::Article;
use drafftink_scene::{ElementId, Kind};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::cmd::element_common::target_refusal;
use crate::args;
use crate::refusal::Refusal;
use crate::session::{Ref, Session};
use crate::spec::{
    Batch, Binding, CommandSpec, Facts, Gate, Mutates, PortSet, Redraw, RingMode, Scope, Selection, TextGate, Typed,
    Undo,
};

use super::group::common::{apply_selection, edit, editing_group, selected_ids, zorder_refusal};

// ---- errors ----

fn wrong_kind(cmd: &str, id: &ElementId, kind: &str) -> Refusal {
    Refusal::custom("wrong_kind", cmd, format!("`{cmd}` works on frames, and `{id}` is {}.", Article::new(kind)))
}

fn frame_refusal(cmd: &str, e: FrameError) -> Refusal {
    match e {
        FrameError::Target(t) => target_refusal(cmd, t),
        FrameError::NotAFrame { id, kind } => wrong_kind(cmd, &id, kind.name()),
        FrameError::NestedFrame(_) => Refusal::custom(
            "nested_frame",
            cmd,
            "Frames cannot be nested. Remove the frame from the selection, or unwrap it with `frame.unwrap`.",
        )
        .suggesting("frame.unwrap", None),
        e @ (FrameError::BadPadding | FrameError::NothingToWrap | FrameError::BadName) => {
            Refusal::bad_args(cmd, e.to_string())
        }
        FrameError::ZOrder(z) => zorder_refusal(cmd, z),
        FrameError::Lifecycle(LifecycleError::Target(t)) => target_refusal(cmd, t),
        other @ (FrameError::Store(_) | FrameError::Index(_) | FrameError::Binding(_) | FrameError::Lifecycle(_)) => {
            Refusal::internal(cmd, other.to_string())
        }
    }
}

/// A frame name outside 1 to 256 characters is the caller's mistake: name the key.
fn name_refusal(cmd: &str, e: FrameError) -> Refusal {
    match e {
        e @ FrameError::BadName => Refusal::bad_arg(cmd, "name", e.to_string()),
        other => frame_refusal(cmd, other),
    }
}

/// An element that was left out of a frame, and why.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct SkippedOut {
    /// The element as given.
    pub id: ElementId,
    /// `is_frame`, `group_contains_frame` or `arrow_not_attached`.
    pub reason: String,
}

fn reason_text(r: SkipReason) -> (&'static str, &'static str) {
    match r {
        SkipReason::IsFrame => ("is_frame", "it is a frame, and frames cannot be nested"),
        SkipReason::GroupContainsFrame => ("group_contains_frame", "its group contains a frame"),
        SkipReason::ArrowNotAttached => {
            ("arrow_not_attached", "no end of the arrow is bound to or inside an element of the frame")
        }
    }
}

/// Warns about each skipped element and lists them for the output.
fn report_skipped(s: &mut Session, skipped: Vec<Skipped>) -> Vec<SkippedOut> {
    skipped
        .into_iter()
        .map(|k| {
            let (code, why) = reason_text(k.reason);
            s.warn(format!("`{}` was not added to the frame: {why}.", k.id));
            SkippedOut { id: k.id, reason: code.to_owned() }
        })
        .collect()
}

// ---- frame.wrap ----

/// Arguments of `frame.wrap`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct WrapArgs {
    /// Elements to wrap. Default: the selection.
    pub ids: Option<Vec<Ref>>,
    /// The frame's label. Default: the frame is unnamed and shows "Frame".
    pub name: Option<String>,
    /// Space between the elements and the frame edge. Forced to 0 when an image is included.
    pub padding: f64,
}

impl Default for WrapArgs {
    fn default() -> Self {
        Self { ids: None, name: None, padding: 16.0 }
    }
}

/// Result of `frame.wrap` and `frame.add`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct AddOut {
    /// The frame (new for `frame.wrap`).
    pub frame: ElementId,
    /// Elements that are children of the frame now, bound text not listed.
    pub added: Vec<ElementId>,
    /// Elements that were left out.
    pub skipped: Vec<SkippedOut>,
}

fn wrap(s: &mut Session, a: WrapArgs) -> Result<AddOut, Refusal> {
    let cmd = s.cmd();
    let padding = args::in_range(cmd, "padding", a.padding, 0.0, args::MAX_WORLD)?;
    let ids = s.targets(a.ids.as_deref())?;
    let opts = WrapOpts { name: None, padding };
    let report = edit(s, |tx, id_source| {
        let r = frame::wrap(tx, &ids, id_source, &opts).map_err(|e| frame_refusal(cmd, e))?;
        if let Some(n) = &a.name {
            frame::rename(tx, &r.frame, Some(n)).map_err(|e| name_refusal(cmd, e))?;
        }
        Ok::<_, Refusal>(r)
    })
    .map_err(|e| e.refusal(cmd, |r| r))?;
    let skipped = report_skipped(s, report.add.skipped);
    let editing = editing_group(s);
    let sel = select_ids(s.store(), std::slice::from_ref(&report.frame), editing.as_ref());
    apply_selection(s, sel);
    Ok(AddOut { frame: report.frame, added: report.add.added, skipped })
}

// ---- frame.unwrap ----

/// Arguments of `frame.unwrap`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct UnwrapArgs {
    /// The frame. Default: the selected frame.
    pub frame: Option<Ref>,
}

/// Result of `frame.unwrap`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct UnwrapOut {
    /// The frame that was deleted.
    pub frame: ElementId,
    /// Its children, now free. They are selected.
    pub children: Vec<ElementId>,
}

/// The frame a command works on: the given one, else the single frame among the selection.
fn frame_target(s: &Session, given: Option<&Ref>) -> Result<ElementId, Refusal> {
    let cmd = s.cmd();
    if let Some(r) = given {
        return s.resolve_ref(r);
    }
    let sel = selected_ids(s);
    let frames: Vec<&ElementId> = sel.iter().filter(|i| s.element(i).is_some_and(|e| e.tag() == Kind::Frame)).collect();
    match (frames.as_slice(), sel.first()) {
        ([one], _) => Ok((*one).clone()),
        ([], None) => Err(Refusal::no_target(cmd)),
        ([], Some(first)) => {
            let kind = s.element(first).map_or("element", |e| e.tag().name());
            Err(wrong_kind(cmd, first, kind))
        }
        (many, _) => Err(Refusal::needs_single(cmd, "frame", many.len())),
    }
}

fn unwrap(s: &mut Session, a: UnwrapArgs) -> Result<UnwrapOut, Refusal> {
    let cmd = s.cmd();
    let target = frame_target(s, a.frame.as_ref())?;
    let children =
        edit(s, |tx, _| frame::unwrap(tx, &target)).map_err(|e| e.refusal(cmd, |e| frame_refusal(cmd, e)))?;
    let editing = editing_group(s);
    let sel = select_ids(s.store(), &children, editing.as_ref());
    apply_selection(s, sel);
    Ok(UnwrapOut { frame: target, children })
}

// ---- frame.rename ----

/// Arguments of `frame.rename`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameArgs {
    /// The frame.
    pub frame: Ref,
    /// The label (1 to 256 characters), or null to restore the default "Frame".
    #[serde(default)]
    pub name: Option<String>,
}

/// Result of `frame.rename`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct RenameOut {
    /// The frame.
    pub frame: ElementId,
    /// The label now, `null` for the default.
    pub name: Option<String>,
}

fn rename(s: &mut Session, a: RenameArgs) -> Result<RenameOut, Refusal> {
    let cmd = s.cmd();
    let target = s.resolve_ref(&a.frame)?;
    edit(s, |tx, _| frame::rename(tx, &target, a.name.as_deref()))
        .map_err(|e| e.refusal(cmd, |e| name_refusal(cmd, e)))?;
    Ok(RenameOut { frame: target, name: a.name })
}

// ---- frame.add ----

/// Arguments of `frame.add`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddArgs {
    /// The frame that takes the elements.
    pub frame: Ref,
    /// Elements to add. Default: the selection.
    #[serde(default)]
    pub ids: Option<Vec<Ref>>,
}

fn add(s: &mut Session, a: AddArgs) -> Result<AddOut, Refusal> {
    let cmd = s.cmd();
    let target = s.resolve_ref(&a.frame)?;
    let ids = s.targets(a.ids.as_deref())?;
    let report = edit(s, |tx, _| frame::add_to_frame(tx, &target, &ids))
        .map_err(|e| e.refusal(cmd, |e| frame_refusal(cmd, e)))?;
    let skipped = report_skipped(s, report.skipped);
    Ok(AddOut { frame: target, added: report.added, skipped })
}

// ---- frame.remove ----

/// Arguments of `frame.remove`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct RemoveArgs {
    /// Elements to take out of their frames. Default: the selection, or with `frame` all its children.
    pub ids: Option<Vec<Ref>>,
    /// Take out all children of this frame. With `ids` too, only those of `ids` that are its children.
    pub frame: Option<Ref>,
}

/// Result of `frame.remove`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct RemoveOut {
    /// Elements that left their frame, bound text not listed.
    pub removed: Vec<ElementId>,
}

fn remove(s: &mut Session, a: RemoveArgs) -> Result<RemoveOut, Refusal> {
    let cmd = s.cmd();
    let nothing = || Refusal::nothing_to_do(cmd, "remove", "none of the given elements is in a frame");
    let frame_id = a.frame.as_ref().map(|r| s.resolve_ref(r)).transpose()?;
    let given: Option<Vec<ElementId>> = match (&a.ids, &frame_id) {
        (Some(refs), _) => Some(s.targets(Some(refs))?),
        (None, Some(_)) => None,
        (None, None) => Some(s.targets(None)?),
    };
    let removed = match (frame_id, given) {
        (Some(f), None) => edit(s, |tx, _| frame::remove_all_children(tx, &f))
            .map_err(|e| e.refusal(cmd, |e| frame_refusal(cmd, e)))?,
        (f, Some(ids)) => {
            let ids: Vec<ElementId> = match &f {
                Some(f) => ids
                    .into_iter()
                    .filter(|i| s.element(i).is_some_and(|e| e.frame_id.as_ref() == Some(f)))
                    .collect(),
                None => ids,
            };
            if !ids.iter().any(|i| s.element(i).is_some_and(|e| e.frame_id.is_some())) {
                return Err(nothing());
            }
            edit(s, |tx, _| frame::remove_from_frame(tx, &ids)).map_err(|e| e.refusal(cmd, |e| frame_refusal(cmd, e)))?
        }
        (None, None) => return Err(Refusal::no_target(cmd)),
    };
    if removed.is_empty() {
        return Err(nothing());
    }
    Ok(RemoveOut { removed })
}

// ---- frame.select_children ----

/// Arguments of `frame.select_children`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectChildrenArgs {
    /// The frame.
    pub frame: Ref,
}

/// Result of `frame.select_children`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct SelectChildrenOut {
    /// The frame.
    pub frame: ElementId,
    /// The selected children (bound text is represented by its container).
    pub selected: Vec<ElementId>,
}

fn select_children(s: &mut Session, a: SelectChildrenArgs) -> Result<SelectChildrenOut, Refusal> {
    let cmd = s.cmd();
    let target = s.resolve_ref(&a.frame)?;
    let children = frame::children_of(s.store(), &target).map_err(|e| frame_refusal(cmd, e))?;
    let free: Vec<ElementId> = children.into_iter().filter(|i| s.element(i).is_some_and(|e| !e.locked)).collect();
    let sel = select_ids(s.store(), &free, None);
    if sel.ids.is_empty() {
        return Err(Refusal::custom("nothing_to_do", cmd, format!("`{target}` has no children.")));
    }
    let selected: Vec<ElementId> = sel.ids.iter().cloned().collect();
    apply_selection(s, sel);
    Ok(SelectChildrenOut { frame: target, selected })
}

// ---- registration ----

const DOC: Facts = Facts {
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
};

const WRAP_KEYS: &[Binding] = &[Binding::new(&["W"], Scope::Canvas)];

pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "frame.wrap",
        "frame",
        "Wrap in frame",
        "Creates a frame around the elements with padding, makes them its children and selects the frame.",
        &Typed::new(wrap),
        DOC,
    )
    .refusals(&["nested_frame"])
    .shortcuts(WRAP_KEYS)
    .example(r#"{"ids":["a","b"],"name":"Login","padding":16}"#),
    CommandSpec::new(
        "frame.unwrap",
        "frame",
        "Unwrap frame",
        "Releases the children of a frame, deletes the frame and selects the children.",
        &Typed::new(unwrap),
        Facts { needs_selection: Selection::Soft, ..DOC },
    ),
    CommandSpec::new(
        "frame.rename",
        "frame",
        "Rename frame",
        "Sets the label of a frame; null restores the default \"Frame\".",
        &Typed::new(rename),
        Facts { needs_selection: Selection::No, ..DOC },
    )
    .example(r#"{"frame":"f1","name":"Login"}"#),
    CommandSpec::new(
        "frame.add",
        "frame",
        "Add to frame",
        "Makes the elements children of the frame, moving them out of another frame; frames and groups with frames are skipped.",
        &Typed::new(add),
        DOC,
    )
    .example(r#"{"frame":"f1","ids":["a"]}"#),
    CommandSpec::new(
        "frame.remove",
        "frame",
        "Remove from frame",
        "Takes the elements out of their frames, or all children of the given frame; the z-order stays.",
        &Typed::new(remove),
        Facts { needs_selection: Selection::Soft, ..DOC },
    ),
    CommandSpec::new(
        "frame.select_children",
        "frame",
        "Select frame contents",
        "Selects the children of a frame.",
        &Typed::new(select_children),
        Facts {
            undo: Undo::NoUndo,
            mutates: Mutates::OBSERVED,
            needs_selection: Selection::No,
            view_only: Gate::Allowed,
            ..DOC
        },
    )
    .refusals(&["nothing_to_do"])
    .example(r#"{"frame":"f1"}"#),
];

#[cfg(test)]
#[path = "frame_tests.rs"]
mod tests;
