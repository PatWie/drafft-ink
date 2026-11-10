#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! History size, trimming and what it means for undo and for late or stale peers.

mod common;

use common::*;
use drafftink_doc::{DocError, LoroStore, TrimOptions};
use drafftink_scene::*;

/// A store with a long history: 100 elements, then 300 small edits.
fn busy() -> LoroStore {
    let mut s = peer(1);
    let z = zs(100);
    add(&mut s, (0..100).map(|i| rect(&format!("e{i:03}"), &z[i])).collect());
    for i in 0..300 {
        edit(&mut s, &format!("e{:03}", i % 100), |e| e.transform.x = f64::from(i));
    }
    s
}

#[test]
fn history_grows_with_edits_and_trimming_reclaims_it() {
    let mut s = busy();
    let before = s.history_stats().unwrap();
    assert!(before.snapshot_bytes > before.state_only_bytes, "{before:?}");
    assert!(!before.shallow);
    let want = content(&s);
    let report = s.trim_history(&TrimOptions::default()).unwrap();
    assert!(report.bytes_after < report.bytes_before, "{report:?}");
    let after = s.history_stats().unwrap();
    assert!(after.shallow);
    assert!(after.ops < before.ops);
    assert_eq!(content(&s), want, "trimming never changes the scene");
    assert_view_matches_loro(&s);
    // The store keeps working.
    edit(&mut s, "e001", |e| e.locked = true);
    assert!(s.get(&eid("e001")).unwrap().locked);
}

#[test]
fn trimming_clears_the_undo_stacks() {
    let mut s = busy();
    assert!(s.can_undo());
    s.trim_history(&TrimOptions::default()).unwrap();
    assert!(!s.can_undo() && !s.can_redo());
    edit(&mut s, "e001", |e| e.locked = true);
    s.undo().unwrap().unwrap();
    assert!(!s.get(&eid("e001")).unwrap().locked, "undo works again for new steps");
}

#[test]
fn history_stays_bounded_when_trimmed_periodically() {
    let mut s = peer(1);
    add(&mut s, vec![rect("r", &zs(1)[0])]);
    let mut sizes = Vec::new();
    for round in 0..5 {
        for i in 0..400 {
            edit(&mut s, "r", |e| e.transform.x = f64::from(round * 1000 + i));
        }
        s.trim_history(&TrimOptions::default()).unwrap();
        sizes.push(s.export_snapshot().unwrap().len());
    }
    let (first, last) = (sizes[0], sizes[4]);
    assert!(last <= first + first / 10, "snapshot sizes after each trim: {sizes:?}");
}

#[test]
fn trimming_refuses_while_local_changes_are_unacknowledged() {
    let mut s = busy();
    let acked = s.version_vector();
    edit(&mut s, "e001", |e| e.locked = true);
    let err = s.trim_history(&TrimOptions { keep_commits: 0, synced_to: Some(acked) }).unwrap_err();
    assert_eq!(err, DocError::UnsyncedChanges);
    let acked = s.version_vector();
    s.trim_history(&TrimOptions { keep_commits: 0, synced_to: Some(acked) }).unwrap();
}

#[test]
fn a_late_joiner_can_take_a_trimmed_snapshot_and_keep_collaborating() {
    let mut server = busy();
    server.trim_history(&TrimOptions::default()).unwrap();
    let shallow = server.export_shallow_snapshot(0).unwrap();
    let full_len = busy().export_snapshot().unwrap().len();
    assert!(shallow.len() * 10 < full_len * 7, "{} vs {}", shallow.len(), full_len);

    let mut joiner =
        LoroStore::from_snapshot_with(&shallow, drafftink_doc::LoroConfig { peer_id: Some(7), ..Default::default() })
            .unwrap();
    assert_same(&server, &joiner);
    // Both directions of sync work from here on.
    edit(&mut server, "e010", |e| e.transform.y = 5.0);
    push(&server, &mut joiner);
    edit(&mut joiner, "e011", |e| e.transform.y = 6.0);
    push(&joiner, &mut server);
    assert_same(&server, &joiner);
    assert_view_matches_loro(&joiner);
}

#[test]
fn a_peer_older_than_the_trim_gets_an_error_and_recovers_from_a_snapshot() {
    let mut server = busy();
    let mut stale = LoroStore::from_snapshot_with(
        &server.export_snapshot().unwrap(),
        drafftink_doc::LoroConfig { peer_id: Some(2), ..Default::default() },
    )
    .unwrap();
    // The server moves on, then trims everything.
    for i in 0..50 {
        edit(&mut server, "e000", |e| e.transform.y = f64::from(i));
    }
    server.trim_history(&TrimOptions::default()).unwrap();
    edit(&mut server, "e001", |e| e.transform.y = -1.0);

    let err = server.export_updates_since(&stale.version_vector()).unwrap_err();
    assert_eq!(err, DocError::HistoryTrimmed);

    // A current peer is still served with plain updates.
    let current = LoroStore::from_snapshot(&server.export_shallow_snapshot(0).unwrap()).unwrap();
    edit(&mut server, "e002", |e| e.transform.y = -2.0);
    assert!(server.export_updates_since(&current.version_vector()).is_ok());

    // The stale peer replaces its content from a snapshot and is told what changed.
    let cs = stale.replace_from_snapshot(&server.export_shallow_snapshot(0).unwrap()).unwrap();
    assert!(cs.get(&eid("e000")).is_some() && cs.get(&eid("e001")).is_some());
    assert_same(&server, &stale);
    assert!(!stale.can_undo());
    edit(&mut stale, "e050", |e| e.locked = true);
    push(&stale, &mut server);
    assert!(server.get(&eid("e050")).unwrap().locked);
}

#[test]
fn keep_commits_keeps_recent_history_so_slightly_behind_peers_still_get_updates() {
    let mut s = busy();
    let none = s.export_shallow_snapshot(0).unwrap();
    let some = s.export_shallow_snapshot(50).unwrap();
    let all = s.export_snapshot().unwrap();
    assert!(none.len() < some.len() && some.len() < all.len(), "{} {} {}", none.len(), some.len(), all.len());
    assert_same(&s, &LoroStore::from_snapshot(&some).unwrap());

    let cfg = |p| drafftink_doc::LoroConfig { peer_id: Some(p), ..Default::default() };
    let mut far = LoroStore::from_snapshot_with(&s.export_snapshot().unwrap(), cfg(10)).unwrap();
    for i in 0..60 {
        edit(&mut s, "e005", |e| e.transform.y = f64::from(i));
    }
    let mut near = LoroStore::from_snapshot_with(&s.export_snapshot().unwrap(), cfg(11)).unwrap();
    for i in 0..30 {
        edit(&mut s, "e006", |e| e.transform.y = f64::from(i));
    }
    s.trim_history(&TrimOptions { keep_commits: 50, synced_to: None }).unwrap();
    edit(&mut s, "e007", |e| e.locked = true);

    push(&s, &mut near);
    assert_same(&s, &near);
    assert_eq!(s.export_updates_since(&far.version_vector()).unwrap_err(), DocError::HistoryTrimmed);
    far.replace_from_snapshot(&s.export_shallow_snapshot(0).unwrap()).unwrap();
    assert_same(&s, &far);
}

#[test]
fn a_trimmed_snapshot_given_to_an_empty_replica_is_simply_taken() {
    let server = busy();
    let shallow = server.export_shallow_snapshot(0).unwrap();
    let mut fresh = peer(4);
    let cs = fresh.apply_remote(&shallow).unwrap();
    assert_eq!(cs.elements.len(), 100);
    assert_same(&server, &fresh);
    assert_view_matches_loro(&fresh);
    // Same for the smallest form.
    let mut fresh = peer(5);
    fresh.apply_remote(&server.export_state_only().unwrap()).unwrap();
    assert_same(&server, &fresh);
}

#[test]
fn a_trimmed_snapshot_is_never_silently_dropped_by_a_replica_with_its_own_work() {
    let server = busy();
    let shallow = server.export_shallow_snapshot(0).unwrap();
    let mut mine = peer(6);
    add(&mut mine, vec![rect("mine", &zs(1)[0])]);
    assert_eq!(mine.apply_remote(&shallow).unwrap_err(), DocError::SnapshotStartsLater);
    assert!(mine.get(&eid("mine")).is_some() && mine.len() == 1, "nothing changed");
    // The caller chooses: replace (my element is gone) ...
    let cs = mine.replace_from_snapshot(&shallow).unwrap();
    assert_eq!(cs.get(&eid("mine")).unwrap().kind, ChangeKind::Removed);
    assert_same(&server, &mine);
}

#[test]
fn a_trimmed_snapshot_merges_into_a_replica_that_has_the_history_it_starts_from() {
    let server = busy();
    let mut other = peer(3);
    other.apply_remote(&server.export_snapshot().unwrap()).unwrap();
    let shallow = server.export_shallow_snapshot(0).unwrap();
    assert!(other.apply_remote(&shallow).unwrap().is_empty());
    assert_view_matches_loro(&other);
    assert_same(&server, &other);
}
