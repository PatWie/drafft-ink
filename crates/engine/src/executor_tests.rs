//! Tests of the executor, batches, gestures and the ring, with synthetic commands in a private registry.
//! The registry-wide tests over the real commands are in `registry_tests.rs`.

use std::sync::Arc;

use drafftink_scene::{Element, ElementId, ElementKind, SceneRead, SceneTx, StoreError, Transform};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::batch::{BatchOpts, BatchStep};
use crate::ports::fake::Fakes;
use crate::ports::{ClipPayload, PortWrite};
use crate::refusal::Refusal;
use crate::registry::Registry;
use crate::ring::Origin;
use crate::session::Session;
use crate::spec::{
    Batch, CommandSpec, Facts, Gate, Mutates, NoArgs, Redraw, RingMode, Selection, TextGate, Typed, Undo,
};
use crate::state::TextEdit;

// ---- synthetic commands ----

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
struct AddArgs {
    x: f64,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
struct Made {
    ids: Vec<String>,
}

fn insert_rect(s: &mut Session, x: f64) -> Result<ElementId, Refusal> {
    let id = s.ports.ids.element_id();
    let ret = id.clone();
    let cmd = s.cmd();
    s.tab_mut()
        .doc
        .edit::<(), StoreError>(|tx| {
            let z = tx.next_top_index().map_err(|e| StoreError::Backend(e.to_string()))?;
            let mut e = Element::new(id, ElementKind::Rectangle, z);
            e.transform = Transform::new(x, 0.0, 10.0, 10.0);
            tx.insert(e)
        })
        .map_err(|e| Refusal::internal(cmd, e.to_string()))?;
    Ok(ret)
}

fn add(s: &mut Session, a: AddArgs) -> Result<Made, Refusal> {
    let id = insert_rect(s, a.x)?;
    s.tab_mut().observed.selection.insert(id.clone());
    Ok(Made { ids: vec![id.as_str().to_owned()] })
}

fn add_then_fail(s: &mut Session, a: AddArgs) -> Result<Made, Refusal> {
    insert_rect(s, a.x)?;
    s.tab_mut().view.zoom = 7.0;
    s.tool.lock = true;
    s.queue_write(PortWrite::Clipboard(ClipPayload::Text("leaked".into())));
    Err(Refusal::nothing_to_do(s.cmd(), "add", "this test command always fails after changing things"))
}

fn add_then_panic(s: &mut Session, a: AddArgs) -> Result<Made, Refusal> {
    insert_rect(s, a.x)?;
    s.tab_mut().view.zoom = 7.0;
    let v: Vec<u8> = Vec::new();
    let i = v.len() + 3;
    #[allow(clippy::indexing_slicing)]
    let _ = v[i];
    Ok(Made { ids: vec![] })
}

fn noop(_: &mut Session, _: NoArgs) -> Result<Value, Refusal> {
    Ok(json!({}))
}

fn touch_same(s: &mut Session, _: NoArgs) -> Result<Value, Refusal> {
    // Writes the value it already has: a no-op for the store.
    let ids: Vec<ElementId> = s.store().iter().map(|e| e.id.clone()).collect();
    let cmd = s.cmd();
    s.tab_mut()
        .doc
        .edit::<(), StoreError>(|tx| {
            for id in &ids {
                tx.update(id, |_| {})?;
            }
            Ok(())
        })
        .map_err(|e| Refusal::internal(cmd, e.to_string()))?;
    Ok(json!({}))
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct IdArgs {
    id: String,
}

fn select(s: &mut Session, a: IdArgs) -> Result<Value, Refusal> {
    let id = s.resolve_ref(&crate::session::Ref(a.id))?;
    s.tab_mut().observed.selection.clear();
    s.tab_mut().observed.selection.insert(id);
    Ok(json!({}))
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MoveArgs {
    id: String,
    dx: f64,
    dy: f64,
    #[serde(default)]
    fail: bool,
}

#[derive(Default)]
struct LiveCount {
    live: u32,
    final_runs: u32,
}

fn move_el(s: &mut Session, a: MoveArgs) -> Result<Value, Refusal> {
    let id = s.resolve_ref(&crate::session::Ref(a.id.clone()))?;
    if a.fail {
        return Err(Refusal::nothing_to_do(s.cmd(), "move", "asked to fail"));
    }
    let live = s.live();
    s.ext.with::<LiveCount, _>(|c| if live { c.live += 1 } else { c.final_runs += 1 });
    let cmd = s.cmd();
    s.tab_mut()
        .doc
        .edit::<(), StoreError>(|tx| {
            tx.update(&id, |e| {
                e.transform.x += a.dx;
                e.transform.y += a.dy;
            })
        })
        .map_err(|e| Refusal::internal(cmd, e.to_string()))?;
    s.tab_mut().observed.selection.insert(id);
    Ok(json!({ "moved": true }))
}

fn nudge(s: &mut Session, a: IdArgs) -> Result<Value, Refusal> {
    let id = s.resolve_ref(&crate::session::Ref(a.id))?;
    let cmd = s.cmd();
    s.tab_mut()
        .doc
        .edit::<(), StoreError>(|tx| tx.update(&id, |e| e.transform.x += 1.0))
        .map_err(|e| Refusal::internal(cmd, e.to_string()))?;
    Ok(json!({}))
}

fn write_clip(s: &mut Session, _: NoArgs) -> Result<Value, Refusal> {
    s.queue_write(PortWrite::Clipboard(ClipPayload::Text("copied".into())));
    Ok(json!({}))
}

fn set_setting(s: &mut Session, _: NoArgs) -> Result<Value, Refusal> {
    let changed = s.settings.set_raw("snap.grid", json!(true));
    Ok(json!({ "changed": changed }))
}

fn need_selection(s: &Session) -> Result<(), Refusal> {
    if s.selection().is_empty() { Err(Refusal::no_target("t.sel_only")) } else { Ok(()) }
}

fn nested(s: &mut Session, a: AddArgs) -> Result<Value, Refusal> {
    s.run_nested("t.add", json!({ "x": a.x }))
}

fn calls_execute(s: &mut Session, _: NoArgs) -> Result<Value, Refusal> {
    s.execute("t.noop", json!({}))
}

fn forget(s: &mut Session, a: AddArgs) -> Result<Value, Refusal> {
    insert_rect(s, a.x)?;
    s.clear_history();
    Ok(json!({}))
}

fn end_text(s: &mut Session, _: Value) -> Result<Value, Refusal> {
    insert_rect(s, 99.0)?;
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

const TEST_COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("t.add", "t", "Add", "Adds a rectangle and selects it.", &Typed::new(add), DOC),
    CommandSpec::new("t.add_fail", "t", "Add then fail", "Changes things, then refuses.", &Typed::new(add_then_fail), DOC),
    CommandSpec::new("t.add_panic", "t", "Add then panic", "Changes things, then panics.", &Typed::new(add_then_panic), DOC),
    CommandSpec::new("t.noop", "t", "No-op", "Changes nothing.", &Typed::new(noop), DOC),
    CommandSpec::new("t.touch_same", "t", "Touch", "Writes what is already there.", &Typed::new(touch_same), DOC),
    CommandSpec::new(
        "t.select",
        "t",
        "Select",
        "Selects an element.",
        &Typed::new(select),
        Facts { undo: Undo::NoUndo, mutates: Mutates::OBSERVED, redraw: Redraw::Overlay, ..DOC },
    ),
    CommandSpec::new(
        "t.move",
        "t",
        "Move",
        "Moves an element by a total offset; a gesture.",
        &Typed::new(move_el),
        Facts { undo: Undo::Gesture, ..DOC },
    ),
    CommandSpec::new(
        "t.nudge",
        "t",
        "Nudge",
        "Moves an element by one unit.",
        &Typed::new(nudge),
        DOC,
    )
    .coalesce("t.nudge", 500),
    CommandSpec::new(
        "t.write",
        "t",
        "Write clipboard",
        "Queues a clipboard write.",
        &Typed::new(write_clip),
        Facts { undo: Undo::NoUndo, mutates: Mutates::PORTS, view_only: Gate::Allowed, ..DOC },
    ),
    CommandSpec::new(
        "t.setting",
        "t",
        "Change setting",
        "Changes a setting.",
        &Typed::new(set_setting),
        Facts { undo: Undo::NoUndo, mutates: Mutates::SETTINGS, view_only: Gate::Allowed, ..DOC },
    ),
    CommandSpec::new(
        "t.sel_only",
        "t",
        "Needs a selection",
        "Available only with a selection.",
        &Typed::new(noop),
        Facts { needs_selection: Selection::Required, ..DOC },
    )
    .available(need_selection),
    CommandSpec::new(
        "t.last",
        "t",
        "Last only",
        "Only as the final batch step.",
        &Typed::new(noop),
        Facts { batch: Batch::Last, ..DOC },
    ),
    CommandSpec::new(
        "t.never",
        "t",
        "Never in a batch",
        "Not in batches.",
        &Typed::new(noop),
        Facts { batch: Batch::Never, ..DOC },
    ),
    CommandSpec::new(
        "t.read",
        "t",
        "Read",
        "Reads.",
        &Typed::new(noop),
        Facts::READ_ONLY,
    ),
    CommandSpec::new(
        "t.quiet",
        "t",
        "Quiet",
        "High-rate command.",
        &Typed::new(noop),
        Facts { ring: RingMode::Quiet, ..Facts::READ_ONLY },
    ),
    CommandSpec::new(
        "t.text_refuse",
        "t",
        "Refuses during text",
        "Refused during a text edit.",
        &Typed::new(noop),
        Facts { while_text_edit: TextGate::Refused, ..Facts::READ_ONLY },
    ),
    CommandSpec::new(
        "t.nested",
        "t",
        "Nested",
        "Runs t.add through run_nested.",
        &Typed::new(nested),
        DOC,
    ),
    CommandSpec::new(
        "t.calls_execute",
        "t",
        "Calls execute",
        "Wrongly calls execute from a handler.",
        &Typed::new(calls_execute),
        Facts::READ_ONLY,
    ),
    CommandSpec::new(
        "t.forget",
        "t",
        "Forget history",
        "Adds a rectangle and clears the history.",
        &Typed::new(forget),
        Facts { undo: Undo::NoUndo, ..DOC },
    ),
    CommandSpec::new(
        "text.edit_end",
        "text",
        "End text edit",
        "Commits the text edit and adds a rectangle.",
        &Typed::new(end_text),
        DOC,
    ),
    CommandSpec::new(
        "t.planned",
        "t",
        "Planned",
        "Not built yet.",
        &Typed::new(noop),
        Facts::READ_ONLY,
    )
    .planned("test reason"),
];

fn test_session() -> (Session, Fakes) {
    let fakes = Fakes::new();
    let reg = Arc::new(Registry::from_families(&[TEST_COMMANDS, crate::cmd::gesture::COMMANDS]));
    (Session::with_registry(fakes.ports(), reg), fakes)
}

fn ok(s: &mut Session, id: &str, args: Value) -> Value {
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => v,
        Err(r) => panic!("{id} refused: {} ({})", r.message, r.code),
    }
}

fn refused(s: &mut Session, id: &str, args: Value) -> Refusal {
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => panic!("{id} should have refused, got {v}"),
        Err(r) => r,
    }
}

fn live_elements(s: &Session) -> usize {
    s.store().len()
}

/// The document as JSON without the store's local change counters.
fn snapshot_json(s: &Session) -> Value {
    let mut v = serde_json::to_value(s.tab().doc.snapshot()).unwrap();
    if let Some(els) = v.get_mut("elements").and_then(Value::as_array_mut) {
        for e in els {
            if let Some(o) = e.as_object_mut() {
                o.remove("version");
            }
        }
    }
    v
}

fn first_created(v: &Value) -> String {
    v["ids"][0].as_str().unwrap_or_default().to_owned()
}

// ---- execute ----

#[test]
fn unknown_command_suggests_and_is_recorded() {
    let (mut s, _) = test_session();
    let r = refused(&mut s, "t.ad", json!({}));
    assert_eq!(r.code, "unknown_command");
    assert!(r.message.contains("`t.add`"), "{}", r.message);
    let last = s.ring().iter().last().unwrap();
    assert_eq!(last.outcome.code(), Some("unknown_command"));
    assert_eq!(last.id, "t.ad");
}

#[test]
fn bad_args_names_the_key() {
    let (mut s, _) = test_session();
    let r = refused(&mut s, "t.add", json!({"xx": 1}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("`xx`") && r.message.contains("did you mean `x`"), "{}", r.message);
    let r = refused(&mut s, "t.add", json!({"x": "left"}));
    assert!(r.message.contains("`x`"), "{}", r.message);
    assert_eq!(live_elements(&s), 0);
}

#[test]
fn availability_text_matches_the_refusal_from_execute() {
    let (mut s, _) = test_session();
    let a = s.availability("t.sel_only").unwrap_err();
    let e = refused(&mut s, "t.sel_only", json!({}));
    assert_eq!((a.code, a.message.as_str()), (e.code, e.message.as_str()));
    ok(&mut s, "t.add", json!({}));
    assert!(s.availability("t.sel_only").is_ok());
    let info = s.commands();
    assert!(info.iter().any(|c| c.id == "t.planned" && c.available.is_err()));
}

#[test]
fn planned_commands_refuse_with_not_supported() {
    let (mut s, _) = test_session();
    let r = refused(&mut s, "t.planned", json!({}));
    assert_eq!(r.code, "not_supported");
    assert!(r.message.contains("test reason"));
}

#[test]
fn one_undoable_command_is_one_undo_step_and_noops_push_nothing() {
    let (mut s, _) = test_session();
    let out = ok(&mut s, "t.add", json!({"x": 5}));
    assert_eq!(out["changed"], true);
    assert_eq!(out["undo_label"], "Add");
    assert_eq!(out["created"].as_array().unwrap().len(), 1);
    assert_eq!(s.tab().history.undo_depth(), 1);

    let out = ok(&mut s, "t.noop", json!({}));
    assert_eq!(out["changed"], false);
    let out = ok(&mut s, "t.touch_same", json!({}));
    assert_eq!(out["changed"], false, "writing the same value is not a change");
    assert_eq!(s.tab().history.undo_depth(), 1);

    let r = s.undo_steps(1).unwrap();
    assert_eq!(r.labels, ["Add"]);
    assert_eq!(live_elements(&s), 0);
    assert!(s.selection().is_empty(), "undo restores the selection from before the step");
    s.redo_steps(1).unwrap();
    assert_eq!(live_elements(&s), 1);
    assert_eq!(s.selection().len(), 1);
}

#[test]
fn noundo_command_changes_without_a_step_and_selection_alone_makes_none() {
    let (mut s, _) = test_session();
    let id = first_created(&ok(&mut s, "t.add", json!({})));
    let depth = s.tab().history.undo_depth();
    ok(&mut s, "t.select", json!({"id": id}));
    assert_eq!(s.tab().history.undo_depth(), depth);
}

#[test]
fn a_refusal_rolls_back_everything_and_discards_port_writes() {
    let (mut s, fakes) = test_session();
    ok(&mut s, "t.add", json!({}));
    let before = s.fingerprint();
    let ring_before = s.ring().len();
    let r = refused(&mut s, "t.add_fail", json!({"x": 3}));
    assert_eq!(r.code, "nothing_to_do");
    assert_eq!(r.command, "t.add_fail");
    assert_eq!(s.fingerprint(), before);
    assert!(fakes.clipboard.content().is_none(), "a queued write must not survive a refusal");
    assert_eq!(s.ring().len(), ring_before + 1);
    assert_eq!(s.ring().iter().last().unwrap().outcome.code(), Some("nothing_to_do"));
}

#[test]
fn a_panic_becomes_internal_and_rolls_back() {
    let (mut s, _) = test_session();
    ok(&mut s, "t.add", json!({}));
    let before = s.fingerprint();
    let r = refused(&mut s, "t.add_panic", json!({}));
    assert_eq!(r.code, "internal");
    assert!(r.message.contains("rolled back"), "{}", r.message);
    assert_eq!(s.fingerprint(), before);
    // The session still works.
    ok(&mut s, "t.add", json!({}));
}

#[test]
fn port_writes_are_flushed_after_success() {
    let (mut s, fakes) = test_session();
    ok(&mut s, "t.write", json!({}));
    assert_eq!(fakes.clipboard.content(), Some(ClipPayload::Text("copied".into())));
}

#[test]
fn gates_come_from_facts() {
    let (mut s, _) = test_session();
    s.view_only = true;
    let r = refused(&mut s, "t.add", json!({}));
    assert_eq!(r.code, "view_only");
    assert!(r.message.contains("view.set_view_only"));
    ok(&mut s, "t.write", json!({}));
    s.view_only = false;

    s.text_edit = Some(TextEdit { element: None, draft: true });
    let r = refused(&mut s, "t.text_refuse", json!({}));
    assert_eq!(r.code, "text_editing");
    assert!(s.availability("t.text_refuse").is_err());
    s.text_edit = None;
}

#[test]
fn a_commit_gate_commits_the_text_edit_first_as_its_own_step() {
    let (mut s, _) = test_session();
    s.text_edit = Some(TextEdit { element: None, draft: true });
    ok(&mut s, "t.add", json!({"x": 1}));
    assert!(s.text_edit.is_none());
    assert_eq!(live_elements(&s), 2, "the committed text edit and the command both ran");
    assert_eq!(s.tab().history.undo_depth(), 2);
    let ids: Vec<&str> = s.ring().iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["text.edit_end", "t.add"]);
}

#[test]
fn coalesce_merges_repeats_on_the_same_target_within_the_window() {
    let (mut s, fakes) = test_session();
    let id = first_created(&ok(&mut s, "t.add", json!({})));
    for _ in 0..3 {
        ok(&mut s, "t.nudge", json!({"id": id}));
        fakes.clock.advance_ms(100);
    }
    assert_eq!(s.tab().history.undo_depth(), 2, "add + one merged nudge step");
    fakes.clock.advance_ms(2000);
    ok(&mut s, "t.nudge", json!({"id": id}));
    assert_eq!(s.tab().history.undo_depth(), 3);
    s.undo_steps(1).unwrap();
    s.undo_steps(1).unwrap();
    let e = s.element(&ElementId::new(&id).unwrap()).unwrap();
    assert_eq!(e.transform.x, 0.0);
}

#[test]
fn history_depth_setting_caps_the_stack() {
    let (mut s, _) = test_session();
    s.settings.set_raw("history.depth", json!(3));
    for i in 0..6 {
        ok(&mut s, "t.add", json!({"x": i}));
    }
    assert_eq!(s.tab().history.undo_depth(), 3);
}

#[test]
fn run_nested_skips_the_transaction_and_execute_inside_a_handler_is_refused() {
    let (mut s, _) = test_session();
    let out = ok(&mut s, "t.nested", json!({"x": 2}));
    assert_eq!(out["undo_label"], "Nested");
    assert_eq!(s.tab().history.undo_depth(), 1);
    assert_eq!(s.ring().len(), 1, "only the outer command is recorded");
    let r = refused(&mut s, "t.calls_execute", json!({}));
    assert_eq!(r.code, "internal");
    assert!(r.message.contains("run_nested"), "{}", r.message);
}

#[test]
fn clear_history_suppresses_the_undo_step() {
    let (mut s, _) = test_session();
    ok(&mut s, "t.add", json!({}));
    ok(&mut s, "t.forget", json!({}));
    assert_eq!(s.tab().history.undo_depth(), 0);
    let out = ok(&mut s, "t.add", json!({}));
    assert_eq!(out["changed"], true);
    assert_eq!(s.tab().history.undo_depth(), 1);
}

#[test]
fn dirty_is_set_only_when_something_changed() {
    let (mut s, _) = test_session();
    assert_eq!(s.take_dirty(), None);
    ok(&mut s, "t.noop", json!({}));
    assert_eq!(s.take_dirty(), None, "idle means idle");
    ok(&mut s, "t.add", json!({}));
    assert_eq!(s.take_dirty(), Some(Redraw::Scene));
    assert_eq!(s.take_dirty(), None);
}

#[test]
fn typed_execution_skips_json() {
    let (mut s, _) = test_session();
    let out = s.execute_typed(Origin::Ui, "t.add", &AddArgs { x: 4.0 }).unwrap();
    assert_eq!(out["changed"], true);
    let r = s.execute_typed(Origin::Ui, "t.add", &IdArgs { id: "x".into() }).unwrap_err();
    assert_eq!(r.code, "internal");
    assert_eq!(s.ring().iter().next().unwrap().args, r#"{"x":4.0}"#);
}

// ---- ring ----

#[test]
fn ring_records_origin_args_and_outcome_and_skips_quiet_successes() {
    let (mut s, _) = test_session();
    s.execute_from(Origin::Palette, "t.add", json!({"x": 1})).unwrap();
    ok(&mut s, "t.quiet", json!({}));
    s.view_only = false;
    let e: Vec<_> = s.ring().iter().cloned().collect();
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].source, Origin::Palette);
    assert_eq!(e[0].args, r#"{"x":1}"#);
    assert!(e[0].outcome.is_ok() && e[0].mutating);
    assert_eq!(e[0].seq, 1);

    // A Quiet command is recorded when it refuses.
    let r = refused(&mut s, "t.quiet", json!({"bad": 1}));
    assert_eq!(r.code, "bad_args");
    assert_eq!(s.ring().iter().last().unwrap().id, "t.quiet");
}

#[test]
fn ring_keeps_reads_and_filters_them_on_request() {
    let (mut s, _) = test_session();
    ok(&mut s, "t.read", json!({}));
    ok(&mut s, "t.add", json!({}));
    assert_eq!(s.ring().recent(10, false, None).len(), 2);
    assert_eq!(s.ring().recent(10, true, None).len(), 1);
}

// ---- batches ----

fn step(id: &str, args: Value) -> BatchStep {
    BatchStep::new(id, args)
}

#[test]
fn a_batch_is_one_undo_step_with_symbolic_references() {
    let (mut s, _) = test_session();
    let steps = vec![
        step("t.add", json!({"x": 1})).named("box"),
        step("t.nudge", json!({"id": "$box"})),
        step("t.nudge", json!({"id": "$0"})),
    ];
    let out = s.execute_batch(Origin::Test, &steps, &BatchOpts { label: Some("Draw box".into()), ..BatchOpts::default() });
    assert!(out.ok(), "{:?}", out.failed);
    assert_eq!(out.completed, 3);
    assert_eq!(out.undo_label.as_deref(), Some("Draw box"));
    assert_eq!(s.tab().history.undo_depth(), 1);
    let id = ElementId::new(&first_created(&out.results[0])).unwrap();
    assert_eq!(s.element(&id).unwrap().transform.x, 3.0);
    assert_eq!(s.ring().len(), 1, "one ring entry for the batch");
    assert_eq!(s.ring().iter().next().unwrap().steps, Some(3));

    s.undo_steps(1).unwrap();
    assert_eq!(live_elements(&s), 0, "undo reverts the whole batch");
    assert!(s.selection().is_empty());
}

#[test]
fn the_default_batch_label_is_the_first_title_and_the_count() {
    let (mut s, _) = test_session();
    let out = s.execute_batch(Origin::Test, &[step("t.add", json!({})), step("t.add", json!({}))], &BatchOpts::default());
    assert_eq!(out.undo_label.as_deref(), Some("Add + 1 more"));
}

#[test]
fn created_lists_and_literal_dollars() {
    let (mut s, _) = test_session();
    let steps = vec![
        step("t.add", json!({"x": 1})).named("a"),
        step("t.add", json!({"x": 2})).named("b"),
        step("t.add", json!({"x": 2})),
    ];
    assert!(s.execute_batch(Origin::Test, &steps, &BatchOpts::default()).ok());
    // `$$x` stays a literal string: here it is not a valid element id, so the step names the key.
    let out = s.execute_batch(Origin::Test, &[step("t.select", json!({"id": "$$x"}))], &BatchOpts::default());
    let f = out.failed.unwrap();
    assert_eq!(f.refusal.code, "not_found", "{:?}", f.refusal);
}

#[test]
fn an_atomic_batch_rolls_back_everything_at_the_first_refusal() {
    let (mut s, _) = test_session();
    ok(&mut s, "t.add", json!({}));
    let before = s.fingerprint();
    let steps = vec![
        step("t.add", json!({"x": 1})),
        step("t.add", json!({"x": 2})),
        step("t.add_fail", json!({})),
        step("t.add", json!({"x": 3})),
    ];
    let out = s.execute_batch(Origin::Test, &steps, &BatchOpts::default());
    assert!(!out.ok());
    assert_eq!(out.completed, 2);
    assert!(out.rolled_back);
    let f = out.failed.as_ref().unwrap();
    assert_eq!((f.index, f.id.as_str(), f.refusal.code), (2, "t.add_fail", "nothing_to_do"));
    assert_eq!(s.fingerprint(), before, "document, selection, tool and view are back");
}

#[test]
fn a_non_atomic_batch_keeps_earlier_steps_as_one_undo_step() {
    let (mut s, _) = test_session();
    let steps = vec![step("t.add", json!({"x": 1})), step("t.add", json!({"x": 2})), step("t.add_fail", json!({}))];
    let out = s.execute_batch(Origin::Test, &steps, &BatchOpts { atomic: false, ..BatchOpts::default() });
    assert!(!out.ok() && !out.rolled_back);
    assert_eq!(out.completed, 2);
    assert_eq!(live_elements(&s), 2);
    assert_eq!(s.tab().history.undo_depth(), 1);
    s.undo_steps(1).unwrap();
    assert_eq!(live_elements(&s), 0);
}

#[test]
fn preflight_runs_nothing_when_a_later_step_is_certainly_wrong() {
    let (mut s, _) = test_session();
    let before = s.fingerprint();
    let cases: Vec<(Vec<BatchStep>, usize, &str)> = vec![
        (vec![step("t.add", json!({})), step("t.ad", json!({}))], 1, "unknown_command"),
        (vec![step("t.add", json!({})), step("t.never", json!({}))], 1, "not_batchable"),
        (vec![step("t.last", json!({})), step("t.add", json!({}))], 0, "must_be_last"),
        (vec![step("t.add", json!({})), step("t.add", json!({"xx": 1}))], 1, "bad_args"),
        (vec![step("t.add", json!({"x": "no"}))], 0, "bad_args"),
        (vec![step("t.add", json!({})).named("a"), step("t.add", json!({})).named("a")], 1, "bad_args"),
        (vec![step("t.select", json!({"id": "$nope"}))], 0, "bad_args"),
        (vec![step("t.select", json!({"id": "$1"})), step("t.add", json!({}))], 0, "bad_args"),
        (vec![step("t.add", json!({})).named("12")], 0, "bad_args"),
    ];
    for (steps, index, code) in cases {
        let out = s.execute_batch(Origin::Test, &steps, &BatchOpts::default());
        let f = out.failed.as_ref().unwrap_or_else(|| panic!("{steps:?} should fail preflight"));
        assert_eq!((f.index, f.refusal.code), (index, code), "{:?}", f.refusal);
        assert_eq!(out.completed, 0);
        assert!(!out.rolled_back);
        assert!(out.note.as_deref().unwrap_or("").contains("nothing was run"), "{:?}", out.note);
        assert_eq!(s.fingerprint(), before);
    }
}

#[test]
fn preflight_checks_availability_of_step_zero_only() {
    let (mut s, _) = test_session();
    let out = s.execute_batch(Origin::Test, &[step("t.text_refuse", json!({}))], &BatchOpts::default());
    assert!(out.ok());
    s.text_edit = Some(TextEdit { element: None, draft: true });
    let out = s.execute_batch(Origin::Test, &[step("t.text_refuse", json!({}))], &BatchOpts::default());
    assert_eq!(out.failed.unwrap().refusal.code, "text_editing");
}

#[test]
fn a_last_step_is_allowed_at_the_end() {
    let (mut s, _) = test_session();
    let out = s.execute_batch(Origin::Test, &[step("t.add", json!({})), step("t.last", json!({}))], &BatchOpts::default());
    assert!(out.ok(), "{:?}", out.failed);
}

#[test]
fn dry_run_reports_and_rolls_back() {
    let (mut s, _) = test_session();
    let before = s.fingerprint();
    let out = s.execute_batch(
        Origin::Test,
        &[step("t.add", json!({})), step("t.add", json!({}))],
        &BatchOpts { dry_run: true, ..BatchOpts::default() },
    );
    assert!(out.ok() && out.rolled_back);
    assert_eq!(out.results.len(), 2);
    assert_eq!(s.fingerprint(), before);
    assert_eq!(s.tab().history.undo_depth(), 0);
}

#[test]
fn batch_port_writes_flush_only_after_success_and_outside_steps_are_listed() {
    let (mut s, fakes) = test_session();
    let out = s.execute_batch(Origin::Test, &[step("t.write", json!({})), step("t.add_fail", json!({}))], &BatchOpts::default());
    assert!(out.rolled_back);
    assert!(fakes.clipboard.content().is_none());

    let out = s.execute_batch(
        Origin::Test,
        &[step("t.write", json!({})), step("t.setting", json!({})), step("t.add", json!({}))],
        &BatchOpts::default(),
    );
    assert!(out.ok());
    assert_eq!(fakes.clipboard.content(), Some(ClipPayload::Text("copied".into())));
    let listed: Vec<(usize, &str)> = out.not_rolled_back.iter().map(|n| (n.index, n.id.as_str())).collect();
    assert_eq!(listed, [(0, "t.write"), (1, "t.setting")]);

    // A failing batch cannot take back the setting; it says so.
    s.settings.set_raw("snap.grid", json!(false));
    let out = s.execute_batch(Origin::Test, &[step("t.setting", json!({})), step("t.add_fail", json!({}))], &BatchOpts::default());
    assert!(out.rolled_back);
    assert_eq!(out.not_rolled_back.len(), 1);
    assert!(s.settings.bool_or("snap.grid", false), "settings are outside the transaction");
}

#[test]
fn a_batch_ending_in_a_history_clear_pushes_no_step() {
    let (mut s, _) = test_session();
    let out = s.execute_batch(Origin::Test, &[step("t.add", json!({})), step("t.forget", json!({}))], &BatchOpts::default());
    assert!(out.ok());
    assert_eq!(s.tab().history.undo_depth(), 0);
}

#[test]
fn batch_json_validates_shape_and_limits() {
    let (mut s, _) = test_session();
    let r = s.execute_batch_json(Origin::Test, &json!({"stepz": []})).unwrap_err();
    assert_eq!(r.code, "bad_args");
    let r = s.execute_batch_json(Origin::Test, &json!({"steps": [{"id": "t.add", "arg": {}}]})).unwrap_err();
    assert_eq!(r.code, "bad_args");
    let many: Vec<Value> = (0..10_001).map(|_| json!({"id": "t.noop"})).collect();
    let r = s.execute_batch_json(Origin::Test, &json!({"steps": many})).unwrap_err();
    assert_eq!(r.code, "limit_exceeded");
    let v = s
        .execute_batch_json(
            Origin::Test,
            &json!({"steps": [{"id": "t.add", "args": {"x": 1}, "as": "a"}], "opts": {"label": "L"}}),
        )
        .unwrap();
    assert_eq!(v["completed"], 1);
    assert_eq!(v["undo_label"], "L");
}

// ---- gestures ----

fn setup_gesture() -> (Session, String) {
    let (mut s, _) = test_session();
    let id = first_created(&ok(&mut s, "t.add", json!({"x": 0})));
    s.tab_mut().observed.selection.clear();
    (s, id)
}

fn x_of(s: &Session, id: &str) -> f64 {
    s.element(&ElementId::new(id).unwrap()).unwrap().transform.x
}

#[test]
fn begin_updates_and_end_make_one_undo_step_equal_to_a_plain_execute() {
    let (mut s, id) = setup_gesture();
    let depth = s.tab().history.undo_depth();
    let g = ok(&mut s, "gesture.begin", json!({"command": "t.move", "args": {"id": id, "dx": 5, "dy": 0}}));
    let gid = g["gesture"].as_str().unwrap().to_owned();
    assert_eq!(g["changed"], true);
    assert_eq!(x_of(&s, &id), 5.0);
    // Frames are absolute: a later frame replaces the earlier one.
    ok(&mut s, "gesture.update", json!({"gesture": gid, "args": {"id": id, "dx": 40, "dy": 2}}));
    ok(&mut s, "gesture.update", json!({"gesture": gid, "args": {"id": id, "dx": 20, "dy": 1}}));
    assert_eq!(x_of(&s, &id), 20.0);
    assert_eq!(s.tab().history.undo_depth(), depth, "live frames push no undo step");
    let out = ok(&mut s, "gesture.end", json!({"args": {"id": id, "dx": 30, "dy": 3}}));
    assert_eq!(out["undo_label"], "Move");
    assert_eq!(x_of(&s, &id), 30.0);
    assert_eq!(s.tab().history.undo_depth(), depth + 1);
    assert!(!s.gesture_running());
    let after_gesture = snapshot_json(&s);

    // The same final state by a plain execute on the base.
    s.undo_steps(1).unwrap();
    assert_eq!(x_of(&s, &id), 0.0);
    ok(&mut s, "t.move", json!({"id": id, "dx": 30, "dy": 3}));
    assert_eq!(snapshot_json(&s), after_gesture);

    let counts = s.ext.with::<LiveCount, _>(|c| (c.live, c.final_runs));
    assert_eq!(counts.0, 3, "three live frames");
}

#[test]
fn the_ring_gets_one_entry_for_a_gesture_with_its_frame_count() {
    let (mut s, id) = setup_gesture();
    s.ring = crate::ring::Ring::default();
    let g = ok(&mut s, "gesture.begin", json!({"command": "t.move", "args": {"id": id, "dx": 1, "dy": 0}}));
    let gid = g["gesture"].as_str().unwrap().to_owned();
    for i in 0..5 {
        ok(&mut s, "gesture.update", json!({"gesture": gid, "args": {"id": id, "dx": i, "dy": 0}}));
    }
    assert_eq!(s.ring().len(), 0, "live frames are not recorded");
    ok(&mut s, "gesture.end", json!({}));
    assert_eq!(s.ring().len(), 1);
    let e = s.ring().iter().next().unwrap();
    assert_eq!((e.id.as_str(), e.frames), ("t.move", Some(6)));
}

#[test]
fn cancel_restores_the_state_from_before_begin() {
    let (mut s, id) = setup_gesture();
    let before = s.fingerprint();
    ok(&mut s, "gesture.begin", json!({"command": "t.move", "args": {"id": id, "dx": 50, "dy": 0}}));
    assert_ne!(s.fingerprint(), before);
    let out = ok(&mut s, "gesture.cancel", json!({}));
    assert_eq!(out["cancelled"], true);
    assert_eq!(s.fingerprint(), before);
    // A stray cancel is fine.
    let out = ok(&mut s, "gesture.cancel", json!({}));
    assert_eq!(out["changed"], false);
}

#[test]
fn a_refused_frame_keeps_the_last_good_frame() {
    let (mut s, id) = setup_gesture();
    let g = ok(&mut s, "gesture.begin", json!({"command": "t.move", "args": {"id": id, "dx": 5, "dy": 0}}));
    let gid = g["gesture"].as_str().unwrap().to_owned();
    ok(&mut s, "gesture.update", json!({"gesture": gid, "args": {"id": id, "dx": 9, "dy": 0}}));
    let r = refused(&mut s, "gesture.update", json!({"gesture": gid, "args": {"id": id, "dx": 99, "dy": 0, "fail": true}}));
    assert_eq!(r.code, "nothing_to_do");
    assert_eq!(x_of(&s, &id), 9.0);
    assert!(s.gesture_running());
    // The gesture can still end, from the last good frame's arguments.
    ok(&mut s, "gesture.end", json!({}));
    assert_eq!(x_of(&s, &id), 9.0);
}

#[test]
fn a_refusal_at_end_restores_the_base() {
    let (mut s, id) = setup_gesture();
    ok(&mut s, "gesture.begin", json!({"command": "t.move", "args": {"id": id, "dx": 5, "dy": 0}}));
    let r = refused(&mut s, "gesture.end", json!({"args": {"id": id, "dx": 5, "dy": 0, "fail": true}}));
    assert_eq!(r.code, "nothing_to_do");
    assert!(!s.gesture_running());
    assert_eq!(x_of(&s, &id), 0.0);
    assert_eq!(s.tab().history.undo_depth(), 1);
}

#[test]
fn gesture_rules_one_at_a_time_only_gesture_commands_and_gates() {
    let (mut s, id) = setup_gesture();
    let r = refused(&mut s, "gesture.begin", json!({"command": "t.add", "args": {}}));
    assert_eq!(r.code, "not_gesturable");
    assert!(r.message.contains("t.move"), "{}", r.message);

    ok(&mut s, "gesture.begin", json!({"command": "t.move", "args": {"id": id, "dx": 1, "dy": 0}}));
    let r = refused(&mut s, "gesture.begin", json!({"command": "t.move", "args": {"id": id, "dx": 1, "dy": 0}}));
    assert_eq!(r.code, "gesture_active");
    let r = refused(&mut s, "t.add", json!({}));
    assert_eq!(r.code, "gesture_active");
    ok(&mut s, "t.read", json!({}));
    let r = refused(&mut s, "gesture.update", json!({"gesture": "g99", "args": {}}));
    assert_eq!(r.code, "no_gesture");
    s.cancel_gesture_if_running();

    let r = refused(&mut s, "gesture.end", json!({}));
    assert_eq!(r.code, "no_gesture");
    s.view_only = true;
    let r = refused(&mut s, "gesture.begin", json!({"command": "t.move", "args": {"id": id, "dx": 1, "dy": 0}}));
    assert_eq!(r.code, "view_only");
}

#[test]
fn a_gesture_whose_first_frame_is_refused_never_starts() {
    let (mut s, id) = setup_gesture();
    let before = s.fingerprint();
    let r = refused(&mut s, "gesture.begin", json!({"command": "t.move", "args": {"id": id, "dx": 1, "dy": 0, "fail": true}}));
    assert_eq!(r.code, "nothing_to_do");
    assert!(!s.gesture_running());
    assert_eq!(s.fingerprint(), before);
}

#[test]
fn the_watchdog_cancels_a_gesture_without_frames() {
    let (mut s, fakes) = test_session();
    let id = first_created(&ok(&mut s, "t.add", json!({})));
    ok(&mut s, "gesture.begin", json!({"command": "t.move", "args": {"id": id, "dx": 4, "dy": 0}}));
    assert!(!s.tick());
    fakes.clock.advance_ms(29_000);
    assert!(!s.tick());
    fakes.clock.advance_ms(1_500);
    assert!(s.tick());
    assert!(!s.gesture_running());
    assert_eq!(x_of(&s, &id), 0.0);
}

#[test]
fn the_typed_path_builds_no_json_per_frame() {
    let (mut s, id) = setup_gesture();
    let args = |dx: f64| MoveArgs { id: id.clone(), dx, dy: 0.0, fail: false };
    let (gid, _) = s.gesture_begin_typed(Origin::Ui, "t.move", &args(1.0)).unwrap();
    for i in 0..10 {
        s.gesture_update(gid, &args(f64::from(i))).unwrap();
    }
    assert_eq!(x_of(&s, &id), 9.0);
    let out = s.gesture_end_typed(gid, Some(&args(12.0))).unwrap();
    assert_eq!(out["changed"], true);
    assert_eq!(x_of(&s, &id), 12.0);
    assert_eq!(s.ring().iter().last().unwrap().frames, Some(11));
    // Wrong id and wrong type are refusals, not panics.
    let (gid, _) = s.gesture_begin_typed(Origin::Ui, "t.move", &args(1.0)).unwrap();
    assert_eq!(s.gesture_update(crate::GestureId(gid.0 + 100), &args(1.0)).unwrap_err().code, "no_gesture");
    assert_eq!(s.gesture_update(gid, &AddArgs { x: 1.0 }).unwrap_err().code, "internal");
    assert!(s.gesture_running());
    s.gesture_cancel(Some(gid)).unwrap();
}
