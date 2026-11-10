//! `group`: group, ungroup, name, enter and leave (docs/commands.md §2.10).
//!
//! A group is a shared id in the `group_ids` of its members (`drafftink_edit::group`). The rules (who is
//! grouped with whom, z-order of the block, frames, bound text, stale ids) live in that module; a command
//! here resolves its targets, maps each typed error to the refusal of the catalog and updates the selection.
//! The old app had group and ungroup on the keyboard only; here every one is a command with a shortcut.

use drafftink_edit::group::selection::{SelectionError, enter_group, exit_group, select_ids};
use drafftink_edit::group::units::Relations;
use drafftink_edit::group::{self, GroupError};
use drafftink_scene::{ElementId, GroupId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::refusal::Refusal;
use crate::session::{Ref, Session};
use crate::spec::{
    Batch, Binding, CommandSpec, Facts, Gate, Mutates, NoArgs, PortSet, Redraw, RingMode, Scope, Selection, TextGate,
    Typed, Undo,
};

pub(crate) mod common;
#[cfg(test)]
pub(crate) mod testkit;

use common::{
    apply_selection, current_selection, edit, editing_group, fresh_group_id, locked, no_group, not_found,
    selected_ids, zorder_refusal,
};

// ---- errors ----

fn group_refusal(cmd: &str, e: GroupError) -> Refusal {
    match e {
        GroupError::NotFound(id) => not_found(cmd, &id),
        GroupError::Locked(ids) => locked(cmd, &ids),
        GroupError::TooFew { have } => Refusal::custom(
            "too_few",
            cmd,
            format!("Grouping needs at least 2 elements, and {have} were given. Select more, or pass `ids`."),
        ),
        GroupError::AlreadyGrouped => Refusal::custom(
            "already_grouped",
            cmd,
            "These elements already form one group at this level. Ungroup them with `group.ungroup`, or select more elements.",
        )
        .suggesting("group.ungroup", None),
        GroupError::FrameAndChildren { .. } => Refusal::custom(
            "frame_and_children",
            cmd,
            "A frame and its own children cannot be grouped together. Select either the frame or its children.",
        ),
        GroupError::NoGroup => no_group(cmd),
        GroupError::TooDeep { max, .. } => Refusal::limit_exceeded(cmd, max + 1, "nested group levels", max),
        GroupError::NotInEditingGroup(id) => Refusal::bad_arg(
            cmd,
            "ids",
            format!(
                "element `{id}` is not inside the group being edited. Leave the group with `group.exit`, or pass members of that group"
            ),
        )
        .suggesting("group.exit", None),
        GroupError::BadName => bad_name(cmd),
        GroupError::ZOrder(z) => zorder_refusal(cmd, z),
        other @ (GroupError::GroupIdInUse(_) | GroupError::Store(_)) => Refusal::internal(cmd, other.to_string()),
    }
}

fn bad_name(cmd: &str) -> Refusal {
    Refusal::custom(
        "bad_name",
        cmd,
        format!(
            "A group name must be 1 to {} characters and must not contain control characters.",
            group::MAX_GROUP_NAME_CHARS
        ),
    )
}

fn selection_refusal(cmd: &str, e: SelectionError) -> Refusal {
    match e {
        SelectionError::NotFound(id) => not_found(cmd, &id),
        SelectionError::Locked(ids) => locked(cmd, &ids),
        SelectionError::NoGroup => no_group(cmd),
        SelectionError::NotEditing => not_in_group(cmd),
        SelectionError::NotInGroup(id) => Refusal::bad_arg(
            cmd,
            "select",
            format!("element `{id}` is not a member of that group. Pick a member, or omit `select`"),
        ),
        SelectionError::NothingToSelect => Refusal::internal(cmd, e.to_string()),
    }
}

fn not_in_group(cmd: &str) -> Refusal {
    Refusal::custom("not_in_group", cmd, "You are not inside a group, so there is nothing to leave.")
}

// ---- group.create ----

/// Arguments of `group.create`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct CreateArgs {
    /// Elements to group (a group among them counts as one). Default: the selection.
    pub ids: Option<Vec<Ref>>,
}

/// Result of `group.create`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct CreateOut {
    /// The new group id.
    pub group: GroupId,
    /// Its members in z-order (bound text not listed).
    pub members: Vec<ElementId>,
    /// Members that were taken out of their frames because they came from different frames.
    pub left_frames: Vec<ElementId>,
}

fn create(s: &mut Session, a: CreateArgs) -> Result<CreateOut, Refusal> {
    let cmd = s.cmd();
    let ids = s.targets(a.ids.as_deref())?;
    let editing = editing_group(s);
    let new_group = fresh_group_id(s).ok_or_else(|| Refusal::internal(cmd, "could not make a fresh group id"))?;
    let report = edit(s, |tx, _| group::group(tx, &ids, editing.as_ref(), new_group))
        .map_err(|e| e.refusal(cmd, |g| group_refusal(cmd, g)))?;
    let sel = select_ids(s.store(), &report.members, editing.as_ref());
    apply_selection(s, sel);
    Ok(CreateOut { group: report.group, members: report.members, left_frames: report.left_frames })
}

/// `too_few` and `already_grouped` for the current selection, so that a button shows what `execute` says.
fn create_default_targets(s: &Session) -> Result<(), Refusal> {
    let cmd = "group.create";
    drafftink_edit::preflight::check_group(s.store(), &selected_ids(s), editing_group(s).as_ref())
        .map_err(|e| group_refusal(cmd, e))
}

// ---- group.ungroup ----

/// Arguments of `group.ungroup`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct UngroupArgs {
    /// The group to dissolve. Default: the groups among `ids`.
    pub group: Option<GroupId>,
    /// Elements whose group (at the current level) is dissolved. Default: the selection.
    pub ids: Option<Vec<Ref>>,
}

/// Result of `group.ungroup`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct UngroupOut {
    /// The groups that were dissolved.
    pub removed: Vec<GroupId>,
    /// Their members in z-order (bound text not listed). They are selected afterwards.
    pub members: Vec<ElementId>,
}

fn ungroup(s: &mut Session, a: UngroupArgs) -> Result<UngroupOut, Refusal> {
    let cmd = s.cmd();
    let ids: Vec<ElementId> = match &a.ids {
        Some(refs) => s.targets(Some(refs))?,
        None => selected_ids(s),
    };
    if ids.is_empty() && a.group.is_none() {
        return Err(no_group(cmd));
    }
    let editing = editing_group(s);
    let report = edit(s, |tx, _| group::ungroup(tx, &ids, a.group.as_ref(), editing.as_ref()))
        .map_err(|e| e.refusal(cmd, |g| group_refusal(cmd, g)))?;
    let sel = select_ids(s.store(), &report.members, editing.as_ref());
    apply_selection(s, sel);
    Ok(UngroupOut { removed: report.removed, members: report.members })
}

/// `no_group` when the selection holds no group.
fn ungroup_default_targets(s: &Session) -> Result<(), Refusal> {
    let cmd = "group.ungroup";
    let ids = selected_ids(s);
    let rel = Relations::build(s.store());
    match drafftink_edit::preflight::groups_of(s.store(), &rel, &ids, editing_group(s).as_ref()) {
        Ok(g) if !g.is_empty() => Ok(()),
        _ => Err(no_group(cmd)),
    }
}

// ---- group.set_name ----

/// Arguments of `group.set_name`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct SetNameArgs {
    /// The group. Default: the single selected group.
    pub group: Option<GroupId>,
    /// The new name (1 to 64 characters), or null to clear it. `doc.outline` shows it.
    pub name: Option<String>,
}

/// Result of `group.set_name`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct SetNameOut {
    /// The group that was named.
    pub group: GroupId,
    /// The name now, `null` when cleared.
    pub name: Option<String>,
}

fn set_name(s: &mut Session, a: SetNameArgs) -> Result<SetNameOut, Refusal> {
    let cmd = s.cmd();
    let target = match a.group {
        Some(g) => g,
        None => {
            let groups = &s.tab().observed.selected_groups;
            let mut it = groups.iter();
            match (it.next(), it.next()) {
                (Some(g), None) => g.clone(),
                (Some(_), Some(_)) => return Err(Refusal::needs_single(cmd, "group", groups.len())),
                _ => return Err(no_group(cmd)),
            }
        }
    };
    edit(s, |tx, _| group::set_group_name(tx, &target, a.name.as_deref()))
        .map_err(|e| e.refusal(cmd, |g| group_refusal(cmd, g)))?;
    Ok(SetNameOut { group: target, name: a.name })
}

// ---- group.enter ----

/// Arguments of `group.enter`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct EnterArgs {
    /// The group to enter. Default: the single selected group.
    pub group: Option<GroupId>,
    /// Select this element's unit inside the group (the double-click target). Default: keep the selection.
    pub select: Option<Ref>,
}

/// Result of `group.enter`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct EnterOut {
    /// The group being edited now.
    pub editing_group: Option<GroupId>,
    /// The selected elements.
    pub selected: Vec<ElementId>,
}

fn enter(s: &mut Session, a: EnterArgs) -> Result<EnterOut, Refusal> {
    let cmd = s.cmd();
    let pick = a.select.as_ref().map(|r| s.resolve_ref(r)).transpose()?;
    let sel = enter_group(s.store(), &current_selection(s), a.group.as_ref(), pick.as_ref())
        .map_err(|e| selection_refusal(cmd, e))?;
    let out = EnterOut { editing_group: sel.editing_group.clone(), selected: sel.ids.iter().cloned().collect() };
    apply_selection(s, sel);
    Ok(out)
}

// ---- group.exit ----

fn exit(s: &mut Session, _: NoArgs) -> Result<EnterOut, Refusal> {
    let cmd = s.cmd();
    // An editing group that lost its members is not a group any more. Leaving it must still work (Esc runs this
    // command), so the stale id is dropped and the selection stays.
    if s.tab().observed.editing_group.is_some() && editing_group(s).is_none() {
        s.tab_mut().observed.editing_group = None;
        return Ok(EnterOut { editing_group: None, selected: selected_ids(s) });
    }
    let sel = exit_group(s.store(), &current_selection(s)).map_err(|e| selection_refusal(cmd, e))?;
    let out = EnterOut { editing_group: sel.editing_group.clone(), selected: sel.ids.iter().cloned().collect() };
    apply_selection(s, sel);
    Ok(out)
}

/// `not_in_group` when no group is being edited.
fn exit_available(s: &Session) -> Result<(), Refusal> {
    if s.tab().observed.editing_group.is_some() { Ok(()) } else { Err(not_in_group("group.exit")) }
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

/// Changes the editing group and the selection only.
const OBSERVED: Facts = Facts {
    undo: Undo::NoUndo,
    mutates: Mutates::OBSERVED,
    needs_selection: Selection::No,
    view_only: Gate::Allowed,
    ..DOC
};

const CREATE_KEYS: &[Binding] = &[Binding::new(&["Mod+G"], Scope::Canvas)];
const UNGROUP_KEYS: &[Binding] = &[Binding::new(&["Mod+Shift+G"], Scope::Canvas)];

pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "group.create",
        "group",
        "Group",
        "Groups the selected elements (or `ids`) into one unit; ids never change and the members become contiguous in z-order.",
        &Typed::new(create),
        DOC,
    )
    .default_targets(create_default_targets)
    .refusals(&["already_grouped", "frame_and_children"])
    .shortcuts(CREATE_KEYS)
    .example(r#"{"ids":["a","b"]}"#),
    CommandSpec::new(
        "group.ungroup",
        "group",
        "Ungroup",
        "Dissolves the selected group level (or the given `group`); members, ids and z-order stay.",
        &Typed::new(ungroup),
        Facts { needs_selection: Selection::Soft, ..DOC },
    )
    .default_targets(ungroup_default_targets)
    .refusals(&["no_group"])
    .shortcuts(UNGROUP_KEYS)
    .example(r#"{"group":"g1"}"#),
    CommandSpec::new(
        "group.set_name",
        "group",
        "Name group",
        "Sets or clears the name of a group; the name is kept with the members and shown in the document outline.",
        &Typed::new(set_name),
        Facts { mutates: Mutates::DOC, needs_selection: Selection::Soft, palette: false, ..DOC },
    )
    .refusals(&["no_group", "bad_name"])
    .example(r#"{"group":"g1","name":"Header"}"#),
    CommandSpec::new(
        "group.enter",
        "group",
        "Enter group",
        "Starts editing a group: clicks then select its members one level down; optionally selects one member.",
        &Typed::new(enter),
        Facts { while_gesture: Gate::Refused, ..OBSERVED },
    )
    .refusals(&["no_group"])
    .example(r#"{"group":"g1","select":"a"}"#),
    CommandSpec::new(
        "group.exit",
        "group",
        "Leave group",
        "Stops editing the current group and selects that group again.",
        &Typed::new(exit),
        OBSERVED,
    )
    .available(exit_available)
    .refusals(&["not_in_group"]),
];

#[cfg(test)]
#[path = "group_tests.rs"]
mod tests;
