//! `selection`: select, select at a point, select a region, select all, clear, read, navigate
//! (docs/commands.md §2.5).
//!
//! The selection lives in the tab's observed editor state (selection ids, selected groups, editing group,
//! point editor). These commands change only that state, so none of them makes an undo step; the next
//! document step records the selection from before and after it, and undo restores it. The rules (which
//! ids and groups a click, a list of ids, a marquee or select-all selects) are in `drafftink_edit`
//! (`group::selection`, `query`, `flowchart`). A handler here validates arguments, calls
//! them, writes the result into the observed state and shapes the output.

use std::collections::BTreeMap;

use drafftink_edit::flowchart;
use drafftink_edit::group::selection::{
    ClickMode, ClickOpts, RegionMode, RegionOpts, Selection as EditSelection, SelectionError, Within, click,
    in_z_order, select_all, select_region, Pick, click_target, keeps_multi_selection, pick, pick_group,
};
use drafftink_edit::layout::Direction;
use drafftink_edit::query::{ALL_FIELDS, AtOpts, QueryCtx, RefQuery, all_matches, describe_all, elements_at};
use drafftink_edit::transform::common_bounds;
use drafftink_geom::kurbo::Rect;
use drafftink_geom::select::MarqueeMode;
use drafftink_scene::{ElementId, GroupId, Kind, SceneRead, SceneStore};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cmd::element_common::target_refusal;
use crate::args;
use crate::refusal::{Refusal, echo};
use crate::session::{Ref, Session};
use crate::spec::{Binding, CommandSpec, Facts, Gate, Mutates, NoArgs, Redraw, Scope, TextGate, Typed};
use crate::state::Observed;

use super::element_common::{apply_selection, edit_selection, id_strings, locked_refusal, select_exactly};
use super::element_query::{DEFAULT_TOLERANCE, MAX_TOLERANCE, query_refusal, world_pt};
use super::view::RectArg;

/// The most ids one `selection.select` call may name.
const MAX_IDS: usize = 10_000;

/// How a selection change combines with the current selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// The new selection replaces the old one.
    #[default]
    Replace,
    /// The new elements are added.
    Add,
    /// The new elements are removed.
    Remove,
    /// The new elements are removed when they are all selected already, else added (Shift-click).
    Toggle,
}

impl Mode {
    fn click(self) -> ClickMode {
        match self {
            Mode::Replace => ClickMode::Replace,
            Mode::Add => ClickMode::Add,
            Mode::Remove => ClickMode::Remove,
            Mode::Toggle => ClickMode::Toggle,
        }
    }
}

/// How a marquee combines with the current selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RegionCombine {
    /// The marquee replaces the selection and leaves the editing group.
    #[default]
    Replace,
    /// The marquee adds to the selection (Shift).
    Add,
}

/// Which elements a marquee selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Contain {
    /// Elements the rectangle touches (the old app's behaviour).
    Overlap,
    /// Only elements that lie completely inside the rectangle.
    Contain,
}

/// A direction on the canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Dir {
    /// Towards the top.
    Up,
    /// Towards the bottom.
    Down,
    /// Towards the left.
    Left,
    /// Towards the right.
    Right,
}

impl Dir {
    fn name(self) -> &'static str {
        match self {
            Dir::Up => "up",
            Dir::Down => "down",
            Dir::Left => "left",
            Dir::Right => "right",
        }
    }

    fn edit(self) -> Direction {
        match self {
            Dir::Up => Direction::Up,
            Dir::Down => Direction::Down,
            Dir::Left => Direction::Left,
            Dir::Right => Direction::Right,
        }
    }
}

/// The selection after a selection command.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct SelectionOut {
    /// False when the selection was already as requested.
    pub changed: bool,
    /// The selected element ids, back to front (bound text is not listed: it follows its container).
    pub selected: Vec<String>,
    /// Groups selected as a whole.
    pub groups: Vec<String>,
    /// The group whose members are being edited individually.
    pub editing_group: Option<String>,
    /// Locked elements that were left out because `include_locked` was false.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped_locked: Vec<String>,
}

/// Writes `next` into the active tab and builds the output. `before` is the observed state from before.
pub(super) fn finish(s: &mut Session, before: &Observed, next: &EditSelection, skipped: &[ElementId]) -> SelectionOut {
    apply_selection(s, next);
    if !skipped.is_empty() {
        let n = skipped.len();
        let shown: Vec<String> = id_strings(skipped.iter().take(5));
        let more = if n > shown.len() { ", ..." } else { "" };
        s.warn(format!(
            "{n} locked element(s) were skipped ({}{more}). Pass include_locked=true to select them.",
            shown.join(", ")
        ));
    }
    report(s, before, skipped)
}

fn report(s: &Session, before: &Observed, skipped: &[ElementId]) -> SelectionOut {
    let cur = edit_selection(s);
    SelectionOut {
        changed: s.tab().observed != *before,
        selected: id_strings(in_z_order(s.store(), &cur).iter()),
        groups: cur.groups.iter().map(|g| g.as_str().to_owned()).collect(),
        editing_group: cur.editing_group.as_ref().map(|g| g.as_str().to_owned()),
        skipped_locked: id_strings(skipped.iter()),
    }
}

/// The refusal for an error of the edit crate's selection rules.
fn selection_refusal(cmd: &str, e: SelectionError) -> Refusal {
    match e {
        SelectionError::Target(t) => target_refusal(cmd, t),
        SelectionError::NothingToSelect => nothing_to_select(cmd),
        other => Refusal::bad_args(cmd, echo(&other.to_string())),
    }
}

fn nothing_to_select(cmd: &str) -> Refusal {
    Refusal::custom("nothing_to_do", cmd, "There are no elements to select.")
}

// ---- selection.select ----

/// Arguments of `selection.select`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct SelectArgs {
    /// Elements to select, by id or `@name`. Give exactly one of `ids`, `group` or `filter`.
    pub ids: Option<Vec<Ref>>,
    /// A group to select (all its members).
    pub group: Option<GroupId>,
    /// Select every element that matches this filter (the filter of `element.query`).
    pub filter: Option<RefQuery>,
    /// How to combine with the current selection.
    pub mode: Mode,
    /// Select the elements themselves, not their outermost group; the editing group becomes the innermost group
    /// they share.
    pub deep: bool,
    /// Also select locked elements. Without it they are skipped and listed in `skipped_locked`.
    pub include_locked: bool,
}

fn select(s: &mut Session, a: SelectArgs) -> Result<SelectionOut, Refusal> {
    let cmd = s.cmd();
    let given = usize::from(a.ids.is_some()) + usize::from(a.group.is_some()) + usize::from(a.filter.is_some());
    if given != 1 {
        return Err(Refusal::bad_args(cmd, "Give exactly one of `ids`, `group` or `filter`"));
    }
    let opts = ClickOpts { mode: a.mode.click(), deep: a.deep, include_locked: a.include_locked };
    let sel = edit_selection(s);
    let before = s.tab().observed.clone();
    let picked: Pick = if let Some(refs) = &a.ids {
        if refs.is_empty() {
            return Err(Refusal::bad_arg(cmd, "ids", "must name at least one element"));
        }
        if refs.len() > MAX_IDS {
            return Err(Refusal::limit_exceeded(cmd, refs.len(), "ids", MAX_IDS));
        }
        let targets = s.targets(Some(refs))?;
        pick(s.store(), &sel, &targets, opts).map_err(|e| selection_refusal(cmd, e))?
    } else if let Some(g) = &a.group {
        pick_group(s.store(), &sel, g, opts).map_err(|e| match e {
            SelectionError::NoGroup => Refusal::not_found(cmd, g.as_str()).with_details(json!({ "group": g.as_str() })),
            other => selection_refusal(cmd, other),
        })?
    } else if let Some(f) = &a.filter {
        let store = s.store();
        let q = f.resolve(store).map_err(|e| query_refusal(cmd, "filter", e))?;
        let ctx = QueryCtx { selected: Some(s.selection()) };
        let targets = all_matches(store, &q, &ctx).map_err(|e| query_refusal(cmd, "filter", e))?;
        if targets.is_empty() {
            return Err(Refusal::no_match(cmd, &f.describe()));
        }
        pick(store, &sel, &targets, opts).map_err(|e| selection_refusal(cmd, e))?
    } else {
        return Err(Refusal::internal(cmd, "no target form was given"));
    };
    Ok(finish(s, &before, &picked.selection, &picked.skipped_locked))
}

// ---- selection.select_at ----

/// Arguments of `selection.select_at`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectAtArgs {
    /// World point `[x, y]`.
    pub point: [f64; 2],
    /// How to combine with the current selection. A click on empty canvas only clears with `replace`.
    #[serde(default)]
    pub mode: Mode,
    /// Select the element itself, not its outermost group (Ctrl-click).
    #[serde(default)]
    pub deep: bool,
    /// Step through the units under the point, top to bottom (Alt-click).
    #[serde(default)]
    pub cycle: bool,
    /// How far from an outline still counts as a hit, in screen pixels.
    #[serde(default = "default_tolerance")]
    pub tolerance: f64,
    /// Locked elements can be hit.
    #[serde(default)]
    pub include_locked: bool,
}

fn default_tolerance() -> f64 {
    DEFAULT_TOLERANCE
}

fn select_at(s: &mut Session, a: SelectAtArgs) -> Result<SelectionOut, Refusal> {
    let cmd = s.cmd();
    let p = world_pt(cmd, "point", a.point)?;
    let tol = args::in_range(cmd, "tolerance", a.tolerance, 0.0, MAX_TOLERANCE)?;
    let at = AtOpts { tolerance: tol / s.view().zoom, all: a.cycle, deep: a.deep, include_locked: a.include_locked };
    let store = s.store();
    let hits = elements_at(store, p, &at);
    let sel = edit_selection(s);
    let next = if a.mode == Mode::Replace && !a.deep && !a.cycle && keeps_multi_selection(store, &sel, &hits, p) {
        sel.clone()
    } else {
        let target = click_target(store, &sel, &hits, p, a.deep, a.cycle);
        let opts = ClickOpts { mode: a.mode.click(), deep: a.deep, include_locked: a.include_locked };
        click(store, &sel, &target, opts).map_err(|e| selection_refusal(cmd, e))?
    };
    let before = s.tab().observed.clone();
    Ok(finish(s, &before, &next, &[]))
}

// ---- selection.select_region ----

/// Arguments of `selection.select_region`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectRegionArgs {
    /// The marquee in world units. A negative width or height is normalised.
    pub rect: RectArg,
    /// Replace the selection, or add to it.
    #[serde(default)]
    pub mode: RegionCombine,
    /// `overlap` selects what the rectangle touches, `contain` only what lies completely inside. Absent: the
    /// setting `input.box_select`.
    #[serde(default)]
    pub contain: Option<Contain>,
    /// Also select locked elements.
    #[serde(default)]
    pub include_locked: bool,
}

fn select_region_cmd(s: &mut Session, a: SelectRegionArgs) -> Result<SelectionOut, Refusal> {
    let cmd = s.cmd();
    let x = args::world(cmd, "rect.x", a.rect.x)?;
    let y = args::world(cmd, "rect.y", a.rect.y)?;
    let w = args::world(cmd, "rect.w", a.rect.w)?;
    let h = args::world(cmd, "rect.h", a.rect.h)?;
    let rect = Rect::from_points((x, y), (x + w, y + h));
    let contain = a.contain.unwrap_or_else(|| {
        match s.settings.get("input.box_select").and_then(Value::as_str) {
            Some("contain") => Contain::Contain,
            _ => Contain::Overlap,
        }
    });
    let opts = RegionOpts {
        mode: match a.mode {
            RegionCombine::Replace => RegionMode::Replace,
            RegionCombine::Add => RegionMode::Add,
        },
        contain: match contain {
            Contain::Overlap => MarqueeMode::Touch,
            Contain::Contain => MarqueeMode::Contain,
        },
        include_locked: a.include_locked,
    };
    let next = select_region(s.store(), &edit_selection(s), rect, opts);
    let before = s.tab().observed.clone();
    Ok(finish(s, &before, &next, &[]))
}

// ---- selection.all ----

/// Arguments of `selection.all`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct AllArgs {
    /// Select only the children of this frame or the members of this group (id or `@name` of the frame, or a
    /// group id).
    pub within: Option<Ref>,
}

fn has_elements(s: &Session) -> Result<(), Refusal> {
    if s.store().iter().next().is_none() { Err(nothing_to_select("selection.all")) } else { Ok(()) }
}

fn resolve_within(s: &Session, r: &Ref) -> Result<Within, Refusal> {
    let cmd = s.cmd();
    match s.resolve_ref(r) {
        Ok(id) => {
            let kind = s.element(&id).map(|e| e.tag());
            match kind {
                Some(Kind::Frame) => Ok(Within::Frame(id)),
                Some(k) => Err(Refusal::wrong_kind(cmd, k.name(), "frames and groups")),
                None => Err(Refusal::not_found(cmd, &r.0)),
            }
        }
        Err(not_found) => {
            if !r.0.starts_with('@')
                && let Ok(g) = GroupId::new(&r.0)
                && s.store().derived().is_group(&g)
            {
                return Ok(Within::Group(g));
            }
            Err(not_found)
        }
    }
}

fn all(s: &mut Session, a: AllArgs) -> Result<SelectionOut, Refusal> {
    let cmd = s.cmd();
    let within = a.within.as_ref().map(|r| resolve_within(s, r)).transpose()?;
    let next = select_all(s.store(), &edit_selection(s), within.as_ref(), false).map_err(|e| selection_refusal(cmd, e))?;
    let before = s.tab().observed.clone();
    Ok(finish(s, &before, &next, &[]))
}

// ---- selection.clear ----

fn clear(s: &mut Session, _: NoArgs) -> Result<SelectionOut, Refusal> {
    let before = s.tab().observed.clone();
    let o = &mut s.tab_mut().observed;
    o.selection.clear();
    o.selected_groups.clear();
    o.editing_group = None;
    o.close_editors();
    Ok(report(s, &before, &[]))
}

// ---- selection.get ----

/// How much `selection.get` says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Detail {
    /// Only the ids and their count.
    Ids,
    /// Ids, groups, editing group, bounds, kinds and the point editor.
    #[default]
    Summary,
    /// The summary and every field of every selected element.
    Full,
}

/// Arguments of `selection.get`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct GetArgs {
    /// How much to return.
    pub detail: Detail,
}

/// A rectangle in world units.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct BoundsOut {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
}

/// The point editor of a line or arrow.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct LinearEditorOut {
    /// The element.
    pub element: String,
    /// Selected point indexes.
    pub selected_points: Vec<usize>,
}

/// The part of `selection.get` output that `detail=ids` leaves out.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct SelectionSummary {
    /// Groups selected as a whole.
    pub groups: Vec<String>,
    /// The group whose members are being edited individually.
    pub editing_group: Option<String>,
    /// The common bounds of the selected elements (rotation included); null when nothing is selected.
    pub bounds: Option<BoundsOut>,
    /// How many selected elements there are of each kind.
    pub kinds: BTreeMap<String, usize>,
    /// The point editor, when a line or arrow is being edited point by point.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linear_editor: Option<LinearEditorOut>,
    /// The image whose crop is being edited (`image.crop_begin`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crop_editor: Option<String>,
}

/// Result of `selection.get`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct GetOut {
    /// The selected element ids, back to front.
    pub ids: Vec<String>,
    /// How many elements are selected.
    pub count: usize,
    /// Everything except ids and count; absent with `detail=ids`.
    #[serde(flatten)]
    pub summary: Option<SelectionSummary>,
    /// Every field of every selected element; only with `detail=full`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elements: Option<Vec<Value>>,
}

fn get(s: &mut Session, a: GetArgs) -> Result<GetOut, Refusal> {
    let cur = edit_selection(s);
    let store = s.store();
    let ids = in_z_order(store, &cur);
    let summary = (a.detail != Detail::Ids).then(|| {
        let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
        for e in ids.iter().filter_map(|i| store.get(i)) {
            *kinds.entry(e.tag().name().to_owned()).or_default() += 1;
        }
        SelectionSummary {
            groups: cur.groups.iter().map(|g| g.as_str().to_owned()).collect(),
            editing_group: cur.editing_group.as_ref().map(|g| g.as_str().to_owned()),
            bounds: common_bounds(store, &ids).map(|b| BoundsOut { x: b.x0, y: b.y0, w: b.width(), h: b.height() }),
            kinds,
            linear_editor: s.tab().observed.linear_editor.as_ref().map(|l| LinearEditorOut {
                element: l.element.as_str().to_owned(),
                selected_points: l.selected_points.iter().copied().collect(),
            }),
            crop_editor: s.tab().observed.crop_editor.as_ref().map(|id| id.as_str().to_owned()),
        }
    });
    let elements = (a.detail == Detail::Full).then(|| describe_all(store, &ids, &ALL_FIELDS));
    Ok(GetOut { ids: id_strings(ids.iter()), count: ids.len(), summary, elements })
}

// ---- selection.navigate ----

/// Arguments of `selection.navigate`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NavigateArgs {
    /// Which way to go from the selected element.
    pub direction: Dir,
}

fn needs_one(s: &Session) -> Result<(), Refusal> {
    match s.selection().len() {
        1 => Ok(()),
        n => Err(Refusal::needs_single("selection.navigate", "element", n)),
    }
}

fn navigate(s: &mut Session, a: NavigateArgs) -> Result<SelectionOut, Refusal> {
    let cmd = s.cmd();
    needs_one(s).map_err(|r| r.raised_by(cmd))?;
    let Some(from) = s.selection().iter().next().cloned() else {
        return Err(Refusal::needs_single(cmd, "element", 0));
    };
    let Some(to) = flowchart::navigate(s.store(), &from, a.direction.edit()) else {
        return Err(Refusal::custom(
            "nothing_to_do",
            cmd,
            format!("`{}` has no connected element to the {}.", from.as_str(), a.direction.name()),
        ));
    };
    if s.element(&to).is_some_and(|e| e.locked) {
        return Err(locked_refusal(cmd, std::slice::from_ref(&to)));
    }
    let before = s.tab().observed.clone();
    select_exactly(s, std::slice::from_ref(&to), false);
    Ok(report(s, &before, &[]))
}

// ---- registration ----

/// Selection commands: change only the observed editor state, never make an undo step.
pub(super) const SEL: Facts = Facts {
    reads_only: false,
    mutates: Mutates::OBSERVED,
    while_gesture: Gate::Refused,
    while_text_edit: TextGate::Commit,
    redraw: Redraw::Overlay,
    ..Facts::READ_ONLY
};
const SEL_PALETTE: Facts = Facts { palette: true, ..SEL };

const ALL_KEYS: &[Binding] = &[Binding::new(&["Mod+A"], Scope::Canvas)];
const NAVIGATE_KEYS: &[Binding] = &[
    Binding::new(&["Alt+Up"], Scope::Canvas).with_args(r#"{"direction":"up"}"#),
    Binding::new(&["Alt+Down"], Scope::Canvas).with_args(r#"{"direction":"down"}"#),
    Binding::new(&["Alt+Left"], Scope::Canvas).with_args(r#"{"direction":"left"}"#),
    Binding::new(&["Alt+Right"], Scope::Canvas).with_args(r#"{"direction":"right"}"#),
];

/// The commands of the `selection` family.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "selection.select",
        "selection",
        "Select",
        "Selects elements by id, group or filter and combines them with the selection by replace, add, remove or toggle.",
        &Typed::new(select),
        SEL,
    )
    .example(r#"{"ids":["a","@title"],"mode":"add"}"#),
    CommandSpec::new(
        "selection.select_at",
        "selection",
        "Select at point",
        "Selects the unit under a world point, as a click does; a click on empty canvas clears the selection.",
        &Typed::new(select_at),
        SEL,
    )
    .example(r#"{"point":[120,80],"mode":"toggle"}"#),
    CommandSpec::new(
        "selection.select_region",
        "selection",
        "Select region",
        "Selects the elements that overlap or lie inside a world rectangle, as a marquee does.",
        &Typed::new(select_region_cmd),
        SEL,
    )
    .example(r#"{"rect":{"x":0,"y":0,"w":300,"h":200},"contain":"contain"}"#),
    CommandSpec::new(
        "selection.all",
        "selection",
        "Select all",
        "Selects every unlocked element, or the members of the editing group, a frame or a group.",
        &Typed::new(all),
        SEL_PALETTE,
    )
    .available(has_elements)
    .shortcuts(ALL_KEYS),
    CommandSpec::new(
        "selection.clear",
        "selection",
        "Clear selection",
        "Clears the selection and leaves the editing group and the point editor.",
        &Typed::new(clear),
        SEL_PALETTE,
    ),
    CommandSpec::new(
        "selection.get",
        "selection",
        "Get selection",
        "Returns the selected ids, groups, editing group, common bounds and kinds, or the full fields of each element.",
        &Typed::new(get),
        Facts::READ_ONLY,
    ),
    CommandSpec::new(
        "selection.navigate",
        "selection",
        "Select connected element",
        "Selects the element that the selected one is connected to by an arrow, in a direction.",
        &Typed::new(navigate),
        SEL,
    )
    .default_targets(needs_one)
    .shortcuts(NAVIGATE_KEYS)
    .example(r#"{"direction":"right"}"#),
];

#[cfg(test)]
#[path = "selection_tests.rs"]
mod tests;
