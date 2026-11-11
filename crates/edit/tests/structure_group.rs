#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Group, ungroup, nesting, names. Regression tests for the weaknesses of the old grouping are named `old_*`.

mod structure_support;

use drafftink_edit::group::{self, GroupError};
use drafftink_scene::*;
use structure_support::*;

fn four() -> MemoryStore {
    scene(vec![
        rect("a", 0.0, 0.0, 10.0, 10.0),
        rect("b", 20.0, 0.0, 10.0, 10.0),
        rect("c", 40.0, 0.0, 10.0, 10.0),
        rect("d", 60.0, 0.0, 10.0, 10.0),
    ])
}

#[test]
fn group_adds_one_new_outermost_id_and_keeps_element_ids() {
    let mut s = four();
    let r = run(&mut s, |tx| group::group(tx, &ids(&["a", "b"]), None, gid("g1"))).unwrap();
    assert_eq!(r.group, gid("g1"));
    assert_eq!(r.members, ids(&["a", "b"]));
    assert_eq!(stack(&s, "a"), strs(&["g1"]));
    assert_eq!(stack(&s, "b"), strs(&["g1"]));
    assert!(stack(&s, "c").is_empty());
    assert_eq!(order(&s), strs(&["a", "b", "c", "d"]));
    assert_valid(&s);
}

#[test]
fn group_with_ids_takes_an_id_only_when_the_grouping_succeeds() {
    let mut s = four();
    let mut source = SeqIds::new();
    assert!(matches!(
        run(&mut s, |tx| group::group_with_ids(tx, &ids(&["a"]), None, &mut source)),
        Err(GroupError::TooFew { .. })
    ));
    assert!(matches!(
        run(&mut s, |tx| group::group_with_ids(tx, &ids(&["a", "zz"]), None, &mut source)),
        Err(GroupError::NotFound(_))
    ));
    let first = run(&mut s, |tx| group::group_with_ids(tx, &ids(&["a", "b"]), None, &mut source)).unwrap();
    assert_eq!(first.group, SeqIds::new().group_id(), "the refused calls drew no id");
    let second = run(&mut s, |tx| group::group_with_ids(tx, &ids(&["c", "d"]), None, &mut source)).unwrap();
    assert_ne!(second.group, first.group);
}

#[test]
fn old_non_contiguous_selection_keeps_everything_else_in_order() {
    // The old app put the group at the front slot and mis-counted removed shapes.
    let mut s = four();
    run(&mut s, |tx| group::group(tx, &ids(&["a", "c"]), None, gid("g1"))).unwrap();
    // Members join at the slot of the topmost member (c); b ends up below them.
    assert_eq!(order(&s), strs(&["b", "a", "c", "d"]));
    assert!(group::non_contiguous_groups(&s).is_empty());
    assert_eq!(s.len(), 4);
    assert_valid(&s);
}

#[test]
fn only_the_pulled_member_gets_a_new_key() {
    let mut s = four();
    let before: Vec<ZIndex> = ["b", "c", "d"].iter().map(|n| get(&s, n).index.clone()).collect();
    let r = run(&mut s, |tx| group::group(tx, &ids(&["a", "c"]), None, gid("g1"))).unwrap();
    assert_eq!(r.z.moved, ids(&["a"]));
    let after: Vec<ZIndex> = ["b", "c", "d"].iter().map(|n| get(&s, n).index.clone()).collect();
    assert_eq!(before, after);
}

#[test]
fn old_failed_grouping_changes_nothing() {
    let mut s = four();
    let rev = s.revision();
    assert!(matches!(
        run(&mut s, |tx| group::group(tx, &ids(&["a"]), None, gid("g1"))),
        Err(GroupError::TooFew { have: 1 })
    ));
    assert!(matches!(
        run(&mut s, |tx| group::group(tx, &ids(&["a", "zz"]), None, gid("g1"))),
        Err(GroupError::NotFound(_))
    ));
    assert_eq!(s.revision(), rev);
}

#[test]
fn nested_groups_wrap_the_old_one() {
    let mut s = four();
    run(&mut s, |tx| group::group(tx, &ids(&["a", "b"]), None, gid("g1"))).unwrap();
    run(&mut s, |tx| group::group(tx, &ids(&["a", "c"]), None, gid("g2"))).unwrap();
    // a is in g1; selecting a at root level means the unit g1, so g2 wraps g1 and c.
    assert_eq!(stack(&s, "a"), strs(&["g1", "g2"]));
    assert_eq!(stack(&s, "b"), strs(&["g1", "g2"]));
    assert_eq!(stack(&s, "c"), strs(&["g2"]));
    assert_eq!(order(&s), strs(&["a", "b", "c", "d"]));
    assert!(group::non_contiguous_groups(&s).is_empty());
    assert_valid(&s);
}

#[test]
fn grouping_inside_an_editing_group_sits_just_inside_it() {
    let mut s = four();
    run(&mut s, |tx| group::group(tx, &ids(&["a", "b", "c"]), None, gid("outer"))).unwrap();
    run(&mut s, |tx| group::group(tx, &ids(&["a", "b"]), Some(&gid("outer")), gid("inner"))).unwrap();
    assert_eq!(stack(&s, "a"), strs(&["inner", "outer"]));
    assert_eq!(stack(&s, "b"), strs(&["inner", "outer"]));
    assert_eq!(stack(&s, "c"), strs(&["outer"]));
    assert_valid(&s);
}

#[test]
fn editing_group_must_contain_the_elements() {
    let mut s = four();
    run(&mut s, |tx| group::group(tx, &ids(&["a", "b"]), None, gid("g1"))).unwrap();
    let r = run(&mut s, |tx| group::group(tx, &ids(&["c", "d"]), Some(&gid("g1")), gid("g2")));
    assert!(matches!(r, Err(GroupError::NotInEditingGroup(_))));
}

#[test]
fn already_grouped_and_too_few() {
    let mut s = four();
    run(&mut s, |tx| group::group(tx, &ids(&["a", "b"]), None, gid("g1"))).unwrap();
    // Both elements of g1 at root level are one unit.
    assert!(matches!(
        run(&mut s, |tx| group::group(tx, &ids(&["a", "b"]), None, gid("g2"))),
        Err(GroupError::AlreadyGrouped)
    ));
    assert!(matches!(
        run(&mut s, |tx| group::group(tx, &ids(&["a"]), None, gid("g2"))),
        Err(GroupError::AlreadyGrouped)
    ));
    assert!(matches!(
        run(&mut s, |tx| group::group(tx, &ids(&["c"]), None, gid("g2"))),
        Err(GroupError::TooFew { have: 1 })
    ));
}

#[test]
fn group_id_in_use_is_refused() {
    let mut s = four();
    run(&mut s, |tx| group::group(tx, &ids(&["a", "b"]), None, gid("g1"))).unwrap();
    assert!(matches!(
        run(&mut s, |tx| group::group(tx, &ids(&["c", "d"]), None, gid("g1"))),
        Err(GroupError::GroupIdInUse(_))
    ));
}

#[test]
fn locked_elements_refuse_the_whole_call() {
    let mut s = scene(vec![rect("a", 0.0, 0.0, 1.0, 1.0), rect("b", 5.0, 0.0, 1.0, 1.0).locked()]);
    let rev = s.revision();
    let r = run(&mut s, |tx| group::group(tx, &ids(&["a", "b"]), None, gid("g1")));
    assert!(matches!(r, Err(GroupError::Locked(ref v)) if v == &ids(&["b"])));
    assert_eq!(s.revision(), rev);
}

#[test]
fn frame_and_its_own_child_cannot_be_grouped() {
    let mut s = scene(vec![rect("kid", 5.0, 5.0, 10.0, 10.0).in_frame("f"), frame("f", 0.0, 0.0, 100.0, 100.0)]);
    let r = run(&mut s, |tx| group::group(tx, &ids(&["f", "kid"]), None, gid("g1")));
    assert!(matches!(r, Err(GroupError::FrameAndChildren { .. })));
}

#[test]
fn members_from_different_frames_leave_their_frames() {
    let mut s = scene(vec![
        rect("a", 5.0, 5.0, 10.0, 10.0).in_frame("f1"),
        frame("f1", 0.0, 0.0, 50.0, 50.0),
        rect("b", 205.0, 5.0, 10.0, 10.0).in_frame("f2"),
        frame("f2", 200.0, 0.0, 50.0, 50.0),
    ]);
    let r = run(&mut s, |tx| group::group(tx, &ids(&["a", "b"]), None, gid("g1"))).unwrap();
    assert_eq!(r.left_frames.len(), 2);
    assert!(get(&s, "a").frame_id.is_none() && get(&s, "b").frame_id.is_none());
    assert_valid(&s);
}

#[test]
fn members_of_one_frame_keep_it() {
    let mut s = scene(vec![
        rect("a", 5.0, 5.0, 10.0, 10.0).in_frame("f"),
        rect("b", 25.0, 5.0, 10.0, 10.0).in_frame("f"),
        frame("f", 0.0, 0.0, 50.0, 50.0),
    ]);
    run(&mut s, |tx| group::group(tx, &ids(&["a", "b"]), None, gid("g1"))).unwrap();
    assert_eq!(get(&s, "a").frame_id, Some(id("f")));
}

#[test]
fn bound_text_carries_the_container_groups_and_stays_above_it() {
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 100.0, 50.0),
        label("la", "a", 10.0, 10.0, 50.0, 20.0),
        rect("b", 200.0, 0.0, 100.0, 50.0),
    ]);
    run(&mut s, |tx| group::group(tx, &ids(&["a", "b"]), None, gid("g1"))).unwrap();
    assert_eq!(stack(&s, "la"), strs(&["g1"]));
    assert_eq!(order(&s), strs(&["a", "la", "b"]));
    run(&mut s, |tx| group::ungroup(tx, &ids(&["a", "b"]), None, None)).unwrap();
    assert!(stack(&s, "la").is_empty());
}

#[test]
fn selecting_the_bound_text_groups_the_container() {
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 100.0, 50.0),
        label("la", "a", 10.0, 10.0, 50.0, 20.0),
        rect("b", 200.0, 0.0, 10.0, 10.0),
    ]);
    let r = run(&mut s, |tx| group::group(tx, &ids(&["la", "b"]), None, gid("g1"))).unwrap();
    assert_eq!(r.members, ids(&["a", "b"]));
}

#[test]
fn ungroup_removes_only_the_selected_level() {
    let mut s = four();
    run(&mut s, |tx| group::group(tx, &ids(&["a", "b"]), None, gid("g1"))).unwrap();
    run(&mut s, |tx| group::group(tx, &ids(&["a", "c"]), None, gid("g2"))).unwrap();
    let order_before = order(&s);
    let r = run(&mut s, |tx| group::ungroup(tx, &ids(&["a", "b", "c"]), None, None)).unwrap();
    assert_eq!(r.removed, vec![gid("g2")]);
    assert_eq!(stack(&s, "a"), strs(&["g1"]));
    assert_eq!(stack(&s, "c"), Vec::<String>::new());
    assert_eq!(order(&s), order_before, "ungroup never reorders");
    assert_valid(&s);
}

#[test]
fn ungroup_by_group_id_and_without_group() {
    let mut s = four();
    run(&mut s, |tx| group::group(tx, &ids(&["a", "b"]), None, gid("g1"))).unwrap();
    // c and d are not grouped: nothing to ungroup.
    assert!(matches!(run(&mut s, |tx| group::ungroup(tx, &ids(&["c", "d"]), None, None)), Err(GroupError::NoGroup)));
    assert!(matches!(run(&mut s, |tx| group::ungroup(tx, &[], Some(&gid("nope")), None)), Err(GroupError::NoGroup)));
    run(&mut s, |tx| group::ungroup(tx, &[], Some(&gid("g1")), None)).unwrap();
    assert!(stack(&s, "a").is_empty());
}

#[test]
fn ungroup_inside_an_editing_group_dissolves_the_inner_group() {
    let mut s = four();
    run(&mut s, |tx| group::group(tx, &ids(&["a", "b", "c"]), None, gid("outer"))).unwrap();
    run(&mut s, |tx| group::group(tx, &ids(&["a", "b"]), Some(&gid("outer")), gid("inner"))).unwrap();
    // With the editing group "outer", a and b are the unit "inner".
    run(&mut s, |tx| group::ungroup(tx, &ids(&["a", "b"]), None, Some(&gid("outer")))).unwrap();
    assert_eq!(stack(&s, "a"), strs(&["outer"]));
}

#[test]
fn group_then_ungroup_restores_stacks_and_keeps_the_new_order() {
    let mut s = four();
    run(&mut s, |tx| group::group(tx, &ids(&["a", "c"]), None, gid("g1"))).unwrap();
    run(&mut s, |tx| group::ungroup(tx, &ids(&["a", "c"]), None, None)).unwrap();
    for n in ["a", "b", "c", "d"] {
        assert!(stack(&s, n).is_empty());
    }
    assert_eq!(s.len(), 4);
}

#[test]
fn old_ungroup_keeps_member_geometry() {
    // The old ungroup dropped the group rotation. Members keep their own box and angle.
    let mut s = scene(vec![rect("a", 0.0, 0.0, 10.0, 10.0).angle(0.5), rect("b", 30.0, 0.0, 10.0, 10.0)]);
    run(&mut s, |tx| group::group(tx, &ids(&["a", "b"]), None, gid("g1"))).unwrap();
    run(&mut s, |tx| group::ungroup(tx, &ids(&["a", "b"]), None, None)).unwrap();
    assert_eq!(get(&s, "a").transform.angle, 0.5);
    assert_eq!(get(&s, "b").transform.x, 30.0);
}

#[test]
fn stale_singleton_group_ids_are_pruned_when_grouping() {
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 1.0, 1.0).grouped(&["old"]),
        rect("b", 5.0, 0.0, 1.0, 1.0),
        rect("c", 9.0, 0.0, 1.0, 1.0),
    ]);
    // "old" has one live member, so it counts as no group; grouping a and b makes a plain group.
    run(&mut s, |tx| group::group(tx, &ids(&["a", "b"]), None, gid("g1"))).unwrap();
    assert_eq!(stack(&s, "a"), strs(&["g1"]));
    assert_valid(&s);
}

#[test]
fn group_names_live_on_the_members_and_follow_ungroup() {
    let mut s = four();
    run(&mut s, |tx| group::group(tx, &ids(&["a", "b"]), None, gid("g1"))).unwrap();
    assert_eq!(group::group_name(&s, &gid("g1")), None);
    run(&mut s, |tx| group::set_group_name(tx, &gid("g1"), Some("Header"))).unwrap();
    assert_eq!(group::group_name(&s, &gid("g1")).as_deref(), Some("Header"));
    assert!(get(&s, "a").custom.as_ref().unwrap().contains_key("drafftink.group.g1"));
    run(&mut s, |tx| group::set_group_name(tx, &gid("g1"), Some("Footer"))).unwrap();
    assert_eq!(group::group_name(&s, &gid("g1")).as_deref(), Some("Footer"));
    run(&mut s, |tx| group::ungroup(tx, &[], Some(&gid("g1")), None)).unwrap();
    assert!(get(&s, "a").custom.is_none(), "the name is removed with the group");
    assert_valid(&s);
}

#[test]
fn group_name_rules() {
    let mut s = four();
    run(&mut s, |tx| group::group(tx, &ids(&["a", "b"]), None, gid("g1"))).unwrap();
    for bad in ["", "a\nb", &"x".repeat(65)] {
        assert!(matches!(run(&mut s, |tx| group::set_group_name(tx, &gid("g1"), Some(bad))), Err(GroupError::BadName)));
    }
    assert!(matches!(run(&mut s, |tx| group::set_group_name(tx, &gid("zz"), Some("n"))), Err(GroupError::NoGroup)));
    run(&mut s, |tx| group::set_group_name(tx, &gid("g1"), Some("ok"))).unwrap();
    run(&mut s, |tx| group::set_group_name(tx, &gid("g1"), None)).unwrap();
    assert_eq!(group::group_name(&s, &gid("g1")), None);
}

#[test]
fn make_contiguous_repairs_a_merged_group() {
    // A merge can leave a group split by a foreign element.
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 1.0, 1.0).grouped(&["g"]),
        rect("x", 5.0, 0.0, 1.0, 1.0),
        rect("b", 9.0, 0.0, 1.0, 1.0).grouped(&["g"]),
    ]);
    assert_eq!(group::non_contiguous_groups(&s), vec![gid("g")]);
    run(&mut s, |tx| group::make_contiguous(tx, &gid("g"))).unwrap();
    assert!(group::non_contiguous_groups(&s).is_empty());
    assert_eq!(order(&s), strs(&["x", "a", "b"]));
}

#[test]
fn depth_limit_is_a_refusal() {
    let names: Vec<String> = (1..=16).map(|i| format!("g{i}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 1.0, 1.0).grouped(&refs),
        rect("b", 5.0, 0.0, 1.0, 1.0).grouped(&refs),
        rect("c", 9.0, 0.0, 1.0, 1.0),
    ]);
    let rev = s.revision();
    let r = run(&mut s, |tx| group::group(tx, &ids(&["a", "c"]), None, gid("deeper")));
    assert!(matches!(r, Err(GroupError::TooDeep { max: 16, .. })));
    assert_eq!(s.revision(), rev);
}

#[test]
fn old_group_bounds_and_children_stay_individually_addressable() {
    // Ported from the old group tests (creation, bounds, ungroup) and the weakness "children cannot be selected,
    // z-ordered or deleted individually".
    use drafftink_edit::lifecycle::{self, ChildPolicy};
    use drafftink_edit::transform;
    use drafftink_edit::zorder::{self, ZOp};
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 100.0, 50.0),
        rect("b", 200.0, 200.0, 50.0, 100.0),
        rect("c", 400.0, 0.0, 10.0, 10.0),
    ]);
    let r = run(&mut s, |tx| group::group(tx, &ids(&["a", "b"]), None, gid("g1"))).unwrap();
    assert_eq!(r.members.len(), 2);
    let b = transform::common_bounds(&s, &ids(&["a", "b"])).unwrap();
    assert_eq!((b.x0, b.y0, b.x1, b.y1), (0.0, 0.0, 250.0, 300.0));
    // One member can be reordered inside the group, then deleted on its own.
    let g = gid("g1");
    run(&mut s, |tx| zorder::z_order(tx, &ids(&["a"]), ZOp::Front, Some(&g))).unwrap();
    assert_eq!(order(&s), strs(&["b", "a", "c"]));
    run(&mut s, |tx| lifecycle::delete(tx, &ids(&["b"]), ChildPolicy::Delete)).unwrap();
    assert_eq!(order(&s), strs(&["a", "c"]));
    assert_eq!(get(&s, "a").transform.w, 100.0);
    assert_valid(&s);
    // Ungroup returns the members.
    let mut s = scene(vec![rect("a", 0.0, 0.0, 100.0, 50.0), rect("b", 200.0, 200.0, 50.0, 100.0)]);
    run(&mut s, |tx| group::group(tx, &ids(&["a", "b"]), None, gid("g1"))).unwrap();
    let u = run(&mut s, |tx| group::ungroup(tx, &ids(&["a"]), None, None)).unwrap();
    assert_eq!(u.members, ids(&["a", "b"]));
}
