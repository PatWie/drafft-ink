#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The opacity slider is a scrub gesture (docs/exceptions.md E13). These tests drive it through
//! `UiShell::frame` with real pointer events, as the app does, and check what is left behind: no refusal, no
//! open gesture, one undo step, a UI that is not greyed.

use drafftink_engine::Session;
use drafftink_engine::ports::fake::Fakes;
use drafftink_engine::services::Services;
use drafftink_engine::testing::{add_rect, select};
use drafftink_scene::ElementId;
use drafftink_ui::{FrameInput, UiShell};
use egui::{Event, Key, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;

struct App {
    shell: UiShell,
    session: Session,
    /// Every command the frames ran, with the refusal text when there was one.
    ran: Vec<(String, Option<String>)>,
}

type H = Harness<'static, App>;

fn app() -> (H, ElementId) {
    let fakes = Fakes::new();
    let mut session = Session::with_services(fakes.ports(), Services::fake());
    let id = add_rect(&mut session, 0.0, 0.0, 100.0, 60.0).unwrap();
    select(&mut session, std::slice::from_ref(&id));
    let mut h = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .with_context_setup(drafftink_ui::theme::install_fonts)
        .build_ui_state(
            |ui, app: &mut App| {
                let result = app.shell.frame(ui, &mut app.session, FrameInput::default());
                let ran = result.report.ran.iter();
                app.ran.extend(ran.map(|e| (e.call.id.clone(), e.result.as_ref().err().map(|r| r.message.clone()))));
            },
            App { shell: UiShell::new(false), session, ran: Vec::new() },
        );
    h.run();
    (h, id)
}

fn slider(h: &H) -> egui::Rect {
    h.get_by_role_and_label(egui::accesskit::Role::Slider, "Opacity").rect()
}

/// A point on the slider at a fraction of its width.
fn at(h: &H, fraction: f32) -> Pos2 {
    let r = slider(h);
    Pos2::new(r.left() + r.width() * fraction, r.center().y)
}

fn press(h: &mut H, pos: Pos2) {
    h.event(Event::PointerMoved(pos));
    h.step();
    h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
}

fn release(h: &mut H, pos: Pos2) {
    h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.step();
    h.run();
}

fn drag_to(h: &mut H, fractions: &[f32]) {
    for f in fractions {
        let pos = at(h, *f);
        h.event(Event::PointerMoved(pos));
        h.step();
    }
}

fn key(h: &mut H, key: Key) {
    for pressed in [true, false] {
        h.event(Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers: Modifiers::NONE });
        h.step();
    }
}

fn opacity(h: &H) -> f64 {
    h.state().shell.model.style.num_value("opacity").unwrap()
}

fn depth(h: &H) -> usize {
    h.state().session.tab().doc.undo_depth()
}

fn refusals(h: &H) -> Vec<&(String, Option<String>)> {
    h.state().ran.iter().filter(|(_, r)| r.is_some()).collect()
}

fn toasts(h: &H) -> Vec<String> {
    h.state().shell.state.toasts.iter().map(|t| t.text.clone()).collect()
}

fn ids(h: &H) -> Vec<&str> {
    h.state().ran.iter().map(|(id, _)| id.as_str()).collect()
}

/// No command of the registry is greyed because a gesture is running, and the tool buttons are enabled.
fn assert_chrome_enabled(h: &H) {
    let model = &h.state().shell.model;
    let blocked: Vec<_> = model
        .commands
        .iter()
        .filter(|c| c.available.as_ref().err().is_some_and(|r| r.code == "gesture_active"))
        .map(|c| c.id)
        .collect();
    assert!(blocked.is_empty(), "greyed by a gesture: {blocked:?}");
    assert!(!h.state().session.gesture_running());
    for label in ["Rectangle", "Ellipse"] {
        assert!(!h.get_by_label_contains(label).accesskit_node().is_disabled(), "{label} is greyed");
    }
}

#[test]
fn a_drag_of_the_slider_is_one_gesture_and_one_undo_step_without_a_toast() {
    let (mut h, _) = app();
    let before = depth(&h);
    let from = at(&h, 0.9);
    press(&mut h, from);
    drag_to(&mut h, &[0.8, 0.6, 0.4, 0.3]);
    // While the drag runs, the slider is still enabled: a greyed slider loses the drag.
    assert!(!h.get_by_role_and_label(egui::accesskit::Role::Slider, "Opacity").accesskit_node().is_disabled());
    assert!(h.state().session.gesture_running());
    let to = at(&h, 0.3);
    release(&mut h, to);

    let ids = ids(&h);
    assert_eq!(ids.first().copied(), Some("gesture.begin"), "{ids:?}");
    assert_eq!(ids.iter().filter(|i| **i == "gesture.begin").count(), 1, "{ids:?}");
    assert_eq!(ids.iter().filter(|i| **i == "gesture.end").count(), 1, "{ids:?}");
    assert_eq!(ids.last().copied(), Some("gesture.end"), "{ids:?}");
    assert!(refusals(&h).is_empty(), "{:?}", refusals(&h));
    assert!(toasts(&h).is_empty(), "{:?}", toasts(&h));
    assert!(!h.state().session.gesture_running(), "the release ends the gesture");
    assert_eq!(depth(&h), before + 1, "a whole drag is one undo step");
    assert!((opacity(&h) - 0.3).abs() < 0.06, "{}", opacity(&h));
}

#[test]
fn after_a_drag_a_click_on_an_element_and_on_the_canvas_work_and_the_chrome_is_not_greyed() {
    let (mut h, _) = app();
    let from = at(&h, 0.9);
    press(&mut h, from);
    drag_to(&mut h, &[0.7, 0.5]);
    let to = at(&h, 0.5);
    release(&mut h, to);

    // Click on the element, then on empty canvas (what the canvas runs for a click).
    let state = h.state_mut();
    let on_element = state.session.execute("selection.select_at", json!({ "point": [50, 30] }));
    let on_canvas = state.session.execute("selection.select_at", json!({ "point": [900, 700] }));
    assert!(on_element.is_ok(), "{on_element:?}");
    assert!(on_canvas.is_ok(), "{on_canvas:?}");
    h.run();
    assert!(toasts(&h).is_empty(), "{:?}", toasts(&h));
    assert_chrome_enabled(&h);
}

#[test]
fn a_click_on_the_slider_is_one_plain_command_and_no_gesture() {
    let (mut h, _) = app();
    let before = depth(&h);
    let p = at(&h, 0.5);
    press(&mut h, p);
    release(&mut h, p);
    assert!(!h.state().session.gesture_running());
    assert!(refusals(&h).is_empty(), "{:?}", refusals(&h));
    assert_eq!(depth(&h), before + 1);
    assert!((opacity(&h) - 0.5).abs() < 0.06, "{}", opacity(&h));
    assert_chrome_enabled(&h);
}

#[test]
fn a_slider_that_vanishes_mid_drag_leaves_no_open_gesture() {
    let (mut h, _) = app();
    let before = depth(&h);
    let from = at(&h, 0.9);
    press(&mut h, from);
    drag_to(&mut h, &[0.6, 0.4]);
    assert!(h.state().session.gesture_running());

    // The selection goes away (a remote change or an undo does this; a command would be refused during the
    // gesture), so the ribbon card regroups and the next frame does not draw the slider.
    let updates = ids(&h).iter().filter(|i| **i == "gesture.update").count();
    assert!(!ids(&h).contains(&"gesture.end"));
    select(&mut h.state_mut().session, &[]);
    h.step();
    h.step();
    assert!(!h.state().session.gesture_running(), "the frame without the slider ended the gesture");
    assert!(refusals(&h).is_empty(), "{:?}", refusals(&h));
    assert_eq!(ids(&h).iter().filter(|i| **i == "gesture.end").count(), 1, "{:?}", ids(&h));
    assert_eq!(ids(&h).iter().filter(|i| **i == "gesture.update").count(), updates);
    assert_eq!(depth(&h), before + 1, "the drag so far is one undo step");

    // The pointer is released later, far from where the slider was; the next command works.
    release(&mut h, Pos2::new(600.0, 600.0));
    h.state_mut().session.execute("tool.set", json!({ "tool": "ellipse" })).unwrap();
    assert!(toasts(&h).is_empty(), "{:?}", toasts(&h));
    assert_chrome_enabled(&h);
}

#[test]
fn esc_cancels_the_drag_and_restores_the_old_opacity() {
    let (mut h, _) = app();
    let before = depth(&h);
    let old = opacity(&h);
    let from = at(&h, 0.9);
    press(&mut h, from);
    drag_to(&mut h, &[0.6, 0.3]);
    assert!((opacity(&h) - old).abs() > 0.1, "the drag changed the live value");

    key(&mut h, Key::Escape);
    assert!(!h.state().session.gesture_running());
    assert!((opacity(&h) - old).abs() < 1e-9, "restored: {} vs {old}", opacity(&h));
    // The pointer is still down: moving on must not begin a new gesture.
    drag_to(&mut h, &[0.2, 0.1]);
    assert!(!h.state().session.gesture_running());
    let end = at(&h, 0.1);
    release(&mut h, end);

    assert_eq!(depth(&h), before, "a cancelled drag leaves no undo step");
    assert!((opacity(&h) - old).abs() < 1e-9);
    assert_eq!(ids(&h).iter().filter(|i| **i == "gesture.begin").count(), 1, "{:?}", ids(&h));
    assert!(refusals(&h).is_empty() && toasts(&h).is_empty(), "{:?} {:?}", refusals(&h), toasts(&h));
    assert_chrome_enabled(&h);
}

#[test]
fn losing_the_window_focus_mid_drag_ends_the_gesture() {
    let (mut h, _) = app();
    let before = depth(&h);
    let from = at(&h, 0.9);
    press(&mut h, from);
    drag_to(&mut h, &[0.6, 0.4]);
    assert!(h.state().session.gesture_running());

    h.event(Event::WindowFocused(false));
    h.step();
    h.step();
    assert!(!h.state().session.gesture_running());
    assert_eq!(depth(&h), before + 1);
    assert!(refusals(&h).is_empty(), "{:?}", refusals(&h));
    assert_chrome_enabled(&h);
}

#[test]
fn arrow_keys_on_the_slider_are_plain_commands_not_gestures() {
    let (mut h, _) = app();
    let before = depth(&h);
    h.get_by_role_and_label(egui::accesskit::Role::Slider, "Opacity").focus();
    h.run();
    key(&mut h, Key::ArrowLeft);
    h.run();
    assert!(opacity(&h) < 1.0, "the arrow key moved the slider: {}", opacity(&h));
    let ids = ids(&h);
    assert!(!ids.iter().any(|i| i.starts_with("gesture.")), "{ids:?}");
    assert!(ids.contains(&"style.set"), "{ids:?}");
    assert!(refusals(&h).is_empty(), "{:?}", refusals(&h));
    assert_eq!(depth(&h), before + 1);
    assert_chrome_enabled(&h);
}
