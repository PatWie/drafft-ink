//! Tests of the `arrange` family: z-order, align, distribute. Per command: availability, the exact refusal
//! text, the effect with undo and redo, and a negative case.

use drafftink_scene::SceneTx;
use serde_json::json;

use crate::cmd::group::testkit::*;

fn centre_x(s: &crate::session::Session, id: &drafftink_scene::ElementId) -> f64 {
    let t = &get(s, id).transform;
    t.x + t.w / 2.0
}

fn centre_y(s: &crate::session::Session, id: &drafftink_scene::ElementId) -> f64 {
    let t = &get(s, id).transform;
    t.y + t.h / 2.0
}

// ---- arrange.z_order ----

#[test]
fn z_order_needs_a_selection_and_refuses_in_view_only_mode() {
    let (mut s, ids) = rects(2);
    let r = s.availability("arrange.z_order").unwrap_err();
    assert_eq!(r.code, "no_target");
    assert_eq!(r.message, "Nothing to act on: no elements are selected. Select elements first, or pass `ids`.");
    select(&mut s, &[&ids[0]]);
    assert!(s.availability("arrange.z_order").is_ok());
    s.view_only = true;
    assert_eq!(s.availability("arrange.z_order").unwrap_err().code, "view_only");
}

#[test]
fn z_order_front_back_forward_backward_and_undo() {
    let (mut s, _) = rects(4); // r0 r1 r2 r3
    let out = run(&mut s, "arrange.z_order", json!({"op": "front", "ids": ["r0"]}));
    assert_eq!(order(&s), ["r1", "r2", "r3", "r0"]);
    assert_eq!(out["undo_label"], "Change layer order");
    assert_eq!(json_ids(&out, "moved"), ["r0"]);
    run(&mut s, "arrange.z_order", json!({"op": "back", "ids": ["r3"]}));
    assert_eq!(order(&s), ["r3", "r1", "r2", "r0"]);
    run(&mut s, "arrange.z_order", json!({"op": "forward", "ids": ["r3"]}));
    assert_eq!(order(&s), ["r1", "r3", "r2", "r0"]);
    run(&mut s, "arrange.z_order", json!({"op": "backward", "ids": ["r0"]}));
    assert_eq!(order(&s), ["r1", "r3", "r0", "r2"]);
    assert_eq!(s.tab().doc.undo_depth(), 4);
    s.undo_steps(1).unwrap();
    assert_eq!(order(&s), ["r1", "r3", "r2", "r0"]);
    s.redo_steps(1).unwrap();
    assert_eq!(order(&s), ["r1", "r3", "r0", "r2"]);
    s.undo_steps(4).unwrap();
    assert_eq!(order(&s), ["r0", "r1", "r2", "r3"]);
}

#[test]
fn z_order_uses_the_selection_by_default() {
    let (mut s, ids) = rects(3);
    select(&mut s, &[&ids[0]]);
    run(&mut s, "arrange.z_order", json!({"op": "front"}));
    assert_eq!(order(&s), ["r1", "r2", "r0"]);
}

#[test]
fn z_order_at_the_end_is_ok_with_changed_false_and_no_undo_step() {
    let (mut s, _) = rects(3);
    let out = run(&mut s, "arrange.z_order", json!({"op": "front", "ids": ["r2"]}));
    assert_eq!(out["changed"], false);
    assert!(out.get("undo_label").is_none());
    assert_eq!(s.tab().doc.undo_depth(), 0);
    let out = run(&mut s, "arrange.z_order", json!({"op": "backward", "ids": ["r0"]}));
    assert_eq!(out["changed"], false);
    assert_eq!(s.tab().doc.undo_depth(), 0);
}

#[test]
fn z_order_steps_over_a_whole_group() {
    let (mut s, ids) = rects(4); // r0 r1 r2 r3
    run(&mut s, "group.create", ids_arg(&[&ids[1], &ids[2]]));
    assert_eq!(order(&s), ["r0", "r1", "r2", "r3"]);
    run(&mut s, "arrange.z_order", json!({"op": "forward", "ids": ["r0"]}));
    assert_eq!(order(&s), ["r1", "r2", "r0", "r3"], "r0 jumps over both members at once");
    run(&mut s, "arrange.z_order", json!({"op": "forward", "ids": ["r1"]}));
    assert_eq!(order(&s), ["r0", "r1", "r2", "r3"], "any member moves the whole group");
}

#[test]
fn z_order_carries_bound_text_and_keeps_frame_children_below_their_frame() {
    let (mut s, ids) = rects(3);
    label(&mut s, "t0", &ids[0]);
    run(&mut s, "arrange.z_order", json!({"op": "front", "ids": ["r0"]}));
    let o = order(&s);
    let pos = |n: &str| o.iter().position(|x| x == n).unwrap();
    assert_eq!(o.len(), 4);
    assert!(pos("r0") > pos("r2") && pos("t0") > pos("r0"), "the label travels with its container: {o:?}");

    let (mut s, _) = rects(2);
    frame(&mut s, "f", -10.0, -10.0, 300.0, 100.0);
    run(&mut s, "frame.add", json!({"frame": "f", "ids": ["r0", "r1"]}));
    let before = order(&s);
    assert_eq!(before, ["r0", "r1", "f"]);
    run(&mut s, "arrange.z_order", json!({"op": "front", "ids": ["r0"]}));
    assert_eq!(order(&s), ["r1", "r0", "f"], "front stays below the frame");
}

#[test]
fn z_order_refusals_leave_state_alone() {
    let (mut s, ids) = rects(3);
    lock(&mut s, &ids[1]);
    let before = s.fingerprint();
    let r = refuse(&mut s, "arrange.z_order", json!({"op": "front", "ids": ["r0", "r1"]}));
    assert_eq!(r.code, "locked");
    assert_eq!(r.details.unwrap()["ids"], json!(["r1"]));
    assert_eq!(refuse(&mut s, "arrange.z_order", json!({"op": "front", "ids": ["nope"]})).code, "not_found");
    assert_eq!(refuse(&mut s, "arrange.z_order", json!({"op": "sideways", "ids": ["r0"]})).code, "bad_args");
    assert_eq!(refuse(&mut s, "arrange.z_order", json!({"ids": ["r0"]})).code, "bad_args");
    assert_eq!(s.fingerprint(), before);
}

#[test]
fn z_order_has_the_catalog_keys_for_both_platforms() {
    let (s, _) = rects(1);
    let spec = s.spec("arrange.z_order").unwrap();
    let keys = |mac: bool| -> Vec<(String, String)> {
        spec.shortcuts.iter().map(|b| (b.chords(mac).join(","), b.args.to_owned())).collect()
    };
    let other = keys(false);
    let mac = keys(true);
    assert!(other.contains(&("Mod+]".into(), r#"{"op":"forward"}"#.into())));
    assert!(other.contains(&("Mod+[".into(), r#"{"op":"backward"}"#.into())));
    assert!(other.contains(&("Mod+Shift+]".into(), r#"{"op":"front"}"#.into())));
    assert!(other.contains(&("Mod+Shift+[".into(), r#"{"op":"back"}"#.into())));
    assert!(mac.contains(&("Mod+Alt+]".into(), r#"{"op":"front"}"#.into())));
    assert!(mac.contains(&("Mod+Alt+[".into(), r#"{"op":"back"}"#.into())));
}

#[test]
fn z_order_inside_an_editing_group_stays_in_the_groups_range() {
    let (mut s, ids) = rects(4); // r0 r1 r2 r3
    run(&mut s, "group.create", ids_arg(&[&ids[1], &ids[2]]));
    run(&mut s, "group.enter", json!({"select": "r1"}));
    run(&mut s, "arrange.z_order", json!({"op": "back"}));
    assert_eq!(order(&s), ["r0", "r1", "r2", "r3"], "r1 is at the back of its group, so nothing moves out of it");
    run(&mut s, "arrange.z_order", json!({"op": "front"}));
    assert_eq!(order(&s), ["r0", "r2", "r1", "r3"]);
}

// ---- arrange.align ----

#[test]
fn align_says_how_many_elements_it_needs() {
    let (mut s, ids) = rects(3);
    select(&mut s, &[&ids[0]]);
    let msg = "Align needs at least 2 elements (a group counts as one), and 1 were given. Select more, or pass `to` to align to a reference element.";
    let a = s.availability("arrange.align").unwrap_err();
    assert_eq!((a.code, a.message.as_str()), ("too_few", msg));
    let e = refuse(&mut s, "arrange.align", json!({"edge": "left"}));
    assert_eq!((e.code, e.message.as_str()), ("too_few", msg));
    select(&mut s, &[&ids[0], &ids[1]]);
    assert!(s.availability("arrange.align").is_ok());
}

#[test]
fn align_edges_and_centres_by_the_axis_they_equalise() {
    let (mut s, ids) = rects(3);
    setup(&mut s, |tx| {
        tx.update(&ids[1], |e| {
            e.transform.y = 50.0;
            e.transform.w = 80.0;
            e.transform.h = 20.0;
        })
    });
    let all = json!(["r0", "r1", "r2"]);
    run(&mut s, "arrange.align", json!({"edge": "left", "ids": all}));
    assert!(ids.iter().all(|i| x_of(&s, i).abs() < 1e-9));
    s.undo_steps(1).unwrap();

    run(&mut s, "arrange.align", json!({"edge": "right", "ids": all}));
    let right = |i| get(&s, i).transform.x + get(&s, i).transform.w;
    assert!(ids.iter().all(|i| (right(i) - 240.0).abs() < 1e-9), "{:?}", ids.iter().map(right).collect::<Vec<_>>());
    s.undo_steps(1).unwrap();

    run(&mut s, "arrange.align", json!({"edge": "top", "ids": all}));
    assert!(ids.iter().all(|i| y_of(&s, i).abs() < 1e-9));
    s.undo_steps(1).unwrap();

    run(&mut s, "arrange.align", json!({"edge": "bottom", "ids": all}));
    assert!(ids.iter().all(|i| (y_of(&s, i) + get(&s, i).transform.h - 70.0).abs() < 1e-9));
    s.undo_steps(1).unwrap();

    // The old app named these the other way round. center_x makes the x-centres equal.
    run(&mut s, "arrange.align", json!({"edge": "center_x", "ids": all}));
    let c0 = centre_x(&s, &ids[0]);
    assert!(ids.iter().all(|i| (centre_x(&s, i) - c0).abs() < 1e-9));
    assert!(ids.iter().any(|i| (centre_y(&s, i) - centre_y(&s, &ids[0])).abs() > 1.0), "y is untouched");
    s.undo_steps(1).unwrap();
    run(&mut s, "arrange.align", json!({"edge": "center_y", "ids": all}));
    let c0 = centre_y(&s, &ids[0]);
    assert!(ids.iter().all(|i| (centre_y(&s, i) - c0).abs() < 1e-9));
    assert!(ids.iter().any(|i| (centre_x(&s, i) - centre_x(&s, &ids[0])).abs() > 1.0), "x is untouched");
}

#[test]
fn align_one_step_and_undo() {
    let (mut s, ids) = rects(3);
    let out = run(&mut s, "arrange.align", json!({"edge": "left", "ids": ["r0", "r1", "r2"]}));
    assert_eq!(out["undo_label"], "Align");
    assert_eq!(s.tab().doc.undo_depth(), 1);
    assert!(x_of(&s, &ids[2]).abs() < 1e-9);
    // already aligned: nothing moves, no step
    let out = run(&mut s, "arrange.align", json!({"edge": "left", "ids": ["r0", "r1", "r2"]}));
    assert_eq!(out["changed"], false);
    assert_eq!(s.tab().doc.undo_depth(), 1);
    s.undo_steps(1).unwrap();
    assert!((x_of(&s, &ids[2]) - 200.0).abs() < 1e-9);
    s.redo_steps(1).unwrap();
    assert!(x_of(&s, &ids[2]).abs() < 1e-9);
}

#[test]
fn align_to_a_reference_element_leaves_it_in_place() {
    let (mut s, ids) = rects(3);
    setup(&mut s, |tx| tx.update(&ids[2], |e| e.transform.y = 90.0));
    run(&mut s, "arrange.align", json!({"edge": "top", "ids": ["r0", "r1"], "to": "r2"}));
    assert!((y_of(&s, &ids[0]) - 90.0).abs() < 1e-9 && (y_of(&s, &ids[1]) - 90.0).abs() < 1e-9);
    assert!((y_of(&s, &ids[2]) - 90.0).abs() < 1e-9);
    // one element is enough with `to`
    run(&mut s, "arrange.align", json!({"edge": "left", "ids": ["r0"], "to": "r1"}));
    assert!((x_of(&s, &ids[0]) - 100.0).abs() < 1e-9);
    assert_eq!(refuse(&mut s, "arrange.align", json!({"edge": "left", "ids": ["r0"], "to": "nope"})).code, "not_found");
}

#[test]
fn align_treats_a_group_as_one_unit_and_moves_bound_text() {
    let (mut s, ids) = rects(3);
    label(&mut s, "t0", &ids[0]);
    setup(&mut s, |tx| tx.update(&ids[2], |e| e.transform.y = 100.0));
    run(&mut s, "group.create", ids_arg(&[&ids[0], &ids[1]]));
    // The group is one unit, so two units are enough, and its members keep their distance.
    run(&mut s, "arrange.align", json!({"edge": "left", "ids": ["r0", "r2"]}));
    assert!((x_of(&s, &ids[0]) - x_of(&s, &ids[2])).abs() < 1e-9, "the group and r2 share a left edge");
    let gap = x_of(&s, &ids[1]) - x_of(&s, &ids[0]);
    assert!((gap - 100.0).abs() < 1e-9, "members keep their distance: {gap}");
    let label_x = x_of(&s, &drafftink_scene::ElementId::new("t0").unwrap());
    assert!((label_x - (x_of(&s, &ids[0]) + 5.0)).abs() < 1e-9, "the label moved with its container");
}

#[test]
fn align_refusals_leave_state_alone() {
    let (mut s, ids) = rects(3);
    lock(&mut s, &ids[1]);
    let before = s.fingerprint();
    let r = refuse(&mut s, "arrange.align", json!({"edge": "left", "ids": ["r0", "r1"]}));
    assert_eq!(r.code, "locked");
    assert_eq!(refuse(&mut s, "arrange.align", json!({"edge": "middle", "ids": ["r0", "r2"]})).code, "bad_args");
    assert_eq!(refuse(&mut s, "arrange.align", json!({"edge": "left", "ids": ["r0", "nope"]})).code, "not_found");
    assert_eq!(s.fingerprint(), before);
}

#[test]
fn align_has_the_catalog_keys() {
    let (s, _) = rects(1);
    let spec = s.spec("arrange.align").unwrap();
    let got: Vec<(String, String)> = spec.shortcuts.iter().map(|b| (b.chords(false).join(","), b.args.to_owned())).collect();
    for (k, e) in [("Left", "left"), ("Right", "right"), ("Up", "top"), ("Down", "bottom")] {
        assert!(got.contains(&(format!("Mod+Shift+{k}"), format!(r#"{{"edge":"{e}"}}"#))), "{got:?}");
    }
}

// ---- arrange.distribute ----

fn row(widths: &[f64], xs: &[f64]) -> (crate::session::Session, Vec<drafftink_scene::ElementId>) {
    let (mut s, ids) = rects(widths.len());
    setup(&mut s, |tx| {
        for (i, id) in ids.iter().enumerate() {
            tx.update(id, |e| {
                e.transform.x = xs[i];
                e.transform.w = widths[i];
            })?;
        }
        Ok(())
    });
    (s, ids)
}

#[test]
fn distribute_says_how_many_elements_it_needs() {
    let (mut s, ids) = rects(3);
    select(&mut s, &[&ids[0], &ids[1]]);
    let msg = "Distribute needs at least 3 elements (a group counts as one), and 2 were given. Select more, or pass `ids`.";
    let a = s.availability("arrange.distribute").unwrap_err();
    assert_eq!((a.code, a.message.as_str()), ("too_few", msg));
    let e = refuse(&mut s, "arrange.distribute", json!({"axis": "horizontal"}));
    assert_eq!((e.code, e.message.as_str()), ("too_few", msg));
    select(&mut s, &[&ids[0], &ids[1], &ids[2]]);
    assert!(s.availability("arrange.distribute").is_ok());
}

#[test]
fn distribute_equalises_the_gaps_and_keeps_the_outer_elements() {
    let (mut s, ids) = row(&[40.0, 20.0, 60.0, 40.0], &[0.0, 50.0, 70.0, 300.0]);
    let out = run(&mut s, "arrange.distribute", json!({"axis": "horizontal", "ids": ["r0", "r1", "r2", "r3"]}));
    assert_eq!(out["undo_label"], "Distribute");
    assert!(x_of(&s, &ids[0]).abs() < 1e-9, "outer elements stay");
    assert!((x_of(&s, &ids[3]) - 300.0).abs() < 1e-9);
    // the free space is 340 - 160 = 180 over three gaps
    let gap = |a: usize, b: usize| x_of(&s, &ids[b]) - (x_of(&s, &ids[a]) + get(&s, &ids[a]).transform.w);
    for (a, b) in [(0, 1), (1, 2), (2, 3)] {
        assert!((gap(a, b) - 60.0).abs() < 1e-6, "gap {a}-{b} = {}", gap(a, b));
    }
    s.undo_steps(1).unwrap();
    assert!((x_of(&s, &ids[1]) - 50.0).abs() < 1e-9);
}

#[test]
fn distribute_with_an_exact_gap_starts_from_the_first_element() {
    let (mut s, ids) = row(&[40.0, 40.0, 40.0], &[10.0, 20.0, 400.0]);
    run(&mut s, "arrange.distribute", json!({"axis": "horizontal", "ids": ["r0", "r1", "r2"], "gap": 10}));
    assert!((x_of(&s, &ids[0]) - 10.0).abs() < 1e-9);
    assert!((x_of(&s, &ids[1]) - 60.0).abs() < 1e-9);
    assert!((x_of(&s, &ids[2]) - 110.0).abs() < 1e-9);
}

#[test]
fn distribute_vertical_moves_only_y() {
    let (mut s, ids) = rects(3);
    setup(&mut s, |tx| {
        tx.update(&ids[1], |e| e.transform.y = 10.0)?;
        tx.update(&ids[2], |e| e.transform.y = 300.0)
    });
    run(&mut s, "arrange.distribute", json!({"axis": "vertical", "ids": ["r0", "r1", "r2"]}));
    assert_eq!([x_of(&s, &ids[0]), x_of(&s, &ids[1]), x_of(&s, &ids[2])], [0.0, 100.0, 200.0]);
    assert!((y_of(&s, &ids[1]) - 150.0).abs() < 1e-9);
}

#[test]
fn distribute_counts_a_group_as_one_unit() {
    let (mut s, ids) = rects(4);
    run(&mut s, "group.create", ids_arg(&[&ids[0], &ids[1]]));
    // units: the group (r0, r1), r2, r3 -> three units are enough
    assert!(s.availability("arrange.distribute").is_err(), "nothing selected yet");
    run(&mut s, "arrange.distribute", json!({"axis": "horizontal", "ids": ["r0", "r2", "r3"]}));
    assert!((x_of(&s, &ids[1]) - x_of(&s, &ids[0]) - 100.0).abs() < 1e-9, "members keep their distance");
    // with only the group and one more element it is too few
    let r = refuse(&mut s, "arrange.distribute", json!({"axis": "horizontal", "ids": ["r0", "r1", "r2"]}));
    assert_eq!(r.code, "too_few");
    assert!(r.message.contains("and 2 were given"), "{}", r.message);
}

#[test]
fn distribute_refusals_leave_state_alone() {
    let (mut s, ids) = rects(3);
    lock(&mut s, &ids[2]);
    let before = s.fingerprint();
    assert_eq!(refuse(&mut s, "arrange.distribute", json!({"axis": "horizontal", "ids": ["r0", "r1", "r2"]})).code, "locked");
    assert_eq!(refuse(&mut s, "arrange.distribute", json!({"axis": "diagonal", "ids": ["r0", "r1", "r2"]})).code, "bad_args");
    assert_eq!(
        refuse(&mut s, "arrange.distribute", json!({"axis": "vertical", "ids": ["r0", "r1", "r2"], "gap": 1e300})).code,
        "bad_args"
    );
    assert_eq!(s.fingerprint(), before);
}

#[test]
fn distribute_has_the_catalog_keys() {
    let (s, _) = rects(1);
    let spec = s.spec("arrange.distribute").unwrap();
    let got: Vec<(String, String)> = spec.shortcuts.iter().map(|b| (b.chords(false).join(","), b.args.to_owned())).collect();
    assert!(got.contains(&("Alt+H".into(), r#"{"axis":"horizontal"}"#.into())));
    assert!(got.contains(&("Alt+V".into(), r#"{"axis":"vertical"}"#.into())));
}
