#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The opacity slider is a scrub gesture (docs/exceptions.md E13). These tests drive it through
//! `UiShell::frame` with real pointer events, as the app does, and check what is left behind: no refusal, no
//! open gesture, one undo step, a UI that is not greyed.

use drafftink_engine::Session;
use drafftink_engine::ports::fake::Fakes;
use drafftink_engine::services::Services;
use drafftink_engine::testing::{add_rect, select};
use drafftink_scene::{ElementId, SceneRead};
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

fn slider_named(h: &H, label: &str) -> egui::Rect {
    h.get_by_role_and_label(egui::accesskit::Role::Slider, label).rect()
}

/// A point on the slider `label` at a fraction of its width.
fn at_on(h: &H, label: &str, fraction: f32) -> Pos2 {
    let r = slider_named(h, label);
    Pos2::new(r.left() + r.width() * fraction, r.center().y)
}

/// A point on the opacity slider at a fraction of its width.
fn at(h: &H, fraction: f32) -> Pos2 {
    at_on(h, "Opacity", fraction)
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

// ---- the corner radius slider ----

const RADIUS: &str = "Corner radius";

/// The radius button of the strip opens the menu that holds the slider.
fn open_radius_menu(h: &mut H) {
    h.get_by_role_and_label(egui::accesskit::Role::Button, RADIUS).click();
    h.run();
}

fn radius_app() -> (H, ElementId) {
    let (mut h, id) = app();
    open_radius_menu(&mut h);
    (h, id)
}

fn radius(h: &H) -> Option<f64> {
    h.state().shell.model.style.num_value("corner_radius")
}

/// What the session holds for the radius of one element.
fn stored_radius(h: &H, id: &ElementId) -> f64 {
    <drafftink_scene::MemoryStore as SceneRead>::get(h.state().session.store(), id).unwrap().style.corner_radius
}

fn drag_radius(h: &mut H, fractions: &[f32]) {
    for f in fractions {
        let pos = at_on(h, RADIUS, *f);
        h.event(Event::PointerMoved(pos));
        h.step();
    }
}

#[test]
fn a_drag_of_the_corner_slider_is_one_gesture_and_one_undo_step() {
    let (mut h, id) = radius_app();
    let before = depth(&h);
    let from = at_on(&h, RADIUS, 0.0);
    press(&mut h, from);
    drag_radius(&mut h, &[0.1, 0.3, 0.5]);
    assert!(h.state().session.gesture_running(), "the drag is a gesture");
    assert!(stored_radius(&h, &id) > 0.0, "the shape follows the handle while it is dragged");
    let to = at_on(&h, RADIUS, 0.5);
    release(&mut h, to);

    let ids = ids(&h);
    assert_eq!(ids.iter().filter(|i| **i == "gesture.begin").count(), 1, "{ids:?}");
    assert_eq!(ids.last().copied(), Some("gesture.end"), "{ids:?}");
    assert!(refusals(&h).is_empty() && toasts(&h).is_empty(), "{:?} {:?}", refusals(&h), toasts(&h));
    assert!(!h.state().session.gesture_running());
    assert_eq!(depth(&h), before + 1, "a whole drag is one undo step");
    let r = stored_radius(&h, &id);
    assert!((r - 50.0).abs() < 6.0 && r.fract() == 0.0, "a whole number near the middle of the slider: {r}");
    h.state_mut().session.undo_steps(1).unwrap();
    assert_eq!(stored_radius(&h, &id), 0.0, "one undo takes the whole drag back");
}

#[test]
fn esc_cancels_a_corner_drag_and_restores_the_radius() {
    let (mut h, id) = radius_app();
    let before = depth(&h);
    let from = at_on(&h, RADIUS, 0.0);
    let end = at_on(&h, RADIUS, 0.6);
    press(&mut h, from);
    drag_radius(&mut h, &[0.2, 0.6]);
    assert!(stored_radius(&h, &id) > 10.0);
    // Esc cancels the gesture; it also closes the menu, as it closes every popup.
    key(&mut h, Key::Escape);
    assert!(!h.state().session.gesture_running());
    assert_eq!(stored_radius(&h, &id), 0.0);
    release(&mut h, end);
    assert_eq!(depth(&h), before, "a cancelled drag leaves no undo step");
    assert!(refusals(&h).is_empty(), "{:?}", refusals(&h));
}

#[test]
fn the_corner_slider_works_on_a_selection_of_shapes_with_different_radii() {
    let (mut h, a) = radius_app();
    let b = add_rect(&mut h.state_mut().session, 200.0, 0.0, 100.0, 60.0).unwrap();
    h.state_mut()
        .session
        .execute("style.set", json!({ "ids": [a.as_str()], "props": { "corner_radius": 24 } }))
        .unwrap();
    select(&mut h.state_mut().session, &[a.clone(), b.clone()]);
    h.run();
    assert_eq!(radius(&h), None, "two radii are no value");
    assert!(
        h.state().shell.model.style.prop("corner_radius").is_some_and(drafftink_ui::model::StyleProp::is_mixed),
        "the model says mixed"
    );
    h.get_by_label("mixed");
    // A drag sets one radius on both.
    let from = at_on(&h, RADIUS, 0.0);
    press(&mut h, from);
    drag_radius(&mut h, &[0.1, 0.2]);
    let to = at_on(&h, RADIUS, 0.2);
    release(&mut h, to);
    let (ra, rb) = (stored_radius(&h, &a), stored_radius(&h, &b));
    assert_eq!(ra, rb, "both shapes got the same radius");
    assert!(ra > 0.0);
    assert_eq!(radius(&h), Some(ra));
}

#[test]
fn arrow_keys_on_the_corner_slider_are_plain_commands() {
    let (mut h, id) = radius_app();
    let before = depth(&h);
    h.get_by_role_and_label(egui::accesskit::Role::Slider, RADIUS).focus();
    h.run();
    key(&mut h, Key::ArrowRight);
    h.run();
    assert!(!h.state().session.gesture_running());
    assert!(stored_radius(&h, &id) > 0.0, "the key moved the radius");
    assert_eq!(depth(&h), before + 1);
    assert!(refusals(&h).is_empty(), "{:?}", refusals(&h));
}

// ---- the custom colour picker ----

const SQUARE: &str = "Saturation and value";
const HUE: &str = "Hue";
const ALPHA: &str = "Alpha";

/// The element a colour picker test edits.
#[derive(Clone, Copy)]
enum Subject {
    Rectangle,
    Text,
}

/// The harness with the colour popover of `target` open in its custom view over a selected element.
fn picker_app(target: drafftink_ui::ColorTarget, subject: Subject) -> (H, ElementId) {
    let (mut h, rect) = app();
    let id = match subject {
        Subject::Rectangle => rect,
        Subject::Text => {
            let out = h
                .state_mut()
                .session
                .execute("element.create", json!({ "kind": "text", "text": "hello", "at": [0, 0] }))
                .unwrap();
            let id = ElementId::new(out["id"].as_str().unwrap()).unwrap();
            select(&mut h.state_mut().session, std::slice::from_ref(&id));
            id
        }
    };
    h.state_mut().session.execute("settings.set", json!({ "key": "ui.color_picker", "value": "custom" })).unwrap();
    h.state_mut().session.execute("settings.set", json!({ "key": "theme", "value": "light" })).unwrap();
    h.state_mut().shell.state.color_popover = Some(target);
    h.run();
    h.run();
    (h, id)
}

fn widget(h: &H, label: &str) -> egui::Rect {
    h.get_by_role_and_label(egui::accesskit::Role::Slider, label).rect()
}

/// A point of a widget of the picker at fractions of its width and height.
fn on_widget(h: &H, label: &str, x: f32, y: f32) -> Pos2 {
    let r = widget(h, label);
    Pos2::new(r.left() + r.width() * x, r.top() + r.height() * y)
}

fn press_on(h: &mut H, label: &str, x: f32, y: f32) {
    let p = on_widget(h, label, x, y);
    press(h, p);
}

fn release_on(h: &mut H, label: &str, x: f32, y: f32) {
    let p = on_widget(h, label, x, y);
    release(h, p);
}

/// Moves the pointer over points of widgets, one frame each.
fn move_on(h: &mut H, points: &[(&str, f32, f32)]) {
    for (label, x, y) in points {
        let p = on_widget(h, label, *x, *y);
        h.event(Event::PointerMoved(p));
        h.step();
    }
}

fn move_to(h: &mut H, points: &[Pos2]) {
    for p in points {
        h.event(Event::PointerMoved(*p));
        h.step();
    }
}

fn element_style(h: &H, id: &ElementId) -> drafftink_scene::Style {
    <drafftink_scene::MemoryStore as SceneRead>::get(h.state().session.store(), id).unwrap().style.clone()
}

fn count(h: &H, id: &str) -> usize {
    ids(h).iter().filter(|i| **i == id).count()
}

#[test]
fn dragging_the_square_of_the_stroke_picker_is_one_gesture_one_undo_step_and_previews_live() {
    let (mut h, id) = picker_app(drafftink_ui::ColorTarget::Stroke, Subject::Rectangle);
    let before = depth(&h);
    let old = element_style(&h, &id).stroke_color;
    let from = on_widget(&h, SQUARE, 0.2, 0.2);
    press(&mut h, from);
    move_on(&mut h, &[(SQUARE, 0.5, 0.4), (SQUARE, 0.8, 0.5)]);
    assert!(h.state().session.gesture_running(), "the drag is a gesture");
    let live = element_style(&h, &id).stroke_color;
    assert_ne!(live, old, "the canvas shows the colour while it is dragged");
    let to = on_widget(&h, SQUARE, 0.9, 0.3);
    move_to(&mut h, &[to]);
    release(&mut h, to);

    assert_eq!(count(&h, "gesture.begin"), 1, "{:?}", ids(&h));
    assert_eq!(count(&h, "gesture.end"), 1, "{:?}", ids(&h));
    assert!(count(&h, "gesture.update") >= 2, "{:?}", ids(&h));
    assert!(refusals(&h).is_empty() && toasts(&h).is_empty(), "{:?} {:?}", refusals(&h), toasts(&h));
    assert!(!h.state().session.gesture_running());
    assert_eq!(depth(&h), before + 1, "a whole drag is one undo step");
    assert_chrome_enabled(&h);
    h.state_mut().session.undo_steps(1).unwrap();
    assert_eq!(element_style(&h, &id).stroke_color, old, "one undo takes the whole drag back");
}

#[test]
fn a_drag_that_starts_on_the_square_and_goes_over_the_bars_stays_one_gesture() {
    let (mut h, id) = picker_app(drafftink_ui::ColorTarget::Stroke, Subject::Rectangle);
    let before = depth(&h);
    press_on(&mut h, SQUARE, 0.3, 0.3);
    move_on(&mut h, &[(SQUARE, 0.6, 0.6), (HUE, 0.5, 0.5), (ALPHA, 0.5, 0.5)]);
    release_on(&mut h, ALPHA, 0.5, 0.5);
    assert_eq!(count(&h, "gesture.begin"), 1, "{:?}", ids(&h));
    assert_eq!(count(&h, "gesture.end"), 1, "{:?}", ids(&h));
    assert_eq!(depth(&h), before + 1);
    assert!(refusals(&h).is_empty(), "{:?}", refusals(&h));
    let _ = id;
}

#[test]
fn dragging_the_hue_bar_changes_the_hue_and_the_alpha_bar_the_alpha_in_one_step_each() {
    let (mut h, id) = picker_app(drafftink_ui::ColorTarget::Stroke, Subject::Rectangle);
    let before = depth(&h);
    // Start from a saturated red so that the hue shows.
    h.state_mut().session.execute("style.set", json!({ "props": { "stroke_color": "#ff0000" } })).unwrap();
    h.run();
    h.run();
    let steps = depth(&h);
    assert_eq!(steps, before + 1);
    press_on(&mut h, HUE, 0.01, 0.5);
    move_on(&mut h, &[(HUE, 0.33, 0.5)]);
    release_on(&mut h, HUE, 0.33, 0.5);
    let c = element_style(&h, &id).stroke_color;
    assert!(c.g > 200 && c.r < 40 && c.b < 40, "the hue moved from red to green: {c:?}");
    assert_eq!(depth(&h), steps + 1, "the hue drag is one step");

    press_on(&mut h, ALPHA, 0.99, 0.5);
    move_on(&mut h, &[(ALPHA, 0.5, 0.5)]);
    release_on(&mut h, ALPHA, 0.5, 0.5);
    let c = element_style(&h, &id).stroke_color;
    assert!((120..=135).contains(&c.a), "half transparent: {c:?}");
    assert_eq!(depth(&h), steps + 2, "the alpha drag is one step");
}

#[test]
fn the_fill_picker_drags_as_a_gesture_too() {
    let (mut h, id) = picker_app(drafftink_ui::ColorTarget::Fill, Subject::Rectangle);
    let before = depth(&h);
    press_on(&mut h, SQUARE, 0.2, 0.3);
    move_on(&mut h, &[(SQUARE, 0.5, 0.3), (SQUARE, 0.7, 0.5)]);
    release_on(&mut h, SQUARE, 0.7, 0.5);
    assert_eq!(count(&h, "gesture.begin"), 1, "{:?}", ids(&h));
    assert_eq!(depth(&h), before + 1);
    assert!(element_style(&h, &id).fill_color.is_some(), "the fill was set");
    assert!(refusals(&h).is_empty(), "{:?}", refusals(&h));
}

#[test]
fn the_text_colour_is_the_stroke_picker() {
    let (mut h, id) = picker_app(drafftink_ui::ColorTarget::Stroke, Subject::Text);
    let before = depth(&h);
    let old = element_style(&h, &id).stroke_color;
    press_on(&mut h, SQUARE, 0.9, 0.5);
    move_on(&mut h, &[(SQUARE, 0.6, 0.5)]);
    release_on(&mut h, SQUARE, 0.6, 0.5);
    assert_ne!(element_style(&h, &id).stroke_color, old, "the colour of the text changed");
    assert_eq!(depth(&h), before + 1);
    assert_eq!(count(&h, "gesture.begin"), 1, "{:?}", ids(&h));
}

#[test]
fn the_page_colour_has_no_alpha_and_one_command_on_release() {
    let (mut h, _) = picker_app(drafftink_ui::ColorTarget::Background, Subject::Rectangle);
    assert!(h.query_by_role_and_label(egui::accesskit::Role::Slider, ALPHA).is_none(), "a page colour is solid");
    let old = h.state().session.store().meta().background;
    press_on(&mut h, SQUARE, 0.2, 0.6);
    move_on(&mut h, &[(SQUARE, 0.4, 0.5), (SQUARE, 0.6, 0.4)]);
    assert_eq!(count(&h, "doc.set_background"), 0, "nothing is applied while the drag goes on: {:?}", ids(&h));
    release_on(&mut h, SQUARE, 0.6, 0.4);
    assert_eq!(count(&h, "doc.set_background"), 1, "{:?}", ids(&h));
    assert!(!ids(&h).iter().any(|i| i.starts_with("gesture.")), "{:?}", ids(&h));
    assert!(refusals(&h).is_empty(), "{:?}", refusals(&h));
    assert_ne!(h.state().session.store().meta().background, old, "the page has the new colour");
    h.state_mut().session.undo_steps(1).unwrap();
    assert_eq!(h.state().session.store().meta().background, old, "one undo takes the whole drag back");
}

#[test]
fn esc_cancels_a_colour_drag_and_restores_the_colour() {
    let (mut h, id) = picker_app(drafftink_ui::ColorTarget::Stroke, Subject::Rectangle);
    let before = depth(&h);
    let old = element_style(&h, &id).stroke_color;
    press_on(&mut h, SQUARE, 0.2, 0.2);
    move_on(&mut h, &[(SQUARE, 0.7, 0.6)]);
    assert_ne!(element_style(&h, &id).stroke_color, old);
    let end = on_widget(&h, SQUARE, 0.7, 0.6);
    key(&mut h, Key::Escape);
    assert!(!h.state().session.gesture_running());
    assert_eq!(element_style(&h, &id).stroke_color, old, "restored");
    release(&mut h, end);
    assert_eq!(depth(&h), before, "a cancelled drag leaves no undo step");
    assert!(refusals(&h).is_empty(), "{:?}", refusals(&h));
}

#[test]
fn arrow_keys_on_the_bars_are_plain_commands() {
    let (mut h, id) = picker_app(drafftink_ui::ColorTarget::Stroke, Subject::Rectangle);
    h.state_mut().session.execute("style.set", json!({ "props": { "stroke_color": "#ff0000" } })).unwrap();
    h.run();
    h.run();
    let before = depth(&h);
    h.get_by_role_and_label(egui::accesskit::Role::Slider, HUE).focus();
    h.run();
    key(&mut h, Key::ArrowRight);
    h.run();
    assert!(!ids(&h).iter().any(|i| i.starts_with("gesture.")), "{:?}", ids(&h));
    assert_eq!(count(&h, "style.set"), 1, "one command for the key: {:?}", ids(&h));
    assert!(depth(&h) >= before);
    let c = element_style(&h, &id).stroke_color;
    assert!(c.g > 0 && c.r == 255, "one degree of hue off red: {c:?}");
}

#[test]
fn the_hex_field_follows_the_picker_and_a_colour_set_from_outside_moves_the_picker() {
    let (mut h, _) = picker_app(drafftink_ui::ColorTarget::Stroke, Subject::Rectangle);
    let hex =
        |h: &H| h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Hex colour").value().unwrap_or_default();
    h.state_mut().session.execute("style.set", json!({ "props": { "stroke_color": "#3b82f6" } })).unwrap();
    h.run();
    h.run();
    assert_eq!(hex(&h), "#3b82f6", "the field shows the colour of the selection");
    press_on(&mut h, HUE, 0.9, 0.5);
    move_on(&mut h, &[(HUE, 0.1, 0.5)]);
    release_on(&mut h, HUE, 0.1, 0.5);
    h.run();
    let after = h.state().shell.model.style.str_value("stroke_color").unwrap().to_owned();
    assert_eq!(hex(&h), after, "the field follows what was picked");
    assert_ne!(after, "#3b82f6");
    // A swatch-less change from outside (an undo) moves the picker and the field with it.
    h.state_mut().session.undo_steps(1).unwrap();
    h.run();
    h.run();
    assert_eq!(hex(&h), "#3b82f6");
}

#[test]
fn in_the_dark_theme_the_picker_stores_the_inverse_of_what_it_shows() {
    use drafftink_scene::Color;
    use drafftink_ui::hsv::Hsv;
    let (mut h, id) = picker_app(drafftink_ui::ColorTarget::Stroke, Subject::Rectangle);
    h.state_mut().session.execute("settings.set", json!({ "key": "theme", "value": "dark" })).unwrap();
    h.run();
    h.run();
    // The square shows displayed colours: the new stroke is stored as the colour whose display the point is.
    let hex_now = h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Hex colour").value().unwrap_or_default();
    let hue = Hsv::from_color(Color::parse(&hex_now).unwrap()).hue();
    press_on(&mut h, SQUARE, 0.3, 0.1);
    move_on(&mut h, &[(SQUARE, 0.35, 0.12)]);
    release_on(&mut h, SQUARE, 0.35, 0.12);
    h.run();
    let stored = element_style(&h, &id).stroke_color;
    let rect = widget(&h, SQUARE);
    let seen = Hsv::new(hue, 0.35, 1.0 - 0.12).to_color(255);
    assert_eq!(rect.width().round(), 260.0, "{rect:?}");
    assert_eq!(stored, seen.from_dark_theme(), "the document holds the inverse of the filter");
    assert_ne!(stored, seen, "and not the colour as it was seen");
    // The hex field shows what the canvas draws for it.
    let hex = h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Hex colour").value().unwrap_or_default();
    assert_eq!(Color::parse(&hex).unwrap(), stored.for_dark_theme());
    let distance = |a: Color, b: Color| {
        [(a.r, b.r), (a.g, b.g), (a.b, b.b)].iter().map(|(x, y)| i32::from(*x).abs_diff(i32::from(*y))).max().unwrap()
    };
    assert!(distance(stored.for_dark_theme(), seen) <= 3, "the canvas draws what the square showed");
}

#[test]
fn the_picker_has_finger_sized_targets_on_the_touch_layout() {
    let (mut h, _) = picker_app(drafftink_ui::ColorTarget::Stroke, Subject::Rectangle);
    let desktop = (widget(&h, HUE).height(), widget(&h, SQUARE).height());
    h.state_mut().shell.state.layout = drafftink_ui::state::LayoutPreference::Touch;
    h.run();
    h.run();
    let touch = (widget(&h, HUE).height(), widget(&h, SQUARE).height());
    assert!(touch.0 > desktop.0 && touch.1 > desktop.1, "{desktop:?} against {touch:?}");
    assert!(touch.0 >= 24.0, "a bar is at least as tall as a touch toggle: {}", touch.0);
}
