#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Z-order: front, back, forward, backward, with groups, frames, bound text, tombstones and key growth.

mod structure_support;

use drafftink_edit::TargetError;
use drafftink_edit::group;
use drafftink_edit::lifecycle::{self, ChildPolicy};
use drafftink_edit::zorder::{self, REBALANCE_LEN, ZOp, ZOrderError};
use drafftink_scene::*;
use structure_support::*;

fn plain(names: &[&str]) -> MemoryStore {
    scene(names.iter().enumerate().map(|(i, n)| rect(n, i as f64 * 20.0, 0.0, 10.0, 10.0)).collect())
}

fn z(s: &mut MemoryStore, who: &[&str], op: ZOp) -> zorder::ZReport {
    run(s, |tx| zorder::z_order(tx, &ids(who), op, None)).unwrap()
}

#[test]
fn old_front_and_back() {
    // Ported from the old `test_z_order`: two shapes, bring the first to the front and send it back.
    let mut s = plain(&["id1", "id2"]);
    z(&mut s, &["id1"], ZOp::Front);
    assert_eq!(order(&s), strs(&["id2", "id1"]));
    z(&mut s, &["id1"], ZOp::Back);
    assert_eq!(order(&s), strs(&["id1", "id2"]));
}

#[test]
fn forward_and_backward_step_one_unit() {
    let mut s = plain(&["a", "b", "c", "d"]);
    z(&mut s, &["b"], ZOp::Forward);
    assert_eq!(order(&s), strs(&["a", "c", "b", "d"]));
    z(&mut s, &["b"], ZOp::Backward);
    z(&mut s, &["b"], ZOp::Backward);
    assert_eq!(order(&s), strs(&["b", "a", "c", "d"]));
}

#[test]
fn already_at_the_end_is_an_empty_report() {
    let mut s = plain(&["a", "b", "c"]);
    let rev = s.revision();
    assert!(!z(&mut s, &["c"], ZOp::Front).changed());
    assert!(!z(&mut s, &["c"], ZOp::Forward).changed());
    assert!(!z(&mut s, &["a"], ZOp::Back).changed());
    assert!(!z(&mut s, &["a"], ZOp::Backward).changed());
    assert_eq!(s.revision(), rev);
}

#[test]
fn only_the_moved_elements_get_new_keys() {
    let mut s = plain(&["a", "b", "c", "d"]);
    let keys: Vec<ZIndex> = ["b", "c", "d"].iter().map(|n| get(&s, n).index.clone()).collect();
    let r = z(&mut s, &["a"], ZOp::Front);
    assert_eq!(r.moved, ids(&["a"]));
    assert!(!r.rebalanced);
    let after: Vec<ZIndex> = ["b", "c", "d"].iter().map(|n| get(&s, n).index.clone()).collect();
    assert_eq!(keys, after);
    assert_eq!(order(&s), strs(&["b", "c", "d", "a"]));
}

#[test]
fn a_selection_keeps_its_relative_order_and_moves_together() {
    let mut s = plain(&["a", "b", "c", "d", "e"]);
    z(&mut s, &["d", "b"], ZOp::Front);
    assert_eq!(order(&s), strs(&["a", "c", "e", "b", "d"]));
    z(&mut s, &["b", "d"], ZOp::Back);
    assert_eq!(order(&s), strs(&["b", "d", "a", "c", "e"]));
    let mut s = plain(&["a", "b", "c", "d", "e"]);
    z(&mut s, &["a", "b"], ZOp::Forward);
    assert_eq!(order(&s), strs(&["c", "a", "b", "d", "e"]));
    z(&mut s, &["b", "d"], ZOp::Backward);
    assert_eq!(order(&s), strs(&["c", "b", "d", "a", "e"]));
}

fn grouped() -> MemoryStore {
    scene(vec![
        rect("x", 0.0, 0.0, 10.0, 10.0),
        rect("g1", 20.0, 0.0, 10.0, 10.0).grouped(&["G"]),
        rect("g2", 40.0, 0.0, 10.0, 10.0).grouped(&["G"]),
        rect("y", 60.0, 0.0, 10.0, 10.0),
    ])
}

#[test]
fn a_group_jumps_over_a_unit_as_a_whole() {
    let mut s = grouped();
    z(&mut s, &["x"], ZOp::Forward);
    assert_eq!(order(&s), strs(&["g1", "g2", "x", "y"]), "x jumps the whole group");
    z(&mut s, &["y"], ZOp::Backward);
    assert_eq!(order(&s), strs(&["g1", "g2", "y", "x"]));
    z(&mut s, &["y"], ZOp::Backward);
    assert_eq!(order(&s), strs(&["y", "g1", "g2", "x"]), "y jumps the whole group backwards");
}

#[test]
fn moving_one_member_at_root_level_moves_the_whole_group() {
    let mut s = grouped();
    z(&mut s, &["g1"], ZOp::Front);
    assert_eq!(order(&s), strs(&["x", "y", "g1", "g2"]));
    assert!(group::non_contiguous_groups(&s).is_empty());
    z(&mut s, &["g2"], ZOp::Back);
    assert_eq!(order(&s), strs(&["g1", "g2", "x", "y"]));
}

#[test]
fn inside_an_editing_group_moves_stay_in_the_groups_range() {
    let mut s = scene(vec![
        rect("g1", 0.0, 0.0, 10.0, 10.0).grouped(&["G"]),
        rect("g2", 20.0, 0.0, 10.0, 10.0).grouped(&["G"]),
        rect("g3", 40.0, 0.0, 10.0, 10.0).grouped(&["G"]),
        rect("y", 60.0, 0.0, 10.0, 10.0),
    ]);
    let g = gid("G");
    run(&mut s, |tx| zorder::z_order(tx, &ids(&["g1"]), ZOp::Front, Some(&g))).unwrap();
    assert_eq!(order(&s), strs(&["g2", "g3", "g1", "y"]), "front within G, still below y");
    run(&mut s, |tx| zorder::z_order(tx, &ids(&["g1"]), ZOp::Forward, Some(&g))).unwrap();
    assert_eq!(order(&s), strs(&["g2", "g3", "g1", "y"]), "already the top of G");
    run(&mut s, |tx| zorder::z_order(tx, &ids(&["g1"]), ZOp::Back, Some(&g))).unwrap();
    assert_eq!(order(&s), strs(&["g1", "g2", "g3", "y"]));
}

#[test]
fn inside_an_editing_group_the_next_level_down_is_the_unit() {
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 10.0, 10.0).grouped(&["B", "A"]),
        rect("b", 20.0, 0.0, 10.0, 10.0).grouped(&["B", "A"]),
        rect("c", 40.0, 0.0, 10.0, 10.0).grouped(&["A"]),
    ]);
    run(&mut s, |tx| zorder::z_order(tx, &ids(&["a"]), ZOp::Forward, Some(&gid("A")))).unwrap();
    assert_eq!(order(&s), strs(&["c", "a", "b"]), "B (a, b) jumps over c as one unit");
}

fn framed() -> MemoryStore {
    scene(vec![
        rect("k1", 5.0, 5.0, 10.0, 10.0).in_frame("f"),
        rect("k2", 25.0, 5.0, 10.0, 10.0).in_frame("f"),
        frame("f", 0.0, 0.0, 100.0, 100.0),
        rect("o", 200.0, 0.0, 10.0, 10.0),
    ])
}

#[test]
fn a_frame_and_its_children_move_as_one_block() {
    let mut s = framed();
    z(&mut s, &["o"], ZOp::Backward);
    assert_eq!(order(&s), strs(&["o", "k1", "k2", "f"]), "o steps under the whole frame block");
    z(&mut s, &["f"], ZOp::Front);
    assert_eq!(order(&s), strs(&["o", "k1", "k2", "f"]));
    z(&mut s, &["o"], ZOp::Front);
    assert_eq!(order(&s), strs(&["k1", "k2", "f", "o"]));
    z(&mut s, &["f"], ZOp::Forward);
    assert_eq!(order(&s), strs(&["o", "k1", "k2", "f"]));
    z(&mut s, &["f"], ZOp::Back);
    assert_eq!(order(&s), strs(&["k1", "k2", "f", "o"]), "the frame block goes to the bottom with its children");
}

#[test]
fn children_move_among_their_siblings_and_stay_below_the_frame() {
    let mut s = framed();
    z(&mut s, &["k1"], ZOp::Front);
    assert_eq!(order(&s), strs(&["k2", "k1", "f", "o"]));
    z(&mut s, &["k1"], ZOp::Forward);
    assert_eq!(order(&s), strs(&["k2", "k1", "f", "o"]), "cannot pass the frame");
    z(&mut s, &["k1"], ZOp::Backward);
    assert_eq!(order(&s), strs(&["k1", "k2", "f", "o"]));
    assert_valid(&s);
}

#[test]
fn bound_text_travels_with_its_container() {
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 100.0, 50.0),
        label("la", "a", 10.0, 10.0, 30.0, 10.0),
        rect("b", 200.0, 0.0, 10.0, 10.0),
    ]);
    z(&mut s, &["a"], ZOp::Forward);
    assert_eq!(order(&s), strs(&["b", "a", "la"]));
    z(&mut s, &["la"], ZOp::Back);
    assert_eq!(order(&s), strs(&["a", "la", "b"]), "asking for the text moves the container");
}

#[test]
fn tombstones_are_stepped_over_and_keep_their_place() {
    let mut s = plain(&["a", "dead", "b", "c"]);
    run(&mut s, |tx| lifecycle::delete(tx, &ids(&["dead"]), ChildPolicy::Delete)).unwrap();
    z(&mut s, &["a"], ZOp::Forward);
    assert_eq!(order(&s), strs(&["b", "a", "c"]), "one visible step");
    assert_eq!(order_all(&s), strs(&["dead", "b", "a", "c"]));
    assert_valid(&s);
}

#[test]
fn locked_and_unknown_elements_are_refused() {
    let mut s = scene(vec![rect("a", 0.0, 0.0, 10.0, 10.0).locked(), rect("b", 20.0, 0.0, 10.0, 10.0)]);
    let rev = s.revision();
    assert!(matches!(
        run(&mut s, |tx| zorder::z_order(tx, &ids(&["a"]), ZOp::Front, None)),
        Err(ZOrderError::Target(TargetError::Locked(_)))
    ));
    assert!(matches!(
        run(&mut s, |tx| zorder::z_order(tx, &ids(&["zz"]), ZOp::Front, None)),
        Err(ZOrderError::Target(TargetError::NotFound(_)))
    ));
    assert_eq!(s.revision(), rev);
}

#[test]
fn keys_stay_in_order_and_long_keys_are_rebalanced() {
    // Build a scene whose keys are long, then force a re-key between two long neighbours.
    let lo = ZIndex::first();
    let mut hi = ZIndex::between(Some(&lo), None).unwrap();
    let mut keys = vec![hi.clone()];
    for _ in 0..400 {
        hi = ZIndex::between(Some(&lo), Some(&hi)).unwrap();
        keys.push(hi.clone());
    }
    keys.push(lo);
    keys.reverse();
    assert!(keys.iter().any(|k| k.as_str().len() > REBALANCE_LEN));
    let mut store = MemoryStore::new();
    run(&mut store, |tx| {
        for (i, k) in keys.iter().enumerate() {
            let mut e = rect(&format!("e{i}"), i as f64, 0.0, 1.0, 1.0);
            e.index = k.clone();
            tx.insert(e).unwrap();
        }
    });
    let before = order(&store);
    let r = run(&mut store, |tx| zorder::z_order(tx, &ids(&["e2"]), ZOp::Forward, None)).unwrap();
    assert!(r.rebalanced, "long neighbours trigger a rebalance");
    assert!(store.iter_all().all(|e| e.index.as_str().len() <= REBALANCE_LEN));
    let mut expect = before.clone();
    let e2 = expect.remove(2);
    expect.insert(3, e2);
    assert_eq!(order(&store), expect);
    assert_valid(&store);
    // An explicit rebalance keeps the order.
    let before = order(&store);
    run(&mut store, |tx| zorder::rebalance(tx)).unwrap();
    assert_eq!(order(&store), before);
}

#[test]
fn repeated_steps_keep_a_valid_order() {
    let mut s = plain(&["a", "b", "c", "d", "e", "f"]);
    for i in 0..200 {
        let who = ["b", "e"][i % 2];
        let op = [ZOp::Forward, ZOp::Backward, ZOp::Front, ZOp::Back][(i / 2) % 4];
        run(&mut s, |tx| zorder::z_order(tx, &ids(&[who]), op, None)).unwrap();
        let keys: Vec<&ZIndex> = s.iter_all().map(|e| &e.index).collect();
        assert!(keys.windows(2).all(|w| w[0] < w[1]), "strictly increasing after step {i}");
    }
    assert_eq!(s.len(), 6);
    assert_valid(&s);
}
