//! Konva-style events: selectors, bubbling through groups, stopping, enter and leave, and the view gestures.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::*;
use drafftink_canvas::{
    Button, Buttons, ElementEvent, EventKind, InputEvent, Modifiers, Node, PointerEvent, PointerMode, PointerPhase,
    Selector, WheelEvent, WheelUnit,
};
use drafftink_canvas_js::{Core, EventDto};
use drafftink_geom::kurbo::{Point, Vec2};
use serde_json::json;

type Log = Rc<RefCell<Vec<String>>>;

fn listen(core: &mut Core, kind: EventKind, selector: &str, log: &Log, stop: bool) -> u32 {
    let log = Rc::clone(log);
    let tag = selector.to_owned();
    core.on(
        kind,
        selector.parse::<Selector>().unwrap(),
        Box::new(move |e: &mut ElementEvent| {
            let at = match &e.current {
                Node::Element(id) => format!("element {id}"),
                Node::Group(id) => format!("group {id}"),
                Node::Stage => "stage".to_owned(),
            };
            log.borrow_mut().push(format!("{tag} @ {at}"));
            if stop {
                e.stop_propagation();
            }
        }),
    )
}

fn click(core: &mut Core, x: f64, y: f64) {
    press(core, x, y, 0.0);
    release(core, x, y, 50.0);
}

#[test]
fn a_click_reaches_the_element_its_groups_and_the_stage_in_that_order() {
    let mut s = stage();
    s.add(rect("a", 0.0, 0.0, 100.0, 100.0)).unwrap();
    s.add(rect("b", 200.0, 0.0, 100.0, 100.0)).unwrap();
    s.add(rect("c", 400.0, 0.0, 100.0, 100.0)).unwrap();
    let g = s.transact(json!([{"op": "group", "ids": ["a", "b"], "id": "pair"}])).unwrap();
    assert_eq!(g[0], "pair");
    let log: Log = Rc::default();
    listen(&mut s, EventKind::Click, "stage", &log, false);
    listen(&mut s, EventKind::Click, "group:pair", &log, false);
    listen(&mut s, EventKind::Click, "#a", &log, false);
    listen(&mut s, EventKind::Click, "rectangle", &log, false);
    // The point (50, 0) in world units is on the top edge of "a".
    click(&mut s, 400.0 + 50.0, 300.0);
    assert_eq!(*log.borrow(), ["#a @ element a", "rectangle @ element a", "group:pair @ group pair", "stage @ stage"]);
    log.borrow_mut().clear();
    // "c" is not in the group.
    click(&mut s, 400.0 + 450.0, 300.0);
    assert_eq!(*log.borrow(), ["rectangle @ element c", "stage @ stage"]);
    log.borrow_mut().clear();
    // Empty space reaches the stage only.
    click(&mut s, 20.0, 20.0);
    assert_eq!(*log.borrow(), ["stage @ stage"]);
}

#[test]
fn stop_propagation_ends_the_bubble_after_the_listeners_of_that_node() {
    let mut s = stage();
    s.add(rect("a", 0.0, 0.0, 100.0, 100.0)).unwrap();
    s.add(rect("b", 200.0, 0.0, 100.0, 100.0)).unwrap();
    s.transact(json!([{"op": "group", "ids": ["a", "b"], "id": "pair"}])).unwrap();
    let log: Log = Rc::default();
    listen(&mut s, EventKind::Click, "stage", &log, false);
    listen(&mut s, EventKind::Click, "group:pair", &log, false);
    listen(&mut s, EventKind::Click, "#a", &log, true);
    listen(&mut s, EventKind::Click, "rectangle", &log, false);
    click(&mut s, 400.0 + 50.0, 300.0);
    assert_eq!(
        *log.borrow(),
        ["#a @ element a", "rectangle @ element a"],
        "both listeners of the node run, nothing above"
    );
}

#[test]
fn off_removes_a_listener_and_names_must_be_known_events() {
    let mut s = stage();
    s.add(rect("a", 0.0, 0.0, 100.0, 100.0)).unwrap();
    let log: Log = Rc::default();
    let h = listen(&mut s, EventKind::Click, "#a", &log, false);
    click(&mut s, 450.0, 300.0);
    assert_eq!(log.borrow().len(), 1);
    assert!(s.off(h));
    assert!(!s.off(h));
    click(&mut s, 450.0, 300.0);
    assert_eq!(log.borrow().len(), 1);
    assert_eq!(drafftink_canvas_js::parse_event_kind("dragmove").unwrap(), EventKind::DragMove);
    let err = drafftink_canvas_js::parse_event_kind("mouseover").unwrap_err().to_string();
    assert!(err.contains("not an event") && err.contains("pointerenter"), "{err}");
    assert_eq!(drafftink_canvas_js::event_name(EventKind::DoubleClick), "dblclick");
}

#[test]
fn enter_and_leave_fire_once_on_a_group_not_between_its_members() {
    let mut s = stage();
    s.add(rect("a", 0.0, 0.0, 100.0, 100.0)).unwrap();
    s.add(rect("b", 120.0, 0.0, 100.0, 100.0)).unwrap();
    s.transact(json!([{"op": "group", "ids": ["a", "b"], "id": "pair"}])).unwrap();
    let log: Log = Rc::default();
    listen(&mut s, EventKind::PointerEnter, "group:pair", &log, false);
    listen(&mut s, EventKind::PointerLeave, "group:pair", &log, false);
    listen(&mut s, EventKind::PointerEnter, "#b", &log, false);
    hover(&mut s, 400.0 + 50.0, 300.0 + 50.0, 0.0); // inside a
    hover(&mut s, 400.0 + 170.0, 300.0 + 50.0, 10.0); // inside b
    hover(&mut s, 20.0, 20.0, 20.0); // away
    assert_eq!(*log.borrow(), ["group:pair @ group pair", "#b @ element b", "group:pair @ group pair"]);
}

#[test]
fn drag_events_carry_totals_and_a_cancelled_drag_says_so() {
    let mut s = stage();
    s.add(rect("a", 0.0, 0.0, 100.0, 100.0)).unwrap();
    let seen: Rc<RefCell<Vec<EventDto>>> = Rc::default();
    for kind in [EventKind::DragStart, EventKind::DragMove, EventKind::DragEnd] {
        let seen = Rc::clone(&seen);
        s.on(kind, Selector::Any, Box::new(move |e| seen.borrow_mut().push(EventDto::from(&*e))));
    }
    drag(&mut s, (450.0, 300.0), (500.0, 340.0));
    let seen = seen.borrow();
    assert_eq!(seen.first().map(|e| e.r#type), Some("dragstart"));
    let end = seen.last().unwrap();
    assert_eq!((end.r#type, end.cancelled, end.target.as_deref()), ("dragend", false, Some("a")));
    assert_eq!(end.delta_world, [50.0, 40.0]);
    assert!(seen.iter().filter(|e| e.r#type == "dragmove").count() >= 3);
}

#[test]
fn the_hand_mode_pans_the_wheel_pans_and_ctrl_wheel_zooms() {
    let mut s = stage();
    s.set_pointer_mode(PointerMode::Hand);
    drag(&mut s, (300.0, 300.0), (350.0, 320.0));
    let c = s.camera();
    assert_eq!(c.scroll, [-50.0, -20.0]);
    s.set_pointer_mode(PointerMode::Select);
    let wheel = |s: &mut Core, dx: f64, dy: f64, ctrl: bool| {
        s.handle_input(&InputEvent::Wheel(WheelEvent {
            pos: Point::new(400.0, 300.0),
            delta: Vec2::new(dx, dy),
            unit: WheelUnit::Pixel,
            modifiers: Modifiers { ctrl, ..Modifiers::NONE },
            time_ms: 0.0,
        }))
    };
    let before = s.camera();
    wheel(&mut s, 0.0, 40.0, false);
    assert_eq!(s.camera().scroll[1], before.scroll[1] + 40.0, "a wheel notch scrolls the content up");
    wheel(&mut s, 0.0, -100.0, true);
    assert!(s.camera().zoom > before.zoom, "ctrl and wheel up zooms in");
    // A middle-button drag pans in every mode.
    let before = s.camera();
    let m = |phase, x, held: u8| {
        InputEvent::Pointer(PointerEvent::mouse(phase, x, 100.0, 0.0).with_button(Button::Middle, Buttons(held)))
    };
    s.handle_input(&m(PointerPhase::Down, 100.0, 4));
    for i in 1..=4 {
        s.handle_input(&InputEvent::Pointer(
            PointerEvent::mouse(PointerPhase::Move, 100.0 + f64::from(i) * 10.0, 100.0, f64::from(i) * 16.0)
                .with_button(Button::None, Buttons::MIDDLE),
        ));
    }
    s.handle_input(&m(PointerPhase::Up, 140.0, 0));
    assert!(s.camera().scroll[0] < before.scroll[0], "{:?} -> {:?}", before.scroll, s.camera().scroll);
}

#[test]
fn a_click_on_the_link_icon_of_a_shape_is_an_event_with_the_flag_set_and_does_not_select_it() {
    let mut s = stage();
    s.add(rect("a", 0.0, 0.0, 100.0, 100.0)).unwrap();
    s.update("a", json!({"link": "https://example.com"})).unwrap();
    let seen: Rc<RefCell<Vec<(Option<String>, bool)>>> = Rc::default();
    let sink = Rc::clone(&seen);
    s.on(
        EventKind::Click,
        "#a".parse::<Selector>().unwrap(),
        Box::new(move |e: &mut ElementEvent| {
            let dto = EventDto::from(&*e);
            sink.borrow_mut().push((dto.target, dto.link_icon));
        }),
    );
    // The icon is at world (108, -8): screen (508, 292). The body is at (450, 350).
    for (x, y) in [(508.0, 292.0), (450.0, 350.0)] {
        press(&mut s, x, y, 0.0);
        release(&mut s, x, y, 20.0);
    }
    assert_eq!(*seen.borrow(), [(Some("a".to_owned()), true), (Some("a".to_owned()), false)]);
    assert_eq!(s.selection(), ["a"], "only the body click selected it");
    // A selected shape shows its handles there, not an icon.
    s.set_selection(&["a".to_owned()]).unwrap();
    assert!(s.overlay().link_icons.is_empty());
    s.set_selection(&[]).unwrap();
    let icons = s.overlay().link_icons;
    assert_eq!((icons.len(), icons[0].shape.as_str(), icons[0].to_shape), (1, "a", false));
}
