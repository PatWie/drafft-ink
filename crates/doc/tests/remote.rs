#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Remote updates are applied incrementally: only the touched elements are decoded and patched.

mod common;

use std::cell::Cell;
use std::rc::Rc;

use common::*;
use drafftink_doc::LoroStore;
use drafftink_scene::*;

const N: usize = 3000;

/// Two synced replicas holding N elements.
fn big_pair() -> (LoroStore, LoroStore) {
    let mut a = peer(1);
    let z = zs(N);
    add(&mut a, (0..N).map(|i| rect(&format!("e{i:05}"), &z[i])).collect());
    let b = LoroStore::from_snapshot_with(
        &a.export_snapshot().unwrap(),
        drafftink_doc::LoroConfig { peer_id: Some(2), ..Default::default() },
    )
    .unwrap();
    (a, b)
}

fn id_of(i: usize) -> String {
    format!("e{i:05}")
}

#[test]
fn a_single_element_update_touches_a_single_element() {
    let (mut a, mut b) = big_pair();
    assert_eq!(b.len(), N);
    edit(&mut a, &id_of(1234), |e| e.transform.x = 999.0);
    let bytes = a.export_updates_since(&b.version_vector().unwrap()).unwrap();
    b.reset_stats();
    let rev = b.revision();
    let versions: Vec<u64> = b.iter_all().map(|e| e.version).collect();

    let cs = b.apply_remote(&bytes).unwrap();

    assert_eq!(cs.elements.len(), 1);
    let c = cs.get(&eid(&id_of(1234))).unwrap();
    assert_eq!((c.kind, c.fields), (ChangeKind::Updated, Fields::POSITION));
    let st = b.stats();
    assert_eq!(st.elements_decoded, 1, "work is proportional to the change");
    assert_eq!((st.full_rebuilds, st.remote_updates), (0, 1));
    assert_eq!(b.revision(), rev + 1);
    assert_eq!(b.get(&eid(&id_of(1234))).unwrap().transform.x, 999.0);
    // Nobody else was touched.
    let after: Vec<u64> = b.iter_all().map(|e| e.version).collect();
    assert_eq!(after.iter().zip(&versions).filter(|(x, y)| x != y).count(), 1);
    assert_view_matches_loro(&b);
}

#[test]
fn work_follows_the_number_of_changed_elements_not_the_scene_size() {
    let (mut a, mut b) = big_pair();
    let mut tx = a.begin();
    for i in [3usize, 900, 2999] {
        tx.update(&eid(&id_of(i)), |e| e.style.opacity = 0.5).unwrap();
    }
    tx.remove(&eid(&id_of(10))).unwrap();
    let top = tx.top_z().map(|z| ZIndex::between(Some(&z), None).unwrap()).unwrap();
    tx.insert(rect("fresh", &top)).unwrap();
    tx.commit().unwrap();
    let bytes = a.export_updates_since(&b.version_vector().unwrap()).unwrap();
    b.reset_stats();
    let cs = b.apply_remote(&bytes).unwrap();
    assert_eq!(cs.elements.len(), 5);
    assert_eq!(b.stats().elements_decoded, 5);
    assert_eq!(cs.get(&eid("fresh")).unwrap().kind, ChangeKind::Added);
    assert_eq!(cs.get(&eid(&id_of(10))).unwrap().kind, ChangeKind::Removed);
    assert!(cs.order_changed());
    assert_same(&a, &b);
}

#[test]
fn text_meta_and_file_updates_do_not_decode_other_elements() {
    let (mut a, mut b) = big_pair();
    let top = a.next_top_index().unwrap();
    add(&mut a, vec![text("t", &top, "abc")]);
    push(&a, &mut b);

    edit(&mut a, "t", |e| {
        if let ElementKind::Text(t) = &mut e.kind {
            t.content = "abcd".into();
        }
    });
    b.reset_stats();
    let cs = push(&a, &mut b);
    assert_eq!(cs.elements.len(), 1);
    assert_eq!(b.stats().elements_decoded, 1);

    let mut tx = a.begin();
    tx.set_name("Renamed").unwrap();
    let (fid, entry) = FileEntry::new("image/png", vec![5u8; 64]);
    tx.put_file(entry).unwrap();
    tx.commit().unwrap();
    b.reset_stats();
    let cs = push(&a, &mut b);
    assert!(cs.elements.is_empty() && cs.meta && cs.files == vec![fid]);
    assert_eq!(b.stats().elements_decoded, 0);
}

#[test]
fn applying_the_same_update_twice_changes_nothing() {
    let (mut a, mut b) = big_pair();
    edit(&mut a, &id_of(5), |e| e.locked = true);
    let bytes = a.export_updates_since(&b.version_vector().unwrap()).unwrap();
    assert_eq!(b.apply_remote(&bytes).unwrap().elements.len(), 1);
    b.reset_stats();
    let rev = b.revision();
    let cs = b.apply_remote(&bytes).unwrap();
    assert!(cs.is_empty());
    assert_eq!(b.stats().elements_decoded, 0);
    assert_eq!(b.revision(), rev);
}

#[test]
fn derived_indexes_follow_remote_changes_without_a_rebuild() {
    let (mut a, mut b) = big_pair();
    let top = a.next_top_index().unwrap();
    let mut ar = arrow("ar", &top);
    if let ElementKind::Arrow(d) = &mut ar.kind {
        d.start_binding = Some(Binding { target: eid(&id_of(7)), mode: BindMode::Orbit, fixed_point: [0.5, 0.5] });
    }
    add(&mut a, vec![ar]);
    edit(&mut a, &id_of(8), |e| e.group_ids = smallvec::smallvec![gid("g")]);
    edit(&mut a, &id_of(9), |e| e.group_ids = smallvec::smallvec![gid("g")]);
    push(&a, &mut b);
    assert_eq!(b.derived().bound_arrows(&eid(&id_of(7))).count(), 1);
    assert!(b.derived().is_group(&gid("g")));
    assert_view_matches_loro(&b);
}

#[test]
fn subscribers_hear_each_remote_batch_once() {
    let (mut a, mut b) = big_pair();
    let calls = Rc::new(Cell::new(0));
    let c = calls.clone();
    let id = b.subscribe(Box::new(move |cs| {
        assert!(!cs.is_empty());
        c.set(c.get() + 1);
    }));
    for i in 0..3 {
        edit(&mut a, &id_of(i), |e| e.transform.y = 1.0);
    }
    push(&a, &mut b);
    assert_eq!(calls.get(), 1, "three commits in one update are one notification");
    edit(&mut a, &id_of(4), |e| e.transform.y = 1.0);
    push(&a, &mut b);
    assert_eq!(calls.get(), 2);
    assert!(b.unsubscribe(id));
    edit(&mut a, &id_of(5), |e| e.transform.y = 1.0);
    push(&a, &mut b);
    assert_eq!(calls.get(), 2);
}

#[test]
fn take_unsent_update_sends_each_commit_once() {
    let mut a = peer(1);
    let mut b = peer(2);
    add(&mut a, vec![rect("r", &zs(1)[0])]);
    let u1 = a.take_unsent_update().unwrap().unwrap();
    assert!(a.take_unsent_update().unwrap().is_none());
    edit(&mut a, "r", |e| e.transform.x = 2.0);
    let u2 = a.take_unsent_update().unwrap().unwrap();
    b.apply_remote(&u1).unwrap();
    b.apply_remote(&u2).unwrap();
    assert_same(&a, &b);
    assert!(u2.len() < u1.len() + 200);
}

#[test]
fn a_full_snapshot_applied_to_an_empty_replica_adds_everything() {
    let (a, _) = big_pair();
    let mut fresh = peer(9);
    let cs = fresh.apply_remote(&a.export_snapshot().unwrap()).unwrap();
    assert_eq!(cs.elements.len(), N);
    assert!(cs.elements.iter().all(|c| c.kind == ChangeKind::Added));
    assert_same(&a, &fresh);
}

#[cfg(feature = "profile")]
#[test]
fn the_profile_work_counter_sees_one_op_for_one_changed_element() {
    use drafftink_profile::{Counter, WorkScope};
    let (mut a, mut b) = big_pair();
    edit(&mut a, &id_of(1), |e| e.transform.x = 3.0);
    let bytes = a.export_updates_since(&b.version_vector().unwrap()).unwrap();
    let scope = WorkScope::begin();
    b.apply_remote(&bytes).unwrap();
    assert_eq!(scope.count(Counter::SyncOpsApplied), 1);
}

/// Regression: operations that arrived from a peer are not "unsent". The sync client calls `take_unsent_update`
/// after every commit, and handing remote operations back would echo every remote edit to the server.
#[test]
fn remote_operations_are_not_handed_back_as_unsent() {
    let mut a = peer(1);
    let mut b = peer(2);
    let z = zs(2);
    add(&mut a, vec![rect("a1", &z[0])]);
    let from_a = a.take_unsent_update().unwrap().unwrap();
    let b_start = b.take_unsent_update().unwrap().unwrap();
    b.apply_remote(&from_a).unwrap();
    assert_eq!(b.take_unsent_update().unwrap(), None, "a remote update must not come back as unsent");

    // A local commit after a remote one still exports, and a third replica converges with it.
    add(&mut b, vec![rect("b1", &z[1])]);
    let from_b = b.take_unsent_update().unwrap().unwrap();
    let mut c = peer(3);
    c.apply_remote(&from_a).unwrap();
    c.apply_remote(&b_start).unwrap();
    c.apply_remote(&from_b).unwrap();
    assert_same(&b, &c);
    assert_eq!(c.take_unsent_update().unwrap().map(|_| ()), Some(()), "c's own schema operations were never sent");
}

// ---- a big import in slices ----

/// A config that stages every import of 1 byte or more.
fn staging(peer_id: u64) -> drafftink_doc::LoroConfig {
    drafftink_doc::LoroConfig { peer_id: Some(peer_id), stage_import_bytes: 1, ..Default::default() }
}

fn empty_staging(peer_id: u64) -> LoroStore {
    LoroStore::with_config(staging(peer_id)).unwrap()
}

#[test]
fn a_big_import_into_an_empty_replica_fills_the_view_in_slices_and_ends_equal_to_a_plain_import() {
    let (a, mut plain) = big_pair();
    let bytes = a.export_snapshot().unwrap();
    let mut b = empty_staging(7);

    let first = b.apply_remote(&bytes).unwrap();

    // The CRDT has everything at once, but the view fills slice by slice.
    assert!(first.is_empty(), "nothing is decoded by the call itself");
    assert_eq!(b.len(), 0);
    let progress = b.import_progress().expect("the import is staged");
    assert_eq!(progress.done, 0);
    assert!(progress.total >= N, "{progress:?}");
    // The CRDT is complete: a peer that asks for what `b` lacks of `a`'s history gets nothing new to apply.
    assert!(a.export_updates_since(&b.version_vector().unwrap()).is_ok());
    assert_eq!(
        b.debug_json()["elements"].as_object().map(serde_json::Map::len),
        Some(N),
        "the CRDT holds every element"
    );
    b.reset_stats();

    let mut slices = 0;
    let mut seen = 0;
    while b.import_progress().is_some() {
        let cs = b.import_step(500).unwrap();
        slices += 1;
        assert!(cs.elements.len() <= 500, "a slice decodes at most what it was given");
        assert_eq!(b.len(), seen + cs.elements.len());
        seen += cs.elements.len();
        assert!(slices < 100, "the import never ends");
    }
    assert!(slices >= N / 500, "{slices} slices");
    assert_eq!(b.stats().elements_decoded as usize, N, "every element is decoded once");
    assert_eq!(b.len(), N);
    assert_same(&b, &a);
    assert_view_matches_loro(&b);

    // The plain path gives the same document.
    plain.reset_stats();
    assert_same(&b, &plain);
    // Nothing the import brought is handed back as an unsent update (a blank replica's own first write is).
    if let Some(unsent) = b.take_unsent_update().unwrap() {
        let mut c = LoroStore::from_snapshot(&a.export_snapshot().unwrap()).unwrap();
        assert!(c.apply_remote(&unsent).unwrap().elements.is_empty(), "no element travels back");
    }
}

#[test]
fn an_import_into_a_replica_that_has_content_is_not_staged() {
    let (a, _) = big_pair();
    let mut b = empty_staging(7);
    let z = zs(1);
    add(&mut b, vec![rect("mine", &z[0])]);
    let cs = b.apply_remote(&a.export_snapshot().unwrap()).unwrap();
    assert!(b.import_progress().is_none());
    assert_eq!(cs.elements.len(), N, "the whole import was applied by the call");
    assert_eq!(b.len(), N + 1);
}

#[test]
fn a_big_import_is_applied_at_once_unless_staging_was_asked_for() {
    let (a, _) = big_pair();
    let mut b = LoroStore::with_config(drafftink_doc::LoroConfig { peer_id: Some(7), ..Default::default() }).unwrap();
    let bytes = a.export_snapshot().unwrap();
    assert!(!b.would_stage(bytes.len()), "off by default");
    b.set_stage_import_bytes(1);
    assert!(b.would_stage(bytes.len()));
    b.set_stage_import_bytes(bytes.len() + 1);
    assert!(!b.would_stage(bytes.len()), "below the threshold");
    b.set_stage_import_bytes(usize::MAX);
    assert_eq!(b.apply_remote(&bytes).unwrap().elements.len(), N);
    assert!(b.import_progress().is_none());
}

#[test]
fn a_small_import_is_not_staged() {
    let mut a = peer(1);
    let z = zs(10);
    add(&mut a, (0..10).map(|i| rect(&format!("e{i:05}"), &z[i])).collect());
    let config = drafftink_doc::LoroConfig { peer_id: Some(7), stage_import_bytes: 256 * 1024, ..Default::default() };
    let mut b = LoroStore::with_config(config).unwrap();
    let cs = b.apply_remote(&a.export_snapshot().unwrap()).unwrap();
    assert!(b.import_progress().is_none(), "below the threshold");
    assert_eq!(cs.elements.len(), 10);
}

#[test]
fn edits_and_updates_between_slices_are_kept() {
    let (mut a, _) = big_pair();
    let mut b = empty_staging(7);
    b.apply_remote(&a.export_snapshot().unwrap()).unwrap();
    b.import_step(1000).unwrap();

    // A local edit between two slices.
    let z = zs(1);
    add(&mut b, vec![rect("mine", &z[0])]);
    // A live update from a peer, for an element that is loaded and for one that is not yet.
    edit(&mut a, &id_of(0), |e| e.transform.x = 123.0);
    edit(&mut a, &id_of(N - 1), |e| e.transform.x = 456.0);
    let update = a.export_updates_since(&b.version_vector().unwrap()).unwrap();
    b.apply_remote(&update).unwrap();
    while b.import_progress().is_some() {
        b.import_step(1000).unwrap();
    }

    assert_eq!(b.get(&eid(&id_of(0))).unwrap().transform.x, 123.0);
    assert_eq!(b.get(&eid(&id_of(N - 1))).unwrap().transform.x, 456.0);
    assert!(b.get(&eid("mine")).is_some());
    assert_eq!(b.len(), N + 1);
    assert_view_matches_loro(&b);
    // The local element is sent; the imported ones are not.
    let unsent = b.take_unsent_update().unwrap().expect("the local edit is unsent");
    let mut c = LoroStore::with_config(drafftink_doc::LoroConfig { peer_id: Some(9), ..Default::default() }).unwrap();
    c.apply_remote(&a.export_snapshot().unwrap()).unwrap();
    c.apply_remote(&unsent).unwrap();
    assert!(c.get(&eid("mine")).is_some());
}

#[test]
fn a_step_without_a_staged_import_does_nothing() {
    let mut b = empty_staging(7);
    assert!(b.import_progress().is_none());
    assert!(b.import_step(100).unwrap().is_empty());
}
