//! Tests of the `tool` family. Pattern per command: availability, refusal text, effect, a negative case.
//! The Esc ladder needs commands of other families (a gesture, `text.edit_end`, `group.exit`,
//! `selection.clear`); a private registry gives it small stand-ins with the same contract.

use std::sync::Arc;

use drafftink_scene::{Color, Element, ElementKind, GroupId, SceneRead, SceneTx, StoreError, Style, Transform};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ports::fake::Fakes;
use crate::refusal::Refusal;
use crate::registry::Registry;
use crate::ring::Origin;
use crate::session::Session;
use crate::spec::{Batch, CommandSpec, Facts, Gate, Mutates, NoArgs, Redraw, RingMode, Selection, TextGate, Typed, Undo};
use crate::state::{LinearEditor, TextEdit, Tool};
use crate::testing;

fn run(s: &mut Session, id: &str, args: Value) -> Value {
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => v,
        Err(r) => panic!("{id} refused: {} ({})", r.message, r.code),
    }
}

fn refuse(s: &mut Session, id: &str, args: Value) -> Refusal {
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => panic!("{id} should have refused, got {v}"),
        Err(r) => r,
    }
}

// ---- stand-ins for the commands of other families ----

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MoveArgs {
    dx: f64,
}

/// Moves the first rectangle; a gesture command, as `element.move` will be.
fn move_first(s: &mut Session, a: MoveArgs) -> Result<Value, Refusal> {
    let cmd = s.cmd();
    let Some(id) = s.store().iter().next().map(|e| e.id.clone()) else {
        return Err(Refusal::no_target(cmd));
    };
    s.tab_mut()
        .doc
        .edit::<(), StoreError>(|tx| tx.update(&id, |e| e.transform.x = a.dx))
        .map_err(|e| Refusal::internal(cmd, e.to_string()))?;
    Ok(json!({}))
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
struct EndArgs {
    commit: bool,
}

impl Default for EndArgs {
    fn default() -> Self {
        Self { commit: true }
    }
}

/// What `text.edit_end` does for the Esc ladder: a committed edit adds an element and ends the edit.
fn end_text(s: &mut Session, a: EndArgs) -> Result<Value, Refusal> {
    let cmd = s.cmd();
    if a.commit {
        let id = s.ports.ids.element_id();
        s.tab_mut()
            .doc
            .edit::<(), StoreError>(|tx| {
                let z = tx.next_top_index().map_err(|e| StoreError::backend(e))?;
                let mut e = Element::new(id, ElementKind::Rectangle, z);
                e.transform = Transform::new(500.0, 500.0, 10.0, 10.0);
                tx.insert(e)
            })
            .map_err(|e| Refusal::internal(cmd, e.to_string()))?;
    }
    s.text_edit = None;
    Ok(json!({}))
}

#[derive(Default)]
struct Calls(Vec<&'static str>);

fn group_exit(s: &mut Session, _: NoArgs) -> Result<Value, Refusal> {
    s.ext.with::<Calls, _>(|c| c.0.push("group.exit"));
    let g = s.tab_mut().observed.editing_group.take();
    if let Some(g) = g {
        s.tab_mut().observed.selected_groups.insert(g);
    }
    Ok(json!({}))
}

fn selection_clear(s: &mut Session, _: NoArgs) -> Result<Value, Refusal> {
    s.ext.with::<Calls, _>(|c| c.0.push("selection.clear"));
    let o = &mut s.tab_mut().observed;
    o.selection.clear();
    o.selected_groups.clear();
    o.editing_group = None;
    o.linear_editor = None;
    Ok(json!({}))
}

const DOC: Facts = Facts {
    undo: Undo::Undoable,
    reads_only: false,
    mutates: Mutates::DOC.with(Mutates::OBSERVED),
    needs_selection: Selection::No,
    ports: crate::spec::PortSet::NONE,
    batch: Batch::Allowed,
    view_only: Gate::Refused,
    while_gesture: Gate::Refused,
    while_text_edit: TextGate::Commit,
    redraw: Redraw::Scene,
    ring: RingMode::Always,
    palette: false,
    agent: true,
};

const OTHERS: &[CommandSpec] = &[
    CommandSpec::new("t.move", "t", "Move", "Moves the first element.", &Typed::new(move_first), Facts { undo: Undo::Gesture, ..DOC }),
    CommandSpec::new(
        "text.edit_end",
        "text",
        "Finish editing text",
        "Ends the text edit.",
        &Typed::new(end_text),
        Facts { while_text_edit: TextGate::Allowed, ..DOC },
    ),
    CommandSpec::new(
        "group.exit",
        "group",
        "Leave group",
        "Leaves the editing group.",
        &Typed::new(group_exit),
        Facts { undo: Undo::NoUndo, mutates: Mutates::OBSERVED, view_only: Gate::Allowed, ..DOC },
    ),
    CommandSpec::new(
        "selection.clear",
        "selection",
        "Clear selection",
        "Clears the selection.",
        &Typed::new(selection_clear),
        Facts { undo: Undo::NoUndo, mutates: Mutates::OBSERVED, view_only: Gate::Allowed, ..DOC },
    ),
];

/// The real tool family beside the stand-ins and the real gesture commands.
fn full() -> (Session, Fakes) {
    let fakes = Fakes::new();
    let reg = Arc::new(Registry::from_families(&[super::COMMANDS, crate::cmd::gesture::COMMANDS, OTHERS]));
    (Session::with_registry(fakes.ports(), reg), fakes)
}

fn group(s: &str) -> GroupId {
    GroupId::new(s).unwrap()
}

/// A session where every rung of the ladder applies except the gesture and the text edit: a selected
/// rectangle, an editing group with a selected group, the point editor open, a drawing tool with lock.
fn busy(s: &mut Session) {
    let a = testing::add_rect(s, 0.0, 0.0, 10.0, 10.0).unwrap();
    testing::select(s, std::slice::from_ref(&a));
    let o = &mut s.tab_mut().observed;
    o.editing_group = Some(group("g1"));
    o.linear_editor = Some(LinearEditor { element: a, selected_points: Default::default() });
    s.tool.active = Tool::Rectangle;
    s.tool.lock = true;
}

// ---- availability ----

#[test]
fn tool_commands_are_available_in_every_mode_except_that_a_drag_blocks_most() {
    let mut s = testing::headless_session();
    s.view_only = true;
    for id in ["tool.set", "tool.lock", "tool.hold", "tool.release", "tool.cancel"] {
        assert!(s.availability(id).is_ok(), "{id} in view-only mode");
    }
    s.view_only = false;
    s.text_edit = Some(TextEdit { element: None, draft: true });
    for id in ["tool.set", "tool.lock", "tool.hold", "tool.release", "tool.cancel"] {
        assert!(s.availability(id).is_ok(), "{id} during a text edit");
    }
}

#[test]
fn a_drag_blocks_tool_set_lock_and_hold_but_not_release_or_cancel() {
    let (mut s, _f) = full();
    testing::add_rect(&mut s, 0.0, 0.0, 10.0, 10.0).unwrap();
    run(&mut s, "gesture.begin", json!({"command": "t.move", "args": {"dx": 5}}));
    for id in ["tool.set", "tool.lock", "tool.hold"] {
        let a = s.availability(id).unwrap_err();
        assert_eq!(a.code, "gesture_active", "{id}");
        let before = s.fingerprint();
        let args = if id == "tool.lock" { json!({}) } else { json!({"tool": "hand"}) };
        let e = refuse(&mut s, id, args);
        assert_eq!((e.code, e.message.as_str()), (a.code, a.message.as_str()), "{id}");
        assert_eq!(s.fingerprint(), before, "{id}");
    }
    assert!(s.availability("tool.release").is_ok());
    assert!(s.availability("tool.cancel").is_ok());
}

// ---- tool.set ----

#[test]
fn set_switches_tool_and_is_not_undoable() {
    let mut s = testing::headless_session();
    testing::add_rect(&mut s, 0.0, 0.0, 10.0, 10.0).unwrap();
    let rev = s.tab().doc.revision();
    for t in [
        "select", "hand", "rectangle", "diamond", "ellipse", "arrow", "line", "freehand", "highlighter", "text", "math",
        "frame", "eraser", "laser",
    ] {
        let out = run(&mut s, "tool.set", json!({"tool": t}));
        assert_eq!(out["tool"], json!(t));
        assert_eq!(s.active_tool().name(), t);
    }
    assert_eq!(s.tab().doc.undo_depth(), 0);
    assert_eq!(s.tab().doc.revision(), rev, "the document is not touched");
}

#[test]
fn drawing_tools_clear_the_selection_and_select_and_hand_keep_it() {
    let mut s = testing::headless_session();
    let a = testing::add_rect(&mut s, 0.0, 0.0, 10.0, 10.0).unwrap();
    testing::select(&mut s, std::slice::from_ref(&a));
    s.tab_mut().observed.editing_group = Some(group("g1"));
    run(&mut s, "tool.set", json!({"tool": "hand"}));
    assert_eq!(s.selection().len(), 1, "hand keeps the selection");
    run(&mut s, "tool.set", json!({"tool": "select"}));
    assert_eq!(s.selection().len(), 1);
    assert!(s.tab().observed.editing_group.is_some());
    run(&mut s, "tool.set", json!({"tool": "rectangle"}));
    assert!(s.selection().is_empty(), "decision §9 item 4: a drawing tool clears the selection");
    assert!(s.tab().observed.editing_group.is_none() && s.tab().observed.selected_groups.is_empty());
}

#[test]
fn keep_selection_overrides_the_default_either_way() {
    let mut s = testing::headless_session();
    let a = testing::add_rect(&mut s, 0.0, 0.0, 10.0, 10.0).unwrap();
    testing::select(&mut s, std::slice::from_ref(&a));
    run(&mut s, "tool.set", json!({"tool": "rectangle", "keep_selection": true}));
    assert_eq!(s.selection().len(), 1);
    run(&mut s, "tool.set", json!({"tool": "hand", "keep_selection": false}));
    assert!(s.selection().is_empty());
}

#[test]
fn set_leaves_the_point_editor() {
    let mut s = testing::headless_session();
    busy(&mut s);
    run(&mut s, "tool.set", json!({"tool": "select"}));
    assert!(s.tab().observed.linear_editor.is_none());
    assert_eq!(s.selection().len(), 1, "select keeps the selection");
}

#[test]
fn setting_the_active_tool_again_changes_nothing() {
    let mut s = testing::headless_session();
    run(&mut s, "tool.set", json!({"tool": "ellipse"}));
    s.take_dirty();
    let out = run(&mut s, "tool.set", json!({"tool": "ellipse"}));
    assert_eq!(out["changed"], json!(false));
    assert_eq!(s.take_dirty(), None, "no redraw for a no-op");
}

#[test]
fn set_requests_an_overlay_redraw() {
    let mut s = testing::headless_session();
    s.take_dirty();
    run(&mut s, "tool.set", json!({"tool": "line"}));
    assert_eq!(s.take_dirty(), Some(Redraw::Overlay));
}

#[test]
fn in_view_only_mode_only_select_hand_and_laser_are_available() {
    let mut s = testing::headless_session();
    s.view_only = true;
    for t in ["select", "hand", "laser"] {
        run(&mut s, "tool.set", json!({"tool": t}));
    }
    for t in ["rectangle", "diamond", "ellipse", "arrow", "line", "freehand", "highlighter", "text", "math", "frame", "eraser"] {
        let before = s.fingerprint();
        let r = refuse(&mut s, "tool.set", json!({"tool": t}));
        assert_eq!(r.code, "view_only_tool", "{t}");
        assert_eq!(
            r.message,
            "In view-only mode only the select, hand and laser tools are available. Turn view-only off with `view.set_view_only` (on=false)."
        );
        assert_eq!(r.suggest.first().map(|x| x.command.as_str()), Some("view.set_view_only"));
        assert_eq!(s.fingerprint(), before);
    }
}

#[test]
fn set_rejects_an_unknown_tool_name() {
    let mut s = testing::headless_session();
    let r = refuse(&mut s, "tool.set", json!({"tool": "pencil"}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("`tool`"), "{}", r.message);
    assert_eq!(refuse(&mut s, "tool.set", json!({})).code, "bad_args");
}

#[test]
fn set_ends_a_running_text_edit_and_keeps_the_text_as_its_own_undo_step() {
    let (mut s, _f) = full();
    s.text_edit = Some(TextEdit { element: None, draft: true });
    run(&mut s, "tool.set", json!({"tool": "rectangle"}));
    assert!(s.text_edit.is_none());
    assert_eq!(s.tab().doc.undo_depth(), 1, "the text edit was committed as one step");
    assert_eq!(s.active_tool(), Tool::Rectangle);
    // Without the text family the state is still ended.
    let mut h = testing::headless_session();
    h.text_edit = Some(TextEdit { element: None, draft: true });
    run(&mut h, "tool.set", json!({"tool": "text"}));
    assert!(h.text_edit.is_none());
}

#[test]
fn the_keys_of_the_tool_bindings_are_the_ones_of_section_6_1() {
    let s = testing::headless_session();
    let spec = s.spec("tool.set").unwrap();
    let mut seen: Vec<(String, Vec<&str>)> = Vec::new();
    for b in spec.shortcuts {
        let args: Value = serde_json::from_str(b.args).unwrap();
        seen.push((args["tool"].as_str().unwrap().to_owned(), b.keys.to_vec()));
        assert!(b.title.is_some(), "every tool binding has a palette title");
    }
    let expect: [(&str, &[&str]); 13] = [
        ("select", &["V", "1"]),
        ("hand", &["H"]),
        ("rectangle", &["R", "2"]),
        ("diamond", &["D", "3"]),
        ("ellipse", &["O", "4"]),
        ("arrow", &["A", "5"]),
        ("line", &["L", "6"]),
        ("freehand", &["P", "7", "X"]),
        ("text", &["T", "8"]),
        ("math", &["M", "9"]),
        ("frame", &["F"]),
        ("eraser", &["E", "0"]),
        ("laser", &["Z", "K"]),
    ];
    assert_eq!(seen.len(), expect.len(), "the highlighter and the lasso have no key");
    for (tool, keys) in expect {
        assert!(seen.iter().any(|(t, k)| t == tool && k == keys), "{tool}: {keys:?}");
    }
}

// ---- style defaults for new elements ----

#[test]
fn new_elements_start_from_the_old_defaults() {
    let s = testing::headless_session();
    let st = &s.tool.style_defaults;
    assert_eq!(st.stroke_color, Color::rgb(0x63, 0x66, 0xf1), "indigo-500");
    assert_eq!((st.stroke_width, st.opacity, st.corner_radius), (2.0, 1.0, 0.0));
    assert_eq!(st.fill_color, None);
    assert_eq!(*st, Style::default());
}

// ---- tool.lock ----

#[test]
fn lock_toggles_or_sets_and_reports_the_state() {
    let mut s = testing::headless_session();
    assert_eq!(run(&mut s, "tool.lock", json!({}))["tool_lock"], json!(true));
    assert_eq!(run(&mut s, "tool.lock", json!({}))["tool_lock"], json!(false));
    let out = run(&mut s, "tool.lock", json!({"on": true}));
    assert_eq!((out["changed"].clone(), out["tool_lock"].clone()), (json!(true), json!(true)));
    let out = run(&mut s, "tool.lock", json!({"on": true}));
    assert_eq!(out["changed"], json!(false));
    s.view_only = true;
    run(&mut s, "tool.lock", json!({"on": false}));
    assert!(!s.tool.lock);
    assert_eq!(s.tab().doc.undo_depth(), 0);
}

#[test]
fn lock_is_on_q_and_a_text_edit_does_not_block_it() {
    let mut s = testing::headless_session();
    let spec = s.spec("tool.lock").unwrap();
    assert_eq!(spec.shortcuts[0].keys, &["Q"]);
    s.text_edit = Some(TextEdit { element: None, draft: true });
    run(&mut s, "tool.lock", json!({"on": true}));
    assert!(s.text_edit.is_some(), "the lock does not end the text edit");
}

#[test]
fn lock_rejects_a_non_boolean() {
    let mut s = testing::headless_session();
    let before = s.fingerprint();
    assert_eq!(refuse(&mut s, "tool.lock", json!({"on": "yes"})).code, "bad_args");
    assert_eq!(s.fingerprint(), before);
}

// ---- tool.hold and tool.release ----

#[test]
fn hold_activates_the_hand_until_release_and_remembers_the_active_tool() {
    let mut s = testing::headless_session();
    run(&mut s, "tool.set", json!({"tool": "rectangle"}));
    let out = run(&mut s, "tool.hold", json!({}));
    assert_eq!((out["tool"].clone(), out["active"].clone(), out["held"].clone()), (json!("hand"), json!("rectangle"), json!("hand")));
    assert_eq!(s.active_tool(), Tool::Hand);
    let out = run(&mut s, "tool.release", json!({}));
    assert_eq!(out["changed"], json!(true));
    assert_eq!(s.active_tool(), Tool::Rectangle);
    assert_eq!(s.tool.temporary, None);
}

#[test]
fn key_repeat_and_a_release_without_a_hold_are_not_errors() {
    let mut s = testing::headless_session();
    let out = run(&mut s, "tool.release", json!({}));
    assert_eq!(out["changed"], json!(false));
    assert_eq!(run(&mut s, "tool.hold", json!({}))["changed"], json!(true));
    let ring_before = s.ring().len();
    assert_eq!(run(&mut s, "tool.hold", json!({}))["changed"], json!(false), "key repeat");
    assert!(s.ring().len() > ring_before);
}

#[test]
fn a_hold_of_another_tool_replaces_the_held_one_and_tool_set_changes_what_returns() {
    let mut s = testing::headless_session();
    run(&mut s, "tool.hold", json!({"tool": "hand"}));
    run(&mut s, "tool.hold", json!({"tool": "laser"}));
    assert_eq!(s.active_tool(), Tool::Laser);
    run(&mut s, "tool.set", json!({"tool": "ellipse"}));
    assert_eq!(s.active_tool(), Tool::Laser, "still held");
    run(&mut s, "tool.release", json!({}));
    assert_eq!(s.active_tool(), Tool::Ellipse);
}

#[test]
fn hold_follows_the_view_only_tool_rule() {
    let mut s = testing::headless_session();
    s.view_only = true;
    run(&mut s, "tool.hold", json!({}));
    run(&mut s, "tool.release", json!({}));
    let before = s.fingerprint();
    let r = refuse(&mut s, "tool.hold", json!({"tool": "eraser"}));
    assert_eq!(r.code, "view_only_tool");
    assert_eq!(s.fingerprint(), before);
}

#[test]
fn release_works_during_a_drag_and_space_is_the_hold_key() {
    let (mut s, _f) = full();
    testing::add_rect(&mut s, 0.0, 0.0, 10.0, 10.0).unwrap();
    run(&mut s, "tool.hold", json!({}));
    run(&mut s, "gesture.begin", json!({"command": "t.move", "args": {"dx": 5}}));
    run(&mut s, "tool.release", json!({}));
    assert!(s.gesture_running(), "release does not end the drag");
    let spec = s.spec("tool.hold").unwrap();
    assert_eq!(spec.shortcuts[0].keys, &["Space"]);
    assert_eq!(serde_json::from_str::<Value>(spec.shortcuts[0].args).unwrap(), json!({"tool": "hand"}));
    assert!(!spec.agent && !s.spec("tool.release").unwrap().agent, "plumbing for the shell");
}

// ---- tool.cancel: the Esc ladder ----

#[test]
fn esc_peels_one_rung_at_a_time_by_default() {
    let (mut s, _f) = full();
    busy(&mut s);
    let mut rungs = Vec::new();
    loop {
        let out = run(&mut s, "tool.cancel", json!({}));
        let r: Vec<String> = serde_json::from_value(out["rungs"].clone()).unwrap();
        if r.is_empty() {
            assert_eq!(out["changed"], json!(false));
            break;
        }
        assert_eq!(r.len(), 1, "one rung per press: {r:?}");
        rungs.extend(r);
    }
    assert_eq!(rungs, ["point_editor", "group", "selection", "tool"]);
    assert_eq!(s.active_tool(), Tool::Select);
    assert!(!s.tool.lock, "the last rung turns the lock off");
    assert!(s.selection().is_empty());
    assert!(s.tab().observed.editing_group.is_none());
}

#[test]
fn esc_with_all_runs_every_rung_at_once() {
    let (mut s, _f) = full();
    busy(&mut s);
    let out = run(&mut s, "tool.cancel", json!({"all": true}));
    assert_eq!(out["rungs"], json!(["point_editor", "group", "selection", "tool"]));
    assert_eq!(out["changed"], json!(true));
    assert_eq!(s.active_tool(), Tool::Select);
    assert!(s.selection().is_empty() && s.tab().observed.selected_groups.is_empty());
    assert_eq!(run(&mut s, "tool.cancel", json!({"all": true}))["changed"], json!(false));
}

#[test]
fn the_group_and_selection_rungs_run_the_commands_of_those_families_when_registered() {
    let (mut s, _f) = full();
    busy(&mut s);
    run(&mut s, "tool.cancel", json!({"all": true}));
    assert_eq!(s.ext.with::<Calls, _>(|c| c.0.clone()), ["group.exit", "selection.clear"]);
    // group.exit selects the group again, which is why the selection rung still has work.
    let (mut s, _f) = full();
    busy(&mut s);
    run(&mut s, "tool.cancel", json!({}));
    run(&mut s, "tool.cancel", json!({}));
    assert_eq!(s.tab().observed.selected_groups.len(), 1);
}

#[test]
fn a_stray_esc_is_ok_with_nothing_changed_and_makes_no_redraw() {
    let mut s = testing::headless_session();
    s.take_dirty();
    let out = run(&mut s, "tool.cancel", json!({}));
    assert_eq!((out["changed"].clone(), out["rungs"].clone()), (json!(false), json!([])));
    assert_eq!(s.take_dirty(), None);
    assert_eq!(s.tab().doc.undo_depth(), 0);
}

#[test]
fn esc_cancels_a_running_drag_first_and_restores_the_document() {
    let (mut s, _f) = full();
    let a = testing::add_rect(&mut s, 0.0, 0.0, 10.0, 10.0).unwrap();
    testing::select(&mut s, std::slice::from_ref(&a));
    run(&mut s, "gesture.begin", json!({"command": "t.move", "args": {"dx": 50}}));
    assert_eq!(s.element(&a).unwrap().transform.x, 50.0);
    let out = run(&mut s, "tool.cancel", json!({}));
    assert_eq!(out["rungs"], json!(["gesture"]));
    assert!(!s.gesture_running());
    assert_eq!(s.element(&a).unwrap().transform.x, 0.0);
    assert_eq!(s.selection().len(), 1, "one press, one rung: the selection stays");
    assert_eq!(s.tab().doc.undo_depth(), 0, "a cancelled drag leaves no undo step");
    // The ring has the cancelled gesture and this call.
    let ids: Vec<String> = s.ring().iter().map(|e| e.id.clone()).collect();
    assert_eq!(&ids[ids.len() - 2..], ["gesture.cancel", "tool.cancel"]);
    // A second press peels the next rung.
    assert_eq!(run(&mut s, "tool.cancel", json!({}))["rungs"], json!(["selection"]));
}

#[test]
fn esc_with_all_cancels_the_drag_and_goes_on() {
    let (mut s, _f) = full();
    let a = testing::add_rect(&mut s, 0.0, 0.0, 10.0, 10.0).unwrap();
    testing::select(&mut s, std::slice::from_ref(&a));
    s.tool.active = Tool::Hand;
    run(&mut s, "gesture.begin", json!({"command": "t.move", "args": {"dx": 50}}));
    let out = run(&mut s, "tool.cancel", json!({"all": true}));
    assert_eq!(out["rungs"], json!(["gesture", "selection", "tool"]));
    assert_eq!(s.element(&a).unwrap().transform.x, 0.0);
}

#[test]
fn esc_ends_a_text_edit_and_keeps_it_as_one_undo_step() {
    let (mut s, _f) = full();
    testing::add_rect(&mut s, 0.0, 0.0, 10.0, 10.0).unwrap();
    s.text_edit = Some(TextEdit { element: None, draft: true });
    let out = run(&mut s, "tool.cancel", json!({}));
    assert_eq!(out["rungs"], json!(["text_edit"]));
    assert!(s.text_edit.is_none());
    assert_eq!(s.store().iter().count(), 2, "the committed text exists");
    assert_eq!(s.tab().doc.undo_depth(), 1);
    assert_eq!(out["undo_label"], json!("Cancel"));
    s.undo_steps(1).unwrap();
    assert_eq!(s.store().iter().filter(|e| !e.deleted).count(), 1, "one undo takes the text edit back");
}

#[test]
fn esc_without_a_text_family_still_ends_the_edit_state() {
    let mut s = testing::headless_session();
    s.text_edit = Some(TextEdit { element: None, draft: true });
    let out = run(&mut s, "tool.cancel", json!({}));
    assert_eq!(out["rungs"], json!(["text_edit"]));
    assert!(s.text_edit.is_none());
}

#[test]
fn esc_is_the_always_scope_key_and_works_in_view_only_mode() {
    let mut s = testing::headless_session();
    let spec = s.spec("tool.cancel").unwrap();
    assert_eq!(spec.shortcuts[0].keys, &["Esc"]);
    assert_eq!(spec.shortcuts[0].when, crate::spec::Scope::Always);
    busy(&mut s);
    s.view_only = true;
    let out = run(&mut s, "tool.cancel", json!({"all": true}));
    assert_eq!(out["changed"], json!(true));
    assert!(s.view_only, "Esc does not leave view-only mode");
}

#[test]
fn a_ladder_without_a_document_change_pushes_no_undo_step() {
    let mut s = testing::headless_session();
    busy(&mut s);
    let out = run(&mut s, "tool.cancel", json!({"all": true}));
    assert!(out.get("undo_label").is_none(), "{out}");
    assert_eq!(s.tab().doc.undo_depth(), 0);
}

#[test]
fn cancel_rejects_a_non_boolean_all() {
    let mut s = testing::headless_session();
    busy(&mut s);
    let before = s.fingerprint();
    assert_eq!(refuse(&mut s, "tool.cancel", json!({"all": "yes"})).code, "bad_args");
    assert_eq!(s.fingerprint(), before);
}

#[test]
fn a_failing_text_commit_leaves_the_session_as_it_was() {
    // The text family refuses (here: a stand-in that always does). The ladder stops, rolls back, and the
    // state is exactly as before.
    fn refusing(s: &mut Session, _: EndArgs) -> Result<Value, Refusal> {
        s.tab_mut().observed.selection.clear();
        Err(Refusal::custom("invalid_latex", s.cmd(), "The formula could not be read."))
    }
    const BAD: &[CommandSpec] = &[CommandSpec::new(
        "text.edit_end",
        "text",
        "Finish editing text",
        "Always refuses.",
        &Typed::new(refusing),
        Facts { while_text_edit: TextGate::Allowed, ..DOC },
    )];
    let fakes = Fakes::new();
    let reg = Arc::new(Registry::from_families(&[super::COMMANDS, BAD]));
    let mut s = Session::with_registry(fakes.ports(), reg);
    busy(&mut s);
    s.text_edit = Some(TextEdit { element: None, draft: true });
    let before = s.fingerprint();
    let r = refuse(&mut s, "tool.cancel", json!({}));
    assert_eq!(r.code, "invalid_latex");
    assert_eq!(s.fingerprint(), before);
    assert!(s.text_edit.is_some());
}
