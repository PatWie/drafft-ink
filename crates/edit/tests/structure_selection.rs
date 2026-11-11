#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The selection model: click, deep click, editing groups, marquee, select all.

mod structure_support;

use std::collections::BTreeSet;

use drafftink_edit::TargetError;
use drafftink_edit::group::selection::*;
use drafftink_edit::{group, lifecycle};
use drafftink_geom::select::MarqueeMode;
use drafftink_scene::*;
use kurbo::Rect;
use structure_support::*;

fn set(names: &[&str]) -> BTreeSet<ElementId> {
    names.iter().map(|n| id(n)).collect()
}

fn gset(names: &[&str]) -> BTreeSet<GroupId> {
    names.iter().map(|n| gid(n)).collect()
}

fn click_el(s: &MemoryStore, sel: &Selection, name: &str, opts: ClickOpts) -> Result<Selection, SelectionError> {
    click(s, sel, &ClickTarget::Element(id(name)), opts)
}

/// a, b, c, d with A = {x, y} inside B = {x, y, z}? No: B = {x, y} inside A = {x, y, z}; w is free.
fn nested() -> MemoryStore {
    scene(vec![
        rect("x", 0.0, 0.0, 10.0, 10.0).grouped(&["B", "A"]),
        rect("y", 20.0, 0.0, 10.0, 10.0).grouped(&["B", "A"]),
        rect("z", 40.0, 0.0, 10.0, 10.0).grouped(&["A"]),
        rect("w", 60.0, 0.0, 10.0, 10.0),
    ])
}

#[test]
fn click_selects_the_outermost_group() {
    let s = nested();
    let sel = click_el(&s, &Selection::default(), "x", ClickOpts::default()).unwrap();
    assert_eq!(sel.ids, set(&["x", "y", "z"]));
    assert_eq!(sel.groups, gset(&["A"]));
    assert_eq!(sel.editing_group, None);
    let sel = click_el(&s, &Selection::default(), "w", ClickOpts::default()).unwrap();
    assert_eq!(sel.ids, set(&["w"]));
    assert!(sel.groups.is_empty());
}

#[test]
fn inside_an_editing_group_a_click_selects_one_level_down() {
    let s = nested();
    let sel = click_el(&s, &Selection::default(), "x", ClickOpts::default()).unwrap();
    let sel = enter_group(&s, &sel, None, Some(&id("x"))).unwrap();
    assert_eq!(sel.editing_group, Some(gid("A")));
    assert_eq!(sel.ids, set(&["x", "y"]), "the unit inside A around x is B");
    assert_eq!(sel.groups, gset(&["B"]));
    // Clicking z inside A selects just z.
    let sel2 = click_el(&s, &sel, "z", ClickOpts::default()).unwrap();
    assert_eq!(sel2.ids, set(&["z"]));
    assert_eq!(sel2.editing_group, Some(gid("A")));
    // Enter B: a click on x selects x alone.
    let sel3 = enter_group(&s, &sel, Some(&gid("B")), Some(&id("x"))).unwrap();
    assert_eq!(sel3.ids, set(&["x"]));
    assert_eq!(sel3.editing_group, Some(gid("B")));
    assert!(sel3.groups.is_empty());
}

#[test]
fn clicking_outside_the_editing_group_leaves_it() {
    let s = nested();
    let sel = click_el(&s, &Selection::default(), "x", ClickOpts::default()).unwrap();
    let sel = enter_group(&s, &sel, None, Some(&id("x"))).unwrap();
    let out = click_el(&s, &sel, "w", ClickOpts::default()).unwrap();
    assert_eq!(out.editing_group, None);
    assert_eq!(out.ids, set(&["w"]));
}

#[test]
fn deep_click_selects_the_element_and_enters_its_innermost_group() {
    let s = nested();
    let sel = click_el(&s, &Selection::default(), "x", ClickOpts { deep: true, ..ClickOpts::default() }).unwrap();
    assert_eq!(sel.ids, set(&["x"]));
    assert!(sel.groups.is_empty(), "a partial selection never claims the group");
    assert_eq!(sel.editing_group, Some(gid("B")));
    let sel = click_el(&s, &Selection::default(), "z", ClickOpts { deep: true, ..ClickOpts::default() }).unwrap();
    assert_eq!(sel.editing_group, Some(gid("A")));
    let sel = click_el(&s, &Selection::default(), "w", ClickOpts { deep: true, ..ClickOpts::default() }).unwrap();
    assert_eq!(sel.editing_group, None);
}

#[test]
fn add_remove_and_toggle_work_on_units() {
    let s = nested();
    let a = click_el(&s, &Selection::default(), "w", ClickOpts::default()).unwrap();
    let add = click_el(&s, &a, "x", ClickOpts { mode: ClickMode::Add, ..ClickOpts::default() }).unwrap();
    assert_eq!(add.ids, set(&["w", "x", "y", "z"]));
    assert_eq!(add.groups, gset(&["A"]));
    let toggled = click_el(&s, &add, "y", ClickOpts { mode: ClickMode::Toggle, ..ClickOpts::default() }).unwrap();
    assert_eq!(toggled.ids, set(&["w"]), "shift-click on a member removes the whole group");
    let toggled = click_el(&s, &toggled, "y", ClickOpts { mode: ClickMode::Toggle, ..ClickOpts::default() }).unwrap();
    assert_eq!(toggled.ids, set(&["w", "x", "y", "z"]));
    let removed = click_el(&s, &add, "w", ClickOpts { mode: ClickMode::Remove, ..ClickOpts::default() }).unwrap();
    assert_eq!(removed.ids, set(&["x", "y", "z"]));
}

#[test]
fn clicking_empty_canvas_clears_but_may_keep_the_editing_group() {
    let s = nested();
    let sel = click_el(&s, &Selection::default(), "x", ClickOpts::default()).unwrap();
    let sel = enter_group(&s, &sel, None, Some(&id("x"))).unwrap();
    let kept = click(&s, &sel, &ClickTarget::Empty { inside_editing_group: true }, ClickOpts::default()).unwrap();
    assert!(kept.ids.is_empty());
    assert_eq!(kept.editing_group, Some(gid("A")));
    let left = click(&s, &sel, &ClickTarget::Empty { inside_editing_group: false }, ClickOpts::default()).unwrap();
    assert_eq!(left, Selection::default());
    let same = click(
        &s,
        &sel,
        &ClickTarget::Empty { inside_editing_group: false },
        ClickOpts { mode: ClickMode::Add, ..ClickOpts::default() },
    )
    .unwrap();
    assert_eq!(same, sel, "shift-click on empty canvas changes nothing");
}

#[test]
fn bound_text_selects_its_container() {
    let s = scene(vec![rect("a", 0.0, 0.0, 100.0, 50.0), label("la", "a", 10.0, 10.0, 50.0, 20.0)]);
    let sel = click_el(&s, &Selection::default(), "la", ClickOpts::default()).unwrap();
    assert_eq!(sel.ids, set(&["a"]));
}

#[test]
fn a_selected_frame_excludes_its_children() {
    let s = scene(vec![
        rect("kid", 5.0, 5.0, 10.0, 10.0).in_frame("f"),
        frame("f", 0.0, 0.0, 100.0, 100.0),
        rect("o", 200.0, 0.0, 5.0, 5.0),
    ]);
    let kid = click_el(&s, &Selection::default(), "kid", ClickOpts::default()).unwrap();
    assert_eq!(kid.ids, set(&["kid"]));
    let both = click_el(&s, &kid, "f", ClickOpts { mode: ClickMode::Add, ..ClickOpts::default() }).unwrap();
    assert_eq!(both.ids, set(&["f"]));
    let all = select_all(&s, &Selection::default(), None, false).unwrap();
    assert_eq!(all.ids, set(&["f", "o"]));
}

#[test]
fn locked_elements_are_never_selected_unless_asked() {
    let s = scene(vec![rect("a", 0.0, 0.0, 10.0, 10.0).locked(), rect("b", 20.0, 0.0, 10.0, 10.0)]);
    assert!(matches!(
        click_el(&s, &Selection::default(), "a", ClickOpts::default()),
        Err(SelectionError::Target(TargetError::Locked(_)))
    ));
    let sel =
        click_el(&s, &Selection::default(), "a", ClickOpts { include_locked: true, ..ClickOpts::default() }).unwrap();
    assert_eq!(sel.ids, set(&["a"]));
    assert_eq!(select_all(&s, &Selection::default(), None, false).unwrap().ids, set(&["b"]));
    let region = select_region(&s, &Selection::default(), Rect::new(-5.0, -5.0, 100.0, 100.0), RegionOpts::default());
    assert_eq!(region.ids, set(&["b"]));
    let pruned = prune(&s, &sel);
    assert!(pruned.ids.is_empty());
}

#[test]
fn unknown_and_deleted_targets_are_not_found() {
    let mut s = scene(vec![rect("a", 0.0, 0.0, 10.0, 10.0), rect("b", 20.0, 0.0, 10.0, 10.0)]);
    run(&mut s, |tx| lifecycle::delete(tx, &ids(&["b"]), lifecycle::ChildPolicy::Delete)).unwrap();
    assert!(matches!(
        click_el(&s, &Selection::default(), "b", ClickOpts::default()),
        Err(SelectionError::Target(TargetError::NotFound(_)))
    ));
    assert!(matches!(
        click_el(&s, &Selection::default(), "nope", ClickOpts::default()),
        Err(SelectionError::Target(TargetError::NotFound(_)))
    ));
}

#[test]
fn enter_and_exit_group() {
    let s = nested();
    let sel = click_el(&s, &Selection::default(), "x", ClickOpts::default()).unwrap();
    assert!(matches!(enter_group(&s, &Selection::default(), None, None), Err(SelectionError::NoGroup)));
    let inside = enter_group(&s, &sel, None, None).unwrap();
    assert_eq!(inside.editing_group, Some(gid("A")));
    assert_eq!(inside.ids, set(&["x", "y", "z"]), "entering keeps the selected members");
    assert!(matches!(enter_group(&s, &sel, None, Some(&id("w"))), Err(SelectionError::NotInGroup(_))));
    // Enter B from inside A, then leave: back in A with B selected.
    let in_b = enter_group(&s, &inside, Some(&gid("B")), Some(&id("x"))).unwrap();
    let out = exit_group(&s, &in_b).unwrap();
    assert_eq!(out.editing_group, Some(gid("A")));
    assert_eq!(out.ids, set(&["x", "y"]));
    assert_eq!(out.groups, gset(&["B"]));
    let out2 = exit_group(&s, &out).unwrap();
    assert_eq!(out2.editing_group, None);
    assert_eq!(out2.ids, set(&["x", "y", "z"]));
    assert_eq!(out2.groups, gset(&["A"]));
    assert!(matches!(exit_group(&s, &out2), Err(SelectionError::NotEditing)));
}

#[test]
fn select_all_variants() {
    let s = scene(vec![
        rect("g1", 0.0, 0.0, 10.0, 10.0).grouped(&["G"]),
        rect("g2", 20.0, 0.0, 10.0, 10.0).grouped(&["G"]),
        rect("kid", 5.0, 105.0, 10.0, 10.0).in_frame("f"),
        frame("f", 0.0, 100.0, 100.0, 100.0),
        rect("o", 200.0, 0.0, 5.0, 5.0),
    ]);
    let all = select_all(&s, &Selection::default(), None, false).unwrap();
    assert_eq!(all.ids, set(&["g1", "g2", "f", "o"]));
    assert_eq!(all.groups, gset(&["G"]));
    let in_frame = select_all(&s, &Selection::default(), Some(&Within::Frame(id("f"))), false).unwrap();
    assert_eq!(in_frame.ids, set(&["kid"]));
    let in_group = select_all(&s, &Selection::default(), Some(&Within::Group(gid("G"))), false).unwrap();
    assert_eq!(in_group.ids, set(&["g1", "g2"]));
    // Inside an editing group only its members.
    let sel = click_el(&s, &Selection::default(), "g1", ClickOpts::default()).unwrap();
    let editing = enter_group(&s, &sel, None, Some(&id("g1"))).unwrap();
    let inside = select_all(&s, &editing, None, false).unwrap();
    assert_eq!(inside.ids, set(&["g1", "g2"]));
    assert_eq!(inside.editing_group, Some(gid("G")));
    assert!(matches!(
        select_all(&scene(vec![]), &Selection::default(), None, false),
        Err(SelectionError::NothingToSelect)
    ));
    assert!(select_all(&s, &Selection::default(), Some(&Within::Frame(id("o"))), false).is_err(), "o is not a frame");
}

fn filled(mut e: Element) -> Element {
    e.style.fill_color = Some(Color::rgb(200, 0, 0));
    e
}

#[test]
fn marquee_touch_and_contain() {
    let s = scene(vec![filled(rect("a", 0.0, 0.0, 100.0, 100.0)), filled(rect("b", 200.0, 0.0, 100.0, 100.0))]);
    let r = Rect::new(50.0, 50.0, 250.0, 60.0);
    let touch = select_region(&s, &Selection::default(), r, RegionOpts::default());
    assert_eq!(touch.ids, set(&["a", "b"]));
    let contain = select_region(
        &s,
        &Selection::default(),
        Rect::new(-10.0, -10.0, 150.0, 150.0),
        RegionOpts { contain: MarqueeMode::Contain, ..RegionOpts::default() },
    );
    assert_eq!(contain.ids, set(&["a"]));
    let none = select_region(
        &s,
        &Selection::default(),
        r,
        RegionOpts { contain: MarqueeMode::Contain, ..RegionOpts::default() },
    );
    assert!(none.ids.is_empty());
}

#[test]
fn old_marquee_and_click_agree_on_unfilled_shapes() {
    // The old marquee used the group's AABB while a click used the children: an unfilled shape selected by a
    // marquee in its hollow middle, but not by a click. Both now use the outline.
    let s = scene(vec![
        rect("a", 0.0, 0.0, 200.0, 200.0).grouped(&["G"]),
        rect("b", 300.0, 0.0, 10.0, 10.0).grouped(&["G"]),
    ]);
    let inside_hole =
        select_region(&s, &Selection::default(), Rect::new(50.0, 50.0, 100.0, 100.0), RegionOpts::default());
    assert!(inside_hole.ids.is_empty());
    let crossing = select_region(&s, &Selection::default(), Rect::new(-10.0, 50.0, 10.0, 100.0), RegionOpts::default());
    assert_eq!(crossing.ids, set(&["a", "b"]), "one member hit selects the whole group");
    assert_eq!(crossing.groups, gset(&["G"]));
}

#[test]
fn marquee_modes_and_editing_group() {
    let s = nested();
    let sel = click_el(&s, &Selection::default(), "x", ClickOpts::default()).unwrap();
    let editing = enter_group(&s, &sel, None, Some(&id("x"))).unwrap();
    let w_only = Rect::new(55.0, -5.0, 75.0, 15.0);
    let replaced = select_region(&s, &editing, w_only, RegionOpts::default());
    assert_eq!(replaced.ids, set(&["w"]));
    assert_eq!(replaced.editing_group, None, "a plain marquee leaves the editing group");
    let added = select_region(&s, &editing, w_only, RegionOpts { mode: RegionMode::Add, ..RegionOpts::default() });
    assert_eq!(added.ids, set(&["x", "y", "w"]));
    assert_eq!(added.editing_group, Some(gid("A")));
}

#[test]
fn marquee_never_selects_children_of_a_selected_frame_with_it() {
    let s = scene(vec![rect("kid", 5.0, 5.0, 10.0, 10.0).in_frame("f"), frame("f", 0.0, 0.0, 100.0, 100.0)]);
    let both = select_region(
        &s,
        &Selection::default(),
        Rect::new(-10.0, -10.0, 150.0, 150.0),
        RegionOpts { contain: MarqueeMode::Contain, ..RegionOpts::default() },
    );
    assert_eq!(both.ids, set(&["f"]));
    // A marquee inside the frame only catches the child (frames react on their outline).
    let inner = select_region(&s, &Selection::default(), Rect::new(1.0, 1.0, 30.0, 30.0), RegionOpts::default());
    assert_eq!(inner.ids, set(&["kid"]));
}

#[test]
fn marquee_ignores_bound_text() {
    let s = scene(vec![rect("a", 0.0, 0.0, 100.0, 50.0), label("la", "a", 10.0, 10.0, 50.0, 20.0)]);
    // Over the label only (inside the hollow rectangle): the label is ignored and the container is not hit.
    let over_label = select_region(&s, &Selection::default(), Rect::new(20.0, 12.0, 30.0, 20.0), RegionOpts::default());
    assert!(over_label.ids.is_empty());
    // Over the container outline: the container is selected, never the label.
    let over_outline =
        select_region(&s, &Selection::default(), Rect::new(-5.0, 20.0, 5.0, 30.0), RegionOpts::default());
    assert_eq!(over_outline.ids, set(&["a"]));
}

#[test]
fn deleting_a_member_inside_an_editing_group_falls_back_to_the_rest() {
    let mut s = scene(vec![
        rect("a", 0.0, 0.0, 10.0, 10.0).grouped(&["G"]),
        rect("b", 20.0, 0.0, 10.0, 10.0).grouped(&["G"]),
        rect("c", 40.0, 0.0, 10.0, 10.0).grouped(&["G"]),
    ]);
    let sel = click_el(&s, &Selection::default(), "a", ClickOpts::default()).unwrap();
    let sel = enter_group(&s, &sel, None, Some(&id("a"))).unwrap();
    assert_eq!(sel.ids, set(&["a"]));
    run(&mut s, |tx| lifecycle::delete(tx, &ids(&["a"]), lifecycle::ChildPolicy::Delete)).unwrap();
    let pruned = prune(&s, &sel);
    assert!(pruned.ids.is_empty());
    assert_eq!(pruned.editing_group, Some(gid("G")), "two members remain, so G is still a group");
    run(&mut s, |tx| lifecycle::delete(tx, &ids(&["b"]), lifecycle::ChildPolicy::Delete)).unwrap();
    let pruned = prune(&s, &sel);
    assert_eq!(pruned.editing_group, None, "one member left: no group to edit");
}

#[test]
fn select_ids_derives_groups_and_keeps_partial_selections_partial() {
    let s = nested();
    let sel = select_ids(&s, &ids(&["x", "y", "z"]), None);
    assert_eq!(sel.groups, gset(&["A"]));
    let part = select_ids(&s, &ids(&["x", "y"]), None);
    assert!(part.groups.is_empty(), "A needs all members to count as selected");
    let inside = select_ids(&s, &ids(&["x", "y"]), Some(&gid("A")));
    assert_eq!(inside.groups, gset(&["B"]));
    let z_sorted = in_z_order(&s, &select_ids(&s, &ids(&["z", "w", "x"]), None));
    assert_eq!(z_sorted, ids(&["x", "z", "w"]));
}

#[test]
fn group_create_then_selection_is_the_group() {
    let mut s = scene(vec![rect("a", 0.0, 0.0, 10.0, 10.0), rect("b", 20.0, 0.0, 10.0, 10.0)]);
    run(&mut s, |tx| group::group(tx, &ids(&["a", "b"]), None, gid("g1"))).unwrap();
    let sel = select_ids(&s, &ids(&["a", "b"]), None);
    assert_eq!(sel.groups, gset(&["g1"]));
}
