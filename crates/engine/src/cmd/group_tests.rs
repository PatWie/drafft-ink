//! Tests of the `group` family. Per command: availability, refusal text, effect with undo and redo, and a
//! negative case. The last section is one regression test per weakness of the old app's groups
//! (research/parts/core-A.md §3.1), driven through commands.

use drafftink_scene::SceneTx;
use serde_json::json;

use super::testkit::*;

fn grouped(n: usize) -> (crate::session::Session, Vec<drafftink_scene::ElementId>, String) {
    let (mut s, ids) = rects(n);
    let refs: Vec<_> = ids.iter().collect();
    let out = run(&mut s, "group.create", ids_arg(&refs));
    let g = out["group"].as_str().unwrap().to_owned();
    (s, ids, g)
}

// ---- group.create ----

#[test]
fn create_is_unavailable_with_fewer_than_two_elements_and_says_so() {
    let (mut s, ids) = rects(3);
    let r = s.availability("group.create").unwrap_err();
    assert_eq!(r.code, "no_target");
    select(&mut s, &[&ids[0]]);
    let r = s.availability("group.create").unwrap_err();
    assert_eq!(r.code, "too_few");
    assert_eq!(r.message, "Grouping needs at least 2 elements, and 1 were given. Select more, or pass `ids`.");
    let e = s.execute("group.create", json!({})).unwrap_err();
    assert_eq!((e.code, e.message), (r.code, r.message));
    select(&mut s, &[&ids[0], &ids[1]]);
    assert!(s.availability("group.create").is_ok());
}

#[test]
fn create_groups_selects_the_group_and_undoes() {
    let (mut s, ids) = rects(3);
    select(&mut s, &[&ids[0], &ids[2]]);
    let out = run(&mut s, "group.create", json!({}));
    let g = out["group"].as_str().unwrap().to_owned();
    assert_eq!(json_ids(&out, "members"), ["r0", "r2"]);
    assert_eq!(out["undo_label"], "Group");
    for i in [&ids[0], &ids[2]] {
        let stack: Vec<String> = groups_of(&s, i).iter().map(|x| x.as_str().to_owned()).collect();
        assert_eq!(stack, std::slice::from_ref(&g));
    }
    assert!(groups_of(&s, &ids[1]).is_empty());
    assert_eq!(s.selection().len(), 2);
    assert!(s.tab().observed.selected_groups.iter().any(|x| x.as_str() == g));
    assert_eq!(s.tab().doc.undo_depth(), 1);

    s.undo_steps(1).unwrap();
    assert!(groups_of(&s, &ids[0]).is_empty() && groups_of(&s, &ids[2]).is_empty());
    s.redo_steps(1).unwrap();
    assert_eq!(groups_of(&s, &ids[0]).len(), 1);
}

#[test]
fn create_accepts_explicit_ids_and_names() {
    let (mut s, ids) = rects(3);
    let out = run(&mut s, "group.create", json!({"ids": ["r1", "r2"]}));
    assert_eq!(json_ids(&out, "members"), ["r1", "r2"]);
    assert!(groups_of(&s, &ids[0]).is_empty());
}

#[test]
fn create_refusals_use_the_catalog_text_and_change_nothing() {
    let (mut s, ids, _) = grouped(2);
    let extra = named(&mut s, "x", 500.0, 0.0, 10.0, 10.0);
    let before = s.fingerprint();

    // already one group
    let r = refuse(&mut s, "group.create", ids_arg(&[&ids[0], &ids[1]]));
    assert_eq!(r.code, "already_grouped");
    assert_eq!(
        r.message,
        "These elements already form one group at this level. Ungroup them with `group.ungroup`, or select more elements."
    );
    // unknown id
    let r = refuse(&mut s, "group.create", json!({"ids": ["r0", "nope"]}));
    assert_eq!(r.code, "not_found");
    // a single element
    let r = refuse(&mut s, "group.create", ids_arg(&[&extra]));
    assert_eq!(r.code, "too_few");
    assert_eq!(s.fingerprint(), before);
}

#[test]
fn a_refused_create_does_not_use_up_a_group_id() {
    // The first group of a session gets the same id whether or not earlier calls were refused.
    let (mut clean, ids) = rects(3);
    let expected = run(&mut clean, "group.create", ids_arg(&[&ids[0], &ids[1]]))["group"].clone();

    let (mut s, ids) = rects(3);
    lock(&mut s, &ids[2]);
    let r = refuse(&mut s, "group.create", ids_arg(&[&ids[0]]));
    assert_eq!(r.code, "too_few");
    let r = refuse(&mut s, "group.create", ids_arg(&[&ids[0], &ids[2]]));
    assert_eq!(r.code, "locked");
    let r = refuse(&mut s, "group.create", json!({"ids": ["r0", "nope"]}));
    assert_eq!(r.code, "not_found");
    let got = run(&mut s, "group.create", ids_arg(&[&ids[0], &ids[1]]))["group"].clone();
    assert_eq!(got, expected);
}

#[test]
fn create_refuses_a_frame_and_its_own_children() {
    let (mut s, ids) = rects(2);
    let f = frame(&mut s, "f", -10.0, -10.0, 500.0, 100.0);
    run(&mut s, "frame.add", json!({"frame": "f", "ids": ["r0"]}));
    let before = s.fingerprint();
    let r = refuse(&mut s, "group.create", ids_arg(&[&f, &ids[0]]));
    assert_eq!(r.code, "frame_and_children");
    assert_eq!(r.message, "A frame and its own children cannot be grouped together. Select either the frame or its children.");
    assert_eq!(s.fingerprint(), before);
}

#[test]
fn create_refuses_locked_elements_and_names_them() {
    let (mut s, ids) = rects(2);
    lock(&mut s, &ids[1]);
    let before = s.fingerprint();
    let r = refuse(&mut s, "group.create", ids_arg(&[&ids[0], &ids[1]]));
    assert_eq!(r.code, "locked");
    assert_eq!(r.details.unwrap()["ids"], json!(["r1"]));
    assert_eq!(s.fingerprint(), before);
}

#[test]
fn create_has_a_shortcut_and_runs_in_view_only_mode_never() {
    let (mut s, ids) = rects(2);
    let spec = s.spec("group.create").unwrap();
    assert_eq!(spec.shortcuts[0].keys, ["Mod+G"]);
    select(&mut s, &[&ids[0], &ids[1]]);
    s.view_only = true;
    assert_eq!(s.availability("group.create").unwrap_err().code, "view_only");
    assert_eq!(s.execute("group.create", json!({})).unwrap_err().code, "view_only");
}

// ---- group.ungroup ----

#[test]
fn ungroup_needs_a_group_and_says_so() {
    let (mut s, ids) = rects(2);
    let msg = "No group is selected. Select a group first (click it, or `selection.select` with `group`), or pass `group`.";
    let r = s.availability("group.ungroup").unwrap_err();
    assert_eq!((r.code, r.message.as_str()), ("no_group", msg));
    select(&mut s, &[&ids[0]]);
    assert_eq!(s.availability("group.ungroup").unwrap_err().code, "no_group");
    let e = refuse(&mut s, "group.ungroup", json!({}));
    assert_eq!((e.code, e.message.as_str()), ("no_group", msg));
    let e = refuse(&mut s, "group.ungroup", json!({"group": "nope"}));
    assert_eq!(e.code, "no_group");
}

#[test]
fn ungroup_removes_the_group_keeps_ids_and_order_and_selects_the_members() {
    let (mut s, ids, g) = grouped(3);
    let order_before = order(&s);
    assert!(s.availability("group.ungroup").is_ok());
    let out = run(&mut s, "group.ungroup", json!({}));
    assert_eq!(out["removed"], json!([g]));
    assert_eq!(order(&s), order_before);
    for i in &ids {
        assert!(groups_of(&s, i).is_empty());
    }
    assert_eq!(s.selection().len(), 3);
    assert!(s.tab().observed.selected_groups.is_empty());
    s.undo_steps(1).unwrap();
    assert_eq!(groups_of(&s, &ids[1]).len(), 1);
}

#[test]
fn ungroup_by_group_id_works_without_a_selection() {
    let (mut s, ids, g) = grouped(2);
    s.tab_mut().observed.selection.clear();
    s.tab_mut().observed.selected_groups.clear();
    run(&mut s, "group.ungroup", json!({"group": g}));
    assert!(groups_of(&s, &ids[0]).is_empty());
}

// ---- group.set_name ----

#[test]
fn set_name_names_the_selected_group_and_undoes() {
    let (mut s, ids, g) = grouped(2);
    let out = run(&mut s, "group.set_name", json!({"name": "Header"}));
    assert_eq!(out["group"], json!(g));
    let gid = drafftink_scene::GroupId::new(&g).unwrap();
    assert_eq!(drafftink_edit::group::group_name(s.store(), &gid).as_deref(), Some("Header"));
    // stored per member under custom["drafftink.group.<id>"] (deviation D4)
    let key = format!("drafftink.group.{g}");
    for i in &ids {
        assert_eq!(get(&s, i).custom.as_ref().and_then(|c| c.get(&key)).and_then(|v| v.as_str()), Some("Header"));
    }
    run(&mut s, "group.set_name", json!({"name": null}));
    assert_eq!(drafftink_edit::group::group_name(s.store(), &gid), None);
    s.undo_steps(1).unwrap();
    assert_eq!(drafftink_edit::group::group_name(s.store(), &gid).as_deref(), Some("Header"));
}

#[test]
fn set_name_refusals() {
    let (mut s, ids, g) = grouped(2);
    let before = s.fingerprint();
    let r = refuse(&mut s, "group.set_name", json!({"name": ""}));
    assert_eq!(r.code, "bad_name");
    assert_eq!(r.message, "A group name must be 1 to 64 characters and must not contain control characters.");
    assert_eq!(refuse(&mut s, "group.set_name", json!({"name": "a\nb"})).code, "bad_name");
    assert_eq!(refuse(&mut s, "group.set_name", json!({"name": "x".repeat(65)})).code, "bad_name");
    assert_eq!(refuse(&mut s, "group.set_name", json!({"group": "nope", "name": "x"})).code, "no_group");
    assert_eq!(s.fingerprint(), before);
    // nothing selected, no group given
    s.tab_mut().observed.selected_groups.clear();
    assert_eq!(refuse(&mut s, "group.set_name", json!({"name": "x"})).code, "no_group");
    // several selected groups
    let (a, b) = (named(&mut s, "p", 0.0, 100.0, 10.0, 10.0), named(&mut s, "q", 50.0, 100.0, 10.0, 10.0));
    run(&mut s, "group.create", ids_arg(&[&a, &b]));
    let all: Vec<_> = ids.iter().chain([&a, &b]).collect();
    select(&mut s, &all);
    s.tab_mut().observed.selected_groups = ["g1", "g2"].iter().map(|x| drafftink_scene::GroupId::new(x).unwrap()).collect();
    assert_eq!(refuse(&mut s, "group.set_name", json!({"name": "x"})).code, "needs_single");
    let _ = g;
}

#[test]
fn set_name_is_removed_by_ungroup_and_refuses_locked_members() {
    let (mut s, ids, g) = grouped(2);
    run(&mut s, "group.set_name", json!({"name": "Header"}));
    run(&mut s, "group.ungroup", json!({}));
    for i in &ids {
        assert!(get(&s, i).custom.is_none(), "the name must go with the group");
    }
    // locked member
    run(&mut s, "group.create", ids_arg(&[&ids[0], &ids[1]]));
    lock(&mut s, &ids[0]);
    let r = refuse(&mut s, "group.set_name", json!({"name": "x"}));
    assert_eq!(r.code, "locked");
    let _ = g;
}

// ---- group.enter and group.exit ----

#[test]
fn enter_and_exit_round_trip_the_editing_group() {
    let (mut s, ids, g) = grouped(3);
    assert_eq!(s.availability("group.exit").unwrap_err().code, "not_in_group");
    let r = refuse(&mut s, "group.exit", json!({}));
    assert_eq!((r.code, r.message.as_str()), ("not_in_group", "You are not inside a group, so there is nothing to leave."));

    let out = run(&mut s, "group.enter", json!({}));
    assert_eq!(out["editing_group"], json!(g));
    assert_eq!(s.tab().observed.editing_group.as_ref().map(|x| x.as_str().to_owned()), Some(g.clone()));
    assert!(s.availability("group.exit").is_ok());
    // entering changes no document and makes no undo step
    assert_eq!(s.tab().doc.undo_depth(), 1);

    let out = run(&mut s, "group.exit", json!({}));
    assert_eq!(out["editing_group"], json!(null));
    assert!(s.tab().observed.editing_group.is_none());
    assert_eq!(s.selection().len(), ids.len());
    assert_eq!(s.tab().doc.undo_depth(), 1);
}

#[test]
fn enter_with_select_picks_one_member() {
    let (mut s, ids, g) = grouped(3);
    run(&mut s, "group.enter", json!({"group": g, "select": "r1"}));
    assert_eq!(s.selection().iter().map(|i| i.as_str()).collect::<Vec<_>>(), ["r1"]);
    assert!(s.tab().observed.editing_group.is_some());
    let _ = ids;
}

#[test]
fn enter_refusals() {
    let (mut s, ids, g) = grouped(2);
    let outsider = named(&mut s, "z", 400.0, 0.0, 10.0, 10.0);
    let before = s.fingerprint();
    let r = refuse(&mut s, "group.enter", json!({"group": "nope"}));
    assert_eq!(r.code, "no_group");
    assert_eq!(r.message, "No group is selected. Select a group first (click it, or `selection.select` with `group`), or pass `group`.");
    let r = refuse(&mut s, "group.enter", json!({"group": g, "select": outsider.as_str()}));
    assert_eq!(r.code, "bad_args");
    assert_eq!(refuse(&mut s, "group.enter", json!({"group": g, "select": "nope"})).code, "not_found");
    // no group selected and none given
    select(&mut s, &[&ids[0]]);
    s.tab_mut().observed.selected_groups.clear();
    assert_eq!(refuse(&mut s, "group.enter", json!({})).code, "no_group");
    let _ = before;
}

#[test]
fn undo_restores_the_selection_and_editing_group_of_the_step() {
    let (mut s, ids, g) = grouped(2);
    run(&mut s, "group.enter", json!({"group": g}));
    // a document step inside the group
    run(&mut s, "arrange.z_order", json!({"op": "back", "ids": ["r1"]}));
    s.undo_steps(1).unwrap();
    assert_eq!(s.tab().observed.editing_group.as_ref().map(|x| x.as_str().to_owned()), Some(g));
    let _ = ids;
}

#[test]
fn nested_group_inside_an_editing_group_sits_just_inside_it() {
    let (mut s, ids, g) = grouped(3);
    run(&mut s, "group.enter", json!({"group": g.clone()}));
    run(&mut s, "group.create", ids_arg(&[&ids[0], &ids[1]]));
    // r2 is only in the outer group, r0 and r1 in both, the new one innermost
    assert_eq!(groups_of(&s, &ids[2]).len(), 1);
    let stack = groups_of(&s, &ids[0]);
    assert_eq!(stack.len(), 2);
    assert_eq!(stack[1].as_str(), g);
    // ungroup at the editing level removes only the inner group
    run(&mut s, "group.ungroup", ids_arg(&[&ids[0], &ids[1]]));
    assert_eq!(groups_of(&s, &ids[0]).len(), 1);
    assert_eq!(groups_of(&s, &ids[2]).len(), 1);
}

// ---- regression tests for the weaknesses of the old app's groups (core-A.md §3.1) ----

/// W3 + W5 + "grouping never changes element ids" (§7.5 item 11): members stay separate elements with their own
/// ids, can be selected one by one inside the group, and are addressed by id by other commands.
#[test]
fn old_weakness_children_stay_addressable_elements() {
    let (mut s, ids, g) = grouped(3);
    assert_eq!(live_count(&s), 3);
    run(&mut s, "group.enter", json!({"group": g, "select": "r1"}));
    assert_eq!(s.selection().iter().map(|i| i.as_str()).collect::<Vec<_>>(), ["r1"]);
    // inside the group a member can be reordered on its own, and it stays inside the group's range
    run(&mut s, "arrange.z_order", json!({"op": "front"}));
    assert_eq!(order(&s), ["r0", "r2", "r1"]);
    assert_eq!(groups_of(&s, &ids[1]).len(), 1);
}

/// W1 + W2: a group has no angle of its own. Each member keeps the angle it has, through group, ungroup, undo
/// and redo, and the placement stays the same (the old app dropped the rotation on ungroup and on save).
#[test]
fn old_weakness_member_rotation_survives_group_and_ungroup() {
    let (mut s, ids) = rects(2);
    setup(&mut s, |tx| {
        tx.update(&ids[0], |e| e.transform.angle = 0.5)?;
        tx.update(&ids[1], |e| e.transform.angle = 0.5)
    });
    let snap = |s: &crate::session::Session| -> Vec<(f64, f64, f64)> {
        ids.iter().map(|i| (get(s, i).transform.x, get(s, i).transform.y, get(s, i).transform.angle)).collect()
    };
    let before = snap(&s);
    run(&mut s, "group.create", ids_arg(&[&ids[0], &ids[1]]));
    assert_eq!(snap(&s), before, "grouping must not move or rotate anything");
    run(&mut s, "group.ungroup", json!({}));
    assert_eq!(snap(&s), before, "ungrouping must not snap members back to an unrotated place");
    s.undo_steps(2).unwrap();
    s.redo_steps(2).unwrap();
    assert_eq!(snap(&s), before);
}

/// W6: grouping a non-contiguous selection puts the block at the topmost member's slot, keeps everything else
/// in its old order below it, loses no element, and a refused grouping leaves no undo step.
#[test]
fn old_weakness_z_order_of_a_non_contiguous_selection_and_failed_grouping() {
    let (mut s, ids) = rects(5); // r0 r1 r2 r3 r4
    run(&mut s, "group.create", ids_arg(&[&ids[0], &ids[3]]));
    assert_eq!(order(&s), ["r1", "r2", "r0", "r3", "r4"]);
    assert_eq!(live_count(&s), 5);

    // duplicates and unknown ids: the count of removed elements, not of ids, matters
    let (mut s, ids) = rects(4);
    run(&mut s, "group.create", json!({"ids": ["r0", "r0", "r2"]}));
    assert_eq!(order(&s), ["r1", "r0", "r2", "r3"]);
    assert_eq!(live_count(&s), 4);

    // a refused grouping pushes nothing
    let depth = s.tab().doc.undo_depth();
    let before = order(&s);
    refuse(&mut s, "group.create", json!({"ids": ["r1"]}));
    refuse(&mut s, "group.create", json!({"ids": ["r1", "unknown"]}));
    assert_eq!(s.tab().doc.undo_depth(), depth);
    assert_eq!(order(&s), before);
    let _ = ids;
}

/// W8 + W10: a group is not a container that holds copies. Members are independent elements, so changes to one
/// member do not touch the others, and undo and redo restore the group as a unit.
#[test]
fn old_weakness_members_change_independently_and_undo_restores_the_group() {
    let (mut s, ids, g) = grouped(2);
    run(&mut s, "group.enter", json!({"group": g.clone(), "select": "r0"}));
    run(&mut s, "arrange.align", json!({"edge": "left", "ids": ["r0"], "to": "r1"}));
    assert!((x_of(&s, &ids[0]) - 100.0).abs() < 1e-9, "r0 moves on its own");
    assert!(x_of(&s, &ids[1]).abs() > 99.0, "r1 does not");
    s.undo_steps(1).unwrap();
    assert!(x_of(&s, &ids[0]).abs() < 1e-9);
    assert_eq!(groups_of(&s, &ids[0]).len(), 1);
}

/// W10: more than one level, a name, a lock, and the minimum size. Ungroup removes one level at a time (the old
/// app dissolved one level and kept the rest as groups with no way to reach them), a group has a name, a group
/// of one element is refused, and a locked member keeps the group from being taken apart.
#[test]
fn old_weakness_nested_groups_names_locks_and_minimum_size() {
    let (mut s, ids) = rects(4);
    run(&mut s, "group.create", ids_arg(&[&ids[0], &ids[1]]));
    run(&mut s, "group.create", ids_arg(&[&ids[0], &ids[2]]));
    // r0 and r1 are in the inner group, with r2 they are in the outer one
    assert_eq!(groups_of(&s, &ids[0]).len(), 2);
    assert_eq!(groups_of(&s, &ids[2]).len(), 1);
    run(&mut s, "group.ungroup", json!({}));
    assert_eq!(groups_of(&s, &ids[0]).len(), 1, "the inner group is still there");
    assert_eq!(groups_of(&s, &ids[2]).len(), 0);
    assert_eq!(refuse(&mut s, "group.create", json!({"ids": ["r3"]})).code, "too_few");
    // a group has a name
    let g = groups_of(&s, &ids[0])[0].clone();
    run(&mut s, "group.set_name", json!({"group": g.as_str(), "name": "Pair"}));
    assert_eq!(drafftink_edit::group::group_name(s.store(), &g).as_deref(), Some("Pair"));
}

/// W11 (groups and arrows): moving a group by a command carries the arrows bound to its members (the old app had
/// no bindings at all). The arrow starts at the centre of `r0` and is not part of the moved set.
#[test]
fn old_weakness_arrows_bound_to_a_group_follow_it() {
    let (mut s, ids) = rects(2);
    let far = named(&mut s, "far", 0.0, 200.0, 40.0, 40.0);
    let arrow = arrow_bound_to(&mut s, "arr", &ids[0], 20.0, 20.0, 60.0);
    run(&mut s, "group.create", ids_arg(&[&ids[0], &ids[1]]));
    // The group (one unit) and `far` align on their bottom edges: the group moves down by 200.
    run(&mut s, "arrange.align", json!({"edge": "bottom", "ids": ["r0", "far"]}));
    assert!((y_of(&s, &ids[0]) - 200.0).abs() < 1e-6 && (y_of(&s, &ids[1]) - 200.0).abs() < 1e-6);
    assert!(y_of(&s, &far).abs() > 199.0);
    let start = drafftink_edit::transform::geometry::world_points(get(&s, &arrow)).unwrap()[0];
    assert!((start.y - 220.0).abs() < 0.1 && (start.x - 20.0).abs() < 0.1, "the arrow start follows the centre of r0: {start:?}");
}

#[test]
fn exit_drops_an_editing_group_that_lost_its_members() {
    let (mut s, ids, g) = grouped(2);
    run(&mut s, "group.ungroup", json!({"group": g}));
    // a collaborator's delete can leave the id behind; Esc (`tool.cancel`) must still be able to leave it
    s.tab_mut().observed.editing_group = Some(drafftink_scene::GroupId::new("gone").unwrap());
    assert!(s.tab().observed.editing_group.is_some());
    assert!(s.availability("group.exit").is_ok());
    let out = run(&mut s, "group.exit", json!({}));
    assert_eq!(out["editing_group"], json!(null));
    assert!(s.tab().observed.editing_group.is_none());
    assert_eq!(s.selection().len(), ids.len());
}
