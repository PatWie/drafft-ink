#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Undo and redo: one step per commit, local only, with the observed editor state.

mod common;

use common::*;
use drafftink_doc::{LoroConfig, LoroStore, ObservedState};
use drafftink_scene::*;

fn state(ids: &[&str], group: Option<&str>) -> ObservedState {
    ObservedState {
        selection: ids.iter().map(|i| eid(i)).collect(),
        editing_group: group.map(gid),
        ..ObservedState::default()
    }
}

fn labeled<R>(
    s: &mut LoroStore,
    label: &str,
    before: ObservedState,
    after: ObservedState,
    f: impl FnOnce(&mut drafftink_doc::LoroTx<'_>) -> R,
) -> R {
    let mut tx = s.begin();
    tx.describe(label, before, after);
    let r = f(&mut tx);
    tx.commit().unwrap();
    r
}

#[test]
fn each_commit_is_one_step_and_undo_redo_round_trip() {
    let mut s = peer(1);
    let z = zs(3);
    add(&mut s, vec![rect("a", &z[0]), rect("b", &z[1]), rect("c", &z[2])]);
    assert_eq!(s.undo_depth(), 1, "a batch of inserts in one commit is one step");
    edit(&mut s, "a", |e| e.transform.x = 5.0);
    edit(&mut s, "a", |e| e.style.stroke_color = Color::parse("#ff0000").unwrap());
    assert_eq!(s.undo_depth(), 3);

    let step = s.undo().unwrap().unwrap();
    assert_eq!(step.changes.get(&eid("a")).unwrap().fields, Fields::STYLE);
    assert_eq!(s.get(&eid("a")).unwrap().style.stroke_color, Style::default().stroke_color);
    assert_eq!(s.get(&eid("a")).unwrap().transform.x, 5.0);
    s.undo().unwrap().unwrap();
    assert_eq!(s.get(&eid("a")).unwrap().transform.x, 10.0);
    s.undo().unwrap().unwrap();
    assert!(s.is_empty(), "undoing the batch removes all three");
    assert!(s.undo().unwrap().is_none());

    assert_eq!(s.redo_depth(), 3);
    s.redo().unwrap().unwrap();
    assert_eq!(s.len(), 3);
    s.redo().unwrap().unwrap();
    s.redo().unwrap().unwrap();
    assert_eq!(s.get(&eid("a")).unwrap().transform.x, 5.0);
    assert_eq!(s.get(&eid("a")).unwrap().style.stroke_color, Color::parse("#ff0000").unwrap());
    assert!(s.redo().unwrap().is_none());
    assert_view_matches_loro(&s);
}

#[test]
fn a_new_commit_clears_the_redo_stack() {
    let mut s = peer(1);
    add(&mut s, vec![rect("a", &zs(1)[0])]);
    edit(&mut s, "a", |e| e.transform.x = 1.0);
    s.undo().unwrap().unwrap();
    assert!(s.can_redo());
    edit(&mut s, "a", |e| e.transform.y = 1.0);
    assert!(!s.can_redo());
}

#[test]
fn undo_never_reverts_a_collaborators_edit() {
    let mut a = peer(1);
    let mut b = peer(2);
    add(&mut a, vec![rect("shared", &zs(1)[0])]);
    sync(&mut a, &mut b);

    edit(&mut a, "shared", |e| e.transform.x = 100.0); // A's step
    edit(&mut b, "shared", |e| e.style.stroke_color = Color::parse("#00ff00").unwrap()); // B's edit
    let top = b.next_top_index().unwrap();
    add(&mut b, vec![rect("b_new", &top)]);
    edit(&mut b, "shared", |e| e.locked = true);
    sync(&mut a, &mut b);

    let step = a.undo().unwrap().unwrap();
    assert_eq!(step.label, "edit");
    let e = a.get(&eid("shared")).unwrap();
    assert_eq!(e.transform.x, 10.0, "A's own move is reverted");
    assert_eq!(e.style.stroke_color, Color::parse("#00ff00").unwrap(), "B's recolor stays");
    assert!(e.locked, "B's lock stays");
    assert!(a.get(&eid("b_new")).is_some(), "B's new element stays");

    // B cannot undo A's work either: B's history has only B's own steps.
    push(&a, &mut b);
    assert_eq!(b.undo_depth(), 3);
    b.undo().unwrap().unwrap(); // un-lock
    b.undo().unwrap().unwrap(); // remove b_new
    b.undo().unwrap().unwrap(); // un-color
    assert!(b.undo().unwrap().is_none(), "A's creation and move are not on B's stack");
    assert_eq!(b.get(&eid("shared")).unwrap().transform.x, 10.0, "A's undone move is not redone by B");
    assert!(b.get(&eid("shared")).is_some());
}

#[test]
fn undoing_a_creation_leaves_a_peers_element_alone_and_redo_brings_it_back() {
    let mut a = peer(1);
    let mut b = peer(2);
    let z = zs(2);
    add(&mut a, vec![rect("from_a", &z[0])]);
    add(&mut b, vec![rect("from_b", &z[1])]);
    sync(&mut a, &mut b);
    let cs = a.undo().unwrap().unwrap().changes;
    assert_eq!(cs.get(&eid("from_a")).unwrap().kind, ChangeKind::Removed);
    assert!(a.get(&eid("from_b")).is_some());
    // The undo is an ordinary edit for B.
    let cs = push(&a, &mut b);
    assert_eq!(cs.get(&eid("from_a")).unwrap().kind, ChangeKind::Removed);
    assert_same(&a, &b);
    a.redo().unwrap().unwrap();
    assert_eq!(a.get(&eid("from_a")).unwrap().transform.w, 100.0);
    push(&a, &mut b);
    assert_same(&a, &b);
    assert_view_matches_loro(&a);
    assert_view_matches_loro(&b);
}

#[test]
fn undo_after_a_peer_overwrote_the_same_field_does_not_clobber_their_value() {
    let mut a = peer(1);
    let mut b = peer(2);
    add(&mut a, vec![rect("r", &zs(1)[0])]);
    sync(&mut a, &mut b);
    edit(&mut a, "r", |e| e.transform.x = 50.0);
    sync(&mut a, &mut b);
    edit(&mut b, "r", |e| e.transform.x = 70.0);
    sync(&mut a, &mut b);
    a.undo().unwrap();
    let after_a = a.get(&eid("r")).unwrap().transform.x;
    push(&a, &mut b);
    assert_eq!(b.get(&eid("r")).unwrap().transform.x, after_a, "replicas agree");
    assert_view_matches_loro(&a);
}

#[test]
fn the_observed_editor_state_is_restored_on_undo_and_redo() {
    let mut s = peer(1);
    add(&mut s, vec![rect("a", &zs(1)[0])]);
    s.clear_history();
    let before = state(&["a"], None);
    let after = state(&["a", "b"], Some("g1"));
    labeled(&mut s, "Move", before.clone(), after.clone(), |tx| {
        tx.update(&eid("a"), |e| e.transform.x = 3.0).unwrap();
    });
    assert_eq!(s.undo_label().as_deref(), Some("Move"));
    assert_eq!(s.undo_restore_state(), Some(before.clone()));
    let u = s.undo().unwrap().unwrap();
    assert_eq!((u.label.as_str(), &u.restore), ("Move", &before));
    assert_eq!(s.redo_label().as_deref(), Some("Move"));
    let r = s.redo().unwrap().unwrap();
    assert_eq!((r.label.as_str(), &r.restore), ("Move", &after));
    assert_eq!(r.changes.get(&eid("a")).unwrap().fields, Fields::POSITION);
}

#[test]
fn steps_keep_their_own_state_across_several_undos() {
    let mut s = peer(1);
    add(&mut s, vec![rect("a", &zs(1)[0])]);
    s.clear_history();
    for (i, label) in ["One", "Two", "Three"].iter().enumerate() {
        let n = i.to_string();
        labeled(&mut s, label, state(&[&n], None), state(&[&n, "x"], None), |tx| {
            tx.update(&eid("a"), |e| e.transform.x = i as f64).unwrap();
        });
    }
    let labels: Vec<String> = (0..3).map(|_| s.undo().unwrap().unwrap().label).collect();
    assert_eq!(labels, ["Three", "Two", "One"]);
    let labels: Vec<String> = (0..3).map(|_| s.redo().unwrap().unwrap().label).collect();
    assert_eq!(labels, ["One", "Two", "Three"]);
}

#[test]
fn depth_is_configurable_and_defaults_to_200() {
    assert_eq!(LoroConfig::default().undo_depth, 200);
    let mut s = LoroStore::with_config(LoroConfig { undo_depth: 5, peer_id: Some(1), ..LoroConfig::default() });
    add(&mut s, vec![rect("a", &zs(1)[0])]);
    for i in 0..12 {
        edit(&mut s, "a", |e| e.transform.x = f64::from(i) + 1000.0);
    }
    assert_eq!(s.undo_depth(), 5);
    let mut n = 0;
    while s.undo().unwrap().is_some() {
        n += 1;
    }
    assert_eq!(n, 5);
    assert_eq!(s.get(&eid("a")).unwrap().transform.x, 1006.0);

    let mut big = peer(2);
    add(&mut big, vec![rect("a", &zs(1)[0])]);
    for i in 0..250 {
        edit(&mut big, "a", |e| e.transform.x = f64::from(i) + 1000.0);
    }
    assert_eq!(big.undo_depth(), 200);
    big.set_undo_depth(3);
    edit(&mut big, "a", |e| e.transform.x = -1.0);
    assert!(big.undo_depth() <= 3, "depth is {}", big.undo_depth());
}

#[test]
fn loading_and_system_commits_are_not_undo_steps() {
    let doc = Document { elements: vec![rect("a", &zs(1)[0])], ..Document::new() };
    let mut s = LoroStore::from_document(&doc).unwrap();
    assert!(!s.can_undo());
    let mut tx = s.begin();
    tx.not_undoable();
    tx.update(&eid("a"), |e| e.transform.x = 9.0).unwrap();
    tx.commit().unwrap();
    assert!(!s.can_undo());
    edit(&mut s, "a", |e| e.transform.y = 9.0);
    assert!(s.can_undo());
    s.undo().unwrap().unwrap();
    assert_eq!(s.get(&eid("a")).unwrap().transform.x, 9.0, "the system commit stays");
}

#[test]
fn undo_covers_text_z_order_meta_and_files() {
    let mut s = peer(1);
    let z = zs(2);
    add(&mut s, vec![text("t", &z[0], "hello"), rect("r", &z[1])]);
    s.clear_history();

    edit(&mut s, "t", |e| {
        if let ElementKind::Text(t) = &mut e.kind {
            t.content = "hello there".into();
            t.runs = vec![TextRun { start: 0, end: 5, color: Some(Color::parse("#ff0000").unwrap()) }];
        }
    });
    let top = s.next_top_index().unwrap();
    edit(&mut s, "t", |e| e.index = top);
    let (fid, entry) = FileEntry::new("image/png", vec![1u8, 2, 3]);
    let mut tx = s.begin();
    tx.set_name("Renamed").unwrap();
    tx.set_background(Color::parse("#000000").unwrap());
    tx.put_file(entry).unwrap();
    tx.commit().unwrap();
    edit(&mut s, "r", |e| e.deleted = true);

    s.undo().unwrap().unwrap();
    assert!(!s.get(&eid("r")).unwrap().deleted);
    s.undo().unwrap().unwrap();
    assert_eq!(s.meta().name, "Untitled");
    assert!(s.file(&fid).is_none());
    assert_eq!(s.meta().background, Color::WHITE);
    s.undo().unwrap().unwrap();
    assert_eq!(order(&s), vec!["t", "r"]);
    s.undo().unwrap().unwrap();
    let t = s.get(&eid("t")).unwrap().text_data().unwrap();
    assert_eq!((t.content.as_str(), t.runs.len()), ("hello", 0));
    assert_view_matches_loro(&s);
    for _ in 0..4 {
        s.redo().unwrap().unwrap();
    }
    assert_eq!(s.get(&eid("t")).unwrap().text_data().unwrap().content, "hello there");
    assert!(s.get(&eid("r")).unwrap().deleted);
    assert!(s.file(&fid).is_some());
    assert_view_matches_loro(&s);
}

#[test]
fn undo_updates_derived_indexes_and_versions_and_notifies_subscribers() {
    let mut s = peer(1);
    let z = zs(3);
    let mut ar = arrow("ar", &z[2]);
    if let ElementKind::Arrow(d) = &mut ar.kind {
        d.end_binding = Some(Binding { target: eid("r"), mode: BindMode::Orbit, fixed_point: [0.5, 0.5] });
    }
    add(&mut s, vec![rect("r", &z[0]), ar]);
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::<u64>::new()));
    let sink = seen.clone();
    s.subscribe(Box::new(move |cs| sink.borrow_mut().push(cs.revision)));
    edit(&mut s, "ar", |e| {
        if let ElementKind::Arrow(d) = &mut e.kind {
            d.end_binding = None;
        }
    });
    assert_eq!(s.derived().bound_arrows(&eid("r")).count(), 0);
    let v = s.get(&eid("ar")).unwrap().version;
    s.undo().unwrap().unwrap();
    assert_eq!(s.derived().bound_arrows(&eid("r")).count(), 1);
    assert_eq!(s.get(&eid("ar")).unwrap().version, v + 1);
    assert_eq!(seen.borrow().len(), 2);
    assert_view_matches_loro(&s);
}

#[test]
fn an_undo_is_exported_like_any_edit() {
    let mut a = peer(1);
    let mut b = peer(2);
    add(&mut a, vec![rect("r", &zs(1)[0])]);
    edit(&mut a, "r", |e| e.transform.x = 33.0);
    sync(&mut a, &mut b);
    let vv = b.version_vector();
    a.undo().unwrap().unwrap();
    let bytes = a.export_updates_since(&vv).unwrap();
    let cs = b.apply_remote(&bytes).unwrap();
    assert_eq!(cs.get(&eid("r")).unwrap().fields, Fields::POSITION);
    assert_eq!(b.get(&eid("r")).unwrap().transform.x, 10.0);
}
