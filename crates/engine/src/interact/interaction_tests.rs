//! Tests of the interaction layer itself: hover (E1), pan (E10), zoom (E11), the Space-hold pan, context menus,
//! external camera changes. The tools have their own test files next to their code.
#![allow(non_snake_case)]

use drafftink_canvas::{
    Button, Buttons, Cursor, InputEvent, PointerEvent, PointerId, PointerKind, PointerPhase, WheelEvent, WheelUnit,
};
use drafftink_geom::kurbo::{Point, Vec2};
use serde_json::json;

use super::testutil::Rig;
use super::{ContextTarget, UiRequest};
use crate::spec::Redraw;
use crate::state::Tool;

fn wheel(rig: &mut Rig, x: f64, y: f64, dy: f64, ctrl: bool) -> super::Outcome {
    rig.now += 16.0;
    let mut mods = rig.mods;
    mods.ctrl = ctrl;
    rig.input(InputEvent::Wheel(WheelEvent {
        pos: Point::new(x, y),
        delta: Vec2::new(0.0, dy),
        unit: WheelUnit::Pixel,
        modifiers: mods,
        time_ms: rig.now,
    }))
}

fn touch(rig: &mut Rig, finger: u64, phase: PointerPhase, x: f64, y: f64) -> super::Outcome {
    rig.now += 16.0;
    let buttons = if phase == PointerPhase::Up { Buttons::NONE } else { Buttons::PRIMARY };
    let ev = PointerEvent::touch(finger, phase, x, y, rig.now).with_button(Button::Primary, buttons);
    rig.input(InputEvent::Pointer(ev))
}

#[test]
fn the_rig_starts_with_screen_equal_to_world() {
    let rig = Rig::new();
    assert_eq!(rig.s.view().zoom, 1.0);
    assert_eq!(rig.s.view().scroll.x, 400.0);
    assert_eq!(rig.s.view().viewport.w, 800.0);
    assert_eq!(rig.s.view().screen_to_world(Point::new(100.0, 100.0)), Point::new(100.0, 100.0));
}

#[test]
fn exception_E1_hover_requests_redraw_only_on_change() {
    let mut rig = Rig::new();
    let a = rig.rect(100.0, 100.0, 100.0, 100.0);
    rig.hover(10.0, 10.0);
    // Entering the element: one overlay redraw (the outline) and a move cursor.
    let enter = rig.hover(150.0, 150.0);
    assert_eq!(enter.redraw, Some(Redraw::Overlay));
    assert_eq!(enter.cursor, Some(Cursor::Move));
    assert!(rig.i.overlay(&rig.s).canvas.hover.is_some());
    // A hundred moves over the same element: no redraw, no cursor change.
    for k in 0..100 {
        let out = rig.hover(120.0 + f64::from(k) * 0.5, 130.0);
        assert_eq!(out.redraw, None, "move {k} over the same element must not redraw");
        assert_eq!(out.cursor, None);
    }
    // Leaving it: one redraw to drop the outline.
    let leave = rig.hover(400.0, 400.0);
    assert_eq!(leave.redraw, Some(Redraw::Overlay));
    assert_eq!(leave.cursor, Some(Cursor::Default));
    assert!(rig.i.overlay(&rig.s).canvas.hover.is_none());
    assert_eq!(rig.hover(410.0, 400.0).redraw, None);
    // Nothing was committed by hovering.
    assert!(rig.ring_all_ids().is_empty());
    assert_eq!(rig.undo_depth(), 0);
    let _ = a;
}

#[test]
fn hovering_a_selected_element_shows_no_outline() {
    let mut rig = Rig::new();
    let a = rig.rect(100.0, 100.0, 100.0, 100.0);
    rig.run("selection.select", json!({ "ids": [a.as_str()] }));
    rig.hover(150.0, 150.0);
    assert!(rig.i.overlay(&rig.s).canvas.hover.is_none());
}

#[test]
fn the_hover_follows_an_element_that_moves_away_from_under_the_pointer() {
    let mut rig = Rig::new();
    let a = rig.rect(100.0, 100.0, 100.0, 100.0);
    rig.hover(150.0, 150.0);
    assert!(rig.i.overlay(&rig.s).canvas.hover.is_some());
    rig.run("element.move", json!({ "ids": [a.as_str()], "dx": 300, "dy": 0, "snap": "none" }));
    // The next event of any kind picks again under the resting pointer.
    let out = rig.tick(1.0);
    assert!(rig.i.overlay(&rig.s).canvas.hover.is_none());
    assert_eq!(out.redraw, Some(Redraw::Overlay));
}

#[test]
fn exception_E10_pan_ends_with_view_set() {
    let mut rig = Rig::new();
    rig.rect(100.0, 100.0, 100.0, 100.0);
    rig.tool(Tool::Hand);
    let revision = rig.s.tab().doc.revision();
    rig.down(300.0, 300.0);
    for k in 1..=4 {
        let out = rig.drag_to(300.0 + f64::from(k) * 5.0, 300.0 + f64::from(k) * 2.0);
        assert_eq!(out.redraw, Some(Redraw::Camera), "a pan frame asks for exactly one camera redraw");
    }
    // The live camera is ahead of the session: only the first (throttled) report reached it.
    let live = rig.i.camera();
    assert_eq!(live.scroll, Point::new(400.0 - 20.0, 300.0 - 8.0));
    assert_ne!(rig.s.view().scroll, live.scroll, "the session camera lags during the pan");
    for k in 5..=20 {
        let out = rig.drag_to(300.0 + f64::from(k) * 5.0, 300.0 + f64::from(k) * 2.0);
        assert_eq!(out.redraw, Some(Redraw::Camera));
    }
    let live = rig.i.camera();
    assert_eq!(live.scroll, Point::new(400.0 - 100.0, 300.0 - 40.0));
    rig.up(400.0, 340.0);
    // The end reports the final camera: equal within 1e-9.
    assert!((rig.s.view().scroll.x - live.scroll.x).abs() < 1e-9);
    assert!((rig.s.view().scroll.y - live.scroll.y).abs() < 1e-9);
    assert!((rig.s.view().zoom - live.zoom).abs() < 1e-9);
    // No document change, no undo step, nothing selected.
    assert_eq!(rig.s.tab().doc.revision(), revision);
    assert_eq!(rig.undo_depth(), 0);
    assert!(rig.selected().is_empty());
    assert_eq!(rig.ring_ids(), vec!["tool.set"], "a camera move leaves no ring entry of its own");
}

#[test]
fn a_pan_reports_the_camera_at_most_every_100_ms() {
    let mut rig = Rig::new();
    rig.tool(Tool::Hand);
    rig.down(300.0, 300.0);
    // The first move is reported at once, the next ones are held back.
    rig.drag_to(310.0, 300.0);
    let first = rig.s.view().scroll;
    assert_eq!(first, Point::new(390.0, 300.0));
    rig.drag_to(320.0, 300.0);
    rig.drag_to(330.0, 300.0);
    assert_eq!(rig.s.view().scroll, first, "within 100 ms nothing more is reported");
    assert!(rig.i.next_deadline().is_some(), "the held report has a deadline");
    // Time passes beyond the throttle: the deadline report catches the session up.
    rig.tick(150.0);
    assert_eq!(rig.s.view().scroll, rig.i.camera().scroll);
    assert_eq!(rig.i.next_deadline(), None, "idle means idle");
    rig.up(330.0, 300.0);
}

#[test]
fn space_hold_pans_instead_of_selecting_or_moving() {
    let mut rig = Rig::new();
    let a = rig.rect(100.0, 100.0, 100.0, 100.0);
    rig.run("tool.hold", json!({ "tool": "hand" }));
    rig.drag((150.0, 150.0), (250.0, 200.0));
    assert!(rig.selected().is_empty());
    assert_eq!(rig.bounds(&a), (100.0, 100.0, 100.0, 100.0));
    assert_eq!(rig.s.view().scroll, Point::new(300.0, 250.0));
    rig.run("tool.release", json!({}));
    // Back to the select tool: the same drag now moves the rectangle.
    rig.drag((150.0, 150.0), (250.0, 200.0));
    assert_eq!(rig.s.view().scroll, Point::new(300.0, 250.0));
}

#[test]
fn a_middle_button_drag_pans_whatever_the_tool() {
    let mut rig = Rig::new();
    rig.rect(100.0, 100.0, 100.0, 100.0);
    let mid = |phase, x: f64, y: f64, t: f64| {
        let held = if phase == PointerPhase::Up { Buttons::NONE } else { Buttons::MIDDLE };
        InputEvent::Pointer(
            PointerEvent::new(PointerId(0), PointerKind::Mouse, phase, Point::new(x, y), t)
                .with_button(Button::Middle, held),
        )
    };
    rig.input(mid(PointerPhase::Down, 150.0, 150.0, 1.0));
    let out = rig.input(mid(PointerPhase::Move, 170.0, 160.0, 17.0));
    assert_eq!(out.redraw, Some(Redraw::Camera));
    assert_eq!(out.cursor, Some(Cursor::Grabbing));
    rig.input(mid(PointerPhase::Up, 170.0, 160.0, 33.0));
    assert_eq!(rig.s.view().scroll, Point::new(380.0, 290.0));
    assert!(rig.selected().is_empty());
}

#[test]
fn exception_E11_zoom_ends_with_view_set() {
    let mut rig = Rig::new();
    // One ctrl-wheel notch zooms by 1.1 about the pointer.
    let before = rig.s.view().screen_to_world(Point::new(200.0, 100.0));
    let out = wheel(&mut rig, 200.0, 100.0, -100.0, true);
    assert_eq!(out.redraw, Some(Redraw::Camera));
    let cam = rig.i.camera();
    assert!((cam.zoom - 1.1).abs() < 1e-9);
    // The point under the cursor stays fixed.
    let after = rig.i.camera();
    let world_under =
        Point::new(after.scroll.x + (200.0 - 400.0) / after.zoom, after.scroll.y + (100.0 - 300.0) / after.zoom);
    assert!((world_under.x - before.x).abs() < 1e-9 && (world_under.y - before.y).abs() < 1e-9);
    // The session learns the final camera at the latest at the deadline.
    rig.tick(150.0);
    assert!((rig.s.view().zoom - rig.i.camera().zoom).abs() < 1e-9);
    assert!((rig.s.view().scroll.x - rig.i.camera().scroll.x).abs() < 1e-9);
    // Zoom out is the inverse notch, and the range is clamped.
    for _ in 0..200 {
        wheel(&mut rig, 400.0, 300.0, 100.0, true);
    }
    assert!(rig.i.camera().zoom >= 0.1 - 1e-12);
    rig.tick(150.0);
    assert!((rig.s.view().zoom - rig.i.camera().zoom).abs() < 1e-9);
    assert!(rig.ring_ids().is_empty());
    assert_eq!(rig.undo_depth(), 0);
}

#[test]
fn a_plain_wheel_pans_and_a_pinch_zooms_and_ends_with_view_set() {
    let mut rig = Rig::new();
    let out = wheel(&mut rig, 400.0, 300.0, 40.0, false);
    assert_eq!(out.redraw, Some(Redraw::Camera));
    assert_eq!(rig.i.camera().scroll, Point::new(400.0, 340.0));
    rig.tick(150.0);
    // Two fingers: spread apart doubles the distance.
    touch(&mut rig, 0, PointerPhase::Down, 300.0, 300.0);
    touch(&mut rig, 1, PointerPhase::Down, 400.0, 300.0);
    touch(&mut rig, 0, PointerPhase::Move, 250.0, 300.0);
    touch(&mut rig, 1, PointerPhase::Move, 450.0, 300.0);
    assert!((rig.i.camera().zoom - 2.0).abs() < 1e-9, "zoom {}", rig.i.camera().zoom);
    touch(&mut rig, 1, PointerPhase::Up, 450.0, 300.0);
    assert!((rig.s.view().zoom - 2.0).abs() < 1e-9, "the end of the pinch reports the camera");
    touch(&mut rig, 0, PointerPhase::Up, 250.0, 300.0);
    assert!(rig.selected().is_empty());
    assert!(rig.ring_ids().is_empty());
}

#[test]
fn a_command_that_moves_the_camera_is_adopted() {
    let mut rig = Rig::new();
    let a = rig.rect(100.0, 100.0, 100.0, 100.0);
    rig.run("view.set", json!({ "scroll": [1000, 1000], "zoom": 2 }));
    // The rectangle is now far off screen; hovering where it used to be finds nothing.
    rig.hover(150.0, 150.0);
    assert!(rig.i.overlay(&rig.s).canvas.hover.is_none());
    // At a camera that centres it, the same screen position hits it.
    rig.run("view.set", json!({ "scroll": [150, 150], "zoom": 1 }));
    rig.hover(400.0, 300.0);
    assert_eq!(rig.i.overlay(&rig.s).canvas.hover.map(|h| h.id), Some(a));
}

#[test]
fn a_right_click_asks_for_a_context_menu() {
    let mut rig = Rig::new();
    let a = rig.rect(100.0, 100.0, 100.0, 100.0);
    let right = |phase, x: f64, y: f64, t: f64| {
        let held = if phase == PointerPhase::Up { Buttons::NONE } else { Buttons::SECONDARY };
        InputEvent::Pointer(
            PointerEvent::new(PointerId(0), PointerKind::Mouse, phase, Point::new(x, y), t)
                .with_button(Button::Secondary, held),
        )
    };
    rig.input(right(PointerPhase::Down, 150.0, 150.0, 1.0));
    let out = rig.input(right(PointerPhase::Up, 150.0, 150.0, 17.0));
    assert_eq!(
        out.ui_request,
        Some(UiRequest::OpenContextMenu { at: Point::new(150.0, 150.0), target: ContextTarget::Selection })
    );
    assert_eq!(rig.selected(), vec![a.as_str().to_owned()]);
    rig.input(right(PointerPhase::Down, 600.0, 500.0, 100.0));
    let out = rig.input(right(PointerPhase::Up, 600.0, 500.0, 117.0));
    assert_eq!(
        out.ui_request,
        Some(UiRequest::OpenContextMenu { at: Point::new(600.0, 500.0), target: ContextTarget::Canvas })
    );
    assert!(rig.selected().is_empty());
}

fn right_click(rig: &mut Rig, x: f64, y: f64) -> super::Outcome {
    let ev = |phase, t: f64| {
        let held = if phase == PointerPhase::Up { Buttons::NONE } else { Buttons::SECONDARY };
        InputEvent::Pointer(
            PointerEvent::new(PointerId(0), PointerKind::Mouse, phase, Point::new(x, y), t)
                .with_button(Button::Secondary, held),
        )
    };
    rig.now += 100.0;
    rig.input(ev(PointerPhase::Down, rig.now));
    rig.input(ev(PointerPhase::Up, rig.now + 16.0))
}

#[test]
fn a_right_click_on_a_locked_element_asks_for_a_menu_about_it_and_selects_nothing() {
    let mut rig = Rig::new();
    let a = rig.rect(100.0, 100.0, 100.0, 100.0);
    let b = rig.rect(400.0, 100.0, 100.0, 100.0);
    rig.run("element.lock", json!({ "ids": [a.as_str()], "locked": true }));
    rig.run("selection.select", json!({ "ids": [b.as_str()] }));
    let out = right_click(&mut rig, 150.0, 150.0);
    assert_eq!(
        out.ui_request,
        Some(UiRequest::OpenContextMenu {
            at: Point::new(150.0, 150.0),
            target: ContextTarget::Locked { element: a.clone() }
        })
    );
    assert!(
        rig.selected().is_empty(),
        "a locked element cannot be selected, and the old selection is not the menu's subject"
    );
    assert!(out.refusals.is_empty(), "{:?}", out.refusals);
    // The menu's Unlock row: the id goes to `element.unlock_all`, which selects what it unlocks.
    rig.run("element.unlock_all", json!({ "ids": [a.as_str()] }));
    assert_eq!(rig.selected(), vec![a.as_str().to_owned()]);
    assert!(!rig.element(&a).locked);
}

#[test]
fn a_left_click_still_skips_a_locked_element() {
    let mut rig = Rig::new();
    let a = rig.rect(100.0, 100.0, 100.0, 100.0);
    rig.run("element.lock", json!({ "ids": [a.as_str()], "locked": true }));
    rig.click(150.0, 150.0);
    assert!(rig.selected().is_empty());
    rig.hover(150.0, 150.0);
    assert!(rig.i.overlay(&rig.s).canvas.hover.is_none());
}

#[test]
fn a_right_click_where_an_unlocked_element_covers_a_locked_one_is_about_the_unlocked_one() {
    let mut rig = Rig::new();
    let back = rig.rect(100.0, 100.0, 200.0, 200.0);
    let front = rig.rect(150.0, 150.0, 50.0, 50.0);
    rig.run("element.lock", json!({ "ids": [back.as_str()], "locked": true }));
    let out = right_click(&mut rig, 170.0, 170.0);
    assert_eq!(
        out.ui_request,
        Some(UiRequest::OpenContextMenu { at: Point::new(170.0, 170.0), target: ContextTarget::Selection })
    );
    assert_eq!(rig.selected(), vec![front.as_str().to_owned()]);
    // Beside the front element, the locked one is on top of nothing.
    let out = right_click(&mut rig, 280.0, 280.0);
    assert_eq!(
        out.ui_request,
        Some(UiRequest::OpenContextMenu {
            at: Point::new(280.0, 280.0),
            target: ContextTarget::Locked { element: back }
        })
    );
}

#[test]
fn a_right_click_on_the_label_of_a_locked_shape_names_the_shape() {
    let mut rig = Rig::new();
    let a = rig.rect(100.0, 100.0, 200.0, 100.0);
    rig.run("element.relabel", json!({ "id": a.as_str(), "text": "Hi" }));
    rig.run("element.lock", json!({ "ids": [a.as_str()], "locked": true }));
    let out = right_click(&mut rig, 200.0, 150.0);
    assert_eq!(
        out.ui_request,
        Some(UiRequest::OpenContextMenu { at: Point::new(200.0, 150.0), target: ContextTarget::Locked { element: a } })
    );
}

/// A rectangle at (100, 100, 100, 100) with a link; its icon is centred on (208, 92).
fn linked_rect(rig: &mut Rig, link: &str) -> drafftink_scene::ElementId {
    let a = rig.rect(100.0, 100.0, 100.0, 100.0);
    rig.run("element.set_link", json!({ "id": a.as_str(), "link": link }));
    a
}

#[test]
fn a_click_on_a_link_icon_asks_the_shell_to_open_the_address() {
    let mut rig = Rig::new();
    let a = linked_rect(&mut rig, "https://example.com/x");
    rig.down(208.0, 92.0);
    let out = rig.up(208.0, 92.0);
    assert_eq!(out.ui_request, Some(UiRequest::OpenUrl { url: "https://example.com/x".into() }));
    assert!(out.refusals.is_empty(), "{:?}", out.refusals);
    assert!(rig.selected().is_empty(), "the icon is not the element");
    assert_eq!(rig.undo_depth(), 1, "only the set_link step: opening is not a document change");
    // The body of the element selects it as always, and a click elsewhere opens nothing.
    let out = {
        rig.down(150.0, 150.0);
        rig.up(150.0, 150.0)
    };
    assert_eq!(out.ui_request, None);
    assert_eq!(rig.selected(), vec![a.as_str().to_owned()]);
}

#[test]
fn hovering_a_link_icon_shows_the_pointing_hand_and_leaving_it_the_arrow() {
    let mut rig = Rig::new();
    linked_rect(&mut rig, "https://example.com/x");
    assert_eq!(rig.hover(208.0, 92.0).cursor, Some(Cursor::Pointer));
    assert_eq!(rig.hover(600.0, 500.0).cursor, Some(Cursor::Default));
}

#[test]
fn the_icon_of_a_selected_element_is_not_live() {
    let mut rig = Rig::new();
    let a = linked_rect(&mut rig, "https://example.com/x");
    rig.run("selection.select", json!({ "ids": [a.as_str()] }));
    rig.down(208.0, 92.0);
    let out = rig.up(208.0, 92.0);
    assert_eq!(out.ui_request, None, "the handles are there instead");
}

#[test]
fn the_icon_wins_over_the_element_under_it_and_a_drag_from_it_is_a_marquee_not_a_move() {
    let mut rig = Rig::new();
    let a = linked_rect(&mut rig, "https://example.com/x");
    // A second element lies under the icon (208, 92).
    let under = rig.rect(190.0, 60.0, 60.0, 60.0);
    rig.run("selection.clear", json!({}));
    let out = {
        rig.down(208.0, 92.0);
        rig.up(208.0, 92.0)
    };
    assert_eq!(out.ui_request, Some(UiRequest::OpenUrl { url: "https://example.com/x".into() }));
    assert!(rig.selected().is_empty(), "{:?}", rig.selected());
    let before = (rig.bounds(&a), rig.bounds(&under));
    rig.drag((208.0, 92.0), (400.0, 400.0));
    assert_eq!((rig.bounds(&a), rig.bounds(&under)), before, "nothing moved");
    assert_eq!(rig.selected(), vec![under.as_str().to_owned()], "the rubber band touched the element under the icon");
}

#[test]
fn only_the_select_tool_reads_the_icon() {
    let mut rig = Rig::new();
    linked_rect(&mut rig, "https://example.com/x");
    rig.run("selection.clear", json!({}));
    rig.tool(Tool::Rectangle);
    rig.down(208.0, 92.0);
    let out = rig.up(208.0, 92.0);
    assert_eq!(out.ui_request, None);
}

#[test]
fn a_click_on_the_icon_of_an_element_link_goes_to_the_target() {
    let mut rig = Rig::new();
    let a = rig.rect(100.0, 100.0, 100.0, 100.0);
    let b = rig.rect(900.0, 700.0, 50.0, 50.0);
    rig.run("element.set_link", json!({ "id": a.as_str(), "link": format!("#{}", b.as_str()) }));
    rig.run("selection.clear", json!({}));
    rig.down(208.0, 92.0);
    let out = rig.up(208.0, 92.0);
    assert_eq!(out.ui_request, None, "no browser is involved");
    assert_eq!(rig.selected(), vec![b.as_str().to_owned()]);
    let v = rig.s.view();
    assert!((v.scroll.x - 925.0).abs() < 1e-6 && (v.scroll.y - 725.0).abs() < 1e-6, "{:?}", v.scroll);
}

#[test]
fn a_click_on_an_unsafe_stored_link_shows_the_refusal_and_opens_nothing() {
    let mut rig = Rig::new();
    let a = rig.rect(100.0, 100.0, 100.0, 100.0);
    rig.s
        .tab_mut()
        .doc
        .edit::<(), drafftink_scene::StoreError>(|tx| {
            use drafftink_scene::SceneTx;
            tx.update(&a, |e| e.link = Some("javascript:alert(1)".into()))
        })
        .unwrap();
    rig.s.tab_mut().doc.commit_untracked().unwrap();
    rig.down(208.0, 92.0);
    let out = rig.up(208.0, 92.0);
    assert_eq!(out.ui_request, None);
    assert_eq!(out.refusals.len(), 1);
    assert_eq!(out.refusals[0].code, "invalid_link");
}

#[test]
fn a_tool_change_drops_what_the_old_tool_was_doing() {
    let mut rig = Rig::new();
    rig.rect(100.0, 100.0, 100.0, 100.0);
    rig.tool(Tool::Hand);
    rig.down(300.0, 300.0);
    rig.tool(Tool::Select);
    rig.up(300.0, 300.0);
    assert!(!rig.i.is_busy());
}

fn rectangle_script() -> Vec<InputEvent> {
    let mouse = |phase, x: f64, y: f64, t: f64| {
        let (button, held) = match phase {
            PointerPhase::Down => (Button::Primary, Buttons::PRIMARY),
            PointerPhase::Up => (Button::Primary, Buttons::NONE),
            _ => (Button::None, Buttons::PRIMARY),
        };
        InputEvent::Pointer(PointerEvent::mouse(phase, x, y, t).with_button(button, held))
    };
    vec![
        mouse(PointerPhase::Down, 100.0, 100.0, 1.0),
        mouse(PointerPhase::Move, 150.0, 130.0, 17.0),
        mouse(PointerPhase::Move, 200.0, 160.0, 33.0),
        mouse(PointerPhase::Up, 200.0, 160.0, 49.0),
    ]
}

#[test]
fn a_recorded_script_replays_to_the_same_document() {
    let script = rectangle_script();
    let mut a = Rig::new();
    let mut b = Rig::new();
    a.tool(Tool::Rectangle);
    b.tool(Tool::Rectangle);
    let outcomes = a.i.run_script(&mut a.s, &script);
    assert_eq!(outcomes.len(), script.len());
    for ev in &script {
        b.i.input(&mut b.s, ev);
    }
    assert_eq!(a.elements().len(), 1);
    assert_eq!(a.normalized_doc(), b.normalized_doc());
}

#[test]
fn the_overlay_lists_the_link_icons_of_unselected_elements_in_view() {
    let mut rig = Rig::new();
    let a = linked_rect(&mut rig, "https://example.com/x");
    let far = rig.rect(5000.0, 5000.0, 50.0, 50.0);
    rig.run("element.set_link", json!({ "id": far.as_str(), "link": "https://example.com/far" }));
    rig.run("selection.clear", json!({}));
    let icons = rig.i.overlay(&rig.s).canvas.link_icons;
    assert_eq!(icons.len(), 1, "the icon out of view is left out: {icons:?}");
    assert_eq!(icons[0].element, a);
    assert!(!icons[0].to_element);
    assert!(icons[0].rect.contains(Point::new(208.0, 92.0)));
    rig.run("selection.select", json!({ "ids": [a.as_str()] }));
    assert!(rig.i.overlay(&rig.s).canvas.link_icons.is_empty(), "a selected element shows its handles instead");
}
