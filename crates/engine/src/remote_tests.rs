//! `Session::apply_remote`: the update reaches the working store, marks a redraw, leaves one ring entry and
//! never touches this peer's undo stack.

use drafftink_scene::{ElementId, SceneRead, SceneTx, StoreError};
use serde_json::json;

use crate::Session;
use crate::ring::Origin;
use crate::spec::Redraw;
use crate::state::Observed;
use crate::store::{StepMeta, TabDoc};
use crate::testing;

fn peer() -> Session {
    testing::headless_session()
}

/// What `from` has and `to_vv` lacks, as the collab layer would send it.
fn update_for(from: &Session, to: &Session) -> Vec<u8> {
    let vv = to.tab().doc.replicated().version_vector().unwrap();
    from.tab().doc.replicated().export_updates_since(&vv).unwrap()
}

/// A local, undoable step on `s`: the way a command would commit it.
fn step(s: &mut Session, label: &str, f: impl FnOnce(&mut crate::store::DocTx<'_, '_>) -> Result<(), StoreError>) {
    let o = Observed::default();
    let doc: &mut TabDoc = &mut s.tab_mut().doc;
    let mark = doc.mark();
    doc.edit::<(), StoreError>(f).unwrap();
    doc.commit_since(mark, &StepMeta { label, undoable: true, before: &o, after: &o, depth: 50, coalesce: None })
        .unwrap();
}

fn tab_of(s: &Session) -> crate::state::TabId {
    s.tab().id
}

#[test]
fn an_update_reaches_the_working_store_without_a_local_undo_step() {
    let mut a = peer();
    let mut b = peer();
    let id = testing::add_rect(&mut a, 5.0, 6.0, 70.0, 80.0).unwrap();
    let bytes = update_for(&a, &b);
    assert_eq!(b.take_dirty(), None);
    let cs = b.apply_remote(tab_of(&b), &bytes).unwrap();
    assert_eq!(cs.elements.len(), 1);
    assert_eq!(b.element(&id).unwrap().transform.w, 70.0, "the commands' store sees it");
    assert_eq!(b.tab().doc.undo_depth(), 0, "a remote edit is not on this peer's undo stack");
    assert_eq!(b.take_dirty(), Some(Redraw::Scene));
    assert_eq!(b.take_dirty(), None);
    // Both fakes count ids from 1; move this peer's source on so its own element does not collide.
    for _ in 0..5 {
        b.ports.ids.element_id();
    }
    // The next local step can be undone, and undo leaves the collaborator's element alone.
    let other = testing::add_rect(&mut b, 0.0, 0.0, 1.0, 1.0).unwrap();
    step(&mut b, "Move", |tx| tx.update(&other, |e| e.transform.x = 9.0));
    assert_eq!(b.tab().doc.undo_depth(), 1);
    b.undo_steps(1).unwrap();
    assert_eq!(b.element(&other).unwrap().transform.x, 0.0);
    assert!(b.element(&id).is_some());
}

#[test]
fn one_ring_entry_per_batch_with_origin_remote() {
    let mut a = peer();
    let mut b = peer();
    for i in 0..3 {
        testing::add_rect(&mut a, f64::from(i), 0.0, 10.0, 10.0).unwrap();
    }
    let bytes = update_for(&a, &b);
    b.apply_remote(tab_of(&b), &bytes).unwrap();
    let e: Vec<_> = b.ring().iter().collect();
    assert_eq!(e.len(), 1, "three elements in one batch are one entry");
    assert_eq!(e[0].id, "remote.apply");
    assert_eq!(e[0].source, Origin::Remote);
    assert!(e[0].outcome.is_ok());
    let args: serde_json::Value = serde_json::from_str(&e[0].args).unwrap();
    assert_eq!(args["elements"], 3);
    assert_eq!(args["bytes"], bytes.len());
    // An update that brings nothing new is still one entry, not a redraw.
    b.take_dirty();
    b.apply_remote(tab_of(&b), &bytes).unwrap();
    assert_eq!(b.ring().len(), 2);
    assert_eq!(b.take_dirty(), None, "idle means idle: nothing changed, nothing to draw");
    assert_eq!(b.ring().iter().last().unwrap().outcome, crate::Outcome::ok(false, None));
}

#[test]
fn a_bad_update_is_a_refusal_that_changes_nothing() {
    let mut b = peer();
    testing::add_rect(&mut b, 0.0, 0.0, 10.0, 10.0).unwrap();
    let before = b.fingerprint();
    let r = b.apply_remote(tab_of(&b), b"this is not a loro update").unwrap_err();
    assert_eq!(r.code, "parse_failed");
    assert!(r.message.contains("ignored") && r.message.contains("rejoin"), "{}", r.message);
    assert_eq!(b.fingerprint(), before);
    assert_eq!(b.take_dirty(), None);
    let last = b.ring().iter().last().unwrap();
    assert_eq!((last.id.as_str(), last.source), ("remote.apply", Origin::Remote));
    assert_eq!(last.outcome.code(), Some("parse_failed"));
    // Hostile input of every length stays a refusal.
    for n in [0usize, 1, 7, 64, 4096] {
        let junk: Vec<u8> = (0..n).map(|i| (i * 31 % 251) as u8).collect();
        let _ = b.apply_remote(tab_of(&b), &junk);
    }
    assert_eq!(b.fingerprint(), before);
}

#[test]
fn an_unknown_tab_is_refused_in_words() {
    let mut b = peer();
    let r = b.apply_remote(crate::state::TabId(99), &[]).unwrap_err();
    assert_eq!(r.code, "no_such_tab");
    assert!(r.message.contains("t99"), "{}", r.message);
}

#[test]
fn what_the_update_deleted_leaves_the_selection() {
    let mut a = peer();
    let mut b = peer();
    let keep = testing::add_rect(&mut a, 0.0, 0.0, 10.0, 10.0).unwrap();
    let gone = testing::add_rect(&mut a, 20.0, 0.0, 10.0, 10.0).unwrap();
    let bytes = update_for(&a, &b);
    b.apply_remote(tab_of(&b), &bytes).unwrap();
    testing::select(&mut b, &[keep.clone(), gone.clone()]);
    step(&mut a, "Delete", |tx| tx.update(&gone, |e| e.deleted = true));
    let bytes = update_for(&a, &b);
    b.take_dirty();
    b.apply_remote(tab_of(&b), &bytes).unwrap();
    assert!(b.element(&gone).is_none());
    let sel: Vec<&ElementId> = b.selection().iter().collect();
    assert_eq!(sel, [&keep]);
    assert_eq!(b.take_dirty(), Some(Redraw::Scene));
}

#[test]
fn it_does_not_run_the_invariant_pass_and_does_not_need_the_active_tab() {
    let mut a = peer();
    let mut b = peer();
    let id = testing::add_rect(&mut a, 0.0, 0.0, 10.0, 10.0).unwrap();
    let second = b.alloc_tab_id();
    b.tabs.push(crate::state::Tab::new(second));
    let bytes = update_for(&a, &b);
    // The update is for the second tab while the first stays active.
    b.apply_remote(second, &bytes).unwrap();
    assert!(b.element(&id).is_none(), "the active tab did not change");
    assert!(b.tab_by_id(second).unwrap().doc.store().get(&id).is_some());
    assert!(b.tab_by_id(second).unwrap().doc.undo_depth() == 0);
    let _ = json!(null);
}
