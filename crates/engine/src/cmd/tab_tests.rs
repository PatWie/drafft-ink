//! Tests of the `tab` family and of the workspace record. No UI: a session over the fake ports.
//! Commands of other families that the tab commands call (`doc.save`, `text.edit_end`, a drag) are small
//! stand-ins in a private registry.

use std::sync::Arc;

use drafftink_scene::{Element, ElementKind, SceneRead, SceneTx, StoreError, Transform};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::workspace::{INDEX_KEY, QUIET_MS};
use super::{MAX_TABS, NEVER_SAVED, RoomBinding};
use crate::batch::{BatchOpts, BatchStep};
use crate::ports::fake::Fakes;
use crate::ports::{Ns, PortError, Storage, StoredInfo};
use crate::refusal::Refusal;
use crate::registry::Registry;
use crate::ring::Origin;
use crate::session::Session;
use crate::spec::{Batch, CommandSpec, Facts, Gate, Mutates, NoArgs, Redraw, RingMode, Selection, TextGate, Typed, Undo};
use crate::state::{LinearEditor, TextEdit};
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

// ---- stand-ins ----

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddArgs {
    x: f64,
}

/// An undoable document edit: adds a rectangle to the active tab.
fn add(s: &mut Session, a: AddArgs) -> Result<Value, Refusal> {
    let cmd = s.cmd();
    let id = s.ports.ids.element_id();
    s.tab_mut()
        .doc
        .edit::<(), StoreError>(|tx| {
            let z = tx.next_top_index().map_err(StoreError::backend)?;
            let mut e = Element::new(id, ElementKind::Rectangle, z);
            e.transform = Transform::new(a.x, 0.0, 10.0, 10.0);
            tx.insert(e)
        })
        .map_err(|e| Refusal::internal(cmd, e.to_string()))?;
    Ok(json!({}))
}

/// Marks the active document as saved, as `doc.save` does.
fn save(s: &mut Session, _: NoArgs) -> Result<Value, Refusal> {
    let rev = s.tab().doc.revision();
    s.tab_mut().saved_revision = rev;
    Ok(json!({}))
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DragArgs {
    dx: f64,
}

fn drag(s: &mut Session, a: DragArgs) -> Result<Value, Refusal> {
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

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
struct EndArgs {
    commit: bool,
}

fn end_text(s: &mut Session, _: EndArgs) -> Result<Value, Refusal> {
    s.text_edit = None;
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
    CommandSpec::new("t.add", "t", "Add", "Adds a rectangle.", &Typed::new(add), DOC),
    CommandSpec::new("t.drag", "t", "Drag", "Moves the first element.", &Typed::new(drag), Facts { undo: Undo::Gesture, ..DOC }),
    CommandSpec::new(
        "doc.save",
        "doc",
        "Save",
        "Marks the document saved.",
        &Typed::new(save),
        Facts { undo: Undo::NoUndo, mutates: Mutates::TABS, view_only: Gate::Allowed, ..DOC },
    ),
    CommandSpec::new(
        "text.edit_end",
        "text",
        "Finish editing text",
        "Ends the text edit.",
        &Typed::new(end_text),
        Facts { while_text_edit: TextGate::Allowed, ..DOC },
    ),
];

/// The real tab family with the stand-ins, over fake ports.
fn full() -> (Session, Fakes) {
    let fakes = Fakes::new();
    let reg = Arc::new(Registry::from_families(&[
        super::COMMANDS,
        crate::cmd::gesture::COMMANDS,
        crate::cmd::settings::COMMANDS,
        crate::cmd::view::COMMANDS,
        OTHERS,
    ]));
    (Session::with_registry(fakes.ports(), reg), fakes)
}

fn names(s: &Session) -> Vec<String> {
    s.tabs.iter().map(|t| t.doc.store().meta().name.clone()).collect()
}

fn active_name(s: &Session) -> String {
    s.tab().doc.store().meta().name.clone()
}

fn count(s: &Session) -> usize {
    s.store().iter().filter(|e| !e.deleted).count()
}

/// Three tabs named A, B, C with the first active.
fn three() -> (Session, Fakes) {
    let (mut s, f) = full();
    run(&mut s, "tab.rename", json!({"name": "A"}));
    run(&mut s, "tab.new", json!({"name": "B"}));
    run(&mut s, "tab.new", json!({"name": "C"}));
    run(&mut s, "tab.switch", json!({"tab": 0}));
    (s, f)
}

// ---- availability ----

#[test]
fn tab_commands_work_in_view_only_mode_except_rename_which_edits_the_document() {
    let mut s = testing::headless_session();
    s.view_only = true;
    for id in ["tab.new", "tab.switch", "tab.close", "tab.move", "tab.duplicate", "tab.list"] {
        assert!(s.availability(id).is_ok(), "{id}");
    }
    let r = s.availability("tab.rename").unwrap_err();
    assert_eq!(r.code, "view_only");
    run(&mut s, "tab.new", json!({}));
    run(&mut s, "tab.switch", json!({"tab": 0}));
    run(&mut s, "tab.close", json!({"tab": 1}));
}

#[test]
fn a_drag_blocks_every_tab_command_except_the_list() {
    let (mut s, _f) = full();
    testing::add_rect(&mut s, 0.0, 0.0, 10.0, 10.0).unwrap();
    run(&mut s, "tab.new", json!({"name": "B"}));
    run(&mut s, "tab.switch", json!({"tab": 0}));
    run(&mut s, "gesture.begin", json!({"command": "t.drag", "args": {"dx": 9}}));
    for (id, args) in [
        ("tab.new", json!({})),
        ("tab.switch", json!({"tab": 1})),
        ("tab.close", json!({"tab": 1})),
        ("tab.rename", json!({"name": "X"})),
        ("tab.move", json!({"to_index": 1})),
        ("tab.duplicate", json!({})),
    ] {
        let a = s.availability(id).unwrap_err();
        assert_eq!(a.code, "gesture_active", "{id}");
        let before = s.fingerprint();
        let e = refuse(&mut s, id, args);
        assert_eq!((e.code, e.message.as_str()), (a.code, a.message.as_str()), "{id}");
        assert_eq!(s.fingerprint(), before, "{id}");
    }
    assert_eq!(s.tabs.len(), 2);
    run(&mut s, "tab.list", json!({}));
}

// ---- tab.new ----

#[test]
fn new_adds_a_blank_tab_after_the_active_one_and_activates_it() {
    let (mut s, _f) = three();
    run(&mut s, "tab.switch", json!({"tab": "A"}));
    let out = run(&mut s, "tab.new", json!({}));
    assert_eq!(names(&s), ["A", "Untitled", "B", "C"]);
    assert_eq!(s.active_index(), 1);
    assert_eq!((out["index"].clone(), out["name"].clone(), out["changed"].clone()), (json!(1), json!("Untitled"), json!(true)));
    assert_eq!(out["active"], out["id"]);
    assert_eq!(count(&s), 0);
    assert!(!s.tab().dirty(), "a blank document is not dirty");
    assert_eq!(s.tab().doc.undo_depth(), 0);
}

#[test]
fn new_uses_the_name_and_the_default_background_setting() {
    let (mut s, _f) = full();
    run(&mut s, "settings.set", json!({"key": "doc.default_background", "value": "#fafafa"}));
    run(&mut s, "tab.new", json!({"name": "  Sketch  "}));
    assert_eq!(active_name(&s), "Sketch");
    assert_eq!(s.store().meta().background.to_hex(), "#fafafa");
    assert_eq!(s.tabs[0].doc.store().meta().background.to_hex(), "#ffffff", "the other tab is not touched");
}

#[test]
fn new_refuses_an_empty_name_and_a_name_with_a_line_break() {
    let mut s = testing::headless_session();
    let before = s.fingerprint();
    let r = refuse(&mut s, "tab.new", json!({"name": "   "}));
    assert_eq!((r.code, r.message.as_str()), ("empty_name", "The name is empty. Give a name of 1 to 256 characters."));
    assert_eq!(refuse(&mut s, "tab.new", json!({"name": "a\nb"})).code, "bad_args");
    assert_eq!(s.tabs.len(), 1);
    assert_eq!(s.fingerprint(), before);
}

#[test]
fn new_from_active_copies_content_selection_camera_and_dirt_but_not_history() {
    let (mut s, _f) = full();
    run(&mut s, "t.add", json!({"x": 5}));
    let a = s.store().iter().next().unwrap().id.clone();
    testing::select(&mut s, std::slice::from_ref(&a));
    s.tab_mut().view.zoom = 3.0;
    run(&mut s, "tab.rename", json!({"name": "Plan"}));
    let src_id = s.tab().id;
    let out = run(&mut s, "tab.new", json!({"from": "active"}));
    assert_eq!(names(&s), ["Plan", "Plan copy"]);
    assert_eq!(out["name"], json!("Plan copy"));
    assert_ne!(s.tab().id, src_id, "a new tab id");
    assert_eq!(count(&s), 1);
    assert_eq!(s.selection().len(), 1);
    assert_eq!(s.view().zoom, 3.0);
    assert_eq!(s.tab().doc.undo_depth(), 0, "no history is copied");
    assert!(s.tab().dirty(), "the source had unsaved changes");
    // The copy is independent.
    run(&mut s, "t.add", json!({"x": 50}));
    assert_eq!(s.tabs[0].doc.store().iter().count(), 1);
    assert_eq!(s.tabs[1].doc.store().iter().count(), 2);
}

#[test]
fn new_from_active_of_a_clean_document_is_clean() {
    let (mut s, _f) = full();
    run(&mut s, "tab.new", json!({"from": "active", "name": "Copy"}));
    assert!(!s.tab().dirty());
}

#[test]
fn new_from_intro_opens_the_intro_document_in_a_clean_new_tab() {
    let mut s = testing::headless_session();
    let out = run(&mut s, "tab.new", json!({"from": "intro"}));
    assert_eq!(s.tabs.len(), 2);
    assert_eq!(out["changed"], json!(true));
    let intro = super::super::doc::intro_document(&mut s, "test").unwrap();
    assert_eq!(s.tab().doc.store().meta().name, intro.name, "the name of the intro");
    assert_eq!(s.store().len(), intro.elements.iter().filter(|e| !e.deleted).count());
    assert!(s.store().len() > 0);
    assert!(!s.tab().dirty(), "the intro is not unsaved work");
    assert_eq!(s.tab().doc.undo_depth(), 0);
    // `name` renames it, and the other tab is untouched.
    run(&mut s, "tab.new", json!({"from": "intro", "name": "Welcome"}));
    assert_eq!(s.tab().doc.store().meta().name, "Welcome");
    assert_eq!(s.tabs[0].doc.store().len(), 0);
}

#[test]
fn new_stops_at_the_limit_of_50_tabs() {
    let mut s = testing::headless_session();
    for _ in 1..MAX_TABS {
        run(&mut s, "tab.new", json!({}));
    }
    assert_eq!(s.tabs.len(), MAX_TABS);
    let before = s.fingerprint();
    for args in [json!({}), json!({"from": "active"})] {
        let r = refuse(&mut s, "tab.new", args);
        assert_eq!(r.code, "too_many_tabs");
        assert_eq!(r.message, "There are already 50 tabs, which is the limit. Close one first with `tab.close`.");
    }
    assert_eq!(refuse(&mut s, "tab.duplicate", json!({})).code, "too_many_tabs");
    assert_eq!(s.fingerprint(), before);
}

#[test]
fn the_new_tab_takes_over_the_viewport_size() {
    let mut s = testing::headless_session();
    run(&mut s, "view.set_viewport", json!({"width": 800, "height": 600, "scale_factor": 2}));
    run(&mut s, "tab.new", json!({}));
    assert_eq!((s.view().viewport.w, s.view().viewport.h, s.view().viewport.scale_factor), (800.0, 600.0, 2.0));
}

#[test]
fn new_ends_a_text_edit_first_and_leaves_no_undo_step_of_its_own() {
    let (mut s, _f) = full();
    s.text_edit = Some(TextEdit { element: None, draft: true });
    run(&mut s, "tab.new", json!({}));
    assert!(s.text_edit.is_none());
    assert_eq!(s.tab().doc.undo_depth(), 0);
}

// ---- tab.switch ----

#[test]
fn switch_finds_a_tab_by_position_id_or_name() {
    let (mut s, _f) = three();
    let c_id = s.tabs[2].id.to_string();
    run(&mut s, "tab.switch", json!({"tab": 1}));
    assert_eq!(active_name(&s), "B");
    run(&mut s, "tab.switch", json!({"tab": c_id}));
    assert_eq!(active_name(&s), "C");
    run(&mut s, "tab.switch", json!({"tab": "A"}));
    assert_eq!(active_name(&s), "A");
}

#[test]
fn switch_to_a_missing_tab_names_it_and_points_to_the_list() {
    let (mut s, _f) = three();
    let before = s.fingerprint();
    for (arg, shown) in [(json!(7), "7"), (json!("Nope"), "Nope"), (json!("t99"), "t99")] {
        let r = refuse(&mut s, "tab.switch", json!({"tab": arg}));
        assert_eq!(r.code, "no_such_tab");
        assert_eq!(r.message, format!("There is no tab `{shown}`. List tabs with `tab.list`."));
    }
    assert_eq!(refuse(&mut s, "tab.switch", json!({"tab": -1})).code, "bad_args");
    assert_eq!(refuse(&mut s, "tab.switch", json!({})).code, "bad_args");
    assert_eq!(s.fingerprint(), before);
}

#[test]
fn switch_to_the_active_tab_changes_nothing() {
    let (mut s, _f) = three();
    s.take_dirty();
    let out = run(&mut s, "tab.switch", json!({"tab": 0}));
    assert_eq!(out["changed"], json!(false));
    assert_eq!(s.take_dirty(), None);
}

#[test]
fn each_tab_keeps_its_own_selection_camera_and_history() {
    let (mut s, _f) = full();
    run(&mut s, "t.add", json!({"x": 1}));
    run(&mut s, "t.add", json!({"x": 2}));
    let a_sel = s.store().iter().next().unwrap().id.clone();
    testing::select(&mut s, std::slice::from_ref(&a_sel));
    s.tab_mut().view.zoom = 2.5;
    run(&mut s, "tab.new", json!({"name": "B"}));
    run(&mut s, "t.add", json!({"x": 3}));
    assert_eq!(s.tab().doc.undo_depth(), 1);
    assert!(s.selection().is_empty());
    run(&mut s, "tab.switch", json!({"tab": 0}));
    assert_eq!(s.tab().doc.undo_depth(), 2, "the history of tab A was not cleared by the switch");
    assert_eq!(s.selection().len(), 1);
    assert_eq!(s.view().zoom, 2.5);
    run(&mut s, "tab.switch", json!({"tab": 1}));
    assert_eq!(s.tab().doc.undo_depth(), 1);
    assert_ne!(s.view().zoom, 2.5);
    // Undo works in the tab that is active, and only there.
    s.undo_steps(1).unwrap();
    assert_eq!(count(&s), 0);
    assert_eq!(s.tabs[0].doc.store().iter().count(), 2);
}

#[test]
fn switch_leaves_the_point_editor_of_the_tab_it_leaves_and_carries_the_viewport() {
    let (mut s, _f) = three();
    let a = testing::add_rect(&mut s, 0.0, 0.0, 10.0, 10.0).unwrap();
    s.tab_mut().observed.linear_editor = Some(LinearEditor { element: a, selected_points: Default::default() });
    run(&mut s, "view.set_viewport", json!({"width": 640, "height": 480}));
    run(&mut s, "tab.switch", json!({"tab": 2}));
    assert!(s.tabs[0].observed.linear_editor.is_none());
    assert_eq!((s.view().viewport.w, s.view().viewport.h), (640.0, 480.0));
}

#[test]
fn switch_commits_a_running_text_edit_first() {
    let (mut s, _f) = three();
    s.text_edit = Some(TextEdit { element: None, draft: true });
    run(&mut s, "tab.switch", json!({"tab": 1}));
    assert!(s.text_edit.is_none());
    assert_eq!(active_name(&s), "B");
}

// ---- tab.close ----

fn tab_names(s: &mut Session) -> Vec<String> {
    names(s)
}

#[test]
fn closing_the_active_tab_activates_its_right_neighbour_or_the_left_one_at_the_end() {
    let (mut s, _f) = three();
    run(&mut s, "tab.switch", json!({"tab": 1}));
    let out = run(&mut s, "tab.close", json!({}));
    assert_eq!(tab_names(&mut s), ["A", "C"]);
    assert_eq!(active_name(&s), "C", "the right neighbour");
    assert_eq!((out["name"].clone(), out["index"].clone()), (json!("B"), json!(1)));
    run(&mut s, "tab.close", json!({}));
    assert_eq!(tab_names(&mut s), ["A"], "the last tab of the row closed, the left one is active");
    assert_eq!(active_name(&s), "A");
}

#[test]
fn closing_another_tab_keeps_the_active_tab() {
    let (mut s, _f) = three();
    run(&mut s, "tab.switch", json!({"tab": 2}));
    run(&mut s, "tab.close", json!({"tab": "A", "on_unsaved": "discard"}));
    assert_eq!(tab_names(&mut s), ["B", "C"]);
    assert_eq!(active_name(&s), "C");
    run(&mut s, "tab.close", json!({"tab": 1}));
    assert_eq!(tab_names(&mut s), ["B"]);
    assert_eq!(active_name(&s), "B");
}

#[test]
fn the_last_tab_cannot_be_closed() {
    let mut s = testing::headless_session();
    let before = s.fingerprint();
    let r = refuse(&mut s, "tab.close", json!({}));
    assert_eq!(r.code, "last_tab");
    assert_eq!(
        r.message,
        "The last tab cannot be closed. Open another tab first with `tab.new`, or clear this document with `doc.clear`."
    );
    assert_eq!(s.fingerprint(), before);
}

#[test]
fn closing_a_tab_with_unsaved_changes_is_refused_unless_told_otherwise() {
    let (mut s, _f) = three();
    run(&mut s, "t.add", json!({"x": 1}));
    let before = s.fingerprint();
    let r = refuse(&mut s, "tab.close", json!({}));
    assert_eq!(r.code, "unsaved_changes");
    assert!(r.message.starts_with("This document has unsaved changes. Save it with `doc.save`"), "{}", r.message);
    assert_eq!(s.fingerprint(), before);
    assert_eq!(refuse(&mut s, "tab.close", json!({"on_unsaved": "refuse"})).code, "unsaved_changes");
    run(&mut s, "tab.close", json!({"on_unsaved": "discard"}));
    assert_eq!(tab_names(&mut s), ["B", "C"]);
}

#[test]
fn on_unsaved_save_saves_the_active_tab_first() {
    let (mut s, _f) = three();
    run(&mut s, "t.add", json!({"x": 1}));
    run(&mut s, "tab.close", json!({"on_unsaved": "save"}));
    assert_eq!(tab_names(&mut s), ["B", "C"]);
}

#[test]
fn on_unsaved_save_names_the_way_for_a_tab_that_is_not_active() {
    let (mut s, _f) = three();
    run(&mut s, "t.add", json!({"x": 1}));
    run(&mut s, "tab.switch", json!({"tab": 2}));
    let before = s.fingerprint();
    let r = refuse(&mut s, "tab.close", json!({"tab": 0, "on_unsaved": "save"}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("\"save\" works on the active tab only"), "{}", r.message);
    assert_eq!(s.fingerprint(), before);
}

#[test]
fn on_unsaved_save_without_a_save_command_says_so() {
    // A registry without the `doc` family, so `doc.save` is not there to run.
    let reg = Arc::new(Registry::from_families(&[super::COMMANDS]));
    let mut s = Session::with_registry(Fakes::new().headless_ports(), reg);
    run(&mut s, "tab.new", json!({}));
    testing::add_rect(&mut s, 0.0, 0.0, 10.0, 10.0).unwrap();
    let r = refuse(&mut s, "tab.close", json!({"on_unsaved": "save"}));
    assert_eq!(r.code, "not_supported");
    assert_eq!(s.tabs.len(), 2);
}

#[test]
fn on_unsaved_save_runs_doc_save_and_names_a_missing_target() {
    let mut s = testing::headless_session();
    run(&mut s, "tab.new", json!({}));
    testing::add_rect(&mut s, 0.0, 0.0, 10.0, 10.0).unwrap();
    let r = refuse(&mut s, "tab.close", json!({"on_unsaved": "save"}));
    assert_eq!(r.code, "no_save_target");
    assert_eq!(s.tabs.len(), 2);
}

#[test]
fn on_unsaved_save_keeps_the_tab_when_the_write_fails() {
    let fakes = Fakes::new();
    let mut s = Session::with_services(fakes.ports(), crate::services::Services::fake());
    run(&mut s, "tab.new", json!({}));
    testing::add_rect(&mut s, 0.0, 0.0, 10.0, 10.0).unwrap();
    run(&mut s, "doc.save", json!({"to": {"path": "/a.json"}}));
    testing::add_rect(&mut s, 20.0, 0.0, 10.0, 10.0).unwrap();
    assert!(s.tab().dirty());
    fakes.files.fail_writes(true);
    let r = refuse(&mut s, "tab.close", json!({"on_unsaved": "save"}));
    assert_eq!(r.code, "io_failed");
    assert!(r.message.contains("NOT saved"), "{}", r.message);
    assert_eq!(s.tabs.len(), 2, "the tab with the unsaved work must stay");
    assert!(s.tab().dirty());
    fakes.files.fail_writes(false);
    run(&mut s, "tab.close", json!({"on_unsaved": "save"}));
    assert_eq!(s.tabs.len(), 1);
}

#[test]
fn a_tab_in_a_room_is_closed_only_with_leave_room() {
    let (mut s, _f) = three();
    s.tabs[1].ext.set(RoomBinding(Some("r-123".to_owned())));
    let before = s.fingerprint();
    let r = refuse(&mut s, "tab.close", json!({"tab": "B"}));
    assert_eq!(r.code, "in_room");
    assert_eq!(
        r.message,
        "Tab `B` is connected to room r-123. Leave the room first with `collab.leave_room`, or pass leave_room=true."
    );
    assert_eq!(s.fingerprint(), before);
    run(&mut s, "tab.close", json!({"tab": "B", "leave_room": true}));
    assert_eq!(tab_names(&mut s), ["A", "C"]);
}

#[test]
fn close_is_only_the_last_step_of_a_batch() {
    let (mut s, _f) = three();
    let steps = [BatchStep::new("tab.close", json!({"tab": 2})), BatchStep::new("tab.list", json!({}))];
    let out = s.execute_batch(Origin::Test, &steps, &BatchOpts::default());
    let failed = out.failed.expect("a close that is not last is refused");
    assert_eq!(failed.index, 0);
    assert_eq!(s.tabs.len(), 3, "preflight ran nothing");
    let steps = [BatchStep::new("tab.list", json!({})), BatchStep::new("tab.close", json!({"tab": 2}))];
    let out = s.execute_batch(Origin::Test, &steps, &BatchOpts::default());
    assert!(out.failed.is_none());
    assert_eq!(s.tabs.len(), 2);
}

// ---- tab.rename ----

#[test]
fn rename_is_one_undo_step_in_that_tabs_history() {
    let (mut s, _f) = full();
    let out = run(&mut s, "tab.rename", json!({"name": "  Roadmap  "}));
    assert_eq!(active_name(&s), "Roadmap");
    assert_eq!(out["undo_label"], json!("Rename tab"));
    assert_eq!(s.tab().doc.undo_depth(), 1);
    s.undo_steps(1).unwrap();
    assert_eq!(active_name(&s), "Untitled");
    s.redo_steps(1).unwrap();
    assert_eq!(active_name(&s), "Roadmap");
}

#[test]
fn rename_of_a_background_tab_is_a_step_in_its_own_history() {
    let (mut s, _f) = three();
    let out = run(&mut s, "tab.rename", json!({"tab": "C", "name": "Gamma"}));
    assert_eq!(tab_names(&mut s), ["A", "B", "Gamma"]);
    assert_eq!(out["undo_label"], json!("Rename tab"));
    assert_eq!(s.tabs[2].doc.undo_depth(), 1);
    assert_eq!(s.tab().doc.undo_depth(), 1, "the active tab has its own rename step from the fixture and no more");
    run(&mut s, "tab.switch", json!({"tab": 2}));
    s.undo_steps(1).unwrap();
    assert_eq!(active_name(&s), "C");
}

#[test]
fn rename_to_the_same_name_changes_nothing() {
    let (mut s, _f) = three();
    let depth = s.tab().doc.undo_depth();
    let out = run(&mut s, "tab.rename", json!({"name": "A"}));
    assert_eq!(out["changed"], json!(false));
    assert_eq!(s.tab().doc.undo_depth(), depth);
    let out = run(&mut s, "tab.rename", json!({"tab": "B", "name": "B"}));
    assert_eq!(out["changed"], json!(false));
    assert_eq!(s.tabs[1].doc.undo_depth(), 0);
}

#[test]
fn rename_refuses_an_empty_name_a_missing_tab_and_view_only_mode() {
    let (mut s, _f) = three();
    let before = s.fingerprint();
    let r = refuse(&mut s, "tab.rename", json!({"name": ""}));
    assert_eq!((r.code, r.message.as_str()), ("empty_name", "The name is empty. Give a name of 1 to 256 characters."));
    assert_eq!(refuse(&mut s, "tab.rename", json!({"tab": 9, "name": "x"})).code, "no_such_tab");
    assert_eq!(refuse(&mut s, "tab.rename", json!({"name": "a\u{7}b"})).code, "bad_args");
    assert_eq!(refuse(&mut s, "tab.rename", json!({})).code, "bad_args");
    s.view_only = true;
    assert_eq!(refuse(&mut s, "tab.rename", json!({"name": "x"})).code, "view_only");
    s.view_only = false;
    assert_eq!(s.fingerprint(), before);
}

#[test]
fn rename_cuts_a_long_name_to_the_scene_limit() {
    let mut s = testing::headless_session();
    run(&mut s, "tab.rename", json!({"name": "é".repeat(drafftink_scene::limits::MAX_NAME_CHARS + 100)}));
    assert_eq!(active_name(&s).chars().count(), drafftink_scene::limits::MAX_NAME_CHARS);
}

// ---- tab.move ----

#[test]
fn move_reorders_the_tabs_and_the_active_tab_stays_the_same_tab() {
    let (mut s, _f) = three();
    run(&mut s, "tab.switch", json!({"tab": "B"}));
    let out = run(&mut s, "tab.move", json!({"tab": "C", "to_index": 0}));
    assert_eq!(tab_names(&mut s), ["C", "A", "B"]);
    assert_eq!(active_name(&s), "B");
    assert_eq!(s.active_index(), 2);
    assert_eq!(out["index"], json!(0));
    run(&mut s, "tab.move", json!({"to_index": 0}));
    assert_eq!(tab_names(&mut s), ["B", "C", "A"], "the active tab by default");
    assert_eq!(s.active_index(), 0);
}

#[test]
fn move_to_the_same_place_is_ok_unchanged_and_out_of_range_is_named() {
    let (mut s, _f) = three();
    assert_eq!(run(&mut s, "tab.move", json!({"tab": "B", "to_index": 1}))["changed"], json!(false));
    let before = s.fingerprint();
    for bad in [json!(3), json!(-1), json!(100)] {
        let r = refuse(&mut s, "tab.move", json!({"to_index": bad}));
        assert_eq!(r.code, "bad_args");
        assert!(r.message.contains("`to_index`: must be a position from 0 to 2"), "{}", r.message);
    }
    assert_eq!(refuse(&mut s, "tab.move", json!({"tab": "Z", "to_index": 0})).code, "no_such_tab");
    assert_eq!(s.fingerprint(), before);
}

// ---- tab.duplicate ----

#[test]
fn duplicate_puts_the_copy_after_its_source_and_activates_it() {
    let (mut s, _f) = three();
    run(&mut s, "tab.switch", json!({"tab": 2}));
    let src = s.tabs[0].id;
    let out = run(&mut s, "tab.duplicate", json!({"tab": "A"}));
    assert_eq!(tab_names(&mut s), ["A", "A copy", "B", "C"]);
    assert_eq!(s.active_index(), 1);
    assert_eq!(out["name"], json!("A copy"));
    assert_ne!(s.tab().id, src);
    assert_eq!(s.tabs[0].id, src, "the source keeps its id");
}

#[test]
fn duplicate_copies_content_without_history_and_keeps_the_two_apart() {
    let (mut s, _f) = full();
    run(&mut s, "t.add", json!({"x": 1}));
    run(&mut s, "tab.duplicate", json!({"name": "Draft 2"}));
    assert_eq!(active_name(&s), "Draft 2");
    assert_eq!(count(&s), 1);
    assert_eq!(s.tab().doc.undo_depth(), 0);
    assert_eq!(s.tabs[0].doc.undo_depth(), 1, "the source keeps its history");
    assert!(s.tab().dirty() && s.tabs[0].dirty());
    run(&mut s, "t.add", json!({"x": 2}));
    assert_eq!(s.tabs[0].doc.store().iter().count(), 1);
    // A copy is not in the room of its source.
    let mut s2 = testing::headless_session();
    s2.tab_mut().ext.set(RoomBinding(Some("r".to_owned())));
    run(&mut s2, "tab.duplicate", json!({}));
    assert_eq!(s2.tab().ext.get::<RoomBinding>(), None);
}

#[test]
fn duplicate_refuses_a_missing_tab_and_a_bad_name() {
    let (mut s, _f) = three();
    let before = s.fingerprint();
    assert_eq!(refuse(&mut s, "tab.duplicate", json!({"tab": "Z"})).code, "no_such_tab");
    assert_eq!(refuse(&mut s, "tab.duplicate", json!({"name": " "})).code, "empty_name");
    assert_eq!(s.fingerprint(), before);
}

// ---- tab.list ----

#[test]
fn list_describes_each_tab() {
    let (mut s, _f) = three();
    run(&mut s, "t.add", json!({"x": 1}));
    s.tabs[1].ext.set(RoomBinding(Some("room-9".to_owned())));
    let out = run(&mut s, "tab.list", json!({}));
    assert_eq!(out["changed"], json!(false));
    let tabs = out["tabs"].as_array().unwrap();
    assert_eq!(tabs.len(), 3);
    assert_eq!(tabs[0]["name"], json!("A"));
    assert_eq!(tabs[0]["index"], json!(0));
    assert_eq!(tabs[0]["active"], json!(true));
    assert_eq!(tabs[0]["dirty"], json!(true));
    assert_eq!(tabs[1]["dirty"], json!(false));
    assert_eq!(tabs[1]["room"], json!("room-9"));
    assert!(tabs[0].get("room").is_none());
    assert_eq!(tabs[0]["id"], json!(s.tabs[0].id.to_string()));
    assert_eq!(s.tab().doc.undo_depth(), 2, "reading changed nothing");
}

// ---- the workspace record ----

fn key_names(f: &Fakes) -> Vec<String> {
    use crate::ports::Storage as _;
    f.storage.clone().list(Ns::Autosave).unwrap().into_iter().map(|i| i.name).collect()
}

fn index(f: &Fakes) -> Value {
    serde_json::from_slice(&f.storage.bytes(Ns::Autosave, INDEX_KEY).expect("an index")).unwrap()
}

fn now(f: &Fakes) -> u64 {
    use crate::ports::Clock as _;
    f.clock.now_ms()
}

fn modified(f: &Fakes, key: &str) -> u64 {
    use crate::ports::Storage as _;
    f.storage.clone().list(Ns::Autosave).unwrap().into_iter().find(|i| i.name == key).map(|i| i.modified_ms).expect(key)
}

#[test]
fn the_workspace_record_has_an_index_and_one_document_per_tab() {
    let (mut s, f) = three();
    run(&mut s, "t.add", json!({"x": 1}));
    s.tab_mut().view.zoom = 2.0;
    let a = s.store().iter().next().unwrap().id.clone();
    testing::select(&mut s, std::slice::from_ref(&a));
    let saved = s.save_workspace().unwrap();
    assert_eq!((saved.tabs, saved.documents_written), (3, 3));
    assert_eq!(key_names(&f), ["workspace", "workspace.t1", "workspace.t2", "workspace.t3"]);
    let idx = index(&f);
    assert_eq!(idx["format"], json!("drafftink-workspace"));
    assert_eq!(idx["schema"], json!(1));
    assert_eq!(idx["active"], json!(1));
    assert_eq!(idx["saved_ms"], json!(now(&f)));
    let t0 = &idx["tabs"][0];
    assert_eq!((t0["id"].clone(), t0["key"].clone(), t0["name"].clone(), t0["dirty"].clone()), (json!(1), json!("workspace.t1"), json!("A"), json!(true)));
    assert_eq!(t0["view"]["zoom"], json!(2.0));
    assert_eq!(t0["selection"], json!([a.as_str()]));
    assert_eq!(idx["tabs"][1]["dirty"], json!(false));
    let doc = String::from_utf8(f.storage.bytes(Ns::Autosave, "workspace.t1").unwrap()).unwrap();
    assert!(doc.contains("\"format\":\"drafftink\"") && doc.contains("\"name\":\"A\""), "the document is the native format: {doc}");
}

#[test]
fn all_tabs_come_back_after_a_restart_with_their_state_but_no_history() {
    let (mut s, f) = three();
    run(&mut s, "t.add", json!({"x": 7}));
    let a = s.store().iter().next().unwrap().id.clone();
    testing::select(&mut s, std::slice::from_ref(&a));
    s.tab_mut().view.zoom = 2.0;
    s.tab_mut().view.scroll = drafftink_geom::kurbo::Point::new(40.0, -30.0);
    run(&mut s, "tab.switch", json!({"tab": "B"}));
    run(&mut s, "t.add", json!({"x": 9}));
    run(&mut s, "t.add", json!({"x": 10}));
    s.save_workspace().unwrap();
    let docs: Vec<_> = s.tabs.iter().map(|t| t.doc.snapshot()).collect();
    let ids: Vec<_> = s.tabs.iter().map(|t| t.id).collect();

    let mut again = Session::with_registry(f.ports(), s.registry_arc());
    let report = again.restore_workspace().unwrap();
    assert!(report.found && report.skipped.is_empty() && report.unreadable.is_none());
    assert_eq!(report.restored, 3);
    assert_eq!(names(&again), ["A", "B", "C"]);
    assert_eq!(again.tabs.iter().map(|t| t.id).collect::<Vec<_>>(), ids, "tab ids are kept");
    assert_eq!(active_name(&again), "B", "the active tab is the one that was active");
    for (t, d) in again.tabs.iter().zip(&docs) {
        assert_eq!(&t.doc.snapshot(), d, "the document of {} is identical", t.id);
        assert_eq!(t.doc.undo_depth(), 0, "history is not saved");
    }
    let a_tab = &again.tabs[0];
    assert_eq!((a_tab.view.zoom, a_tab.view.scroll.x, a_tab.view.scroll.y), (2.0, 40.0, -30.0));
    assert_eq!(a_tab.observed.selection.len(), 1);
    assert!(a_tab.dirty() && again.tabs[1].dirty(), "the unsaved state is kept");
    assert!(!again.tabs[2].dirty());
    // Ids the session hands out next do not collide with restored ones.
    run(&mut again, "tab.new", json!({}));
    let all: std::collections::BTreeSet<_> = again.tabs.iter().map(|t| t.id).collect();
    assert_eq!(all.len(), 4);
}

#[test]
fn restore_does_not_make_the_next_tick_write_again() {
    let (mut s, f) = three();
    s.save_workspace().unwrap();
    let mut again = Session::with_registry(f.ports(), s.registry_arc());
    again.restore_workspace().unwrap();
    assert_eq!(again.workspace_next_due_ms(), None);
    f.clock.advance_ms(60_000);
    assert!(!again.workspace_tick());
}

#[test]
fn a_save_writes_only_the_tabs_that_changed_and_removes_closed_ones() {
    let (mut s, f) = three();
    s.save_workspace().unwrap();
    let before = (modified(&f, "workspace.t1"), modified(&f, "workspace.t2"), modified(&f, "workspace.t3"));
    f.clock.advance_ms(10_000);
    run(&mut s, "t.add", json!({"x": 1}));
    let saved = s.save_workspace().unwrap();
    assert_eq!(saved.documents_written, 1);
    assert!(modified(&f, "workspace.t1") > before.0);
    assert_eq!(modified(&f, "workspace.t2"), before.1, "an unchanged tab is not rewritten");
    assert_eq!(modified(&f, "workspace.t3"), before.2);
    run(&mut s, "tab.close", json!({"tab": "C"}));
    let saved = s.save_workspace().unwrap();
    assert_eq!(saved.documents_written, 0);
    assert_eq!(key_names(&f), ["workspace", "workspace.t1", "workspace.t2"]);
    // A rename alone is a change.
    run(&mut s, "tab.rename", json!({"tab": "B", "name": "Beta"}));
    assert_eq!(s.save_workspace().unwrap().documents_written, 1);
}

#[test]
fn saving_without_storage_says_what_is_missing() {
    let mut s = testing::headless_session();
    let r = s.save_workspace().unwrap_err();
    assert_eq!(r.code, "port_unavailable");
    assert!(r.message.contains("needs storage"), "{}", r.message);
    assert!(!s.workspace_tick());
    assert_eq!(s.workspace_next_due_ms(), None);
    assert_eq!(s.restore_workspace().unwrap(), Default::default());
}

// ---- debouncing on the clock port ----

#[test]
fn a_pristine_session_has_nothing_to_write() {
    let (mut s, f) = testing::session();
    assert_eq!(s.workspace_next_due_ms(), None);
    f.clock.advance_ms(60_000);
    assert!(!s.workspace_tick());
    assert!(key_names(&f).is_empty());
}

#[test]
fn a_write_follows_a_quiet_second() {
    let (mut s, f) = full();
    s.workspace_tick();
    run(&mut s, "t.add", json!({"x": 1}));
    assert_eq!(s.workspace_next_due_ms(), Some(QUIET_MS), "one timer, set from the answer");
    f.clock.advance_ms(QUIET_MS - 1);
    assert!(!s.workspace_tick(), "not quiet long enough");
    assert_eq!(s.workspace_next_due_ms(), Some(1));
    f.clock.advance_ms(1);
    assert!(s.workspace_tick());
    assert!(key_names(&f).contains(&"workspace".to_owned()));
    assert_eq!(s.workspace_next_due_ms(), None, "idle means no timer");
    f.clock.advance_ms(60_000);
    assert!(!s.workspace_tick());
}

#[test]
fn steady_work_is_written_at_the_latest_after_the_interval() {
    let (mut s, f) = full();
    s.workspace_tick();
    run(&mut s, "t.add", json!({"x": 0}));
    s.workspace_tick();
    // A change every 400 ms never leaves a quiet second, so the interval (5 s) decides.
    let mut wrote_at = None;
    for i in 1..=20_u64 {
        f.clock.advance_ms(400);
        run(&mut s, "t.add", json!({"x": i}));
        if s.workspace_tick() {
            wrote_at = Some(i * 400);
            break;
        }
    }
    assert_eq!(wrote_at, Some(5_200), "the first write is due 5000 ms after the first change; the 13th tick is the first past it");
}

#[test]
fn the_interval_setting_changes_the_longest_wait() {
    let (mut s, f) = full();
    run(&mut s, "settings.set", json!({"key": "autosave.interval_s", "value": 2}));
    s.workspace_tick();
    run(&mut s, "t.add", json!({"x": 0}));
    s.workspace_tick();
    f.clock.advance_ms(1_500);
    run(&mut s, "t.add", json!({"x": 1}));
    s.workspace_tick();
    assert_eq!(s.workspace_next_due_ms(), Some(500), "2000 ms after the first change, sooner than a quiet second");
    f.clock.advance_ms(500);
    assert!(s.workspace_tick());
}

#[test]
fn camera_and_selection_alone_cause_no_write() {
    let (mut s, f) = three();
    run(&mut s, "t.add", json!({"x": 1}));
    s.save_workspace().unwrap();
    s.workspace_tick();
    let a = s.store().iter().next().unwrap().id.clone();
    testing::select(&mut s, std::slice::from_ref(&a));
    s.tab_mut().view.zoom = 5.0;
    f.clock.advance_ms(60_000);
    assert_eq!(s.workspace_next_due_ms(), None);
    assert!(!s.workspace_tick());
}

#[test]
fn a_tab_switch_a_move_and_an_undo_each_cause_a_write() {
    let (mut s, f) = full();
    run(&mut s, "t.add", json!({"x": 1}));
    run(&mut s, "tab.new", json!({}));
    s.save_workspace().unwrap();
    s.workspace_tick();
    for step in 0..3 {
        match step {
            0 => drop(run(&mut s, "tab.switch", json!({"tab": 0}))),
            1 => drop(run(&mut s, "tab.move", json!({"to_index": 1}))),
            _ => drop(s.undo_steps(1).unwrap()),
        }
        assert!(s.workspace_next_due_ms().is_some(), "step {step}");
        f.clock.advance_ms(QUIET_MS);
        assert!(s.workspace_tick(), "step {step}");
    }
}

#[test]
fn a_change_that_is_taken_back_before_the_write_cancels_it() {
    let (mut s, f) = full();
    run(&mut s, "tab.new", json!({}));
    s.save_workspace().unwrap();
    s.workspace_tick();
    run(&mut s, "tab.switch", json!({"tab": 0}));
    assert!(s.workspace_next_due_ms().is_some());
    run(&mut s, "tab.switch", json!({"tab": 1}));
    assert_eq!(s.workspace_next_due_ms(), None);
    f.clock.advance_ms(10_000);
    assert!(!s.workspace_tick());
}

#[test]
fn autosave_off_writes_nothing_and_asks_for_no_timer() {
    let (mut s, f) = full();
    run(&mut s, "settings.set", json!({"key": "autosave.enabled", "value": false}));
    s.workspace_tick();
    run(&mut s, "t.add", json!({"x": 1}));
    assert_eq!(s.workspace_next_due_ms(), None);
    f.clock.advance_ms(60_000);
    assert!(!s.workspace_tick());
    assert!(!s.flush_workspace());
    assert!(key_names(&f).iter().all(|k| !k.starts_with("workspace")));
    // Turned on again: the pending change is picked up.
    run(&mut s, "settings.set", json!({"key": "autosave.enabled", "value": true}));
    assert!(s.workspace_next_due_ms().is_some());
}

#[test]
fn flush_writes_a_pending_change_at_once_for_window_close() {
    let (mut s, f) = full();
    s.workspace_tick();
    run(&mut s, "t.add", json!({"x": 1}));
    s.workspace_tick();
    assert!(s.flush_workspace());
    assert!(key_names(&f).contains(&"workspace".to_owned()));
    assert!(!s.flush_workspace(), "nothing is pending any more");
}

/// Storage whose writes fail.
#[derive(Clone)]
struct Full;

impl Storage for Full {
    fn get(&self, _: Ns, _: &str) -> Result<Option<Vec<u8>>, PortError> {
        Ok(None)
    }
    fn put(&mut self, _: Ns, _: &str, _: &[u8]) -> Result<(), PortError> {
        Err(PortError::Failed("disk full".to_owned()))
    }
    fn delete(&mut self, _: Ns, _: &str) -> Result<(), PortError> {
        Ok(())
    }
    fn list(&self, _: Ns) -> Result<Vec<StoredInfo>, PortError> {
        Ok(Vec::new())
    }
}

#[test]
fn a_failed_write_is_reported_and_retried_after_a_full_interval() {
    let f = Fakes::new();
    let reg = Arc::new(Registry::from_families(&[super::COMMANDS, OTHERS]));
    let mut s = Session::with_registry(f.headless_ports().with_storage(Box::new(Full)), reg);
    s.workspace_tick();
    run(&mut s, "t.add", json!({"x": 1}));
    s.workspace_tick();
    f.clock.advance_ms(QUIET_MS);
    assert!(!s.workspace_tick(), "the write failed");
    let msg = s.workspace_error().unwrap();
    assert!(msg.starts_with("Could not write the workspace document `workspace.t1`: disk full."), "{msg}");
    assert_eq!(s.workspace_next_due_ms(), Some(QUIET_MS), "tried again soon, not on every call");
    // A text-based error is the same refusal the explicit call returns.
    assert_eq!(s.save_workspace().unwrap_err().code, "io_failed");
}

// ---- restore: damaged and foreign records ----

fn put(f: &Fakes, key: &str, text: &str) {
    f.storage.clone().put(Ns::Autosave, key, text.as_bytes()).unwrap();
}

fn fresh(f: &Fakes) -> Session {
    Session::new(f.ports())
}

#[test]
fn nothing_stored_changes_nothing() {
    let (mut s, _f) = testing::session();
    let before = s.fingerprint();
    let r = s.restore_workspace().unwrap();
    assert!(!r.found && r.restored == 0);
    assert_eq!(s.fingerprint(), before);
}

#[test]
fn a_damaged_index_is_reported_and_the_session_stays_as_it_was() {
    for bad in ["not json", "[]", r#"{"format":"x","schema":1,"tabs":[]}"#, r#"{"format":"drafftink-workspace","tabs":[]}"#] {
        let f = Fakes::new();
        put(&f, INDEX_KEY, bad);
        let mut s = fresh(&f);
        let before = s.fingerprint();
        let r = s.restore_workspace().unwrap();
        assert!(r.found && r.unreadable.is_some() && r.restored == 0, "{bad}");
        assert_eq!(s.fingerprint(), before, "{bad}");
    }
}

#[test]
fn a_tab_whose_document_is_damaged_or_missing_is_skipped_and_the_others_open() {
    let (mut s, f) = three();
    s.save_workspace().unwrap();
    put(&f, "workspace.t2", "{ nope");
    let mut storage = f.storage.clone();
    storage.delete(Ns::Autosave, "workspace.t3").unwrap();
    let mut again = fresh(&f);
    let r = again.restore_workspace().unwrap();
    assert_eq!(r.restored, 1);
    assert_eq!(r.skipped.iter().map(|k| k.id).collect::<Vec<_>>(), [2, 3]);
    assert!(r.skipped[0].reason.contains("`workspace.t2` is not a document"), "{}", r.skipped[0].reason);
    assert!(r.skipped[1].reason.contains("`workspace.t3` is missing from storage"), "{}", r.skipped[1].reason);
    assert_eq!(names(&again), ["A"]);
    assert_eq!(again.active_index(), 0, "the active tab was skipped; the first one opens");
}

#[test]
fn when_no_tab_can_be_read_the_session_is_left_alone() {
    let (mut s, f) = three();
    s.save_workspace().unwrap();
    let mut storage = f.storage.clone();
    for k in ["workspace.t1", "workspace.t2", "workspace.t3"] {
        storage.delete(Ns::Autosave, k).unwrap();
    }
    let mut again = fresh(&f);
    let before = again.fingerprint();
    let r = again.restore_workspace().unwrap();
    assert_eq!((r.found, r.restored, r.skipped.len()), (true, 0, 3));
    assert_eq!(again.fingerprint(), before);
}

#[test]
fn hostile_tab_entries_are_skipped_or_cleaned() {
    let (mut s, f) = full();
    run(&mut s, "t.add", json!({"x": 1}));
    s.save_workspace().unwrap();
    let doc = String::from_utf8(f.storage.bytes(Ns::Autosave, "workspace.t1").unwrap()).unwrap();
    put(&f, "workspace.t5", &doc);
    put(&f, "workspace.t2000000", &doc);
    let idx = json!({
        "format": "drafftink-workspace", "schema": 1, "active": 5,
        "tabs": [
            {"id": 5, "view": {"scroll": [1e300, 0], "zoom": 1e9}, "selection": ["e1", "gone", "bad id!", 7]},
            {"id": 5},
            {"id": 0},
            {"id": 2000000},
            {"id": "x"},
            {"id": 1, "view": {"scroll": "far", "zoom": "big"}},
        ],
    });
    put(&f, INDEX_KEY, &idx.to_string());
    let mut again = fresh(&f);
    let r = again.restore_workspace().unwrap();
    assert_eq!(r.restored, 2);
    assert_eq!(r.skipped.len(), 4);
    assert!(r.skipped.iter().any(|k| k.reason == "its id is used twice"));
    assert!(r.skipped.iter().any(|k| k.reason == "its id is missing or out of range"));
    let t5 = again.tabs.iter().find(|t| t.id.0 == 5).unwrap();
    assert_eq!(t5.view.scroll.x, 0.0, "a scroll outside the allowed range is ignored");
    assert_eq!(t5.view.zoom, crate::state::ZOOM_MAX, "the zoom is clamped");
    assert_eq!(t5.observed.selection.len(), 1, "only ids of live elements stay selected");
    assert_eq!(again.tab().id.0, 5);
}

#[test]
fn a_workspace_from_a_newer_version_is_not_read_and_not_overwritten() {
    let (mut s, f) = three();
    s.save_workspace().unwrap();
    let newer = r#"{"format":"drafftink-workspace","schema":9,"tabs":[]}"#;
    put(&f, INDEX_KEY, newer);
    let mut again = fresh(&f);
    let before = again.fingerprint();
    let r = again.restore_workspace().unwrap();
    assert_eq!((r.restored, r.newer_schema), (0, Some(9)));
    assert_eq!(again.fingerprint(), before);
    run(&mut again, "tab.new", json!({}));
    assert!(again.workspace_next_due_ms().is_none(), "no autosave while the newer record is protected");
    let e = again.save_workspace().unwrap_err();
    assert_eq!(e.code, "workspace_newer");
    assert!(e.message.contains("format 9"), "{}", e.message);
    assert_eq!(f.storage.bytes(Ns::Autosave, INDEX_KEY).unwrap(), newer.as_bytes());
}

#[test]
fn restore_ends_a_running_drag_and_a_text_edit() {
    let (mut s, f) = full();
    run(&mut s, "t.add", json!({"x": 1}));
    s.save_workspace().unwrap();
    let mut again = Session::with_registry(f.ports(), s.registry_arc());
    run(&mut again, "tab.new", json!({}));
    testing::add_rect(&mut again, 0.0, 0.0, 10.0, 10.0).unwrap();
    run(&mut again, "gesture.begin", json!({"command": "t.drag", "args": {"dx": 9}}));
    again.restore_workspace().unwrap();
    assert!(!again.gesture_running());
    assert_eq!(again.tabs.len(), 1);
}

#[test]
fn restore_requests_a_redraw() {
    let (mut s, f) = three();
    s.save_workspace().unwrap();
    let mut again = fresh(&f);
    again.take_dirty();
    again.restore_workspace().unwrap();
    assert_eq!(again.take_dirty(), Some(Redraw::Scene));
}

#[test]
fn never_saved_documents_stay_dirty_whatever_their_revision() {
    let (mut s, _f) = full();
    run(&mut s, "t.add", json!({"x": 1}));
    run(&mut s, "tab.duplicate", json!({}));
    assert_eq!(s.tab().saved_revision, NEVER_SAVED);
    run(&mut s, "t.add", json!({"x": 2}));
    run(&mut s, "t.add", json!({"x": 3}));
    assert!(s.tab().dirty());
    run(&mut s, "doc.save", json!({}));
    assert!(!s.tab().dirty());
}
