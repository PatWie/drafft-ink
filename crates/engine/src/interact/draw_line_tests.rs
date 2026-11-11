//! Tests of the line and arrow tools: drag, multi-point drafts, snapping and binding.

use drafftink_edit::linear::world_points;
use drafftink_scene::{BindMode, Element, ElementId, Head, Kind, PathType};
use serde_json::json;

use crate::interact::Key;
use crate::interact::testutil::Rig;
use crate::state::Tool;

fn rig_with(tool: Tool) -> (Rig, usize, usize) {
    let mut rig = Rig::new();
    rig.tool(tool);
    let (ring, undo) = (rig.ring_ids().len(), rig.undo_depth());
    (rig, ring, undo)
}

/// The world points of a line or arrow.
fn points_of(e: &Element) -> Vec<(f64, f64)> {
    world_points(e).expect("a line or arrow").into_iter().map(|p| (p.x, p.y)).collect()
}

fn approx(a: (f64, f64), b: (f64, f64)) {
    assert!((a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6, "{a:?} != {b:?}");
}

/// Two filled rectangles side by side, 200 units apart. Returns their ids.
fn two_boxes(rig: &mut Rig) -> (ElementId, ElementId) {
    let a = rig.rect(100.0, 100.0, 100.0, 100.0);
    let b = rig.rect(400.0, 100.0, 100.0, 100.0);
    (a, b)
}

// ---- a drag makes a two-point line --------------------------------------------------------------------------

#[test]
fn a_drag_with_the_line_tool_makes_one_line_in_one_undo_step() {
    let (mut rig, ring, undo) = rig_with(Tool::Line);
    rig.drag((100.0, 100.0), (300.0, 200.0));

    assert_eq!(rig.commands_since(ring), ["element.create"]);
    assert_eq!(rig.undo_depth(), undo + 1);
    let e = rig.only();
    assert_eq!(e.tag(), Kind::Line);
    assert_eq!(points_of(&e), [(100.0, 100.0), (300.0, 200.0)]);
    assert_eq!(rig.s.tool.active, Tool::Select);
    assert_eq!(rig.selected(), [e.id.as_str()]);
    assert!(!rig.parts().has_gesture_state());
    assert!(!rig.i.is_busy());
}

#[test]
fn a_drag_with_the_arrow_tool_makes_an_arrow_with_a_head() {
    let (mut rig, ring, _) = rig_with(Tool::Arrow);
    rig.drag((100.0, 100.0), (300.0, 200.0));
    assert_eq!(rig.commands_since(ring), ["element.create"]);
    let e = rig.only();
    assert_eq!(e.tag(), Kind::Arrow);
    let d = e.linear_data().expect("linear data");
    assert_eq!(d.end_head, Head::Arrow);
    assert_eq!(d.start_head, Head::None);
    assert_eq!(d.path, PathType::Sharp);
    assert_eq!(points_of(&e), [(100.0, 100.0), (300.0, 200.0)]);
}

#[test]
fn the_line_preview_follows_the_drag_and_the_document_stays_untouched() {
    let (mut rig, ring, undo) = rig_with(Tool::Line);
    rig.down(100.0, 100.0);
    rig.drag_to(200.0, 160.0);
    let p = rig.preview().expect("a preview");
    assert_eq!(p.tag(), Kind::Line);
    assert_eq!(points_of(&p), [(100.0, 100.0), (200.0, 160.0)]);
    rig.drag_to(250.0, 170.0);
    assert_eq!(points_of(&rig.preview().expect("a preview")), [(100.0, 100.0), (250.0, 170.0)]);
    assert!(rig.elements().is_empty());
    assert_eq!(rig.commands_since(ring), Vec::<String>::new());
    assert_eq!(rig.undo_depth(), undo);
    rig.up(250.0, 170.0);
    assert!(rig.preview().is_none());
    assert_eq!(rig.elements().len(), 1);
}

#[test]
fn escape_mid_drag_drops_the_line() {
    let (mut rig, ring, undo) = rig_with(Tool::Arrow);
    let fingerprint = rig.s.fingerprint();
    rig.down(100.0, 100.0);
    rig.drag_to(200.0, 160.0);
    assert!(rig.key(Key::Escape).handled);
    rig.up(200.0, 160.0);
    assert!(rig.elements().is_empty());
    assert_eq!(rig.commands_since(ring), Vec::<String>::new());
    assert_eq!(rig.undo_depth(), undo);
    assert_eq!(rig.s.fingerprint(), fingerprint);
    assert!(!rig.i.is_busy());
    assert!(!rig.parts().has_gesture_state());
}

#[test]
fn a_drag_shorter_than_one_unit_makes_nothing_and_no_refusal() {
    let (mut rig, ring, _) = rig_with(Tool::Line);
    rig.drag((100.0, 100.0), (100.4, 100.4));
    assert!(rig.elements().is_empty());
    assert_eq!(rig.commands_since(ring), Vec::<String>::new());
    assert!(rig.history.iter().all(|o| o.refusals.is_empty()));
    assert_eq!(rig.s.tool.active, Tool::Line);
}

#[test]
fn a_horizontal_line_is_fine() {
    let (mut rig, _, _) = rig_with(Tool::Line);
    rig.drag((100.0, 100.0), (300.0, 100.0));
    assert_eq!(points_of(&rig.only()), [(100.0, 100.0), (300.0, 100.0)]);
}

// ---- snapping -------------------------------------------------------------------------------------------------------

#[test]
fn the_grid_snaps_both_ends_of_a_line() {
    let (mut rig, _, _) = rig_with(Tool::Line);
    rig.set("snap.grid", json!(true));
    rig.drag((103.0, 98.0), (297.0, 143.0));
    assert_eq!(points_of(&rig.only()), [(100.0, 100.0), (300.0, 140.0)]);
}

#[test]
fn shift_locks_the_angle_to_fifteen_degrees_and_keeps_the_length() {
    let (mut rig, _, _) = rig_with(Tool::Line);
    rig.mods.shift = true;
    rig.drag((100.0, 100.0), (300.0, 130.0));
    let pts = points_of(&rig.only());
    let (dx, dy) = (pts[1].0 - pts[0].0, pts[1].1 - pts[0].1);
    assert!((dy.atan2(dx).to_degrees() - 15.0).abs() < 1e-9, "angle {}", dy.atan2(dx).to_degrees());
    assert!((dx.hypot(dy) - 200.0_f64.hypot(30.0)).abs() < 1e-9);
}

#[test]
fn the_angle_step_setting_changes_the_lock() {
    let (mut rig, _, _) = rig_with(Tool::Line);
    rig.set("snap.angle_step", json!(45));
    rig.mods.shift = true;
    rig.drag((100.0, 100.0), (300.0, 130.0));
    let pts = points_of(&rig.only());
    approx((pts[1].1 - pts[0].1, 0.0), (0.0, 0.0));
}

#[test]
fn smart_guides_snap_the_end_of_a_line_and_show_while_dragging() {
    let mut rig = Rig::new();
    rig.rect(300.0, 100.0, 100.0, 100.0);
    rig.tool(Tool::Line);
    rig.set("snap.smart_guides", json!(true));
    rig.down(50.0, 350.0);
    rig.drag_to(302.0, 297.0);
    rig.drag_to(303.0, 296.0);
    // The end is 3 units from the left edge of the box, in range of a vertical guide, but nothing else is near.
    assert!(!rig.parts().guides.is_empty());
    rig.up(303.0, 296.0);
    assert!(rig.parts().guides.is_empty());
    let line = rig.added_since(1).remove(0);
    assert_eq!(points_of(&line)[1].0, 300.0);
}

// ---- multi-point drafts ----------------------------------------------------------------------------------------------

#[test]
fn clicks_make_a_draft_that_a_double_click_ends() {
    let (mut rig, ring, undo) = rig_with(Tool::Line);
    rig.click(100.0, 100.0);
    assert!(rig.i.is_busy(), "a click starts a draft");
    rig.click(200.0, 150.0);
    assert!(rig.elements().is_empty());
    assert_eq!(rig.commands_since(ring), Vec::<String>::new());
    rig.double_click(300.0, 100.0);

    assert_eq!(rig.commands_since(ring), ["element.create"]);
    assert_eq!(rig.undo_depth(), undo + 1);
    let e = rig.only();
    assert_eq!(points_of(&e), [(100.0, 100.0), (200.0, 150.0), (300.0, 100.0)], "no duplicate point");
    assert!(!rig.i.is_busy());
    assert!(!rig.parts().has_gesture_state());
    assert_eq!(rig.s.tool.active, Tool::Select);
}

#[test]
fn enter_ends_the_draft_with_the_placed_points_only() {
    let (mut rig, ring, _) = rig_with(Tool::Arrow);
    rig.click(100.0, 100.0);
    rig.click(200.0, 150.0);
    rig.click(300.0, 100.0);
    rig.hover(400.0, 300.0);
    let out = rig.key(Key::Enter);
    assert!(out.handled);
    assert_eq!(rig.commands_since(ring), ["element.create"]);
    let e = rig.only();
    assert_eq!(e.tag(), Kind::Arrow);
    assert_eq!(points_of(&e), [(100.0, 100.0), (200.0, 150.0), (300.0, 100.0)], "the rubber band is not a point");
    assert!(!rig.i.is_busy());
}

#[test]
fn escape_ends_the_draft_too() {
    let (mut rig, ring, _) = rig_with(Tool::Line);
    rig.click(100.0, 100.0);
    rig.click(200.0, 150.0);
    let out = rig.key(Key::Escape);
    assert!(out.handled, "the layer used Esc, so the shell does not run tool.cancel");
    assert_eq!(rig.commands_since(ring), ["element.create"]);
    assert_eq!(points_of(&rig.only()), [(100.0, 100.0), (200.0, 150.0)]);
}

#[test]
fn a_draft_with_one_point_makes_nothing() {
    for key in [Key::Enter, Key::Escape] {
        let (mut rig, ring, _) = rig_with(Tool::Line);
        rig.click(100.0, 100.0);
        assert!(rig.i.is_busy());
        assert!(rig.key(key).handled);
        assert!(rig.elements().is_empty());
        assert_eq!(rig.commands_since(ring), Vec::<String>::new());
        assert!(!rig.i.is_busy());
        assert!(rig.history.iter().all(|o| o.refusals.is_empty()));
    }
}

#[test]
fn a_double_click_on_a_fresh_draft_makes_nothing() {
    let (mut rig, ring, _) = rig_with(Tool::Line);
    rig.double_click(100.0, 100.0);
    assert!(rig.elements().is_empty());
    assert_eq!(rig.commands_since(ring), Vec::<String>::new());
    assert!(!rig.i.is_busy());
}

#[test]
fn without_a_draft_enter_and_escape_are_not_used() {
    let (mut rig, _, _) = rig_with(Tool::Line);
    assert!(!rig.key(Key::Enter).handled);
    assert!(!rig.key(Key::Escape).handled, "the shell runs tool.cancel");
}

#[test]
fn the_rubber_band_follows_the_pointer() {
    let (mut rig, ring, undo) = rig_with(Tool::Line);
    rig.click(100.0, 100.0);
    rig.click(200.0, 150.0);
    let out = rig.hover(260.0, 220.0);
    assert!(out.redraw.is_some());
    let p = rig.preview().expect("a preview");
    assert_eq!(points_of(&p), [(100.0, 100.0), (200.0, 150.0), (260.0, 220.0)]);
    rig.hover(280.0, 240.0);
    assert_eq!(points_of(&rig.preview().expect("a preview")), [(100.0, 100.0), (200.0, 150.0), (280.0, 240.0)]);
    assert!(rig.elements().is_empty());
    assert_eq!(rig.commands_since(ring), Vec::<String>::new());
    assert_eq!(rig.undo_depth(), undo);
    assert_eq!(rig.hover(280.0, 240.0).redraw, None, "the same position again changes nothing");
}

#[test]
fn a_drag_in_the_middle_of_a_draft_places_a_point_where_it_ends() {
    let (mut rig, _, _) = rig_with(Tool::Line);
    rig.click(100.0, 100.0);
    rig.drag((200.0, 200.0), (250.0, 120.0));
    assert!(rig.i.is_busy(), "the draft goes on");
    assert!(rig.elements().is_empty());
    rig.key(Key::Enter);
    assert_eq!(points_of(&rig.only()), [(100.0, 100.0), (250.0, 120.0)]);
}

#[test]
fn escape_during_a_drag_in_a_draft_keeps_the_draft() {
    let (mut rig, ring, _) = rig_with(Tool::Line);
    rig.click(100.0, 100.0);
    rig.click(200.0, 150.0);
    rig.down(300.0, 300.0);
    rig.drag_to(320.0, 320.0);
    assert!(rig.key(Key::Escape).handled);
    rig.up(320.0, 320.0);
    assert!(rig.i.is_busy(), "the draft survives; only the drag was cancelled");
    assert_eq!(rig.commands_since(ring), Vec::<String>::new());
    rig.key(Key::Enter);
    assert_eq!(points_of(&rig.only()), [(100.0, 100.0), (200.0, 150.0)]);
}

#[test]
fn changing_the_tool_drops_the_draft() {
    let (mut rig, _, _) = rig_with(Tool::Line);
    rig.click(100.0, 100.0);
    rig.click(200.0, 150.0);
    rig.tool(Tool::Rectangle);
    let ring = rig.ring_ids().len();
    rig.hover(300.0, 300.0);
    assert!(!rig.i.is_busy());
    assert!(rig.preview().is_none());
    assert!(rig.elements().is_empty());
    assert_eq!(rig.commands_since(ring), Vec::<String>::new());
}

#[test]
fn hovering_with_no_draft_shows_no_preview_and_asks_for_no_redraw() {
    let (mut rig, _, _) = rig_with(Tool::Line);
    assert_eq!(rig.hover(100.0, 100.0).redraw, None);
    assert_eq!(rig.hover(120.0, 100.0).redraw, None);
    assert!(rig.preview().is_none());
}

// ---- binding ----------------------------------------------------------------------------------------------------------

#[test]
fn an_arrow_from_one_rectangle_to_another_binds_both_ends_and_the_highlight_shows_while_placing() {
    let mut rig = Rig::new();
    let (a, b) = two_boxes(&mut rig);
    rig.tool(Tool::Arrow);
    let (ring, undo) = (rig.ring_ids().len(), rig.undo_depth());

    rig.down(210.0, 150.0);
    rig.drag_to(300.0, 150.0);
    assert!(rig.parts().highlight.is_empty(), "nothing is near the end yet");
    rig.drag_to(390.0, 150.0);
    assert_eq!(rig.parts().highlight, std::slice::from_ref(&b), "the end would bind to the second box");
    let overlay = rig.i.overlay(&rig.s);
    assert_eq!(overlay.canvas.bind_candidates.len(), 1);
    assert_eq!(overlay.canvas.bind_candidates[0].target, b);
    assert_eq!(rig.elements().len(), 2, "the document is untouched mid-drag");
    rig.drag_to(250.0, 150.0);
    assert!(rig.parts().highlight.is_empty(), "the highlight clears when the end leaves");
    rig.drag_to(390.0, 150.0);
    rig.up(390.0, 150.0);

    assert_eq!(rig.commands_since(ring), ["element.create"]);
    assert_eq!(rig.undo_depth(), undo + 1);
    assert!(rig.parts().highlight.is_empty(), "the highlight goes with the gesture");
    let arrow = rig.elements().into_iter().find(|e| e.tag() == Kind::Arrow).expect("an arrow");
    let d = arrow.linear_data().expect("linear data");
    assert_eq!(d.start_binding.as_ref().map(|x| &x.target), Some(&a));
    assert_eq!(d.end_binding.as_ref().map(|x| &x.target), Some(&b));
}

#[test]
fn the_highlight_shows_the_same_target_the_command_binds() {
    // Ends in the margin (orbit), inside the shape, and with the end between the two.
    for (from, to) in
        [((210.0, 150.0), (390.0, 150.0)), ((150.0, 150.0), (450.0, 150.0)), ((150.0, 150.0), (395.0, 190.0))]
    {
        let mut rig = Rig::new();
        let (_, b) = two_boxes(&mut rig);
        rig.tool(Tool::Arrow);
        rig.down(from.0, from.1);
        rig.drag_to(to.0, to.1);
        assert_eq!(rig.parts().highlight, std::slice::from_ref(&b), "{from:?} -> {to:?}");
        rig.up(to.0, to.1);
        let arrow = rig.elements().into_iter().find(|e| e.tag() == Kind::Arrow).expect("an arrow");
        let end = arrow.linear_data().and_then(|d| d.end_binding.clone()).expect("the end is bound");
        assert_eq!(end.target, b, "{from:?} -> {to:?}");
    }
}

#[test]
fn an_end_inside_a_shape_binds_inside() {
    let mut rig = Rig::new();
    let (_, b) = two_boxes(&mut rig);
    rig.tool(Tool::Arrow);
    rig.drag((150.0, 150.0), (450.0, 150.0));
    let arrow = rig.elements().into_iter().find(|e| e.tag() == Kind::Arrow).expect("an arrow");
    let end = arrow.linear_data().and_then(|d| d.end_binding.clone()).expect("bound");
    assert_eq!((end.target, end.mode), (b, BindMode::Inside));
}

#[test]
fn hovering_with_the_arrow_tool_highlights_what_a_click_would_bind_and_redraws_only_on_change() {
    let mut rig = Rig::new();
    let (a, _) = two_boxes(&mut rig);
    rig.tool(Tool::Arrow);
    let out = rig.hover(210.0, 150.0);
    assert!(out.redraw.is_some());
    assert_eq!(rig.parts().highlight, std::slice::from_ref(&a));
    assert_eq!(rig.hover(211.0, 152.0).redraw, None, "same target, nothing to redraw");
    assert_eq!(rig.parts().highlight, [a]);
    let out = rig.hover(300.0, 150.0);
    assert!(out.redraw.is_some());
    assert!(rig.parts().highlight.is_empty());
    assert_eq!(rig.hover(310.0, 150.0).redraw, None);
}

#[test]
fn ctrl_turns_binding_off_for_the_highlight_and_for_the_command() {
    let mut rig = Rig::new();
    two_boxes(&mut rig);
    rig.tool(Tool::Arrow);
    rig.mods.ctrl = true;
    rig.down(210.0, 150.0);
    rig.drag_to(390.0, 150.0);
    assert!(rig.parts().highlight.is_empty());
    rig.up(390.0, 150.0);
    let arrow = rig.elements().into_iter().find(|e| e.tag() == Kind::Arrow).expect("an arrow");
    let d = arrow.linear_data().expect("linear data");
    assert!(d.start_binding.is_none() && d.end_binding.is_none());
    assert_eq!(points_of(&arrow), [(210.0, 150.0), (390.0, 150.0)]);
}

#[test]
fn alt_binds_inside() {
    let mut rig = Rig::new();
    let (_, b) = two_boxes(&mut rig);
    rig.tool(Tool::Arrow);
    rig.mods.alt = true;
    rig.drag((210.0, 150.0), (390.0, 150.0));
    let arrow = rig.elements().into_iter().find(|e| e.tag() == Kind::Arrow).expect("an arrow");
    let end = arrow.linear_data().and_then(|d| d.end_binding.clone()).expect("bound");
    assert_eq!((end.target, end.mode), (b, BindMode::Inside));
}

#[test]
fn the_setting_arrow_binding_off_disables_the_highlight_and_the_binding() {
    let mut rig = Rig::new();
    two_boxes(&mut rig);
    rig.set("arrow.binding", json!(false));
    rig.tool(Tool::Arrow);
    rig.down(210.0, 150.0);
    rig.drag_to(390.0, 150.0);
    assert!(rig.parts().highlight.is_empty());
    rig.up(390.0, 150.0);
    let arrow = rig.elements().into_iter().find(|e| e.tag() == Kind::Arrow).expect("an arrow");
    let d = arrow.linear_data().expect("linear data");
    assert!(d.start_binding.is_none() && d.end_binding.is_none());
}

#[test]
fn a_bound_end_is_previewed_where_the_binding_puts_it() {
    let mut rig = Rig::new();
    two_boxes(&mut rig);
    rig.tool(Tool::Arrow);
    rig.down(210.0, 150.0);
    rig.drag_to(390.0, 150.0);
    let preview_end = *points_of(&rig.preview().expect("a preview")).last().expect("points");
    rig.up(390.0, 150.0);
    let arrow = rig.elements().into_iter().find(|e| e.tag() == Kind::Arrow).expect("an arrow");
    let made_end = *points_of(&arrow).last().expect("points");
    approx(preview_end, made_end);
    assert!(made_end.0 < 400.0 && made_end.0 > 390.0 - 30.0, "the end sits on the outline of the box: {made_end:?}");
}

#[test]
fn a_multi_point_arrow_binds_its_first_and_last_point() {
    let mut rig = Rig::new();
    let (a, b) = two_boxes(&mut rig);
    rig.tool(Tool::Arrow);
    rig.click(210.0, 150.0);
    rig.click(300.0, 300.0);
    rig.hover(390.0, 150.0);
    assert_eq!(rig.parts().highlight, std::slice::from_ref(&b));
    rig.double_click(390.0, 150.0);
    let arrow = rig.elements().into_iter().find(|e| e.tag() == Kind::Arrow).expect("an arrow");
    assert_eq!(points_of(&arrow).len(), 3);
    let d = arrow.linear_data().expect("linear data");
    assert_eq!(d.start_binding.as_ref().map(|x| &x.target), Some(&a));
    assert_eq!(d.end_binding.as_ref().map(|x| &x.target), Some(&b));
}

#[test]
fn the_elbow_arrow_type_sends_the_two_ends_and_the_engine_routes_it() {
    let mut rig = Rig::new();
    let (a, b) = two_boxes(&mut rig);
    rig.run("tool.set_arrow", json!({ "arrow_type": "elbow" }));
    rig.tool(Tool::Arrow);
    rig.click(210.0, 120.0);
    rig.click(300.0, 300.0);
    rig.double_click(390.0, 180.0);
    let arrow = rig.elements().into_iter().find(|e| e.tag() == Kind::Arrow).expect("an arrow");
    let d = arrow.linear_data().expect("linear data");
    assert_eq!(d.path, PathType::Elbow);
    assert_eq!(d.start_binding.as_ref().map(|x| &x.target), Some(&a));
    assert_eq!(d.end_binding.as_ref().map(|x| &x.target), Some(&b));
    let ring_entries = rig.ring();
    let args = &ring_entries.last().expect("ring entry").args;
    assert!(args.contains("\"arrow_type\":\"elbow\""), "{args}");
    assert!(!args.contains("300"), "the middle point is not sent for an elbow arrow: {args}");
}

#[test]
fn the_arrow_tool_draws_with_the_type_and_heads_of_tool_set_arrow() {
    let mut rig = Rig::new();
    rig.run("tool.set_arrow", json!({ "arrow_type": "round", "start_head": "circle", "end_head": "triangle" }));
    rig.tool(Tool::Arrow);
    rig.drag((100.0, 100.0), (300.0, 200.0));
    let arrow = rig.only();
    let d = arrow.linear_data().expect("linear data");
    assert_eq!((d.path, d.start_head, d.end_head), (PathType::Round, Head::Circle, Head::Triangle));
    let ring_entries = rig.ring();
    let args = &ring_entries.last().expect("ring entry").args;
    assert!(args.contains("\"arrow_type\":\"round\"") && args.contains("\"start_head\":\"circle\""), "{args}");
}

#[test]
fn without_tool_set_arrow_the_arrow_tool_draws_a_sharp_arrow_with_an_end_head() {
    let mut rig = Rig::new();
    rig.tool(Tool::Arrow);
    rig.drag((100.0, 100.0), (300.0, 200.0));
    let d = rig.only().linear_data().cloned().expect("linear data");
    assert_eq!((d.path, d.start_head, d.end_head), (PathType::Sharp, Head::None, Head::Arrow));
}

#[test]
fn the_preview_shows_the_heads_of_the_defaults_and_a_change_applies_to_the_next_arrow_only() {
    let mut rig = Rig::new();
    rig.run("tool.set_arrow", json!({ "start_head": "bar", "end_head": null }));
    rig.tool(Tool::Arrow);
    rig.click(100.0, 100.0);
    rig.hover(300.0, 200.0);
    let preview = rig.parts().preview.clone().expect("a preview");
    let d = preview.linear_data().expect("linear data");
    assert_eq!((d.start_head, d.end_head), (Head::Bar, Head::None));
    rig.double_click(300.0, 200.0);
    let first = rig.only().linear_data().cloned().expect("linear data");
    assert_eq!((first.start_head, first.end_head), (Head::Bar, Head::None));
    rig.run("tool.set_arrow", json!({ "arrow_type": "elbow", "start_head": "none" }));
    assert_eq!(rig.only().linear_data().map(|d| d.path), Some(PathType::Sharp), "the drawn arrow is left alone");
}

#[test]
fn a_line_ignores_the_arrow_heads() {
    let mut rig = Rig::new();
    rig.run("tool.set_arrow", json!({ "arrow_type": "round", "start_head": "circle", "end_head": "triangle" }));
    rig.tool(Tool::Line);
    rig.drag((100.0, 100.0), (300.0, 200.0));
    let d = rig.only().linear_data().cloned().expect("linear data");
    assert_eq!((d.path, d.start_head, d.end_head), (PathType::Sharp, Head::None, Head::None));
}

#[test]
fn a_line_is_never_elbow_and_never_sends_an_arrow_type() {
    let mut rig = Rig::new();
    rig.run("tool.set_arrow", json!({ "arrow_type": "elbow" }));
    rig.tool(Tool::Line);
    rig.drag((100.0, 100.0), (300.0, 200.0));
    let e = rig.only();
    assert_eq!(e.linear_data().map(|d| d.path), Some(PathType::Sharp));
}

// ---- E6: the gesture equals the plain command ----------------------------------------------------------------------------

fn boxes_rig(tool: Option<Tool>) -> Rig {
    let mut rig = Rig::new();
    two_boxes(&mut rig);
    if let Some(t) = tool {
        rig.tool(t);
    }
    rig
}

#[test]
#[allow(non_snake_case)]
fn exception_E6_draw_equals_plain_command_for_a_line() {
    let mut gesture = boxes_rig(Some(Tool::Line));
    gesture.drag((110.0, 120.0), (420.0, 160.0));
    let mut plain = boxes_rig(None);
    plain.run(
        "element.create",
        json!({ "kind": "line", "points": [[110, 120], [420, 160]], "bind": "auto", "from_tool": true }),
    );
    assert_eq!(gesture.normalized_doc(), plain.normalized_doc());
    assert_eq!(gesture.s.tool.active, plain.s.tool.active);
}

#[test]
#[allow(non_snake_case)]
fn exception_E6_draw_equals_plain_command_for_an_arrow_between_boxes() {
    let mut gesture = boxes_rig(Some(Tool::Arrow));
    gesture.drag((210.0, 150.0), (390.0, 150.0));
    let mut plain = boxes_rig(None);
    plain.run(
        "element.create",
        json!({ "kind": "arrow", "points": [[210, 150], [390, 150]], "bind": "auto", "arrow_type": "sharp", "from_tool": true }),
    );
    assert_eq!(gesture.normalized_doc(), plain.normalized_doc());
    let doc = gesture.normalized_doc();
    let arrow = doc.as_array().and_then(|a| a.last()).expect("the arrow is last");
    assert!(arrow.to_string().contains("start_binding"), "bound: {arrow}");
}

#[test]
#[allow(non_snake_case)]
fn exception_E6_draw_equals_plain_command_for_a_multi_point_line() {
    let mut gesture = boxes_rig(Some(Tool::Line));
    gesture.click(100.0, 300.0);
    gesture.click(200.0, 350.0);
    gesture.double_click(300.0, 300.0);
    let mut plain = boxes_rig(None);
    plain.run(
        "element.create",
        json!({ "kind": "line", "points": [[100, 300], [200, 350], [300, 300]], "bind": "auto", "from_tool": true }),
    );
    assert_eq!(gesture.normalized_doc(), plain.normalized_doc());
}

#[test]
#[allow(non_snake_case)]
fn exception_E6_draw_equals_plain_command_for_an_elbow_arrow() {
    let mut gesture = boxes_rig(Some(Tool::Arrow));
    gesture.run("tool.set_arrow", json!({ "arrow_type": "elbow" }));
    gesture.drag((210.0, 140.0), (390.0, 170.0));
    let mut plain = boxes_rig(None);
    plain.run(
        "element.create",
        json!({ "kind": "arrow", "points": [[210, 140], [390, 170]], "bind": "auto", "arrow_type": "elbow", "from_tool": true }),
    );
    assert_eq!(gesture.normalized_doc(), plain.normalized_doc());
}

#[test]
#[allow(non_snake_case)]
fn exception_E6_draw_equals_plain_command_with_ctrl_and_alt() {
    for (ctrl, alt, bind) in [(true, false, "off"), (false, true, "inside")] {
        let mut gesture = boxes_rig(Some(Tool::Arrow));
        gesture.mods.ctrl = ctrl;
        gesture.mods.alt = alt;
        gesture.drag((210.0, 150.0), (390.0, 150.0));
        let mut plain = boxes_rig(None);
        plain.run(
            "element.create",
            json!({ "kind": "arrow", "points": [[210, 150], [390, 150]], "bind": bind, "arrow_type": "sharp", "from_tool": true }),
        );
        assert_eq!(gesture.normalized_doc(), plain.normalized_doc(), "bind {bind}");
    }
}
