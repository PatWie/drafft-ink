//! Press, click and double-click of the select tool.

use drafftink_canvas::{Button, Handle, PointerKind, Target};
use drafftink_scene::{ElementId, GroupId, Kind};
use serde_json::json;

use super::is_elbow;
use crate::cmd::selection::{Mode, SelectAtArgs};
use crate::interact::UiRequest;
use crate::interact::ctx::Ctx;
use crate::interact::events::Ev;

/// How `selection.select_at` should combine and look.
#[derive(Debug, Clone, Copy)]
struct Pick {
    mode: Mode,
    cycle: bool,
}

impl Pick {
    const REPLACE: Pick = Pick { mode: Mode::Replace, cycle: false };
    const TOGGLE: Pick = Pick { mode: Mode::Toggle, cycle: false };
    const CYCLE: Pick = Pick { mode: Mode::Replace, cycle: true };
}

/// One `selection.select_at` at the pointer. Ctrl or Cmd selects the element itself, not its group.
fn select_at(cx: &mut Ctx<'_>, ev: &Ev<'_>, pick: Pick) {
    let cfg = &cx.world.config;
    let tolerance = if ev.meta.kind == PointerKind::Touch { cfg.hit_tolerance_touch_px } else { cfg.hit_tolerance_px };
    let args = SelectAtArgs {
        point: [ev.at.world.x, ev.at.world.y],
        mode: pick.mode,
        deep: ev.meta.modifiers.command(),
        cycle: pick.cycle,
        tolerance,
        include_locked: false,
    };
    cx.exec_typed("selection.select_at", &args);
}

/// A press: selects what is under the pointer so that the drag that may follow moves it. Returns true when
/// the selection change waits for the release (Shift on an element that is selected already).
pub(super) fn press(cx: &mut Ctx<'_>, ev: &Ev<'_>) -> bool {
    if ev.meta.button != Button::Primary {
        return false;
    }
    let Target::Element { id, .. } = ev.target else { return false };
    let shift = ev.meta.modifiers.shift;
    if shift && cx.s.selection().contains(id) {
        return true;
    }
    select_at(cx, ev, if shift { Pick::TOGGLE } else { Pick::REPLACE });
    false
}

/// Makes sure the element a drag starts on is selected (the press normally did that already).
pub(super) fn ensure_selected(cx: &mut Ctx<'_>, ev: &Ev<'_>, id: &ElementId) {
    if !cx.s.selection().contains(id) {
        select_at(cx, ev, Pick::REPLACE);
    }
}

/// The release of a click that did not select on its press (the lasso tool over an element that was not
/// selected): selects what is under the pointer now.
pub(super) fn select_pressed(cx: &mut Ctx<'_>, ev: &Ev<'_>) {
    select_at(cx, ev, Pick::REPLACE);
}

/// A click: the selection changed on press, so only the clicks that have a meaning of their own remain.
pub(super) fn click(cx: &mut Ctx<'_>, ev: &Ev<'_>, toggle_now: bool) {
    let mods = ev.meta.modifiers;
    match ev.target {
        Target::Empty if !mods.shift => select_at(cx, ev, Pick::REPLACE),
        Target::Element { .. } if toggle_now => select_at(cx, ev, Pick::TOGGLE),
        Target::Element { .. } if mods.alt => select_at(cx, ev, Pick::CYCLE),
        // A vertex of the selected line or arrow: select that point (the point editor opens if it was off).
        Target::Handle(Handle::Point { element, index }) => {
            cx.exec("arrow.edit_points", json!({ "arrow": element.as_str(), "select_point": index }));
        }
        _ => {}
    }
}

/// A double-click: edit what is under the pointer.
pub(super) fn double_click(cx: &mut Ctx<'_>, ev: &Ev<'_>) {
    match ev.target {
        Target::Handle(handle) => on_handle(cx, handle),
        Target::Element { id, unit_group } => on_element(cx, ev, id, unit_group.as_ref()),
        Target::Empty => {
            let at = ev.at.world;
            if cx.exec("text.edit_begin", json!({ "at": [at.x, at.y] })).is_some() {
                cx.out.ui_request = Some(UiRequest::OpenTextEditor { element: None, at, width: None });
            }
        }
        Target::None => {}
    }
}

fn on_handle(cx: &mut Ctx<'_>, handle: &Handle) {
    match handle {
        Handle::Rotate => {
            cx.exec("element.rotate", json!({ "mode": "to", "angle": 0 }));
        }
        // An elbow segment handle: the pin goes away (the canvas counts segments from 0, the command from 1).
        Handle::Midpoint { element, segment } if is_elbow(cx, element) => {
            cx.exec("arrow.release_segment", json!({ "arrow": element.as_str(), "segment": segment + 1 }));
        }
        _ => {}
    }
}

fn on_element(cx: &mut Ctx<'_>, ev: &Ev<'_>, id: &ElementId, unit_group: Option<&GroupId>) {
    if let Some(group) = unit_group {
        cx.exec("group.enter", json!({ "group": group.as_str(), "select": id.as_str() }));
        return;
    }
    let Some(kind) = cx.s.element(id).map(|e| e.tag()) else { return };
    match kind {
        Kind::Text | Kind::Rectangle | Kind::Diamond | Kind::Ellipse | Kind::Arrow | Kind::Math => {
            if cx.exec("text.edit_begin", json!({ "id": id.as_str() })).is_some() {
                cx.out.ui_request = Some(if kind == Kind::Math {
                    UiRequest::OpenEquationEditor { element: id.clone() }
                } else {
                    UiRequest::OpenTextEditor { element: Some(id.clone()), at: ev.at.world, width: None }
                });
            }
        }
        Kind::Line => {
            cx.exec("arrow.edit_points", json!({ "arrow": id.as_str() }));
        }
        Kind::Freehand | Kind::Image | Kind::Frame => {}
    }
}
