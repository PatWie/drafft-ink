#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Duplicate, delete, restore, lock.

mod structure_support;

use std::collections::BTreeSet;

use drafftink_edit::CopyError;
use drafftink_edit::group;
use drafftink_edit::lifecycle::{self, ChildPolicy, DuplicateOpts, LifecycleError};
use drafftink_edit::transform;
use drafftink_scene::*;
use kurbo::Vec2;
use structure_support::*;

fn dup(s: &mut MemoryStore, who: &[&str], editing: Option<GroupId>) -> lifecycle::DuplicateReport {
    // Fresh ids that cannot collide with the fixtures.
    let mut src = Prefixed { inner: SeqIds::new() };
    run(s, |tx| {
        lifecycle::duplicate(tx, &ids(who), &DuplicateOpts { offset: Vec2::new(20.0, 20.0), editing }, &mut src)
    })
    .unwrap()
}

/// Ids `n1`, `n2`, ... and groups `ng1`, ... so copies are easy to tell from originals.
struct Prefixed {
    inner: SeqIds,
}

impl IdSource for Prefixed {
    fn element_id(&mut self) -> ElementId {
        id(&format!("n{}", self.inner.element_id().as_str().trim_start_matches('e')))
    }
    fn group_id(&mut self) -> GroupId {
        gid(&format!("n{}", self.inner.group_id()))
    }
    fn seed(&mut self) -> u32 {
        self.inner.seed() + 1000
    }
}

#[test]
fn duplicate_makes_new_ids_seeds_and_offsets() {
    let mut s = scene(vec![rect("a", 0.0, 0.0, 10.0, 10.0), rect("b", 50.0, 0.0, 10.0, 10.0)]);
    run(&mut s, |tx| tx.update(&id("a"), |e| e.name = Some("hero".into()))).unwrap();
    let r = dup(&mut s, &["a"], None);
    assert_eq!(r.copies.len(), 1);
    let new = &r.copies[0].1;
    assert_ne!(*new, id("a"));
    let c = get(&s, new.as_str());
    assert_eq!((c.transform.x, c.transform.y), (20.0, 20.0));
    assert!(c.style.seed >= 1000 && c.style.seed != get(&s, "a").style.seed);
    assert!(c.name.is_none(), "names address one element");
    assert_eq!(get(&s, "a").name.as_deref(), Some("hero"));
    assert_eq!(r.selection, vec![new.clone()]);
    assert_eq!(order(&s), strs(&["a", new.as_str(), "b"]), "directly above the original");
    assert_valid(&s);
}

#[test]
fn old_duplicating_a_group_regenerates_every_id() {
    // The old copy only changed the group id, so copies shared child ids (and renderer cache keys).
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 10.0, 10.0).grouped(&["G"]),
        rect("b", 20.0, 0.0, 10.0, 10.0).grouped(&["G"]),
        rect("c", 40.0, 0.0, 10.0, 10.0),
    ]);
    run(&mut s, |tx| group::set_group_name(tx, &gid("G"), Some("Header"))).unwrap();
    let r = dup(&mut s, &["a", "b"], None);
    let new_ids: BTreeSet<ElementId> = r.copies.iter().map(|(_, n)| n.clone()).collect();
    assert_eq!(new_ids.len(), 2);
    assert!(!new_ids.contains(&id("a")) && !new_ids.contains(&id("b")));
    assert_eq!(r.groups.len(), 1);
    let (old, new) = r.groups[0].clone();
    assert_eq!(old, gid("G"));
    assert_ne!(new, old);
    for (_, n) in &r.copies {
        assert_eq!(get(&s, n.as_str()).group_ids.iter().cloned().collect::<Vec<_>>(), vec![new.clone()]);
    }
    assert_eq!(stack(&s, "a"), strs(&["G"]), "the originals keep their group");
    assert_eq!(group::group_name(&s, &new).as_deref(), Some("Header"), "group names are copied");
    assert_eq!(group::group_name(&s, &gid("G")).as_deref(), Some("Header"));
    assert!(group::non_contiguous_groups(&s).is_empty());
    assert_eq!(order(&s)[..2], strs(&["a", "b"])[..]);
    assert_eq!(order(&s).len(), 5);
    assert_valid(&s);
}

#[test]
fn a_partial_copy_of_a_group_does_not_join_or_claim_it() {
    let mut s =
        scene(vec![rect("a", 0.0, 0.0, 10.0, 10.0).grouped(&["G"]), rect("b", 20.0, 0.0, 10.0, 10.0).grouped(&["G"])]);
    let r = dup(&mut s, &["a"], None);
    let c = get(&s, r.copies[0].1.as_str());
    assert!(c.group_ids.is_empty());
    assert_valid(&s);
}

#[test]
fn duplicating_one_member_does_not_split_its_group() {
    // Found by the property tests: the copy went right above its original, inside the group's range.
    let mut s = scene(vec![
        rect("e0", 0.0, 0.0, 10.0, 10.0).grouped(&["G"]),
        rect("e1", 20.0, 0.0, 10.0, 10.0).grouped(&["G"]),
        rect("x", 40.0, 0.0, 10.0, 10.0),
    ]);
    let r = dup(&mut s, &["e0"], None);
    let copy = r.copies[0].1.to_string();
    assert_eq!(order(&s), strs(&["e0", "e1", &copy, "x"]));
    assert!(group::non_contiguous_groups(&s).is_empty());
}

#[test]
fn inside_an_editing_group_the_copy_stays_in_the_parent_group() {
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 10.0, 10.0).grouped(&["B", "A"]),
        rect("b", 20.0, 0.0, 10.0, 10.0).grouped(&["B", "A"]),
        rect("c", 40.0, 0.0, 10.0, 10.0).grouped(&["A"]),
    ]);
    // Editing A: duplicating the unit B makes a new inner group inside A.
    let r = dup(&mut s, &["a", "b"], Some(gid("A")));
    let copy = get(&s, r.copies[0].1.as_str());
    assert_eq!(copy.group_ids.len(), 2);
    assert_ne!(copy.group_ids[0], gid("B"));
    assert_eq!(copy.group_ids[1], gid("A"), "stays in the editing group");
    // Duplicating c alone while editing A: the copy is another member of A.
    let r = dup(&mut s, &["c"], Some(gid("A")));
    assert_eq!(get(&s, r.copies[0].1.as_str()).group_ids.iter().cloned().collect::<Vec<_>>(), vec![gid("A")]);
    assert_valid(&s);
    assert!(group::non_contiguous_groups(&s).is_empty());
}

#[test]
fn bindings_between_copies_are_remapped_and_others_dropped() {
    let mut ar = arrow("arr", &[(100.0, 25.0), (250.0, 25.0)]);
    bind(&mut ar, true, "t", [1.0, 0.5]);
    bind(&mut ar, false, "far", [0.0, 0.5]);
    let mut s = scene(vec![rect("t", 0.0, 0.0, 100.0, 50.0), rect("far", 250.0, 0.0, 50.0, 50.0), ar]);
    let r = dup(&mut s, &["t", "arr"], None);
    let map: std::collections::BTreeMap<_, _> = r.copies.iter().cloned().collect();
    let copy = get(&s, map[&id("arr")].as_str()).linear_data().unwrap().clone();
    assert_eq!(copy.start_binding.unwrap().target, map[&id("t")], "to the copy of t");
    assert!(copy.end_binding.is_none(), "far was not copied: that end is unbound");
    let orig = get(&s, "arr").linear_data().unwrap().clone();
    assert_eq!(orig.start_binding.unwrap().target, id("t"));
    assert_eq!(orig.end_binding.unwrap().target, id("far"));
    // The arrow alone keeps no bindings.
    let r = dup(&mut s, &["arr"], None);
    let d = get(&s, r.copies[0].1.as_str()).linear_data().unwrap().clone();
    assert!(d.start_binding.is_none() && d.end_binding.is_none());
    assert_valid(&s);
}

#[test]
fn bound_text_is_duplicated_with_its_container() {
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 100.0, 50.0),
        label("la", "a", 10.0, 10.0, 30.0, 10.0),
        rect("z", 300.0, 0.0, 10.0, 10.0),
    ]);
    let r = dup(&mut s, &["a"], None);
    assert_eq!(r.copies.len(), 2);
    let map: std::collections::BTreeMap<_, _> = r.copies.iter().cloned().collect();
    let l = get(&s, map[&id("la")].as_str());
    assert_eq!(l.container_id(), Some(&map[&id("a")]));
    assert_eq!(r.selection, vec![map[&id("a")].clone()], "the label is not selected on its own");
    let o = order(&s);
    assert_eq!(o, strs(&["a", "la", map[&id("a")].as_str(), map[&id("la")].as_str(), "z"]));
    assert_valid(&s);
    // Asking by the label's id copies the container as well.
    let r = dup(&mut s, &["la"], None);
    assert_eq!(r.copies.len(), 2);
}

#[test]
fn copying_a_frame_copies_its_children_and_a_child_alone_leaves_the_frame() {
    let mut s = scene(vec![rect("k", 5.0, 5.0, 10.0, 10.0).in_frame("f"), frame("f", 0.0, 0.0, 100.0, 100.0)]);
    let r = dup(&mut s, &["f"], None);
    let map: std::collections::BTreeMap<_, _> = r.copies.iter().cloned().collect();
    assert_eq!(get(&s, map[&id("k")].as_str()).frame_id, Some(map[&id("f")].clone()));
    assert_eq!(r.selection, vec![map[&id("f")].clone()]);
    let o = order(&s);
    assert!(
        o.iter().position(|x| x == map[&id("k")].as_str()) < o.iter().position(|x| x == map[&id("f")].as_str()),
        "child below its frame"
    );
    assert_valid(&s);
    let r = dup(&mut s, &["k"], None);
    assert!(get(&s, r.copies[0].1.as_str()).frame_id.is_none());
}

#[test]
fn duplicate_refusals_change_nothing() {
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 10.0, 10.0),
        rect("lk", 50.0, 0.0, 10.0, 10.0).locked(),
        rect("edge", 999_999_990.0, 0.0, 5.0, 5.0),
    ]);
    let rev = s.revision();
    let mut src = Prefixed { inner: SeqIds::new() };
    let o = DuplicateOpts { offset: Vec2::new(20.0, 20.0), editing: None };
    assert!(matches!(
        run(&mut s, |tx| lifecycle::duplicate(tx, &ids(&["a", "lk"]), &o, &mut src)),
        Err(LifecycleError::Locked(_))
    ));
    assert!(matches!(run(&mut s, |tx| lifecycle::duplicate(tx, &[], &o, &mut src)), Err(LifecycleError::NoTarget)));
    assert!(matches!(
        run(&mut s, |tx| lifecycle::duplicate(tx, &ids(&["zz"]), &o, &mut src)),
        Err(LifecycleError::NotFound(_))
    ));
    let bad = DuplicateOpts { offset: Vec2::new(f64::NAN, 0.0), editing: None };
    assert!(matches!(
        run(&mut s, |tx| lifecycle::duplicate(tx, &ids(&["a"]), &bad, &mut src)),
        Err(LifecycleError::NotFinite)
    ));
    assert!(matches!(
        run(&mut s, |tx| lifecycle::duplicate(tx, &ids(&["a", "edge"]), &o, &mut src)),
        Err(LifecycleError::Copy(CopyError::Invalid { .. }))
    ));
    assert_eq!(s.revision(), rev);
}

#[test]
fn a_stuck_id_source_is_an_error_not_a_loop() {
    struct Stuck;
    impl IdSource for Stuck {
        fn element_id(&mut self) -> ElementId {
            id("a")
        }
        fn group_id(&mut self) -> GroupId {
            gid("g")
        }
        fn seed(&mut self) -> u32 {
            1
        }
    }
    let mut s = scene(vec![rect("a", 0.0, 0.0, 10.0, 10.0)]);
    let r = run(&mut s, |tx| lifecycle::duplicate(tx, &ids(&["a"]), &DuplicateOpts::default(), &mut Stuck));
    assert!(matches!(r, Err(LifecycleError::Copy(CopyError::IdsExhausted))));
}

#[test]
fn duplicates_stack_directly_above_the_topmost_original() {
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 10.0, 10.0),
        rect("b", 20.0, 0.0, 10.0, 10.0),
        rect("c", 40.0, 0.0, 10.0, 10.0),
    ]);
    let r = dup(&mut s, &["a", "b"], None);
    let map: std::collections::BTreeMap<_, _> = r.copies.iter().cloned().collect();
    assert_eq!(order(&s), strs(&["a", "b", map[&id("a")].as_str(), map[&id("b")].as_str(), "c"]));
}

// ---- delete ----------------------------------------------------------------------------------------------------

#[test]
fn delete_is_soft_and_takes_bound_text_along() {
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 100.0, 50.0),
        label("la", "a", 10.0, 10.0, 30.0, 10.0),
        rect("b", 200.0, 0.0, 10.0, 10.0),
    ]);
    let r = run(&mut s, |tx| lifecycle::delete(tx, &ids(&["a"]), ChildPolicy::Delete)).unwrap();
    assert_eq!(r.deleted, ids(&["a", "la"]));
    assert!(get(&s, "a").deleted && get(&s, "la").deleted);
    assert_eq!(order(&s), strs(&["b"]));
    assert_eq!(s.len_all(), 3, "tombstones stay");
    assert_valid(&s);
    // Deleting again is a no-op; an unknown id is an error.
    let rev = s.revision();
    assert!(run(&mut s, |tx| lifecycle::delete(tx, &ids(&["a"]), ChildPolicy::Delete)).unwrap().deleted.is_empty());
    assert_eq!(s.revision(), rev);
    assert!(matches!(
        run(&mut s, |tx| lifecycle::delete(tx, &ids(&["zz"]), ChildPolicy::Delete)),
        Err(LifecycleError::NotFound(_))
    ));
    assert!(matches!(run(&mut s, |tx| lifecycle::delete(tx, &[], ChildPolicy::Delete)), Err(LifecycleError::NoTarget)));
}

#[test]
fn deleting_only_the_bound_text_keeps_the_container() {
    let mut s = scene(vec![rect("a", 0.0, 0.0, 100.0, 50.0), label("la", "a", 10.0, 10.0, 30.0, 10.0)]);
    run(&mut s, |tx| lifecycle::delete(tx, &ids(&["la"]), ChildPolicy::Delete)).unwrap();
    assert!(!get(&s, "a").deleted && get(&s, "la").deleted);
}

#[test]
fn deleting_a_frame_deletes_or_releases_its_children() {
    let build = || {
        scene(vec![
            rect("k", 5.0, 5.0, 10.0, 10.0).in_frame("f"),
            label("kl", "k", 6.0, 6.0, 4.0, 4.0).in_frame("f"),
            frame("f", 0.0, 0.0, 100.0, 100.0),
        ])
    };
    let mut s = build();
    let r = run(&mut s, |tx| lifecycle::delete(tx, &ids(&["f"]), ChildPolicy::Delete)).unwrap();
    assert_eq!(r.deleted.len(), 3);
    assert!(s.is_empty());
    let mut s = build();
    let r = run(&mut s, |tx| lifecycle::delete(tx, &ids(&["f"]), ChildPolicy::Release)).unwrap();
    assert_eq!(r.deleted, ids(&["f"]));
    assert_eq!(r.released, ids(&["k", "kl"]));
    assert!(get(&s, "k").frame_id.is_none() && !get(&s, "k").deleted);
    assert_eq!(order(&s), strs(&["k", "kl"]));
    assert_valid(&s);
}

#[test]
fn deleting_a_locked_element_or_a_frame_with_a_locked_child_is_refused() {
    let mut s = scene(vec![
        rect("k", 5.0, 5.0, 10.0, 10.0).in_frame("f").locked(),
        frame("f", 0.0, 0.0, 100.0, 100.0),
        rect("lk", 300.0, 0.0, 5.0, 5.0).locked(),
    ]);
    let rev = s.revision();
    assert!(matches!(
        run(&mut s, |tx| lifecycle::delete(tx, &ids(&["lk"]), ChildPolicy::Delete)),
        Err(LifecycleError::Locked(_))
    ));
    assert!(matches!(
        run(&mut s, |tx| lifecycle::delete(tx, &ids(&["f"]), ChildPolicy::Delete)),
        Err(LifecycleError::Locked(_))
    ));
    assert!(matches!(
        run(&mut s, |tx| lifecycle::delete(tx, &ids(&["f"]), ChildPolicy::Release)),
        Err(LifecycleError::Locked(_))
    ));
    assert_eq!(s.revision(), rev);
}

#[test]
fn deleting_a_group_member_leaves_a_stale_id_that_counts_as_no_group() {
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 10.0, 10.0).grouped(&["G"]),
        rect("b", 20.0, 0.0, 10.0, 10.0).grouped(&["G"]),
        rect("c", 40.0, 0.0, 10.0, 10.0),
    ]);
    run(&mut s, |tx| lifecycle::delete(tx, &ids(&["a"]), ChildPolicy::Delete)).unwrap();
    assert_eq!(stack(&s, "b"), strs(&["G"]), "the stale id stays until b is edited");
    assert!(!s.derived().is_group(&gid("G")));
    // b is a plain element now: grouping it with c works and prunes the stale id.
    run(&mut s, |tx| group::group(tx, &ids(&["b", "c"]), None, gid("H"))).unwrap();
    assert_eq!(stack(&s, "b"), strs(&["H"]));
    assert_valid(&s);
}

#[test]
fn restore_brings_back_the_element_and_its_text() {
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 100.0, 50.0),
        label("la", "a", 10.0, 10.0, 30.0, 10.0),
        rect("b", 200.0, 0.0, 10.0, 10.0),
    ]);
    let before = order(&s);
    run(&mut s, |tx| lifecycle::delete(tx, &ids(&["a"]), ChildPolicy::Delete)).unwrap();
    let r = run(&mut s, |tx| lifecycle::restore(tx, &ids(&["a"]))).unwrap();
    assert_eq!(r.restored, ids(&["a", "la"]));
    assert_eq!(order(&s), before, "tombstones kept their z-order keys");
    let rev = s.revision();
    assert!(run(&mut s, |tx| lifecycle::restore(tx, &ids(&["a"]))).unwrap().restored.is_empty());
    assert_eq!(s.revision(), rev);
    assert!(matches!(run(&mut s, |tx| lifecycle::restore(tx, &ids(&["zz"]))), Err(LifecycleError::NotFound(_))));
}

#[test]
fn delete_then_restore_returns_to_the_start() {
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 10.0, 10.0).grouped(&["G"]),
        rect("b", 20.0, 0.0, 10.0, 10.0).grouped(&["G"]),
        rect("c", 40.0, 0.0, 10.0, 10.0),
    ]);
    let before: Vec<Element> = s.iter_all().cloned().collect();
    run(&mut s, |tx| lifecycle::delete(tx, &ids(&["a", "b"]), ChildPolicy::Delete)).unwrap();
    run(&mut s, |tx| lifecycle::restore(tx, &ids(&["a", "b"]))).unwrap();
    let after: Vec<Element> = s.iter_all().cloned().collect();
    for (b, a) in before.iter().zip(&after) {
        assert_eq!(
            (&b.id, &b.transform, &b.group_ids, &b.index, b.deleted),
            (&a.id, &a.transform, &a.group_ids, &a.index, a.deleted)
        );
    }
}

// ---- lock ------------------------------------------------------------------------------------------------------

#[test]
fn locking_locks_the_whole_outermost_group_and_bound_text() {
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 100.0, 50.0).grouped(&["G"]),
        label("la", "a", 10.0, 10.0, 30.0, 10.0).grouped(&["G"]),
        rect("b", 200.0, 0.0, 10.0, 10.0).grouped(&["G"]),
        rect("c", 400.0, 0.0, 10.0, 10.0),
    ]);
    let r = run(&mut s, |tx| lifecycle::lock(tx, &ids(&["b"]), Some(true))).unwrap();
    assert!(r.locked);
    assert_eq!(r.changed.len(), 3);
    assert!(get(&s, "a").locked && get(&s, "la").locked && get(&s, "b").locked && !get(&s, "c").locked);
    assert!(matches!(
        run(&mut s, |tx| transform::move_by(tx, &ids(&["a"]), Vec2::new(1.0, 0.0))),
        Err(transform::TransformError::Locked(_))
    ));
    // Toggle: everything is locked, so it unlocks.
    let r = run(&mut s, |tx| lifecycle::lock(tx, &ids(&["a"]), None)).unwrap();
    assert!(!r.locked);
    assert!(!get(&s, "b").locked);
    // Toggle with a mix locks.
    run(&mut s, |tx| lifecycle::lock(tx, &ids(&["c"]), Some(true))).unwrap();
    let r = run(&mut s, |tx| lifecycle::lock(tx, &ids(&["a", "c"]), None)).unwrap();
    assert!(r.locked);
    assert!(get(&s, "a").locked);
    assert!(matches!(run(&mut s, |tx| lifecycle::lock(tx, &[], None)), Err(LifecycleError::NoTarget)));
    assert_valid(&s);
}

// ---- restore after the structure changed (found by the random-operation test) ------------------------------------

#[test]
fn restore_adopts_the_groups_that_now_surround_its_group() {
    let mut s = scene(vec![
        rect("o1", 0.0, 0.0, 10.0, 10.0).grouped(&["I", "H"]),
        rect("o2", 20.0, 0.0, 10.0, 10.0).grouped(&["I", "H"]),
        rect("o3", 40.0, 0.0, 10.0, 10.0).grouped(&["H"]),
        rect("c", 60.0, 0.0, 10.0, 10.0),
    ]);
    run(&mut s, |tx| lifecycle::delete(tx, &ids(&["o1"]), ChildPolicy::Delete)).unwrap();
    // While o1 is gone, H (o2, o3) is wrapped in N together with c.
    run(&mut s, |tx| group::group(tx, &ids(&["o2", "c"]), None, gid("N"))).unwrap();
    run(&mut s, |tx| lifecycle::restore(tx, &ids(&["o1"]))).unwrap();
    assert_eq!(stack(&s, "o1"), strs(&["H", "N"]), "I has no live member any more; H is inside N now");
    assert!(group::non_contiguous_groups(&s).is_empty());
    assert_valid(&s);
}

#[test]
fn restored_elements_return_next_to_their_group() {
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 10.0, 10.0).grouped(&["G"]),
        rect("b", 20.0, 0.0, 10.0, 10.0).grouped(&["G"]),
        rect("c", 40.0, 0.0, 10.0, 10.0).grouped(&["G"]),
        rect("x", 60.0, 0.0, 10.0, 10.0),
    ]);
    run(&mut s, |tx| lifecycle::delete(tx, &ids(&["a"]), ChildPolicy::Delete)).unwrap();
    run(&mut s, |tx| drafftink_edit::zorder::z_order(tx, &ids(&["b"]), drafftink_edit::zorder::ZOp::Front, None))
        .unwrap();
    assert_eq!(order(&s), strs(&["x", "b", "c"]));
    run(&mut s, |tx| lifecycle::restore(tx, &ids(&["a"]))).unwrap();
    assert!(group::non_contiguous_groups(&s).is_empty());
    assert_eq!(order(&s), strs(&["x", "b", "c", "a"]));
    assert_valid(&s);
}

#[test]
fn restored_bound_text_follows_its_container_and_orphans_stay_deleted() {
    let mut s = scene(vec![
        rect("k", 5.0, 5.0, 10.0, 10.0).in_frame("f"),
        label("kl", "k", 6.0, 6.0, 4.0, 4.0).in_frame("f"),
        frame("f", 0.0, 0.0, 100.0, 100.0),
        frame("w", 200.0, 0.0, 100.0, 100.0),
    ]);
    // The label alone is deleted; the container moves to another frame; then the label comes back.
    run(&mut s, |tx| lifecycle::delete(tx, &ids(&["kl"]), ChildPolicy::Delete)).unwrap();
    run(&mut s, |tx| drafftink_edit::frame::add_to_frame(tx, &id("w"), &ids(&["k"]))).unwrap();
    run(&mut s, |tx| lifecycle::restore(tx, &ids(&["kl"]))).unwrap();
    assert_eq!(get(&s, "kl").frame_id, Some(id("w")));
    let o = order(&s);
    assert!(o.iter().position(|n| n == "k") < o.iter().position(|n| n == "kl"), "text above its container");
    assert_valid(&s);
    assert_eq!(validate_scene(&s).warnings().count(), 0);
    // A label whose container is still deleted stays deleted.
    run(&mut s, |tx| lifecycle::delete(tx, &ids(&["kl", "k"]), ChildPolicy::Delete)).unwrap();
    let r = run(&mut s, |tx| lifecycle::restore(tx, &ids(&["kl"]))).unwrap();
    assert_eq!(r.skipped, ids(&["kl"]));
    assert!(r.restored.is_empty());
    assert!(get(&s, "kl").deleted);
}

#[test]
fn a_restored_child_of_a_deleted_frame_is_released() {
    let mut s = scene(vec![rect("k", 5.0, 5.0, 10.0, 10.0).in_frame("f"), frame("f", 0.0, 0.0, 100.0, 100.0)]);
    run(&mut s, |tx| lifecycle::delete(tx, &ids(&["f"]), ChildPolicy::Delete)).unwrap();
    run(&mut s, |tx| lifecycle::restore(tx, &ids(&["k"]))).unwrap();
    assert!(get(&s, "k").frame_id.is_none());
    assert_valid(&s);
    assert_eq!(validate_scene(&s).warnings().count(), 0);
}

// ---- with the real binding solver ------------------------------------------------------------------------------

fn two_targets() -> MemoryStore {
    let mut a = arrow("arr", &[(100.0, 50.0), (300.0, 50.0)]);
    bind(&mut a, true, "t1", [1.0, 0.5]);
    bind(&mut a, false, "t2", [0.0, 0.5]);
    let mut s = scene(vec![rect("t1", 0.0, 0.0, 100.0, 100.0), rect("t2", 300.0, 0.0, 100.0, 100.0), a]);
    run(&mut s, |tx| drafftink_edit::binding::update_bound_arrows(tx, &ids(&["t1", "t2"]))).unwrap();
    s
}

fn bindings(s: &MemoryStore) -> (Option<ElementId>, Option<ElementId>) {
    let d = get(s, "arr").linear_data().unwrap();
    (d.start_binding.as_ref().map(|b| b.target.clone()), d.end_binding.as_ref().map(|b| b.target.clone()))
}

#[test]
fn deleting_a_target_frees_that_end_where_it_is() {
    let mut s = two_targets();
    let before = drafftink_edit::transform::geometry::world_points(get(&s, "arr")).unwrap();
    let r = run(&mut s, |tx| lifecycle::delete(tx, &ids(&["t1"]), ChildPolicy::Delete)).unwrap();
    assert_eq!(r.freed, ids(&["arr"]));
    assert_eq!(bindings(&s), (None, Some(id("t2"))), "only the end bound to the deleted shape is freed");
    let after = drafftink_edit::transform::geometry::world_points(get(&s, "arr")).unwrap();
    assert_eq!(before, after, "the arrow does not move");
    assert_valid(&s);
    assert_eq!(validate_scene(&s).warnings().count(), 0, "no binding to a tombstone is left");
    // The other target still drives its end.
    run(&mut s, |tx| transform::move_by(tx, &ids(&["t2"]), Vec2::new(0.0, 40.0))).unwrap();
    let moved = drafftink_edit::transform::geometry::world_points(get(&s, "arr")).unwrap();
    assert!((moved[0] - before[0]).hypot() < 1e-9, "the freed start stays");
    assert!(moved[1].y > 50.5, "the end bound to t2 followed it down: {moved:?}");
}

#[test]
fn deleting_a_frame_with_an_arrow_target_inside_frees_the_arrow() {
    let mut a = arrow("arr", &[(30.0, 20.0), (300.0, 20.0)]);
    bind(&mut a, true, "kid", [1.0, 0.5]);
    let mut s = scene(vec![rect("kid", 10.0, 10.0, 20.0, 20.0).in_frame("f"), frame("f", 0.0, 0.0, 100.0, 100.0), a]);
    let r = run(&mut s, |tx| lifecycle::delete(tx, &ids(&["f"]), ChildPolicy::Delete)).unwrap();
    assert_eq!(r.freed, ids(&["arr"]));
    assert_eq!(bindings(&s), (None, None));
}

#[test]
fn restoring_a_target_does_not_bind_a_freed_arrow_again_but_restoring_both_does() {
    // The arrow was freed when its target was deleted: the restored target is just a shape again.
    let mut s = two_targets();
    run(&mut s, |tx| lifecycle::delete(tx, &ids(&["t1"]), ChildPolicy::Delete)).unwrap();
    run(&mut s, |tx| lifecycle::restore(tx, &ids(&["t1"]))).unwrap();
    assert_eq!(bindings(&s), (None, Some(id("t2"))));
    // When the arrow is deleted together with its targets it keeps its bindings, and restoring all of them brings
    // the arrow back attached and re-solved against where the targets are now.
    let mut s = two_targets();
    run(&mut s, |tx| lifecycle::delete(tx, &ids(&["t1", "t2", "arr"]), ChildPolicy::Delete)).unwrap();
    assert_eq!(bindings(&s), (Some(id("t1")), Some(id("t2"))));
    run(&mut s, |tx| tx.update(&id("t1"), |e| e.transform.x -= 50.0)).unwrap();
    let r = run(&mut s, |tx| lifecycle::restore(tx, &ids(&["t1", "t2", "arr"]))).unwrap();
    assert_eq!(r.arrows, ids(&["arr"]), "the solver re-routed the restored arrow");
    assert_eq!(bindings(&s), (Some(id("t1")), Some(id("t2"))));
    let p = drafftink_edit::transform::geometry::world_points(get(&s, "arr")).unwrap();
    let gap = drafftink_geom::binding::binding_gap(get(&s, "arr").style.stroke_width);
    assert!((p[0].x - (50.0 + gap)).abs() < 1e-6, "start follows the moved target: {p:?}");
    assert_valid(&s);
}

#[test]
fn duplicated_pairs_follow_their_own_copies() {
    let mut a = arrow("arr", &[(100.0, 50.0), (300.0, 50.0)]);
    bind(&mut a, true, "t", [1.0, 0.5]);
    let mut s = scene(vec![rect("t", 0.0, 0.0, 100.0, 100.0), a]);
    run(&mut s, |tx| drafftink_edit::binding::update_bound_arrows(tx, &ids(&["t"]))).unwrap();
    let r = dup(&mut s, &["t", "arr"], None);
    let map: std::collections::BTreeMap<_, _> = r.copies.iter().cloned().collect();
    let (ct, ca) = (map[&id("t")].clone(), map[&id("arr")].clone());
    let orig_before = drafftink_edit::transform::geometry::world_points(get(&s, "arr")).unwrap();
    run(&mut s, |tx| transform::move_by(tx, std::slice::from_ref(&ct), Vec2::new(0.0, 100.0))).unwrap();
    let copy = drafftink_edit::transform::geometry::world_points(get(&s, ca.as_str())).unwrap();
    let gap = drafftink_geom::binding::binding_gap(get(&s, ca.as_str()).style.stroke_width);
    assert!(
        (copy[0].x - (120.0 + gap)).abs() < 1e-6 && copy[0].y > 150.0,
        "the copied arrow follows the copied target: {copy:?}"
    );
    assert_eq!(drafftink_edit::transform::geometry::world_points(get(&s, "arr")).unwrap(), orig_before);
}

#[test]
fn a_restored_arrow_is_not_left_bound_to_a_target_that_is_still_deleted() {
    // Found by the random-operation test once deleting a target started to free its arrows.
    let mut s = two_targets();
    run(&mut s, |tx| lifecycle::delete(tx, &ids(&["arr", "t2"]), ChildPolicy::Delete)).unwrap();
    let r = run(&mut s, |tx| lifecycle::restore(tx, &ids(&["arr"]))).unwrap();
    assert_eq!(r.freed, ids(&["arr"]));
    assert_eq!(bindings(&s), (Some(id("t1")), None));
    assert_eq!(validate_scene(&s).warnings().count(), 0);
}
