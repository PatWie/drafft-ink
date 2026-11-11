//! `history.*`: availability, exact refusal texts, the effect on the document and on the restored editor
//! state, and the rules around gestures, text edits, view-only mode, batches and collaborators.

use drafftink_scene::{ElementId, GroupId, SceneRead, SceneTx, StoreError};
use serde_json::{Value, json};

use crate::batch::{BatchOpts, BatchStep};
use crate::ring::Origin;
use crate::session::Session;
use crate::spec::Redraw;
use crate::state::{Observed, TextEdit};
use crate::store::{DocTx, StepMeta};
use crate::testing;

fn ok(s: &mut Session, id: &str, args: Value) -> Value {
    s.execute_from(Origin::Test, id, args).unwrap_or_else(|r| panic!("{id} refused: {} ({})", r.message, r.code))
}

fn refused(s: &mut Session, id: &str, args: Value) -> crate::Refusal {
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => panic!("{id} should have refused, got {v}"),
        Err(r) => r,
    }
}

/// One undoable step, written the way the executor writes a command's step.
fn step(
    s: &mut Session,
    label: &str,
    before: &Observed,
    after: &Observed,
    f: impl FnOnce(&mut DocTx<'_, '_>) -> Result<(), StoreError>,
) {
    let doc = &mut s.tab_mut().doc;
    let mark = doc.mark();
    doc.edit::<(), StoreError>(f).unwrap();
    doc.commit_since(mark, &StepMeta { label, undoable: true, before, after, depth: 50, coalesce: None }).unwrap();
    s.tab_mut().observed = after.clone();
}

fn sel(ids: &[&ElementId]) -> Observed {
    Observed { selection: ids.iter().map(|i| (*i).clone()).collect(), ..Observed::default() }
}

fn x_of(s: &Session, id: &ElementId) -> f64 {
    s.element(id).unwrap().transform.x
}

/// A session with one rectangle `a` and two undoable steps on it: "Move" (x 0 to 10) and "Move again" (to 30).
fn with_two_moves() -> (Session, ElementId) {
    let mut s = testing::headless_session();
    let a = testing::add_rect(&mut s, 0.0, 0.0, 50.0, 50.0).unwrap();
    let (none, one) = (Observed::default(), sel(&[&a]));
    step(&mut s, "Move", &none, &one, |tx| tx.update(&a, |e| e.transform.x = 10.0));
    step(&mut s, "Move again", &one, &none, |tx| tx.update(&a, |e| e.transform.x = 30.0));
    (s, a)
}

// ---- availability and refusal texts ----

#[test]
fn undo_and_redo_with_nothing_to_do_refuse_in_the_catalog_words() {
    let mut s = testing::headless_session();
    let before = s.fingerprint();
    let r = s.availability("history.undo").unwrap_err();
    assert_eq!((r.code, r.message.as_str()), ("nothing_to_undo", "There is nothing to undo in this tab."));
    let r = refused(&mut s, "history.undo", json!({}));
    assert_eq!((r.code, r.message.as_str()), ("nothing_to_undo", "There is nothing to undo in this tab."));
    assert_eq!(r.command, "history.undo");
    let r = s.availability("history.redo").unwrap_err();
    assert_eq!((r.code, r.message.as_str()), ("nothing_to_redo", "There is nothing to redo in this tab."));
    let r = refused(&mut s, "history.redo", json!({"steps": 2}));
    assert_eq!((r.code, r.message.as_str()), ("nothing_to_redo", "There is nothing to redo in this tab."));
    assert_eq!(s.fingerprint(), before, "a refusal changes nothing");
    assert_eq!(s.take_dirty(), None);
    let last = s.ring().iter().last().unwrap();
    assert_eq!(last.outcome.code(), Some("nothing_to_redo"));
}

#[test]
fn availability_follows_the_stacks() {
    let (mut s, _) = with_two_moves();
    assert!(s.availability("history.undo").is_ok());
    assert!(s.availability("history.redo").is_err());
    ok(&mut s, "history.undo", json!({}));
    assert!(s.availability("history.redo").is_ok());
    ok(&mut s, "history.undo", json!({}));
    assert!(s.availability("history.undo").is_err(), "both steps are undone");
    ok(&mut s, "history.redo", json!({"steps": 2}));
    assert!(s.availability("history.redo").is_err());
}

// ---- effect ----

#[test]
fn undo_takes_back_the_last_step_and_restores_the_selection() {
    let (mut s, a) = with_two_moves();
    assert!(s.selection().is_empty());
    let out = ok(&mut s, "history.undo", json!({}));
    assert_eq!(out["undone"], json!(["Move again"]));
    assert_eq!((out["can_undo"].as_bool(), out["can_redo"].as_bool()), (Some(true), Some(true)));
    assert_eq!(out["changed"], true);
    assert_eq!(x_of(&s, &a), 10.0);
    assert_eq!(s.selection().iter().collect::<Vec<_>>(), [&a], "the selection from before the step is back");
    assert_eq!(s.take_dirty(), Some(Redraw::Scene));
    assert_eq!(s.tab().doc.undo_depth(), 1, "undo is not itself a step");
    assert_eq!(s.ring().iter().last().unwrap().outcome, crate::Outcome::ok(true, None));
}

#[test]
fn redo_reapplies_and_restores_the_selection_from_after_the_step() {
    let (mut s, a) = with_two_moves();
    ok(&mut s, "history.undo", json!({"steps": 2}));
    assert_eq!(x_of(&s, &a), 0.0);
    assert!(s.selection().is_empty());
    let out = ok(&mut s, "history.redo", json!({}));
    assert_eq!(out["redone"], json!(["Move"]));
    assert_eq!(x_of(&s, &a), 10.0);
    assert_eq!(s.selection().iter().collect::<Vec<_>>(), [&a]);
    let out = ok(&mut s, "history.redo", json!({}));
    assert_eq!(out["redone"], json!(["Move again"]));
    assert_eq!(out["can_redo"], false);
    assert_eq!(x_of(&s, &a), 30.0);
    assert!(s.selection().is_empty());
}

#[test]
fn several_steps_at_once_come_back_newest_first_and_too_many_takes_all() {
    let (mut s, a) = with_two_moves();
    let out = ok(&mut s, "history.undo", json!({"steps": 2}));
    assert_eq!(out["undone"], json!(["Move again", "Move"]));
    assert_eq!(out["can_undo"], false);
    assert_eq!(x_of(&s, &a), 0.0);
    let out = ok(&mut s, "history.redo", json!({"steps": 40}));
    assert_eq!(out["redone"], json!(["Move", "Move again"]), "asking for more than exist takes what exists");
    assert_eq!(x_of(&s, &a), 30.0);
}

#[test]
fn the_editing_group_and_point_editor_are_restored() {
    let mut s = testing::headless_session();
    let a = testing::add_rect(&mut s, 0.0, 0.0, 50.0, 50.0).unwrap();
    let before = Observed { editing_group: Some(GroupId::new("g1").unwrap()), ..sel(&[&a]) };
    let after = Observed::default();
    step(&mut s, "Ungroup", &before, &after, |tx| tx.update(&a, |e| e.locked = true));
    assert!(s.tab().observed.editing_group.is_none());
    ok(&mut s, "history.undo", json!({}));
    assert_eq!(s.tab().observed.editing_group, Some(GroupId::new("g1").unwrap()));
    assert_eq!(s.tab().observed.selection.len(), 1);
    ok(&mut s, "history.redo", json!({}));
    assert!(s.tab().observed.editing_group.is_none() && s.tab().observed.selection.is_empty());
}

#[test]
fn undoing_a_creation_drops_the_removed_element_from_the_restored_selection() {
    let mut s = testing::headless_session();
    let none = Observed::default();
    let id = ElementId::new("made").unwrap();
    let z = s.tab().doc.store().next_top_index().unwrap();
    let made = {
        let mut e = drafftink_scene::Element::new(id.clone(), drafftink_scene::ElementKind::Rectangle, z);
        e.transform = drafftink_scene::Transform::new(0.0, 0.0, 5.0, 5.0);
        e
    };
    step(&mut s, "Create", &none, &sel(&[&id]), |tx| tx.insert(made));
    ok(&mut s, "history.undo", json!({}));
    assert!(s.element(&id).is_none());
    assert!(s.selection().is_empty());
    // Redo brings it back and selects it, as the step recorded.
    ok(&mut s, "history.redo", json!({}));
    assert!(s.element(&id).is_some());
    assert_eq!(s.selection().iter().collect::<Vec<_>>(), [&id]);
}

#[test]
fn a_new_step_clears_what_could_be_redone() {
    let (mut s, a) = with_two_moves();
    ok(&mut s, "history.undo", json!({}));
    assert!(s.tab().doc.can_redo());
    let o = Observed::default();
    step(&mut s, "Recolor", &o, &o, |tx| tx.update(&a, |e| e.locked = true));
    assert!(!s.tab().doc.can_redo());
    assert_eq!(refused(&mut s, "history.redo", json!({})).code, "nothing_to_redo");
}

#[test]
fn the_steps_count_must_be_in_range_and_unknown_keys_are_named() {
    let (mut s, _) = with_two_moves();
    let before = s.fingerprint();
    for bad in [json!({"steps": 0}), json!({"steps": 1001}), json!({"steps": -1}), json!({"steps": "two"}), json!({"steps": 1.5})] {
        let r = refused(&mut s, "history.undo", bad.clone());
        assert_eq!(r.code, "bad_args", "{bad}");
        assert!(r.message.contains("steps"), "{}", r.message);
    }
    let r = refused(&mut s, "history.undo", json!({"step": 1}));
    assert!(r.message.contains("did you mean `steps`"), "{}", r.message);
    assert_eq!(s.fingerprint(), before, "refusals change no document or editor state");
    s.settings.set_raw("history.depth", json!(3));
    ok(&mut s, "history.undo", json!({}));
    let r = refused(&mut s, "history.redo", json!({"steps": 4}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("from 1 to 3"), "{}", r.message);
}

// ---- gates ----

#[test]
fn undo_redo_and_clear_refuse_during_a_text_edit_and_undo_redo_in_view_only_mode() {
    let (mut s, _) = with_two_moves();
    ok(&mut s, "history.undo", json!({}));
    let before = s.fingerprint();

    s.text_edit = Some(TextEdit { element: None, draft: true });
    for id in ["history.undo", "history.redo", "history.clear"] {
        let r = refused(&mut s, id, json!({}));
        assert_eq!(r.code, "text_editing", "{id}");
        assert_eq!(s.availability(id).unwrap_err().message, r.message, "{id}: the greyed text is the refusal text");
    }
    s.text_edit = None;

    s.view_only = true;
    for id in ["history.undo", "history.redo"] {
        assert_eq!(refused(&mut s, id, json!({})).code, "view_only", "{id}");
    }
    s.view_only = false;
    assert_eq!(s.fingerprint(), before, "refusals changed nothing");
}

// ---- gestures, batches and coalesced steps need commands that do those things ----

mod synthetic {
    use std::sync::Arc;

    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};
    use serde_json::{Value, json};

    use super::*;
    use crate::refusal::Refusal;
    use crate::registry::Registry;
    use crate::spec::{Batch, CommandSpec, Facts, Gate, Mutates, Redraw, RingMode, Selection, TextGate, Typed, Undo};

    #[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
    #[serde(deny_unknown_fields, default)]
    pub struct DxArgs {
        pub dx: f64,
    }

    fn shift(s: &mut Session, a: DxArgs) -> Result<Value, Refusal> {
        let cmd = s.cmd();
        let ids: Vec<ElementId> = s.store().iter().map(|e| e.id.clone()).collect();
        s.tab_mut()
            .doc
            .edit::<(), StoreError>(|tx| {
                for id in &ids {
                    tx.update(id, |e| e.transform.x += a.dx)?;
                }
                Ok(())
            })
            .map_err(|e| Refusal::internal(cmd, e.to_string()))?;
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

    const T: &[CommandSpec] = &[
        CommandSpec::new("t.drag", "t", "Drag", "Moves everything; a gesture.", &Typed::new(shift), Facts { undo: Undo::Gesture, ..DOC }),
        CommandSpec::new("t.nudge", "t", "Nudge", "Moves everything a little.", &Typed::new(shift), DOC)
            .coalesce("t.nudge", 500),
    ];

    pub fn session() -> (Session, ElementId, crate::ports::fake::Fakes) {
        let fakes = crate::ports::fake::Fakes::new();
        let reg = Arc::new(Registry::from_families(&[crate::cmd::history::COMMANDS, crate::cmd::gesture::COMMANDS, T]));
        let mut s = Session::with_registry(fakes.headless_ports(), reg);
        s.services = crate::Services::fake();
        let a = testing::add_rect(&mut s, 0.0, 0.0, 50.0, 50.0).unwrap();
        (s, a, fakes)
    }
}

#[test]
fn undo_redo_and_clear_refuse_while_a_gesture_runs_and_work_again_after_it() {
    let (mut s, a, _) = synthetic::session();
    ok(&mut s, "t.nudge", json!({"dx": 5.0}));
    s.gesture_begin_json("t.drag", &json!({"dx": 1.0})).unwrap();
    let before = s.fingerprint();
    for id in ["history.undo", "history.redo", "history.clear"] {
        let r = refused(&mut s, id, json!({}));
        assert_eq!(r.code, "gesture_active", "{id}");
    }
    assert_eq!(s.fingerprint(), before);
    assert_eq!(s.availability("history.undo").unwrap_err().code, "gesture_active");
    // history.state is a read and works.
    assert_eq!(ok(&mut s, "history.state", json!({}))["undo_depth"], 1);
    s.gesture_cancel(None).unwrap();
    ok(&mut s, "history.undo", json!({}));
    assert_eq!(x_of(&s, &a), 0.0);
}

#[test]
fn history_commands_cannot_be_steps_of_a_batch() {
    let (mut s, a, _) = synthetic::session();
    ok(&mut s, "t.nudge", json!({"dx": 5.0}));
    for id in ["history.undo", "history.redo"] {
        let steps = [BatchStep::new("t.nudge", json!({"dx": 1.0})), BatchStep::new(id, json!({}))];
        let out = s.execute_batch(Origin::Test, &steps, &BatchOpts::default());
        let f = out.failed.expect("the batch must be refused");
        assert_eq!((f.index, f.id.as_str()), (1, id));
        assert!(out.rolled_back || out.completed == 0);
        assert_eq!(x_of(&s, &a), 5.0, "nothing of the batch ran");
    }
}

#[test]
fn a_coalesced_run_of_repeats_is_one_step_for_undo_and_redo() {
    let (mut s, a, fakes) = synthetic::session();
    for _ in 0..3 {
        ok(&mut s, "t.nudge", json!({"dx": 1.0}));
        fakes.clock.advance_ms(100);
    }
    assert_eq!(x_of(&s, &a), 3.0);
    let out = ok(&mut s, "history.undo", json!({}));
    assert_eq!(out["undone"], json!(["Nudge"]));
    assert_eq!(x_of(&s, &a), 0.0, "the three repeats come back together");
    assert_eq!(out["can_undo"], false);
    ok(&mut s, "history.redo", json!({}));
    assert_eq!(x_of(&s, &a), 3.0);
}

// ---- clear and state ----

#[test]
fn clear_drops_both_stacks_and_leaves_the_document() {
    let (mut s, a) = with_two_moves();
    ok(&mut s, "history.undo", json!({}));
    let out = ok(&mut s, "history.clear", json!({}));
    assert_eq!((out["changed"].as_bool(), out["undo_dropped"].as_u64(), out["redo_dropped"].as_u64()), (Some(true), Some(1), Some(1)));
    assert_eq!(x_of(&s, &a), 10.0, "the document stays as it is");
    assert_eq!(refused(&mut s, "history.undo", json!({})).code, "nothing_to_undo");
    assert_eq!(refused(&mut s, "history.redo", json!({})).code, "nothing_to_redo");
    let again = ok(&mut s, "history.clear", json!({}));
    assert_eq!(again["changed"], false, "already empty: ok, nothing changed");
    // It works in view-only mode (it changes no document).
    s.view_only = true;
    ok(&mut s, "history.clear", json!({}));
}

#[test]
fn state_reports_labels_and_depths_in_every_mode() {
    let (mut s, _) = with_two_moves();
    let st = ok(&mut s, "history.state", json!({}));
    assert_eq!(st["can_undo"], true);
    assert_eq!(st["can_redo"], false);
    assert_eq!(st["undo_label"], "Move again");
    assert_eq!(st["redo_label"], Value::Null);
    assert_eq!((st["undo_depth"].as_u64(), st["redo_depth"].as_u64()), (Some(2), Some(0)));
    ok(&mut s, "history.undo", json!({}));
    let st = ok(&mut s, "history.state", json!({}));
    assert_eq!((st["undo_label"].as_str(), st["redo_label"].as_str()), (Some("Move"), Some("Move again")));
    assert_eq!((st["undo_depth"].as_u64(), st["redo_depth"].as_u64()), (Some(1), Some(1)));
    let before = s.fingerprint();
    s.view_only = true;
    s.text_edit = Some(TextEdit { element: None, draft: true });
    ok(&mut s, "history.state", json!({}));
    s.view_only = false;
    s.text_edit = None;
    assert_eq!(s.fingerprint(), before, "a read changes nothing");
}

// ---- collaborators ----

#[test]
fn undo_never_reverts_a_collaborators_edit_and_its_inverse_is_a_normal_edit_for_peers() {
    let mut a = testing::headless_session();
    let mut b = testing::headless_session();
    let shared = testing::add_rect(&mut a, 0.0, 0.0, 50.0, 50.0).unwrap();
    let ta = a.tab().id;
    let snapshot = a.tab().doc.replicated().export_snapshot().unwrap();
    b.apply_remote(b.tab().id, &snapshot).unwrap();
    for _ in 0..5 {
        b.ports.ids.element_id();
    }
    let (o, s1) = (Observed::default(), sel(&[&shared]));
    step(&mut a, "Move", &o, &s1, |tx| tx.update(&shared, |e| e.transform.x = 100.0));
    step(&mut b, "Lock", &o, &o, |tx| tx.update(&shared, |e| e.locked = true));
    let bytes = b.tab().doc.replicated().export_updates_since(&a.tab().doc.replicated().version_vector().unwrap()).unwrap();
    a.apply_remote(ta, &bytes).unwrap();
    assert!(a.element(&shared).unwrap().locked);

    let vv_before = b.tab().doc.replicated().version_vector().unwrap();
    ok(&mut a, "history.undo", json!({}));
    let e = a.element(&shared).unwrap();
    assert_eq!(e.transform.x, 0.0, "A's own move is taken back");
    assert!(e.locked, "B's lock stays");
    assert_eq!(refused(&mut a, "history.undo", json!({})).code, "nothing_to_undo", "B's edit is not on A's stack");

    // The undo travels like any edit.
    let bytes = a.tab().doc.replicated().export_updates_since(&vv_before).unwrap();
    b.apply_remote(b.tab().id, &bytes).unwrap();
    assert_eq!(b.element(&shared).unwrap().transform.x, 0.0);
    assert_eq!(b.tab().doc.undo_depth(), 1, "and it is not a step on B's stack");
}

#[test]
fn a_step_that_a_collaborator_already_reverted_is_consumed_and_undo_says_there_is_nothing() {
    let mut a = testing::headless_session();
    let mut b = testing::headless_session();
    let shared = testing::add_rect(&mut a, 0.0, 0.0, 50.0, 50.0).unwrap();
    let ta = a.tab().id;
    let snapshot = a.tab().doc.replicated().export_snapshot().unwrap();
    b.apply_remote(b.tab().id, &snapshot).unwrap();
    let o = Observed::default();
    step(&mut a, "Move", &o, &o, |tx| tx.update(&shared, |e| e.transform.x = 100.0));
    let bytes = a.tab().doc.replicated().export_updates_since(&b.tab().doc.replicated().version_vector().unwrap()).unwrap();
    b.apply_remote(b.tab().id, &bytes).unwrap();
    // B puts it back by hand; A's undo would now change nothing.
    step(&mut b, "Move back", &o, &o, |tx| tx.update(&shared, |e| e.transform.x = 0.0));
    let bytes = b.tab().doc.replicated().export_updates_since(&a.tab().doc.replicated().version_vector().unwrap()).unwrap();
    a.apply_remote(ta, &bytes).unwrap();
    assert_eq!(x_of(&a, &shared), 0.0);
    let r = refused(&mut a, "history.undo", json!({}));
    assert_eq!((r.code, r.message.as_str()), ("nothing_to_undo", "There is nothing to undo in this tab."));
    assert!(!a.tab().doc.can_undo(), "the step that changed nothing was consumed");
}

// ---- dirty ----

fn tab_dirty(s: &mut Session) -> bool {
    let list = ok(s, "tab.list", json!({}));
    list["tabs"][0]["dirty"].as_bool().expect("a dirty flag")
}

#[test]
fn undoing_back_to_the_saved_content_leaves_the_document_clean() {
    let (mut s, _f) = testing::session();
    ok(&mut s, "doc.save", json!({"to": {"path": "/a.json"}}));
    assert!(!s.tab().dirty());
    ok(&mut s, "tab.rename", json!({"name": "Renamed"}));
    assert!(s.tab().dirty() && tab_dirty(&mut s));
    ok(&mut s, "history.undo", json!({}));
    assert_eq!(s.store().meta().name, "Untitled");
    assert!(!s.tab().dirty(), "the content is the saved content again");
    assert!(!tab_dirty(&mut s), "tab.list says so too");
    ok(&mut s, "history.redo", json!({}));
    assert!(s.tab().dirty(), "redo leaves the saved content");
}

#[test]
fn undo_across_a_save_and_a_new_edit_keeps_the_dirty_state_honest() {
    let (mut s, _f) = testing::session();
    ok(&mut s, "doc.save", json!({"to": {"path": "/a.json"}}));
    ok(&mut s, "doc.set_background", json!({"color": "#ff0000"}));
    ok(&mut s, "tab.rename", json!({"name": "One"}));
    // Saved with both edits: undoing one of them is dirty, undoing back to the same point is clean.
    ok(&mut s, "doc.save", json!({}));
    assert!(!s.tab().dirty());
    ok(&mut s, "history.undo", json!({}));
    assert!(s.tab().dirty());
    ok(&mut s, "history.redo", json!({}));
    assert!(!s.tab().dirty(), "back at the saved point");
    // Undo twice is further away than the saved point, and a different edit after an undo is dirty.
    ok(&mut s, "history.undo", json!({"steps": 2}));
    assert!(s.tab().dirty());
    ok(&mut s, "tab.rename", json!({"name": "Two"}));
    ok(&mut s, "history.undo", json!({}));
    assert!(s.tab().dirty(), "not the saved content: the background is still the old one");
}

#[test]
fn a_remote_change_between_an_edit_and_its_undo_keeps_the_document_dirty() {
    let (mut s, _f) = testing::session();
    ok(&mut s, "doc.save", json!({"to": {"path": "/a.json"}}));
    ok(&mut s, "tab.rename", json!({"name": "Mine"}));
    // Something else changes the document (as a collaborator's update does): it is no longer the saved one.
    s.tab_mut()
        .doc
        .edit::<(), StoreError>(|tx| {
            tx.set_background(drafftink_scene::Color::parse("#00ff00").unwrap());
            Ok(())
        })
        .unwrap();
    ok(&mut s, "history.undo", json!({}));
    assert!(s.tab().dirty());
}
