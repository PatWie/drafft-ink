//! Tests of the `frame` family. Per command: availability, the exact refusal text, the effect with undo and redo,
//! and a negative case.

use drafftink_scene::ElementKind;
use serde_json::json;

use crate::cmd::group::testkit::*;

fn frame_name(s: &crate::session::Session, id: &drafftink_scene::ElementId) -> Option<String> {
    match &get(s, id).kind {
        ElementKind::Frame(d) => d.name.clone(),
        _ => panic!("{id} is not a frame"),
    }
}

fn parent(s: &crate::session::Session, id: &drafftink_scene::ElementId) -> Option<String> {
    get(s, id).frame_id.as_ref().map(|f| f.as_str().to_owned())
}

// ---- frame.wrap ----

#[test]
fn wrap_needs_a_selection() {
    let (mut s, ids) = rects(2);
    let r = s.availability("frame.wrap").unwrap_err();
    assert_eq!(r.code, "no_target");
    select(&mut s, &[&ids[0]]);
    assert!(s.availability("frame.wrap").is_ok());
    s.view_only = true;
    assert_eq!(s.availability("frame.wrap").unwrap_err().code, "view_only");
}

#[test]
fn wrap_creates_a_frame_with_padding_above_the_elements_and_selects_it() {
    let (mut s, ids) = rects(3); // r0 at 0..40, r1 at 100..140, r2 at 200..240; all y 0..40
    let out = run(&mut s, "frame.wrap", json!({"ids": ["r0", "r1"], "name": "Login"}));
    let f = drafftink_scene::ElementId::new(out["frame"].as_str().unwrap()).unwrap();
    assert_eq!(out["undo_label"], "Wrap in frame");
    assert_eq!(json_ids(&out, "added"), ["r0", "r1"]);
    assert_eq!(out["created"], json!([f.as_str()]));
    let t = &get(&s, &f).transform;
    assert_eq!((t.x, t.y, t.w, t.h), (-16.0, -16.0, 172.0, 72.0));
    assert_eq!(frame_name(&s, &f).as_deref(), Some("Login"));
    assert_eq!(parent(&s, &ids[0]).as_deref(), Some(f.as_str()));
    assert_eq!(parent(&s, &ids[1]).as_deref(), Some(f.as_str()));
    assert_eq!(parent(&s, &ids[2]), None);
    // children sit directly below the frame; the untouched element keeps its place on top
    let o = order(&s);
    assert_eq!(o[..3], ["r0", "r1", f.as_str()]);
    assert_eq!(o[3], "r2");
    assert_eq!(s.selection().iter().map(|i| i.as_str()).collect::<Vec<_>>(), [f.as_str()]);
    assert_eq!(s.tab().doc.undo_depth(), 1);

    s.undo_steps(1).unwrap();
    assert!(s.element(&f).is_none());
    assert_eq!(parent(&s, &ids[0]), None);
    assert!(s.selection().is_empty(), "undo restores the selection from before the step");
    s.redo_steps(1).unwrap();
    assert_eq!(parent(&s, &ids[0]).as_deref(), Some(f.as_str()));
}

#[test]
fn wrap_uses_the_selection_and_the_padding_argument() {
    let (mut s, ids) = rects(2);
    select(&mut s, &[&ids[0]]);
    let out = run(&mut s, "frame.wrap", json!({"padding": 0}));
    let f = drafftink_scene::ElementId::new(out["frame"].as_str().unwrap()).unwrap();
    let t = &get(&s, &f).transform;
    assert_eq!((t.x, t.y, t.w, t.h), (0.0, 0.0, 40.0, 40.0));
    assert_eq!(frame_name(&s, &f), None);
}

#[test]
fn wrap_takes_bound_text_with_its_container() {
    let (mut s, ids) = rects(1);
    let t = label(&mut s, "t0", &ids[0]);
    let out = run(&mut s, "frame.wrap", json!({"ids": ["r0"]}));
    let f = out["frame"].as_str().unwrap().to_owned();
    assert_eq!(parent(&s, &t).as_deref(), Some(f.as_str()));
    assert_eq!(json_ids(&out, "added"), ["r0"], "bound text is not listed");
}

#[test]
fn wrap_refuses_a_frame_with_the_catalog_text() {
    let (mut s, ids) = rects(1);
    let f = frame(&mut s, "f", -10.0, -10.0, 100.0, 100.0);
    let before = s.fingerprint();
    let r = refuse(&mut s, "frame.wrap", ids_arg(&[&ids[0], &f]));
    assert_eq!(r.code, "nested_frame");
    assert_eq!(r.message, "Frames cannot be nested. Remove the frame from the selection, or unwrap it with `frame.unwrap`.");
    assert_eq!(s.fingerprint(), before);
}

#[test]
fn wrap_refusals_leave_state_alone() {
    let (mut s, ids) = rects(2);
    lock(&mut s, &ids[1]);
    let before = s.fingerprint();
    assert_eq!(refuse(&mut s, "frame.wrap", ids_arg(&[&ids[0], &ids[1]])).code, "locked");
    assert_eq!(refuse(&mut s, "frame.wrap", json!({"ids": ["nope"]})).code, "not_found");
    assert_eq!(refuse(&mut s, "frame.wrap", json!({"ids": ["r0"], "padding": -1})).code, "bad_args");
    assert_eq!(refuse(&mut s, "frame.wrap", json!({"ids": ["r0"], "padding": 1e300})).code, "bad_args");
    assert_eq!(refuse(&mut s, "frame.wrap", json!({"ids": ["r0"], "name": ""})).code, "bad_args");
    assert_eq!(s.fingerprint(), before, "a bad name after the frame was made rolls the frame back too");
}

#[test]
fn wrap_has_the_w_key() {
    let (s, _) = rects(1);
    assert_eq!(s.spec("frame.wrap").unwrap().shortcuts[0].keys, ["W"]);
}

// ---- frame.unwrap ----

#[test]
fn unwrap_releases_the_children_deletes_the_frame_and_selects_them() {
    let (mut s, ids) = rects(2);
    let out = run(&mut s, "frame.wrap", json!({"ids": ["r0", "r1"]}));
    let f = drafftink_scene::ElementId::new(out["frame"].as_str().unwrap()).unwrap();
    assert!(s.availability("frame.unwrap").is_ok(), "the wrapped frame is selected");
    let out = run(&mut s, "frame.unwrap", json!({}));
    assert_eq!(out["undo_label"], "Unwrap frame");
    assert_eq!(json_ids(&out, "children"), ["r0", "r1"]);
    assert_eq!(out["deleted"], json!([f.as_str()]));
    assert!(s.element(&f).is_none());
    assert_eq!((parent(&s, &ids[0]), parent(&s, &ids[1])), (None, None));
    assert_eq!(s.selection().len(), 2);
    assert_eq!(order(&s), ["r0", "r1"]);
    s.undo_steps(1).unwrap();
    assert!(s.element(&f).is_some());
    assert_eq!(parent(&s, &ids[0]).as_deref(), Some(f.as_str()));
}

#[test]
fn unwrap_by_ref_works_without_a_selection() {
    let (mut s, ids) = rects(1);
    frame(&mut s, "f", -10.0, -10.0, 100.0, 100.0);
    run(&mut s, "frame.add", json!({"frame": "f", "ids": ["r0"]}));
    s.tab_mut().observed.selection.clear();
    run(&mut s, "frame.unwrap", json!({"frame": "f"}));
    assert_eq!(parent(&s, &ids[0]), None);
}

#[test]
fn unwrap_refuses_a_non_frame_with_the_catalog_text() {
    let (mut s, ids) = rects(2);
    let before = s.fingerprint();
    let r = refuse(&mut s, "frame.unwrap", json!({"frame": "r0"}));
    assert_eq!(r.code, "wrong_kind");
    assert_eq!(r.message, "`frame.unwrap` works on frames, and `r0` is a rectangle.");
    // from the selection: the same text
    select(&mut s, &[&ids[1]]);
    let r = refuse(&mut s, "frame.unwrap", json!({}));
    assert_eq!((r.code, r.message.as_str()), ("wrong_kind", "`frame.unwrap` works on frames, and `r1` is a rectangle."));
    assert_eq!(refuse(&mut s, "frame.unwrap", json!({"frame": "nope"})).code, "not_found");
    s.tab_mut().observed.selection.clear();
    assert_eq!(refuse(&mut s, "frame.unwrap", json!({})).code, "no_target");
    assert_eq!(s.fingerprint(), before);
}

#[test]
fn unwrap_refuses_a_locked_frame_and_several_frames() {
    let (mut s, _) = rects(1);
    let f1 = frame(&mut s, "f1", -10.0, -10.0, 100.0, 100.0);
    let f2 = frame(&mut s, "f2", 300.0, 0.0, 100.0, 100.0);
    select(&mut s, &[&f1, &f2]);
    let r = refuse(&mut s, "frame.unwrap", json!({}));
    assert_eq!(r.code, "needs_single");
    lock(&mut s, &f1);
    let before = s.fingerprint();
    assert_eq!(refuse(&mut s, "frame.unwrap", json!({"frame": "f1"})).code, "locked");
    assert_eq!(s.fingerprint(), before);
}

// ---- frame.rename ----

#[test]
fn rename_sets_and_restores_the_default_label() {
    let (mut s, _) = rects(1);
    let f = frame(&mut s, "f", 0.0, 0.0, 50.0, 50.0);
    let out = run(&mut s, "frame.rename", json!({"frame": "f", "name": "Login"}));
    assert_eq!(out["undo_label"], "Rename frame");
    assert_eq!(frame_name(&s, &f).as_deref(), Some("Login"));
    run(&mut s, "frame.rename", json!({"frame": "f", "name": null}));
    assert_eq!(frame_name(&s, &f), None);
    s.undo_steps(1).unwrap();
    assert_eq!(frame_name(&s, &f).as_deref(), Some("Login"));
    // the same name again changes nothing and pushes no step
    let depth = s.tab().doc.undo_depth();
    let out = run(&mut s, "frame.rename", json!({"frame": "f", "name": "Login"}));
    assert_eq!(out["changed"], false);
    assert_eq!(s.tab().doc.undo_depth(), depth);
}

#[test]
fn rename_refusals() {
    let (mut s, _) = rects(1);
    let f = frame(&mut s, "f", 0.0, 0.0, 50.0, 50.0);
    let before = s.fingerprint();
    let r = refuse(&mut s, "frame.rename", json!({"frame": "r0", "name": "x"}));
    assert_eq!((r.code, r.message.as_str()), ("wrong_kind", "`frame.rename` works on frames, and `r0` is a rectangle."));
    assert_eq!(refuse(&mut s, "frame.rename", json!({"frame": "nope", "name": "x"})).code, "not_found");
    assert_eq!(refuse(&mut s, "frame.rename", json!({"frame": "f", "name": ""})).code, "bad_args");
    assert_eq!(refuse(&mut s, "frame.rename", json!({"frame": "f", "name": "a\u{0}b"})).code, "bad_args");
    assert_eq!(refuse(&mut s, "frame.rename", json!({"frame": "f", "name": "x".repeat(257)})).code, "bad_args");
    assert_eq!(refuse(&mut s, "frame.rename", json!({"name": "x"})).code, "bad_args");
    lock(&mut s, &f);
    assert_eq!(refuse(&mut s, "frame.rename", json!({"frame": "f", "name": "x"})).code, "locked");
    let _ = before;
}

// ---- frame.add ----

#[test]
fn add_makes_children_and_moves_them_out_of_another_frame() {
    let (mut s, ids) = rects(2);
    let f1 = frame(&mut s, "f1", -10.0, -10.0, 60.0, 60.0);
    let f2 = frame(&mut s, "f2", 90.0, -10.0, 60.0, 60.0);
    run(&mut s, "frame.add", json!({"frame": "f1", "ids": ["r0", "r1"]}));
    assert_eq!(parent(&s, &ids[1]).as_deref(), Some("f1"));
    let out = run(&mut s, "frame.add", json!({"frame": "f2", "ids": ["r1"]}));
    assert_eq!(out["undo_label"], "Add to frame");
    assert_eq!(json_ids(&out, "added"), ["r1"]);
    assert_eq!(parent(&s, &ids[1]).as_deref(), Some("f2"));
    assert_eq!(parent(&s, &ids[0]).as_deref(), Some("f1"));
    // children sit right below their own frame
    let o = order(&s);
    let pos = |n: &str| o.iter().position(|x| x == n).unwrap();
    assert!(pos("r0") < pos("f1") && pos("r1") < pos("f2"), "{o:?}");
    s.undo_steps(1).unwrap();
    assert_eq!(parent(&s, &ids[1]).as_deref(), Some("f1"));
    let _ = (f1, f2);
}

#[test]
fn add_uses_the_selection_by_default() {
    let (mut s, ids) = rects(2);
    frame(&mut s, "f", -10.0, -10.0, 300.0, 100.0);
    select(&mut s, &[&ids[0]]);
    run(&mut s, "frame.add", json!({"frame": "f"}));
    assert_eq!(parent(&s, &ids[0]).as_deref(), Some("f"));
    assert_eq!(parent(&s, &ids[1]), None);
}

#[test]
fn add_skips_frames_and_groups_with_frames_and_says_so_in_warnings() {
    let (mut s, ids) = rects(2);
    frame(&mut s, "f", -10.0, -10.0, 300.0, 100.0);
    let other = frame(&mut s, "f2", 500.0, 0.0, 50.0, 50.0);
    let out = run(&mut s, "frame.add", json!({"frame": "f", "ids": ["r0", "f2"]}));
    assert_eq!(json_ids(&out, "added"), ["r0"]);
    assert_eq!(out["skipped"], json!([{"id": "f2", "reason": "is_frame"}]));
    let w = out["warnings"].as_array().unwrap();
    assert_eq!(w.len(), 1);
    assert!(w[0].as_str().unwrap().contains("`f2` was not added to the frame"), "{w:?}");
    assert_eq!(parent(&s, &other), None);
    // a group that contains a frame
    run(&mut s, "group.create", ids_arg(&[&ids[1], &other]));
    let out = run(&mut s, "frame.add", json!({"frame": "f", "ids": ["r1"]}));
    assert_eq!(json_ids(&out, "added").len(), 0);
    assert_eq!(out["skipped"][0]["reason"], "group_contains_frame");
}

#[test]
fn add_joins_an_arrow_only_when_an_end_is_attached_to_a_member() {
    let (mut s, ids) = rects(2);
    frame(&mut s, "f", -10.0, -10.0, 100.0, 100.0);
    let bound = arrow_bound_to(&mut s, "a1", &ids[0], 20.0, 20.0, 60.0);
    let free = arrow_bound_to(&mut s, "a2", &ids[1], 400.0, 400.0, 30.0);
    let out = run(&mut s, "frame.add", json!({"frame": "f", "ids": ["r0", "a1", "a2"]}));
    assert_eq!(parent(&s, &bound).as_deref(), Some("f"));
    assert_eq!(parent(&s, &free), None);
    assert_eq!(out["skipped"], json!([{"id": "a2", "reason": "arrow_not_attached"}]));
}

#[test]
fn add_refusals_leave_state_alone() {
    let (mut s, ids) = rects(2);
    let f = frame(&mut s, "f", -10.0, -10.0, 300.0, 100.0);
    lock(&mut s, &ids[1]);
    let before = s.fingerprint();
    let r = refuse(&mut s, "frame.add", json!({"frame": "r0", "ids": ["r1"]}));
    assert_eq!((r.code, r.message.as_str()), ("wrong_kind", "`frame.add` works on frames, and `r0` is a rectangle."));
    assert_eq!(refuse(&mut s, "frame.add", json!({"frame": "f", "ids": ["r1"]})).code, "locked");
    assert_eq!(refuse(&mut s, "frame.add", json!({"frame": "nope", "ids": ["r0"]})).code, "not_found");
    assert_eq!(refuse(&mut s, "frame.add", json!({"frame": "f", "ids": ["nope"]})).code, "not_found");
    assert_eq!(refuse(&mut s, "frame.add", json!({"frame": "f"})).code, "no_target");
    assert_eq!(refuse(&mut s, "frame.add", json!({"ids": ["r0"]})).code, "bad_args");
    assert_eq!(s.fingerprint(), before);
    lock(&mut s, &f);
    assert_eq!(refuse(&mut s, "frame.add", json!({"frame": "f", "ids": ["r0"]})).code, "locked");
}

// ---- frame.remove ----

#[test]
fn remove_clears_the_frame_of_the_given_elements() {
    let (mut s, ids) = rects(3);
    frame(&mut s, "f", -10.0, -10.0, 300.0, 100.0);
    run(&mut s, "frame.add", json!({"frame": "f", "ids": ["r0", "r1", "r2"]}));
    let out = run(&mut s, "frame.remove", json!({"ids": ["r1"]}));
    assert_eq!(out["undo_label"], "Remove from frame");
    assert_eq!(json_ids(&out, "removed"), ["r1"]);
    assert_eq!(parent(&s, &ids[1]), None);
    assert_eq!(parent(&s, &ids[0]).as_deref(), Some("f"));
    // the remaining children stay one block directly below the frame
    let o = order(&s);
    let pos = |n: &str| o.iter().position(|x| x == n).unwrap();
    assert_eq!(pos("f") - pos("r0").max(pos("r2")), 1, "{o:?}");
    s.undo_steps(1).unwrap();
    assert_eq!(parent(&s, &ids[1]).as_deref(), Some("f"));
}

#[test]
fn remove_a_whole_frame_and_by_selection() {
    let (mut s, ids) = rects(3);
    frame(&mut s, "f", -10.0, -10.0, 300.0, 100.0);
    run(&mut s, "frame.add", json!({"frame": "f", "ids": ["r0", "r1", "r2"]}));
    select(&mut s, &[&ids[0]]);
    run(&mut s, "frame.remove", json!({}));
    assert_eq!(parent(&s, &ids[0]), None);
    let out = run(&mut s, "frame.remove", json!({"frame": "f"}));
    assert_eq!(json_ids(&out, "removed"), ["r1", "r2"]);
    assert!(ids.iter().all(|i| parent(&s, i).is_none()));
}

#[test]
fn remove_with_frame_and_ids_only_takes_those_that_are_its_children() {
    let (mut s, ids) = rects(2);
    frame(&mut s, "f1", -10.0, -10.0, 60.0, 60.0);
    frame(&mut s, "f2", 90.0, -10.0, 60.0, 60.0);
    run(&mut s, "frame.add", json!({"frame": "f1", "ids": ["r0"]}));
    run(&mut s, "frame.add", json!({"frame": "f2", "ids": ["r1"]}));
    run(&mut s, "frame.remove", json!({"frame": "f1", "ids": ["r0", "r1"]}));
    assert_eq!(parent(&s, &ids[0]), None);
    assert_eq!(parent(&s, &ids[1]).as_deref(), Some("f2"), "r1 is a child of f2, not f1");
}

#[test]
fn remove_with_nothing_in_a_frame_is_nothing_to_do() {
    let (mut s, ids) = rects(2);
    let f = frame(&mut s, "f", 500.0, 500.0, 50.0, 50.0);
    let before = s.fingerprint();
    let r = refuse(&mut s, "frame.remove", json!({"ids": ["r0", "r1"]}));
    assert_eq!(r.code, "nothing_to_do");
    assert_eq!(r.message, "Nothing to remove: none of the given elements is in a frame.");
    assert_eq!(refuse(&mut s, "frame.remove", json!({"frame": "f"})).code, "nothing_to_do");
    assert_eq!(refuse(&mut s, "frame.remove", json!({})).code, "no_target");
    assert_eq!(refuse(&mut s, "frame.remove", json!({"frame": "r0"})).code, "wrong_kind");
    assert_eq!(refuse(&mut s, "frame.remove", json!({"ids": ["nope"]})).code, "not_found");
    assert_eq!(s.fingerprint(), before);
    let _ = (ids, f);
}

#[test]
fn remove_refuses_locked_children() {
    let (mut s, ids) = rects(2);
    frame(&mut s, "f", -10.0, -10.0, 300.0, 100.0);
    run(&mut s, "frame.add", json!({"frame": "f", "ids": ["r0", "r1"]}));
    lock(&mut s, &ids[1]);
    let before = s.fingerprint();
    assert_eq!(refuse(&mut s, "frame.remove", json!({"frame": "f"})).code, "locked");
    assert_eq!(s.fingerprint(), before);
}

// ---- frame.select_children ----

#[test]
fn select_children_selects_them_without_bound_text_and_makes_no_undo_step() {
    let (mut s, ids) = rects(3);
    let t = label(&mut s, "t0", &ids[0]);
    frame(&mut s, "f", -10.0, -10.0, 300.0, 100.0);
    run(&mut s, "frame.add", json!({"frame": "f", "ids": ["r0", "r1"]}));
    s.tab_mut().observed.selection.clear();
    let depth = s.tab().doc.undo_depth();
    let out = run(&mut s, "frame.select_children", json!({"frame": "f"}));
    assert_eq!(json_ids(&out, "selected"), ["r0", "r1"]);
    assert!(!s.selection().contains(&t), "no bound text entry");
    assert_eq!(s.selection().len(), 2);
    assert_eq!(s.tab().doc.undo_depth(), depth);
    // allowed in view-only mode: it changes the selection only
    select(&mut s, &[&drafftink_scene::ElementId::new("f").unwrap()]);
    s.view_only = true;
    assert!(s.availability("frame.select_children").is_ok());
}

#[test]
fn select_children_refusals() {
    let (mut s, _) = rects(1);
    frame(&mut s, "f", 500.0, 500.0, 50.0, 50.0);
    let before = s.fingerprint();
    let r = refuse(&mut s, "frame.select_children", json!({"frame": "f"}));
    assert_eq!((r.code, r.message.as_str()), ("nothing_to_do", "`f` has no children."));
    let r = refuse(&mut s, "frame.select_children", json!({"frame": "r0"}));
    assert_eq!((r.code, r.message.as_str()), ("wrong_kind", "`frame.select_children` works on frames, and `r0` is a rectangle."));
    assert_eq!(refuse(&mut s, "frame.select_children", json!({"frame": "nope"})).code, "not_found");
    assert_eq!(s.fingerprint(), before);
}

// ---- availability follows the selection ----

#[test]
fn the_frame_commands_say_why_they_are_unavailable_with_the_text_execute_gives() {
    let (mut s, ids) = rects(2);
    let f = frame(&mut s, "f", -10.0, -10.0, 60.0, 60.0);
    run(&mut s, "frame.add", json!({"frame": "f", "ids": ["r0"]}));
    for id in ["frame.unwrap", "frame.select_children"] {
        assert_eq!(s.availability(id).unwrap_err().code, "no_target", "{id}: nothing selected");
    }
    assert_eq!(s.availability("frame.remove").unwrap_err().code, "nothing_to_do");

    select(&mut s, &[&ids[1]]);
    for id in ["frame.unwrap", "frame.select_children"] {
        assert_eq!(s.availability(id).unwrap_err().code, "wrong_kind", "{id}: a rectangle is selected");
    }
    for id in ["frame.unwrap", "frame.select_children"] {
        let shown = s.availability(id).unwrap_err();
        let ran = refuse(&mut s, id, json!({}));
        assert_eq!(shown.message, ran.message, "{id}: the disabled control shows what a click would answer");
    }
    assert_eq!(s.availability("frame.remove").unwrap_err().code, "nothing_to_do", "r1 is in no frame");

    select(&mut s, &[&ids[0]]);
    assert!(s.availability("frame.remove").is_ok(), "r0 is in a frame");
    select(&mut s, &[&f]);
    assert!(s.availability("frame.unwrap").is_ok() && s.availability("frame.select_children").is_ok());
}

#[test]
fn select_children_takes_the_selected_frame_when_none_is_given() {
    let (mut s, _) = rects(2);
    let f = frame(&mut s, "f", -10.0, -10.0, 60.0, 60.0);
    run(&mut s, "frame.add", json!({"frame": "f", "ids": ["r0"]}));
    select(&mut s, &[&f]);
    let out = run(&mut s, "frame.select_children", json!({}));
    assert_eq!(json_ids(&out, "selected"), ["r0"]);
    let r = refuse(&mut s, "frame.select_children", json!({}));
    assert_eq!(r.code, "wrong_kind", "r0 is selected now, and it is no frame");
}

// ---- interplay ----

#[test]
fn grouping_members_from_different_frames_takes_them_out_of_their_frames() {
    let (mut s, ids) = rects(2);
    frame(&mut s, "f1", -10.0, -10.0, 60.0, 60.0);
    frame(&mut s, "f2", 90.0, -10.0, 60.0, 60.0);
    run(&mut s, "frame.add", json!({"frame": "f1", "ids": ["r0"]}));
    run(&mut s, "frame.add", json!({"frame": "f2", "ids": ["r1"]}));
    let out = run(&mut s, "group.create", ids_arg(&[&ids[0], &ids[1]]));
    assert_eq!(json_ids(&out, "left_frames"), ["r0", "r1"]);
    assert_eq!((parent(&s, &ids[0]), parent(&s, &ids[1])), (None, None));
}

#[test]
fn a_group_joins_a_frame_as_a_whole() {
    let (mut s, ids) = rects(3);
    frame(&mut s, "f", -10.0, -10.0, 300.0, 100.0);
    run(&mut s, "group.create", ids_arg(&[&ids[0], &ids[1]]));
    run(&mut s, "frame.add", json!({"frame": "f", "ids": ["r0"]}));
    assert_eq!(parent(&s, &ids[0]).as_deref(), Some("f"));
    assert_eq!(parent(&s, &ids[1]).as_deref(), Some("f"), "the group's other member comes along");
    assert_eq!(parent(&s, &ids[2]), None);
}
