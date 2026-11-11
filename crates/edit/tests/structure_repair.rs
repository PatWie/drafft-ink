#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! `drafftink_edit::repair`: the fixes for what a merge of concurrent edits leaves broken. Each rule on its own,
//! on a store that holds the broken state, and each one twice: the second run must write nothing.

mod structure_support;

use drafftink_edit::repair::{self, Released};
use drafftink_scene::*;
use structure_support::*;

/// Everything in the store as "added": what a merge of a whole scene reports.
fn all_changed(s: &MemoryStore) -> Vec<Change> {
    s.iter_all()
        .map(|e| Change { id: e.id.clone(), kind: ChangeKind::Added, fields: Fields::all(), version: e.version })
        .collect()
}

/// Runs `rule` on the whole store, then again, and returns the first run's result. The second must change nothing.
fn twice<R>(s: &mut MemoryStore, rule: impl Fn(&mut <MemoryStore as SceneStore>::Tx<'_>, &[Change]) -> R) -> R {
    let changed = all_changed(s);
    let first = run(s, |tx| rule(tx, &changed));
    let rev = s.revision();
    let before = snapshot(s);
    run(s, |tx| rule(tx, &changed));
    assert_eq!(snapshot(s), before, "the second run changed the scene");
    assert_eq!(s.revision(), rev, "the second run committed");
    first
}

fn with_index(mut e: Element, index: &ZIndex) -> Element {
    e.index = index.clone();
    e
}

#[test]
fn an_arrow_end_bound_to_a_deleted_or_missing_shape_is_freed_and_the_other_end_stays() {
    let mut a = arrow("arr", &[(0.0, 0.0), (200.0, 0.0)]);
    bind(&mut a, true, "gone", [0.5, 0.5]);
    bind(&mut a, false, "dead", [0.5, 0.5]);
    let mut keep = arrow("keep", &[(0.0, 100.0), (200.0, 100.0)]);
    bind(&mut keep, true, "alive", [0.5, 0.5]);
    let mut s = scene(vec![rect("alive", 0.0, 90.0, 20.0, 20.0), rect("dead", 190.0, -10.0, 20.0, 20.0), a, keep]);
    run(&mut s, |tx| tx.update(&id("dead"), |e| e.deleted = true)).unwrap();
    // "gone" never existed in this store: a hard removal elsewhere.
    let released: Released = twice(&mut s, |tx, c| repair::dangling_references(tx, c).unwrap());
    assert_eq!(released.arrow_ends, ids(&["arr"]));
    let ElementKind::Arrow(d) = &get(&s, "arr").kind else { panic!("an arrow") };
    assert!(d.start_binding.is_none() && d.end_binding.is_none());
    let ElementKind::Arrow(k) = &get(&s, "keep").kind else { panic!("an arrow") };
    assert!(k.start_binding.is_some(), "a binding to a live shape stays");
    assert!(!validate_scene(&s).has_errors());
}

#[test]
fn labels_without_a_usable_container_become_free_text_and_the_smallest_id_keeps_a_shared_container() {
    let mut s = scene(vec![
        rect("box", 0.0, 0.0, 100.0, 50.0),
        label("l2", "box", 10.0, 10.0, 20.0, 10.0),
        label("l1", "box", 10.0, 20.0, 20.0, 10.0),
        rect("dead", 200.0, 0.0, 100.0, 50.0),
        label("orphan", "dead", 210.0, 10.0, 20.0, 10.0),
        label("lost", "nowhere", 300.0, 10.0, 20.0, 10.0),
    ]);
    run(&mut s, |tx| tx.update(&id("dead"), |e| e.deleted = true)).unwrap();
    let released = twice(&mut s, |tx, c| repair::dangling_references(tx, c).unwrap());
    assert_eq!(released.labels, ids(&["l2", "lost", "orphan"]));
    assert_eq!(get(&s, "l1").container_id(), Some(&id("box")));
    for free in ["l2", "orphan", "lost"] {
        let e = get(&s, free);
        assert_eq!(e.container_id(), None, "{free}");
        assert!(!e.deleted, "{free} stays as free text");
    }
}

#[test]
fn a_child_of_a_missing_deleted_or_wrong_frame_leaves_it_and_a_label_follows_its_container() {
    let mut s = scene(vec![
        rect("a", 10.0, 10.0, 10.0, 10.0).in_frame("nowhere"),
        rect("b", 10.0, 30.0, 10.0, 10.0).in_frame("c"),
        rect("c", 10.0, 50.0, 10.0, 10.0),
        frame("f", 0.0, 0.0, 100.0, 100.0),
        frame("dead", 300.0, 0.0, 100.0, 100.0),
        rect("d", 310.0, 10.0, 10.0, 10.0).in_frame("dead"),
        rect("box", 20.0, 20.0, 30.0, 30.0).in_frame("f").grouped(&["g1", "g2"]),
        label("lab", "box", 25.0, 25.0, 10.0, 10.0),
    ]);
    run(&mut s, |tx| tx.update(&id("dead"), |e| e.deleted = true)).unwrap();
    let released = twice(&mut s, |tx, c| repair::dangling_references(tx, c).unwrap());
    assert_eq!(released.frame_children, ids(&["a", "b", "d", "lab"]));
    for n in ["a", "b", "d"] {
        assert_eq!(get(&s, n).frame_id, None, "{n}");
    }
    assert_eq!(get(&s, "lab").frame_id, Some(id("f")));
    assert_eq!(stack(&s, "lab"), strs(&["g1", "g2"]));
}

#[test]
fn groups_that_disagree_about_the_groups_around_them_are_dissolved_smallest_first() {
    let mut s = scene(vec![
        rect("x", 0.0, 0.0, 10.0, 10.0).grouped(&["g1", "g2"]),
        rect("y", 20.0, 0.0, 10.0, 10.0).grouped(&["g1"]),
        rect("z", 40.0, 0.0, 10.0, 10.0).grouped(&["g2"]),
    ]);
    let dissolved = twice(&mut s, |tx, c| repair::inconsistent_groups(tx, c).unwrap());
    assert_eq!(dissolved, [gid("g1")]);
    assert_eq!(stack(&s, "x"), strs(&["g2"]));
    assert!(stack(&s, "y").is_empty());
    assert!(!validate_scene(&s).has_errors());
}

#[test]
fn equal_z_order_keys_become_distinct_without_changing_the_order_readers_see() {
    let k = ZIndex::first();
    let next = ZIndex::between(Some(&k), None).unwrap();
    let mut s = MemoryStore::new();
    run(&mut s, |tx| {
        for (name, key) in [("c", &k), ("a", &k), ("b", &k), ("top", &next)] {
            tx.insert(with_index(rect(name, 0.0, 0.0, 10.0, 10.0), key)).unwrap();
        }
    });
    let before = order_all(&s);
    assert_eq!(before, strs(&["a", "b", "c", "top"]), "ties read by id");
    let rekeyed = twice(&mut s, |tx, c| repair::tied_order_keys(tx, c).unwrap());
    assert_eq!(rekeyed, ids(&["b", "c"]));
    assert_eq!(order_all(&s), before);
    assert_eq!(get(&s, "a").index, k, "the smallest id keeps the key");
    assert_eq!(validate_scene(&s).warnings().count(), 0, "no tie is left");
}

#[test]
fn a_child_above_its_frame_and_a_label_below_its_container_move_back_into_place() {
    let mut s = scene(vec![
        frame("f", 0.0, 0.0, 100.0, 100.0),
        rect("kid", 10.0, 10.0, 10.0, 10.0).in_frame("f"),
        rect("box", 40.0, 40.0, 20.0, 20.0),
        label("lab", "box", 45.0, 45.0, 10.0, 5.0),
        rect("top", 200.0, 0.0, 10.0, 10.0),
    ]);
    // Concurrent reorders: the label to the back, the frame to the back (so the kid is above it).
    let bottom = ZIndex::between(None, Some(&get(&s, "f").index)).unwrap();
    run(&mut s, |tx| tx.update(&id("lab"), |e| e.index = bottom.clone())).unwrap();
    let below_all = ZIndex::between(None, Some(&bottom)).unwrap();
    run(&mut s, |tx| tx.update(&id("f"), |e| e.index = below_all.clone())).unwrap();
    assert!(validate_scene(&s).warnings().count() >= 2);
    let moved = twice(&mut s, |tx, c| repair::children_below_frame(tx, c).unwrap());
    assert_eq!(moved, ids(&["kid"]));
    let moved = twice(&mut s, |tx, c| repair::labels_above_container(tx, c).unwrap());
    assert_eq!(moved, ids(&["lab"]));
    assert_eq!(validate_scene(&s).warnings().count(), 0);
}
