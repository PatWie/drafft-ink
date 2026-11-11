//! The built-in pointer behaviour: select, move, transform, rubber band, and what an idle stage costs.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use drafftink_canvas::{InputEvent, Modifiers};
use drafftink_canvas_js::{InteractionOptions, StageOptions};
use serde_json::json;

fn selected(core: &drafftink_canvas_js::Core) -> Vec<String> {
    core.selection()
}

#[test]
fn a_press_selects_the_element_or_its_whole_group_and_empty_space_clears() {
    let mut s = stage();
    s.add(rect("a", 0.0, 0.0, 100.0, 100.0)).unwrap();
    s.add(rect("b", 200.0, 0.0, 100.0, 100.0)).unwrap();
    s.add(rect("c", 400.0, 0.0, 100.0, 100.0)).unwrap();
    s.transact(json!([{"op": "group", "ids": ["a", "b"], "id": "pair"}])).unwrap();
    press(&mut s, 450.0, 350.0, 0.0);
    release(&mut s, 450.0, 350.0, 20.0);
    assert_eq!(selected(&s), ["a", "b"], "a group is one unit");
    press(&mut s, 400.0 + 450.0, 350.0, 100.0);
    release(&mut s, 400.0 + 450.0, 350.0, 120.0);
    assert_eq!(selected(&s), ["c"]);
    press(&mut s, 5.0, 5.0, 200.0);
    release(&mut s, 5.0, 5.0, 220.0);
    assert!(selected(&s).is_empty());
}

#[test]
fn shift_adds_and_removes_from_the_selection() {
    let mut s = stage();
    s.add(rect("a", 0.0, 0.0, 100.0, 100.0)).unwrap();
    s.add(rect("c", 400.0, 0.0, 100.0, 100.0)).unwrap();
    let shift = |s: &mut drafftink_canvas_js::Core, x: f64, t: f64| {
        let mods = Modifiers { shift: true, ..Modifiers::NONE };
        for (phase, dt) in [(drafftink_canvas::PointerPhase::Down, 0.0), (drafftink_canvas::PointerPhase::Up, 20.0)] {
            s.handle_input(&InputEvent::Pointer(
                drafftink_canvas::PointerEvent::mouse(phase, x, 350.0, t + dt).with_modifiers(mods),
            ));
        }
    };
    press(&mut s, 450.0, 350.0, 0.0);
    release(&mut s, 450.0, 350.0, 20.0);
    shift(&mut s, 400.0 + 450.0, 1000.0);
    assert_eq!(selected(&s), ["a", "c"]);
    shift(&mut s, 450.0, 2000.0);
    assert_eq!(selected(&s), ["c"]);
}

#[test]
fn dragging_an_element_moves_it_and_the_selection_moves_together() {
    let mut s = stage();
    s.add(rect("a", 0.0, 0.0, 100.0, 100.0)).unwrap();
    s.add(rect("b", 200.0, 0.0, 100.0, 100.0)).unwrap();
    s.set_selection(&["a".to_owned(), "b".to_owned()]).unwrap();
    drag(&mut s, (450.0, 350.0), (480.0, 370.0));
    assert_box(xywh(&s, "a"), [30.0, 20.0, 100.0, 100.0]);
    assert_box(xywh(&s, "b"), [230.0, 20.0, 100.0, 100.0]);
    // Dragging an element that is not selected selects it and moves only it.
    s.add(rect("c", 0.0, 150.0, 50.0, 50.0)).unwrap();
    drag(&mut s, (400.0 + 25.0, 300.0 + 150.0 + 25.0), (400.0 + 25.0, 300.0 + 175.0 + 25.0));
    assert_box(xywh(&s, "c"), [0.0, 175.0, 50.0, 50.0]);
    assert_eq!(selected(&s), ["c"]);
    assert!((xywh(&s, "a")[0] - 30.0).abs() < 1e-6);
}

#[test]
fn escape_during_a_drag_puts_everything_back() {
    let mut s = stage();
    s.add(rect("a", 0.0, 0.0, 100.0, 100.0)).unwrap();
    press(&mut s, 450.0, 350.0, 0.0);
    drag_to(&mut s, 480.0, 380.0, 16.0);
    drag_to(&mut s, 500.0, 400.0, 32.0);
    assert_eq!(xywh(&s, "a")[0], 50.0, "moved while dragging");
    s.handle_input(&InputEvent::CancelAll);
    assert_box(xywh(&s, "a"), [0.0, 0.0, 100.0, 100.0]);
}

#[test]
fn a_shape_can_refuse_to_be_dragged_and_the_behaviour_can_be_turned_off() {
    let mut s = stage();
    s.add(
        json!({"id": "fixed", "type": "rectangle", "x": 0, "y": 0, "w": 100, "h": 100, "custom": {"draggable": false},
                 "style": {"fill_color": "#fff"}}),
    )
    .unwrap();
    drag(&mut s, (450.0, 350.0), (500.0, 380.0));
    assert_eq!(xywh(&s, "fixed")[0], 0.0);
    assert_eq!(selected(&s), ["fixed"], "it still selects");

    let options = StageOptions {
        interaction: InteractionOptions { select: false, drag: false, transform: false, ..Default::default() },
        ..Default::default()
    };
    let mut t = stage_with(&options);
    t.add(rect("a", 0.0, 0.0, 100.0, 100.0)).unwrap();
    drag(&mut t, (450.0, 350.0), (500.0, 380.0));
    assert_eq!(xywh(&t, "a")[0], 0.0);
    assert!(selected(&t).is_empty());
}

#[test]
fn a_rubber_band_selects_what_it_contains_and_whole_groups() {
    let mut s = stage();
    s.add(rect("a", 0.0, 0.0, 50.0, 50.0)).unwrap();
    s.add(rect("b", 100.0, 0.0, 50.0, 50.0)).unwrap();
    s.add(rect("far", 1000.0, 1000.0, 50.0, 50.0)).unwrap();
    s.add(rect("g1", -300.0, 0.0, 50.0, 50.0)).unwrap();
    s.add(rect("g2", 600.0, 600.0, 50.0, 50.0)).unwrap();
    s.transact(json!([{"op": "group", "ids": ["g1", "g2"], "id": "far-pair"}])).unwrap();
    // Screen (350, 250) to (600, 400) is world (-50, -50) to (200, 100).
    drag(&mut s, (350.0, 250.0), (600.0, 400.0));
    assert_eq!(selected(&s), ["a", "b"]);
    // From (60, 250) to (200, 400) the band takes g1 (world -300..-250), and with it the other member of its group.
    drag(&mut s, (60.0, 250.0), (200.0, 400.0));
    let mut sel = selected(&s);
    sel.sort();
    assert_eq!(sel, ["g1", "g2"]);
}

#[test]
fn a_corner_handle_resizes_the_selection_and_cancel_restores_it() {
    let mut s = stage();
    s.add(rect("a", 0.0, 0.0, 100.0, 100.0)).unwrap();
    s.set_selection(&["a".to_owned()]).unwrap();
    // The bottom-right corner is at world (100, 100): screen (500, 400).
    drag(&mut s, (500.0, 400.0), (540.0, 450.0));
    assert_box(xywh(&s, "a"), [0.0, 0.0, 140.0, 150.0]);
    // Dragging past the opposite corner and back ends where a direct drag ends: no drift.
    press(&mut s, 540.0, 450.0, 0.0);
    drag_to(&mut s, 300.0, 200.0, 16.0);
    drag_to(&mut s, 560.0, 460.0, 32.0);
    release(&mut s, 560.0, 460.0, 48.0);
    assert_box(xywh(&s, "a"), [0.0, 0.0, 160.0, 160.0]);
    press(&mut s, 560.0, 460.0, 100.0);
    drag_to(&mut s, 700.0, 500.0, 116.0);
    s.handle_input(&InputEvent::CancelAll);
    assert_box(xywh(&s, "a"), [0.0, 0.0, 160.0, 160.0]);
}

#[test]
fn the_rotation_handle_turns_the_selection() {
    let mut s = stage();
    s.add(rect("a", -50.0, -50.0, 100.0, 100.0)).unwrap();
    s.set_selection(&["a".to_owned()]).unwrap();
    // The handle is 25 px above the top edge: world (0, -75), screen (400, 225).
    drag(&mut s, (400.0, 225.0), (400.0 + 100.0, 300.0));
    let angle = s.get("a").unwrap().transform.angle;
    assert!((angle - std::f64::consts::FRAC_PI_2).abs() < 1e-6, "{angle}");
}

#[test]
fn point_and_midpoint_handles_reshape_a_line() {
    let mut s = stage();
    s.add(json!({"id": "l", "type": "line", "x": -100, "y": 0, "points": [[0, 0], [200, 0]]})).unwrap();
    s.set_selection(&["l".to_owned()]).unwrap();
    // Drag the right end from world (100, 0) to (100, 80).
    drag(&mut s, (500.0, 300.0), (500.0, 380.0));
    let pts = s.get("l").unwrap().points().unwrap().to_vec();
    assert_eq!(pts.len(), 2);
    assert_eq!(s.get("l").unwrap().transform.h, 80.0);
    // The midpoint of that segment is world (0, 40): screen (400, 340). Dragging it inserts a point.
    drag(&mut s, (400.0, 340.0), (400.0, 440.0));
    assert_eq!(s.get("l").unwrap().points().unwrap().len(), 3);
    assert_eq!(s.get("l").unwrap().transform.h, 140.0);
}

#[test]
fn an_idle_stage_asks_for_no_frames() {
    let mut s = stage();
    for i in 0..200 {
        s.add(rect(&format!("r{i}"), f64::from(i % 20) * 60.0, f64::from(i / 20) * 60.0, 50.0, 50.0)).unwrap();
    }
    assert!(s.needs_frame(), "a new stage draws once");
    assert!(s.build_frame().is_some());
    s.frame_done();
    assert!(!s.needs_frame());
    assert!(s.build_frame().is_none());
    // The pointer comes onto the canvas: one change. A thousand moves over empty space after it: nothing to draw.
    hover(&mut s, 1.0, 2.0, 0.0);
    assert!(s.needs_frame());
    s.frame_done();
    for i in 1..1000 {
        hover(&mut s, 1.0 + f64::from(i % 300), 2.0 + f64::from(i % 200), f64::from(i));
        assert!(!s.needs_frame(), "move {i}");
    }
    // Entering an element changes the overlay: one frame. Moving inside it: none.
    hover(&mut s, 410.0, 310.0, 5000.0);
    assert!(s.needs_frame());
    s.frame_done();
    for i in 0..100 {
        hover(&mut s, 410.0 + f64::from(i % 3), 310.0 + f64::from(i % 5), 7000.0 + f64::from(i));
    }
    assert!(!s.needs_frame());
    // A scene change, a camera move and a resize each ask for one frame.
    s.update("r0", json!({"x": 5})).unwrap();
    assert!(s.needs_frame());
    s.frame_done();
    s.pan_by(10.0, 0.0).unwrap();
    assert!(s.needs_frame());
    s.frame_done();
    s.resize(700.0, 500.0, 1.0).unwrap();
    assert!(s.needs_frame());
    s.frame_done();
    assert!(!s.needs_frame());
    assert!(s.next_deadline().is_none(), "no timer is pending");
}

#[test]
fn a_frame_draws_what_is_visible() {
    let mut s = stage();
    for i in 0..300 {
        s.add(rect(
            &format!("r{i}"),
            f64::from(i % 30) * 100.0 - 1500.0,
            f64::from(i / 30) * 100.0 - 500.0,
            80.0,
            80.0,
        ))
        .unwrap();
    }
    let (_, base) = s.build_frame().unwrap();
    assert_eq!(base.to_rgba8().a, 255);
    s.frame_done();
    let report = s.frame_report();
    assert_eq!(report.total, 300);
    assert!(report.drawn > 0 && report.drawn < 300, "culling: {report:?}");
    assert!(report.culled > 0);
    // Zoomed out the whole scene is visible, and a changed element is encoded again.
    s.set_camera(Some([0.0, 0.0]), Some(0.1)).unwrap();
    s.build_frame().unwrap();
    s.frame_done();
    assert!(s.frame_report().in_view >= 290);
}
