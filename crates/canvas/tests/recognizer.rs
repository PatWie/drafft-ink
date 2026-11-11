//! Scripted event sequences against the recognizer state machine, with a fake scene.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use PointerPhase::{Cancel, Down, Move, Up};
use common::{id, identity_viewport};
use drafftink_canvas::geom::kurbo::{Point, Rect, Vec2};
use drafftink_canvas::{
    Button, Buttons, Cursor, FingerMode, Handle, InputConfig, InputEvent, Intent, Modifiers, Picker, PointerEvent,
    PointerKind, PointerMode, PointerPhase, Recognizer, Target, ViewSource, Viewport, WheelEvent, WheelUnit,
};

/// A scene of rectangles on screen: the first region containing the point wins; the rest is empty.
struct Fake {
    vp: Viewport,
    regions: Vec<(Rect, Target)>,
}

fn elem(name: &str) -> Target {
    Target::Element { id: id(name), unit_group: None }
}

impl Fake {
    fn new() -> Self {
        Fake {
            vp: identity_viewport(),
            regions: vec![
                (Rect::new(10.0, 10.0, 110.0, 110.0), elem("a")),
                (Rect::new(150.0, 10.0, 250.0, 110.0), elem("b")),
                (Rect::new(300.0, 10.0, 320.0, 30.0), Target::Handle(Handle::BottomRight)),
            ],
        }
    }
}

impl Picker for Fake {
    fn viewport(&self) -> &Viewport {
        &self.vp
    }
    fn pick(&self, p: Point, _kind: PointerKind) -> Target {
        self.regions.iter().find(|(r, _)| r.contains(p)).map_or(Target::Empty, |(_, t)| t.clone())
    }
    fn cursor(&self, t: &Target) -> Cursor {
        match t {
            Target::Handle(_) => Cursor::ResizeNwse,
            Target::Element { .. } => Cursor::Move,
            _ => Cursor::Default,
        }
    }
}

fn run(rec: &mut Recognizer, fake: &Fake, events: &[PointerEvent]) -> Vec<Intent> {
    let mut out = Vec::new();
    for e in events {
        rec.handle(&InputEvent::Pointer(e.clone()), fake, &mut out);
    }
    out
}

fn m(phase: PointerPhase, x: f64, y: f64, t: f64) -> PointerEvent {
    PointerEvent::mouse(phase, x, y, t)
}

fn mh(x: f64, y: f64, t: f64) -> PointerEvent {
    PointerEvent::mouse(Move, x, y, t).held()
}

fn names(intents: &[Intent]) -> Vec<&'static str> {
    intents
        .iter()
        .map(|i| match i {
            Intent::Press { .. } => "press",
            Intent::Click { .. } => "click",
            Intent::DoubleClick { .. } => "dblclick",
            Intent::LongPress { .. } => "longpress",
            Intent::ContextMenu { .. } => "contextmenu",
            Intent::DragStart { .. } => "dragstart",
            Intent::DragUpdate { .. } => "dragupdate",
            Intent::DragEnd { cancelled: false, .. } => "dragend",
            Intent::DragEnd { cancelled: true, .. } => "dragcancel",
            Intent::Hover { .. } => "hover",
            Intent::Pan { .. } => "pan",
            Intent::Zoom { .. } => "zoom",
            Intent::ViewEnd => "viewend",
        })
        .collect()
}

#[test]
fn press_and_release_is_a_click_on_the_target() {
    let fake = Fake::new();
    let mut r = Recognizer::default();
    let out = run(&mut r, &fake, &[m(Down, 50.0, 50.0, 0.0), m(Up, 50.0, 50.0, 80.0)]);
    assert_eq!(names(&out), ["press", "click", "hover"], "the release also picks the hover: None -> a");
    let Intent::Click { target, at, .. } = &out[1] else { panic!() };
    assert_eq!(target, &elem("a"));
    assert_eq!(at.world, Point::new(50.0, 50.0));
}

#[test]
fn click_versus_drag_depends_on_the_slop() {
    let fake = Fake::new();
    // 3 px of movement is still a click (the circle is "more than 3 px").
    let mut r = Recognizer::default();
    let out = run(&mut r, &fake, &[m(Down, 50.0, 50.0, 0.0), mh(53.0, 50.0, 10.0), m(Up, 53.0, 50.0, 20.0)]);
    assert_eq!(names(&out)[..2], ["press", "click"]);
    assert!(!names(&out).contains(&"dragstart"));

    // 4 px starts a drag; no click follows.
    let mut r = Recognizer::default();
    let out = run(&mut r, &fake, &[m(Down, 50.0, 50.0, 0.0), mh(54.0, 50.0, 10.0), m(Up, 54.0, 50.0, 20.0)]);
    assert_eq!(names(&out)[..4], ["press", "dragstart", "dragupdate", "dragend"]);
    assert!(!names(&out).contains(&"click"));

    // Moving out and back inside the circle before the release still counts as a drag once it left.
    let mut r = Recognizer::default();
    let out = run(
        &mut r,
        &fake,
        &[m(Down, 50.0, 50.0, 0.0), mh(60.0, 50.0, 10.0), mh(50.0, 50.0, 20.0), m(Up, 50.0, 50.0, 30.0)],
    );
    assert!(names(&out).contains(&"dragstart") && !names(&out).contains(&"click"));
}

#[test]
fn the_slop_is_configurable_and_larger_for_fingers() {
    let fake = Fake::new();
    let cfg = InputConfig { drag_slop_px: 10.0, ..InputConfig::default() };
    let mut r = Recognizer::new(cfg);
    let out = run(&mut r, &fake, &[m(Down, 50.0, 50.0, 0.0), mh(58.0, 50.0, 10.0), m(Up, 58.0, 50.0, 20.0)]);
    assert!(names(&out).contains(&"click"));

    let mut r = Recognizer::default();
    let t = |phase, x, tm| PointerEvent::touch(0, phase, x, 50.0, tm);
    let out = run(&mut r, &fake, &[t(Down, 50.0, 0.0), t(Move, 57.0, 10.0), t(Up, 57.0, 20.0)]);
    assert!(names(&out).contains(&"click"), "7 px is within the 8 px finger slop: {:?}", names(&out));
    let mut r = Recognizer::default();
    let out = run(&mut r, &fake, &[t(Down, 50.0, 0.0), t(Move, 59.0, 10.0), t(Up, 59.0, 20.0)]);
    assert!(names(&out).contains(&"dragstart"));
}

#[test]
fn a_drag_reports_origin_totals_and_the_press_target() {
    let fake = Fake::new();
    let mut r = Recognizer::default();
    let out = run(
        &mut r,
        &fake,
        &[
            m(Down, 50.0, 50.0, 0.0),
            mh(60.0, 50.0, 10.0),
            mh(100.0, 80.0, 20.0),
            mh(300.0, 300.0, 30.0),
            m(Up, 300.0, 300.0, 40.0),
        ],
    );
    assert_eq!(names(&out)[..6], ["press", "dragstart", "dragupdate", "dragupdate", "dragupdate", "dragend"]);
    let Intent::DragStart { origin, at, target, .. } = &out[1] else { panic!() };
    assert_eq!((origin.screen, at.screen, target), (Point::new(50.0, 50.0), Point::new(60.0, 50.0), &elem("a")));
    let Intent::DragUpdate { delta_screen, delta_world, target, .. } = &out[3] else { panic!() };
    assert_eq!((*delta_screen, *delta_world), (Vec2::new(50.0, 30.0), Vec2::new(50.0, 30.0)));
    assert_eq!(target, &elem("a"), "the target stays the pressed element though the pointer left it");
    let Intent::DragEnd { delta_screen, cancelled, .. } = &out[5] else { panic!() };
    assert_eq!((*delta_screen, *cancelled), (Vec2::new(250.0, 250.0), false));
}

#[test]
fn dragging_empty_space_is_a_marquee_and_dragging_a_handle_targets_it() {
    let fake = Fake::new();
    let mut r = Recognizer::default();
    let out = run(&mut r, &fake, &[m(Down, 400.0, 400.0, 0.0), mh(420.0, 430.0, 10.0), m(Up, 420.0, 430.0, 20.0)]);
    let Intent::DragStart { target, .. } = &out[1] else { panic!() };
    assert_eq!(target, &Target::Empty);
    let mut r = Recognizer::default();
    let out = run(&mut r, &fake, &[m(Down, 310.0, 20.0, 0.0), mh(330.0, 40.0, 10.0), m(Up, 330.0, 40.0, 20.0)]);
    let Intent::DragStart { target, .. } = &out[1] else { panic!() };
    assert_eq!(target, &Target::Handle(Handle::BottomRight));
}

#[test]
fn double_click_needs_time_distance_and_the_same_target() {
    let fake = Fake::new();
    let click = |x: f64, y: f64, t: f64| vec![m(Down, x, y, t), m(Up, x, y, t + 40.0)];
    let kinds = |out: &[Intent]| names(out).into_iter().filter(|n| n.contains("click")).collect::<Vec<_>>();

    let mut r = Recognizer::default();
    let out = run(&mut r, &fake, &[click(50.0, 50.0, 0.0), click(52.0, 51.0, 300.0)].concat());
    assert_eq!(kinds(&out), ["click", "click", "dblclick"]);

    // Too slow.
    let mut r = Recognizer::default();
    let out = run(&mut r, &fake, &[click(50.0, 50.0, 0.0), click(50.0, 50.0, 700.0)].concat());
    assert_eq!(kinds(&out), ["click", "click"]);

    // Too far apart.
    let mut r = Recognizer::default();
    let out = run(&mut r, &fake, &[click(50.0, 50.0, 0.0), click(60.0, 50.0, 200.0)].concat());
    assert_eq!(kinds(&out), ["click", "click"]);

    // Different targets.
    let mut r = Recognizer::default();
    let out = run(&mut r, &fake, &[click(105.0, 50.0, 0.0), click(155.0, 50.0, 200.0)].concat());
    assert_eq!(kinds(&out), ["click", "click"]);

    // A third click starts over: click, click, dblclick, click, (no second dblclick).
    let mut r = Recognizer::default();
    let out =
        run(&mut r, &fake, &[click(50.0, 50.0, 0.0), click(50.0, 50.0, 100.0), click(50.0, 50.0, 200.0)].concat());
    assert_eq!(kinds(&out), ["click", "click", "dblclick", "click"]);

    // Empty space double-clicks too.
    let mut r = Recognizer::default();
    let out = run(&mut r, &fake, &[click(500.0, 500.0, 0.0), click(500.0, 500.0, 100.0)].concat());
    assert_eq!(kinds(&out), ["click", "click", "dblclick"]);

    // Thresholds are config.
    let mut r = Recognizer::new(InputConfig { double_click_ms: 100.0, ..InputConfig::default() });
    let out = run(&mut r, &fake, &[click(50.0, 50.0, 0.0), click(50.0, 50.0, 300.0)].concat());
    assert_eq!(kinds(&out), ["click", "click"]);
}

#[test]
fn a_drag_between_two_clicks_breaks_the_double_click() {
    let fake = Fake::new();
    let mut r = Recognizer::default();
    let out = run(
        &mut r,
        &fake,
        &[
            m(Down, 50.0, 50.0, 0.0),
            m(Up, 50.0, 50.0, 40.0),
            m(Down, 50.0, 50.0, 100.0),
            mh(80.0, 50.0, 120.0),
            m(Up, 80.0, 50.0, 140.0),
        ],
    );
    assert!(!names(&out).contains(&"dblclick"));
}

#[test]
fn touch_long_press_by_tick_and_by_event_time() {
    let fake = Fake::new();
    let t = |phase, x, tm| PointerEvent::touch(0, phase, x, 50.0, tm);
    let mut r = Recognizer::default();
    let mut out = Vec::new();
    r.handle(&t(Down, 50.0, 0.0).into(), &fake, &mut out);
    assert_eq!(r.next_deadline(), Some(500.0));
    r.tick(499.0, &mut out);
    assert_eq!(names(&out), ["press"]);
    r.tick(500.0, &mut out);
    assert_eq!(names(&out), ["press", "longpress"]);
    assert_eq!(r.next_deadline(), None, "only once");
    r.tick(900.0, &mut out);
    r.handle(&t(Up, 50.0, 1000.0).into(), &fake, &mut out);
    assert_eq!(names(&out), ["press", "longpress"], "the release after a long press is not a click");

    // Nobody called tick, the release arrives late: still a long press, not a click.
    let mut r = Recognizer::default();
    let out = run(&mut r, &fake, &[t(Down, 50.0, 0.0), t(Up, 50.0, 800.0)]);
    assert_eq!(names(&out), ["press", "longpress"]);

    // Moving past the slop first makes it a drag and cancels the long press.
    let mut r = Recognizer::default();
    let mut out = run(&mut r, &fake, &[t(Down, 50.0, 0.0), t(Move, 70.0, 100.0)]);
    r.tick(900.0, &mut out);
    assert!(!names(&out).contains(&"longpress") && names(&out).contains(&"dragstart"));
    assert_eq!(r.next_deadline(), None);
}

#[test]
fn a_mouse_has_no_long_press_unless_enabled() {
    let fake = Fake::new();
    let mut r = Recognizer::default();
    let mut out = run(&mut r, &fake, &[m(Down, 50.0, 50.0, 0.0)]);
    assert_eq!(r.next_deadline(), None);
    r.tick(5000.0, &mut out);
    assert_eq!(names(&out), ["press"]);
    let mut r = Recognizer::new(InputConfig { long_press_mouse: true, ..InputConfig::default() });
    let mut out = run(&mut r, &fake, &[m(Down, 50.0, 50.0, 0.0)]);
    r.tick(600.0, &mut out);
    assert_eq!(names(&out), ["press", "longpress"]);
    let mut r = Recognizer::new(InputConfig { long_press_ms: None, long_press_mouse: true, ..InputConfig::default() });
    let mut out = run(&mut r, &fake, &[m(Down, 50.0, 50.0, 0.0)]);
    r.tick(6000.0, &mut out);
    assert_eq!(names(&out), ["press"]);
}

#[test]
fn secondary_button_gives_a_context_menu_and_middle_drag_pans() {
    let fake = Fake::new();
    let mut r = Recognizer::default();
    let right = |phase, x, y, tm| {
        let held = if phase == Down { Buttons::SECONDARY } else { Buttons::NONE };
        PointerEvent::mouse(phase, x, y, tm).with_button(Button::Secondary, held)
    };
    let out = run(&mut r, &fake, &[right(Down, 50.0, 50.0, 0.0), right(Up, 50.0, 50.0, 30.0)]);
    assert_eq!(names(&out)[..2], ["press", "contextmenu"]);

    // A right-button drag does nothing at all.
    let mut r = Recognizer::default();
    let out = run(
        &mut r,
        &fake,
        &[
            right(Down, 50.0, 50.0, 0.0),
            PointerEvent::mouse(Move, 90.0, 50.0, 10.0).with_button(Button::None, Buttons::SECONDARY),
            right(Up, 90.0, 50.0, 30.0),
        ],
    );
    assert!(!names(&out).iter().any(|n| ["contextmenu", "click", "dragstart"].contains(n)), "{:?}", names(&out));

    // Middle drag pans by the movement and ends with ViewEnd.
    let mut r = Recognizer::default();
    let mid = |phase, x, y, tm| {
        let held = if phase == Down { Buttons::MIDDLE } else { Buttons::NONE };
        PointerEvent::mouse(phase, x, y, tm).with_button(Button::Middle, held)
    };
    let mv = |x, y, tm| {
        let mut e = PointerEvent::mouse(Move, x, y, tm);
        e.buttons = Buttons::MIDDLE;
        e
    };
    let out = run(
        &mut r,
        &fake,
        &[mid(Down, 200.0, 200.0, 0.0), mv(210.0, 205.0, 10.0), mv(230.0, 205.0, 20.0), mid(Up, 230.0, 205.0, 30.0)],
    );
    let pans: Vec<Vec2> = out
        .iter()
        .filter_map(|i| match i {
            Intent::Pan { delta, source: ViewSource::MiddleButton } => Some(*delta),
            _ => None,
        })
        .collect();
    assert_eq!(pans, [Vec2::new(10.0, 5.0), Vec2::new(20.0, 0.0)]);
    assert!(names(&out).contains(&"viewend"));
    assert!(!names(&out).iter().any(|n| n.starts_with("drag")));
}

#[test]
fn releasing_another_mouse_button_does_not_end_the_gesture() {
    let fake = Fake::new();
    let mut r = Recognizer::default();
    let right_up = PointerEvent::mouse(Up, 80.0, 50.0, 25.0).with_button(Button::Secondary, Buttons::PRIMARY);
    let out = run(
        &mut r,
        &fake,
        &[m(Down, 50.0, 50.0, 0.0), mh(70.0, 50.0, 10.0), right_up, mh(90.0, 50.0, 30.0), m(Up, 90.0, 50.0, 40.0)],
    );
    assert_eq!(names(&out).iter().filter(|n| **n == "dragend").count(), 1);
    assert_eq!(names(&out).iter().filter(|n| **n == "dragupdate").count(), 2);
}

#[test]
fn hand_mode_pans_and_draw_mode_drags_at_once() {
    let fake = Fake::new();
    let mut r = Recognizer::default();
    r.set_mode(PointerMode::Hand);
    let out = run(
        &mut r,
        &fake,
        &[m(Down, 50.0, 50.0, 0.0), mh(60.0, 50.0, 10.0), mh(70.0, 60.0, 20.0), m(Up, 70.0, 60.0, 30.0)],
    );
    assert!(names(&out).iter().all(|n| !n.starts_with("drag")));
    let total: Vec2 = out
        .iter()
        .filter_map(|i| if let Intent::Pan { delta, source: ViewSource::HandTool } = i { Some(*delta) } else { None })
        .fold(Vec2::ZERO, |a, b| a + b);
    assert_eq!(total, Vec2::new(20.0, 10.0), "the pan includes the slop distance");
    assert!(names(&out).contains(&"viewend"));

    let mut r = Recognizer::default();
    r.set_mode(PointerMode::Draw);
    let out = run(&mut r, &fake, &[m(Down, 50.0, 50.0, 0.0), mh(50.5, 50.0, 5.0), m(Up, 50.5, 50.0, 10.0)]);
    assert_eq!(names(&out)[..4], ["press", "dragstart", "dragupdate", "dragend"], "half a pixel is enough");
    // But a press without any movement is still a click (tap to place).
    let mut r = Recognizer::default();
    r.set_mode(PointerMode::Draw);
    let out = run(&mut r, &fake, &[m(Down, 50.0, 50.0, 0.0), m(Up, 50.0, 50.0, 10.0)]);
    assert_eq!(names(&out)[..2], ["press", "click"]);
}

#[test]
fn pen_samples_carry_pressure() {
    let fake = Fake::new();
    let mut r = Recognizer::default();
    r.set_mode(PointerMode::Draw);
    let mut mv = PointerEvent::pen(Move, 60.0, 50.0, 0.8, 10.0);
    mv.buttons = Buttons::PRIMARY;
    let out = run(
        &mut r,
        &fake,
        &[PointerEvent::pen(Down, 50.0, 50.0, 0.3, 0.0), mv, PointerEvent::pen(Up, 60.0, 50.0, 0.0, 20.0)],
    );
    let Intent::Press { meta, .. } = &out[0] else { panic!() };
    assert_eq!((meta.kind, meta.pressure), (PointerKind::Pen, 0.3));
    let Intent::DragUpdate { meta, .. } = &out[2] else { panic!() };
    assert_eq!(meta.pressure, 0.8);
}

#[test]
fn modifiers_travel_with_the_intents() {
    let fake = Fake::new();
    let mut r = Recognizer::default();
    let shift = Modifiers { shift: true, ..Modifiers::NONE };
    let out = run(
        &mut r,
        &fake,
        &[m(Down, 50.0, 50.0, 0.0).with_modifiers(shift), m(Up, 50.0, 50.0, 10.0).with_modifiers(shift)],
    );
    let Intent::Click { meta, .. } = &out[1] else { panic!() };
    assert!(meta.modifiers.shift);
}

#[test]
fn two_fingers_pinch_and_pan_and_the_rest_is_ignored() {
    let fake = Fake::new();
    let t = |id, phase, x, y, tm| PointerEvent::touch(id, phase, x, y, tm);
    let mut r = Recognizer::default();
    let out = run(&mut r, &fake, &[t(0, Down, 300.0, 300.0, 0.0), t(1, Down, 400.0, 300.0, 5.0)]);
    assert_eq!(names(&out), ["press"], "landing the second finger starts the view gesture quietly");

    // Spread: finger 1 moves 100 px away. Distance 100 -> 200 gives a factor of 2 about the midpoint.
    let out = run(&mut r, &fake, &[t(1, Move, 500.0, 300.0, 20.0)]);
    assert_eq!(names(&out), ["pan", "zoom"], "pan first, then zoom about the new centre");
    let Intent::Pan { delta, source: ViewSource::Touch } = &out[0] else { panic!() };
    assert_eq!(*delta, Vec2::new(50.0, 0.0));
    let Intent::Zoom { at, factor, .. } = &out[1] else { panic!() };
    assert_eq!((*at, *factor), (Point::new(400.0, 300.0), 2.0));

    // Both fingers move down together: pure pan.
    let out = run(&mut r, &fake, &[t(0, Move, 300.0, 340.0, 30.0), t(1, Move, 500.0, 340.0, 31.0)]);
    let pans: Vec<Vec2> =
        out.iter().filter_map(|i| if let Intent::Pan { delta, .. } = i { Some(*delta) } else { None }).collect();
    assert_eq!(pans.iter().fold(Vec2::ZERO, |a, b| a + *b), Vec2::new(0.0, 40.0));
    // The fingers arrive one event each, so the distance wobbles in between; the net zoom is nil.
    let net: f64 =
        out.iter().filter_map(|i| if let Intent::Zoom { factor, .. } = i { Some(*factor) } else { None }).product();
    assert!((net - 1.0).abs() < 1e-9, "{net}");

    // A third finger changes nothing.
    let out = run(&mut r, &fake, &[t(2, Down, 50.0, 50.0, 40.0), t(2, Up, 50.0, 50.0, 60.0)]);
    assert!(out.is_empty(), "{:?}", names(&out));

    // Lifting one finger ends the gesture; the other finger is ignored until it lifts: no click, no drag.
    let out = run(
        &mut r,
        &fake,
        &[t(1, Up, 500.0, 340.0, 70.0), t(0, Move, 320.0, 340.0, 80.0), t(0, Up, 320.0, 340.0, 90.0)],
    );
    assert_eq!(names(&out), ["viewend"]);
    // And the recognizer is ready for a new tap.
    let out = run(&mut r, &fake, &[t(0, Down, 50.0, 50.0, 100.0), t(0, Up, 50.0, 50.0, 110.0)]);
    assert_eq!(names(&out), ["press", "click"]);
}

#[test]
fn pinch_ignores_tiny_distance_changes() {
    let fake = Fake::new();
    let t = |id, phase, x, y, tm| PointerEvent::touch(id, phase, x, y, tm);
    let mut r = Recognizer::default();
    run(&mut r, &fake, &[t(0, Down, 300.0, 300.0, 0.0), t(1, Down, 400.0, 300.0, 5.0)]);
    // 0.05 px on 100 px is a ratio of 1.0005: below the 0.001 threshold, so no zoom yet...
    let out = run(&mut r, &fake, &[t(1, Move, 400.05, 300.0, 10.0)]);
    assert!(!names(&out).contains(&"zoom"));
    // ...but the change accumulates: another 0.1 px crosses it.
    let out = run(&mut r, &fake, &[t(1, Move, 400.15, 300.0, 20.0)]);
    assert!(names(&out).contains(&"zoom"));
}

#[test]
fn a_second_finger_cancels_a_one_finger_drag() {
    let fake = Fake::new();
    let t = |id, phase, x, y, tm| PointerEvent::touch(id, phase, x, y, tm);
    let mut r = Recognizer::default();
    let out = run(
        &mut r,
        &fake,
        &[t(0, Down, 50.0, 50.0, 0.0), t(0, Move, 80.0, 50.0, 10.0), t(1, Down, 400.0, 300.0, 20.0)],
    );
    assert_eq!(names(&out), ["press", "dragstart", "dragupdate", "dragcancel"]);
}

#[test]
fn cancel_all_aborts_a_drag_and_the_release_is_not_a_click() {
    let fake = Fake::new();
    let mut r = Recognizer::default();
    let mut out = run(&mut r, &fake, &[m(Down, 50.0, 50.0, 0.0), mh(80.0, 50.0, 10.0)]);
    r.handle(&InputEvent::CancelAll, &fake, &mut out);
    out.extend(run(&mut r, &fake, &[m(Up, 80.0, 50.0, 20.0)]));
    assert_eq!(names(&out), ["press", "dragstart", "dragupdate", "dragcancel"]);
    assert!(r.is_idle());

    // A pointer cancel does the same.
    let mut r = Recognizer::default();
    let out = run(&mut r, &fake, &[m(Down, 50.0, 50.0, 0.0), mh(80.0, 50.0, 10.0), m(Cancel, 80.0, 50.0, 20.0)]);
    assert_eq!(names(&out)[3], "dragcancel");

    // Cancelling a press before the slop gives no click.
    let mut r = Recognizer::default();
    let out = run(&mut r, &fake, &[m(Down, 50.0, 50.0, 0.0), m(Cancel, 50.0, 50.0, 20.0)]);
    assert_eq!(names(&out), ["press", "hover"], "no click; the pointer is idle again and hovers what is under it");
}

#[test]
fn hover_is_emitted_only_when_the_target_changes() {
    let fake = Fake::new();
    let mut r = Recognizer::default();
    // 200 moves inside "a": exactly one Hover (None -> a).
    let moves: Vec<PointerEvent> =
        (0..200).map(|i| m(Move, 20.0 + (i % 80) as f64, 20.0 + (i % 70) as f64, i as f64)).collect();
    let out = run(&mut r, &fake, &moves);
    assert_eq!(names(&out), ["hover"]);
    let Intent::Hover { from, to, cursor, .. } = &out[0] else { panic!() };
    assert_eq!((from, to, *cursor), (&Target::None, &elem("a"), Cursor::Move));

    // Same position again: nothing. Out to empty space: one. Onto the handle: one with its cursor.
    assert!(run(&mut r, &fake, &[m(Move, 20.0, 20.0, 300.0)]).is_empty());
    let out =
        run(&mut r, &fake, &[m(Move, 130.0, 50.0, 310.0), m(Move, 135.0, 55.0, 320.0), m(Move, 310.0, 20.0, 330.0)]);
    assert_eq!(names(&out), ["hover", "hover"]);
    let Intent::Hover { to, cursor, .. } = &out[1] else { panic!() };
    assert_eq!((to, *cursor), (&Target::Handle(Handle::BottomRight), Cursor::ResizeNwse));

    // The pointer leaves the canvas: one change to None, and a second leave is silent.
    let mut leave = Vec::new();
    r.handle(&InputEvent::PointerLeave { id: drafftink_canvas::PointerId(0) }, &fake, &mut leave);
    assert_eq!(names(&leave), ["hover"]);
    let Intent::Hover { to, .. } = &leave[0] else { panic!() };
    assert_eq!(to, &Target::None);
    r.handle(&InputEvent::PointerLeave { id: drafftink_canvas::PointerId(0) }, &fake, &mut leave);
    assert_eq!(leave.len(), 1);
}

#[test]
fn touch_never_hovers() {
    let fake = Fake::new();
    let mut r = Recognizer::default();
    let out = run(&mut r, &fake, &[PointerEvent::touch(0, Move, 50.0, 50.0, 0.0)]);
    assert!(out.is_empty());
    let out = run(
        &mut r,
        &fake,
        &[PointerEvent::touch(0, Down, 50.0, 50.0, 10.0), PointerEvent::touch(0, Up, 50.0, 50.0, 20.0)],
    );
    assert_eq!(names(&out), ["press", "click"], "no hover after a tap either");
}

#[test]
fn hover_refresh_reports_a_change_when_the_scene_moved() {
    let mut fake = Fake::new();
    let mut r = Recognizer::default();
    run(&mut r, &fake, &[m(Move, 50.0, 50.0, 0.0)]);
    let mut out = Vec::new();
    r.refresh_hover(&fake, &mut out);
    assert!(out.is_empty(), "nothing changed");
    fake.regions[0].0 = Rect::new(500.0, 500.0, 600.0, 600.0);
    r.refresh_hover(&fake, &mut out);
    assert_eq!(names(&out), ["hover"]);
    let Intent::Hover { from, to, .. } = &out[0] else { panic!() };
    assert_eq!((from, to), (&elem("a"), &Target::Empty));
    // Not during a gesture.
    run(&mut r, &fake, &[m(Down, 50.0, 50.0, 10.0)]);
    fake.regions[1].0 = Rect::new(0.0, 0.0, 1000.0, 1000.0);
    out.clear();
    r.refresh_hover(&fake, &mut out);
    assert!(out.is_empty());
}

fn wheel(dx: f64, dy: f64, unit: WheelUnit, modifiers: Modifiers) -> InputEvent {
    InputEvent::Wheel(WheelEvent {
        pos: Point::new(200.0, 150.0),
        delta: Vec2::new(dx, dy),
        unit,
        modifiers,
        time_ms: 0.0,
    })
}

#[test]
fn wheel_pans_and_ctrl_wheel_zooms() {
    let fake = Fake::new();
    let ctrl = Modifiers { ctrl: true, ..Modifiers::NONE };
    let meta = Modifiers { meta: true, ..Modifiers::NONE };
    let shift = Modifiers { shift: true, ..Modifiers::NONE };
    let mut r = Recognizer::default();
    let mut out = Vec::new();
    r.handle(&wheel(0.0, 100.0, WheelUnit::Pixel, Modifiers::NONE), &fake, &mut out);
    assert_eq!(out, [Intent::Pan { delta: Vec2::new(0.0, -100.0), source: ViewSource::Wheel }]);
    out.clear();
    r.handle(&wheel(0.0, 3.0, WheelUnit::Line, Modifiers::NONE), &fake, &mut out);
    assert_eq!(out, [Intent::Pan { delta: Vec2::new(0.0, -99.0), source: ViewSource::Wheel }]);
    out.clear();
    r.handle(&wheel(0.0, 40.0, WheelUnit::Pixel, shift), &fake, &mut out);
    assert_eq!(
        out,
        [Intent::Pan { delta: Vec2::new(-40.0, 0.0), source: ViewSource::Wheel }],
        "shift turns a vertical wheel sideways"
    );
    out.clear();

    // One notch (100 px) up zooms in by the 1.1 step about the pointer; down zooms out.
    r.handle(&wheel(0.0, -100.0, WheelUnit::Pixel, ctrl), &fake, &mut out);
    let Intent::Zoom { at, factor, source: ViewSource::Wheel } = &out[0] else { panic!("{out:?}") };
    assert_eq!(*at, Point::new(200.0, 150.0));
    assert!((factor - 1.1).abs() < 1e-12);
    out.clear();
    r.handle(&wheel(0.0, 100.0, WheelUnit::Pixel, meta), &fake, &mut out);
    let Intent::Zoom { factor, .. } = &out[0] else { panic!() };
    assert!((factor - 1.0 / 1.1).abs() < 1e-12);
    out.clear();
    // A trackpad pinch (small deltas) zooms smoothly; a hostile delta is clamped.
    r.handle(&wheel(0.0, -2.0, WheelUnit::Pixel, ctrl), &fake, &mut out);
    let Intent::Zoom { factor, .. } = &out[0] else { panic!() };
    assert!(*factor > 1.0 && *factor < 1.003);
    out.clear();
    r.handle(&wheel(0.0, -1e9, WheelUnit::Pixel, ctrl), &fake, &mut out);
    let Intent::Zoom { factor, .. } = &out[0] else { panic!() };
    assert_eq!(*factor, 4.0);
    out.clear();
    r.handle(&wheel(f64::NAN, 1.0, WheelUnit::Pixel, ctrl), &fake, &mut out);
    r.handle(&wheel(0.0, 0.0, WheelUnit::Pixel, Modifiers::NONE), &fake, &mut out);
    assert!(out.is_empty());

    // Figma style: the plain wheel zooms.
    let mut r = Recognizer::new(InputConfig { wheel_zooms: true, ..InputConfig::default() });
    r.handle(&wheel(0.0, -100.0, WheelUnit::Pixel, Modifiers::NONE), &fake, &mut out);
    assert!(matches!(out[0], Intent::Zoom { .. }));
    out.clear();
    r.handle(&wheel(0.0, 100.0, WheelUnit::Pixel, ctrl), &fake, &mut out);
    assert!(matches!(out[0], Intent::Pan { .. }));
}

#[test]
fn the_same_events_give_the_same_intents() {
    let fake = Fake::new();
    let script = [
        m(Move, 50.0, 50.0, 0.0),
        m(Down, 50.0, 50.0, 10.0),
        mh(70.0, 60.0, 20.0),
        m(Up, 70.0, 60.0, 30.0),
        m(Down, 70.0, 60.0, 100.0),
        m(Up, 70.0, 60.0, 130.0),
    ];
    let a = run(&mut Recognizer::default(), &fake, &script);
    let b = run(&mut Recognizer::default(), &fake, &script);
    assert_eq!(a, b);
    // Intents serialise, so a JS host sees the same shapes.
    let json = serde_json::to_string(&a).unwrap();
    let back: Vec<Intent> = serde_json::from_str(&json).unwrap();
    assert_eq!(a, back);
}

#[test]
fn hostile_events_do_not_panic_or_corrupt_state() {
    let fake = Fake::new();
    let mut r = Recognizer::default();
    let out = run(
        &mut r,
        &fake,
        &[
            m(Down, f64::NAN, 1.0, 0.0),
            m(Move, f64::INFINITY, 1.0, 0.0),
            m(Up, 1.0, 1.0, f64::NAN),
            m(Up, 1.0, 1.0, 5.0),
            m(Move, 50.0, 50.0, 1.0),
        ],
    );
    assert_eq!(names(&out), ["hover"]);
    let mut out = Vec::new();
    r.tick(f64::NAN, &mut out);
    r.handle(&InputEvent::CancelAll, &fake, &mut out);
    assert_eq!(out.len(), 0);
}

#[test]
fn a_press_after_a_lost_release_starts_over() {
    let fake = Fake::new();
    let mut r = Recognizer::default();
    // The button went up outside the window; the next press arrives while the drag is still open.
    let out = run(
        &mut r,
        &fake,
        &[m(Down, 50.0, 50.0, 0.0), mh(80.0, 50.0, 10.0), m(Down, 60.0, 60.0, 500.0), m(Up, 60.0, 60.0, 540.0)],
    );
    assert_eq!(names(&out), ["press", "dragstart", "dragupdate", "dragcancel", "press", "click", "hover"]);
    assert!(r.is_idle());
}

#[test]
fn is_dragging_is_true_only_while_content_is_dragged() {
    let fake = Fake::new();
    let mut r = Recognizer::default();
    assert!(!r.is_dragging());
    run(&mut r, &fake, &[m(Down, 50.0, 50.0, 0.0), mh(60.0, 50.0, 10.0), mh(100.0, 80.0, 20.0)]);
    assert!(r.is_dragging());
    run(&mut r, &fake, &[m(Up, 100.0, 80.0, 30.0)]);
    assert!(!r.is_dragging());
}

#[test]
fn a_pen_that_touches_down_takes_over_from_a_finger_that_landed_first() {
    let fake = Fake::new();
    let t = |id, phase, x, y, tm| PointerEvent::touch(id, phase, x, y, tm);
    let mut r = Recognizer::default();
    r.set_mode(PointerMode::Draw);
    // A palm: it pressed and dragged before the pen came.
    let out = run(&mut r, &fake, &[t(0, Down, 200.0, 200.0, 0.0), t(0, Move, 210.0, 200.0, 5.0)]);
    assert_eq!(names(&out), ["press", "dragstart", "dragupdate"]);

    let mut mv = PointerEvent::pen(Move, 60.0, 50.0, 0.5, 20.0);
    mv.buttons = Buttons::PRIMARY;
    let out = run(&mut r, &fake, &[PointerEvent::pen(Down, 50.0, 50.0, 0.5, 10.0), mv]);
    assert_eq!(names(&out), ["dragcancel", "press", "dragstart", "dragupdate"], "the palm's drag is cancelled");

    // The palm moves and lifts while the pen draws: nothing from it.
    let out = run(&mut r, &fake, &[t(0, Move, 220.0, 200.0, 25.0), t(0, Up, 220.0, 200.0, 30.0)]);
    assert!(out.is_empty(), "{:?}", names(&out));
    let out = run(&mut r, &fake, &[PointerEvent::pen(Up, 60.0, 50.0, 0.0, 40.0)]);
    assert_eq!(names(&out), ["dragend", "hover"], "the pen's stroke ends whole; the pen hovers where it lifted");
    assert!(r.is_idle());
}

#[test]
fn a_finger_that_lands_during_a_pen_stroke_is_ignored_and_does_not_start_a_pinch() {
    let fake = Fake::new();
    let t = |id, phase, x, y, tm| PointerEvent::touch(id, phase, x, y, tm);
    let mut r = Recognizer::default();
    r.set_mode(PointerMode::Draw);
    let mut mv = PointerEvent::pen(Move, 60.0, 50.0, 0.5, 10.0);
    mv.buttons = Buttons::PRIMARY;
    run(&mut r, &fake, &[PointerEvent::pen(Down, 50.0, 50.0, 0.5, 0.0), mv]);
    let out = run(
        &mut r,
        &fake,
        &[t(0, Down, 300.0, 300.0, 20.0), t(1, Down, 400.0, 300.0, 21.0), t(1, Move, 450.0, 300.0, 30.0)],
    );
    assert!(out.is_empty(), "{:?}", names(&out));
    assert!(r.is_dragging(), "the pen still draws");
}

#[test]
fn with_pan_only_fingers_a_single_finger_pans_and_a_tap_does_nothing() {
    let fake = Fake::new();
    let t = |id, phase, x, y, tm| PointerEvent::touch(id, phase, x, y, tm);
    let mut r = Recognizer::default();
    r.set_mode(PointerMode::Draw);
    r.set_fingers(FingerMode::PanOnly);
    let out = run(
        &mut r,
        &fake,
        &[t(0, Down, 200.0, 200.0, 0.0), t(0, Move, 230.0, 210.0, 10.0), t(0, Move, 250.0, 210.0, 20.0)],
    );
    assert_eq!(names(&out), ["pan", "pan"], "no press, no drag of content");
    let Intent::Pan { delta, source: ViewSource::Touch } = &out[0] else { panic!() };
    assert_eq!(*delta, Vec2::new(30.0, 10.0));
    let out = run(&mut r, &fake, &[t(0, Up, 250.0, 210.0, 30.0)]);
    assert_eq!(names(&out), ["viewend"]);
    assert!(r.is_idle());

    // A tap is not a click: a resting palm selects nothing.
    let out = run(&mut r, &fake, &[t(0, Down, 50.0, 50.0, 100.0), t(0, Up, 50.0, 50.0, 110.0)]);
    assert_eq!(names(&out), ["viewend"]);

    // The pen and the mouse are not fingers.
    let out = run(&mut r, &fake, &[PointerEvent::pen(Down, 50.0, 50.0, 0.5, 200.0)]);
    assert_eq!(names(&out), ["press"]);
}

#[test]
fn pan_only_fingers_still_pinch_with_two() {
    let fake = Fake::new();
    let t = |id, phase, x, y, tm| PointerEvent::touch(id, phase, x, y, tm);
    let mut r = Recognizer::default();
    r.set_fingers(FingerMode::PanOnly);
    let out = run(&mut r, &fake, &[t(0, Down, 300.0, 300.0, 0.0), t(1, Down, 400.0, 300.0, 5.0)]);
    assert_eq!(names(&out), ["viewend"], "the one-finger pan ends and the two-finger gesture begins");
    let out = run(&mut r, &fake, &[t(1, Move, 500.0, 300.0, 20.0)]);
    assert_eq!(names(&out), ["pan", "zoom"]);
}

#[test]
fn after_a_two_finger_gesture_and_a_pen_stroke_the_recognizer_is_ready_for_the_next_tap() {
    let fake = Fake::new();
    let t = |id, phase, x, y, tm| PointerEvent::touch(id, phase, x, y, tm);
    let mut r = Recognizer::default();
    // Two fingers land, the pen comes down on top of that gesture and draws, everything lifts.
    run(&mut r, &fake, &[t(0, Down, 300.0, 300.0, 0.0), t(1, Down, 400.0, 300.0, 5.0)]);
    run(&mut r, &fake, &[PointerEvent::pen(Down, 50.0, 50.0, 0.5, 10.0)]);
    run(
        &mut r,
        &fake,
        &[PointerEvent::pen(Up, 50.0, 50.0, 0.0, 20.0), t(0, Up, 300.0, 300.0, 30.0), t(1, Up, 400.0, 300.0, 31.0)],
    );
    let out = run(&mut r, &fake, &[t(2, Down, 50.0, 50.0, 100.0), t(2, Up, 50.0, 50.0, 110.0)]);
    assert_eq!(names(&out), ["press", "click"]);
}
