#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The touch layout: larger targets and collapsible panels, automatic on a narrow window or after touch input,
//! and a choice in the preferences.

mod common;
mod dialog;

use common::Fixture;
use drafftink_engine::testing::{add_rect, select};
use drafftink_ui::state::{LayoutPreference, PanelKind};
use egui::{Event, TouchDeviceId, TouchId, TouchPhase, Vec2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

type H = Harness<'static, Fixture>;

fn at(size: Vec2, f: Fixture) -> H {
    let mut h = dialog::sized(f, size, egui::Theme::Light);
    h.run();
    h
}

fn height_of(h: &H, label: &str) -> f32 {
    h.get_by_label(label).rect().height()
}

fn with_selection() -> Fixture {
    let mut f = dialog::fixture();
    let id = add_rect(&mut f.session, 0.0, 0.0, 50.0, 50.0).unwrap();
    select(&mut f.session, std::slice::from_ref(&id));
    f.refresh();
    f
}

#[test]
fn a_wide_mouse_window_keeps_the_desktop_sizes() {
    let h = at(vec2(1280.0, 800.0), dialog::fixture());
    assert_eq!(height_of(&h, "Rectangle tool"), 34.0);
    assert_eq!(height_of(&h, "Undo"), 28.0);
}

#[test]
fn a_narrow_window_gets_larger_targets_by_itself() {
    let h = at(vec2(600.0, 900.0), dialog::fixture());
    // The tools start collapsed on a narrow window, so open them first.
    let mut h = h;
    h.get_by_label("Show the tools").click();
    h.run();
    assert!(height_of(&h, "Rectangle tool") >= 44.0, "{}", height_of(&h, "Rectangle tool"));
    assert!(height_of(&h, "Undo") >= 40.0);
    assert!(height_of(&h, "Sticky note") >= 44.0);
}

#[test]
fn the_preference_decides_over_the_window() {
    let mut f = dialog::fixture();
    f.state.layout = LayoutPreference::Touch;
    let h = at(vec2(1280.0, 800.0), f);
    assert!(height_of(&h, "Rectangle tool") >= 44.0, "touch at a wide window");

    let mut f = dialog::fixture();
    f.state.layout = LayoutPreference::Desktop;
    let h = at(vec2(600.0, 900.0), f);
    assert_eq!(height_of(&h, "Rectangle tool"), 34.0, "desktop at a narrow window");
    assert!(h.query_by_label("Show the tools").is_none(), "no collapsing in the desktop layout");
}

#[test]
fn a_touch_event_switches_to_the_touch_layout() {
    let mut h = at(vec2(1280.0, 800.0), dialog::fixture());
    assert!(!h.state().state.touch_first);
    h.event(Event::Touch {
        device_id: TouchDeviceId(0),
        id: TouchId(1),
        phase: TouchPhase::Start,
        pos: egui::pos2(600.0, 400.0),
        force: None,
    });
    h.run();
    assert!(h.state().state.touch_first);
    assert!(height_of(&h, "Rectangle tool") >= 44.0);
    // A wide touch screen keeps its panels open.
    assert!(!h.state().state.panel_collapsed(PanelKind::Tools, 1280.0));
}

#[test]
fn the_app_can_say_the_device_is_touch_first() {
    let mut f = dialog::fixture();
    f.state.set_touch_first(true);
    let h = at(vec2(1280.0, 800.0), f);
    assert!(height_of(&h, "Rectangle tool") >= 44.0);
}

#[test]
fn panels_start_collapsed_on_a_narrow_window_and_open_with_their_button() {
    let mut h = at(vec2(600.0, 900.0), with_selection());
    assert!(h.query_by_label("Rectangle tool").is_none(), "tools are collapsed");
    assert!(
        h.query_by_label("Align left").is_none() && h.query_by_label("Duplicate").is_none(),
        "so are the properties"
    );
    h.get_by_label("Show the properties").click();
    h.run();
    h.get_by_label("Duplicate");
    assert!(!h.state().state.panel_collapsed(PanelKind::Properties, 600.0));
    h.get_by_label("Hide the properties").click();
    h.run();
    assert!(h.query_by_label("Duplicate").is_none());
}

#[test]
fn the_tools_collapse_and_come_back() {
    let mut f = dialog::fixture();
    f.state.layout = LayoutPreference::Touch;
    let mut h = at(vec2(1280.0, 800.0), f);
    h.get_by_label("Rectangle tool");
    h.get_by_label("Hide the tools").click();
    h.run();
    assert!(h.query_by_label("Rectangle tool").is_none());
    h.get_by_label("Show the tools").click();
    h.run();
    h.get_by_label("Rectangle tool");
}

#[test]
fn menu_rows_and_dialog_buttons_are_larger_in_the_touch_layout() {
    let mut f = dialog::fixture();
    f.state.layout = LayoutPreference::Touch;
    f.state.menu_open = true;
    let h = at(vec2(1280.0, 800.0), f);
    assert!(height_of(&h, "Save") >= 40.0);
    let mut f = dialog::fixture();
    f.state.menu_open = true;
    let h = at(vec2(1280.0, 800.0), f);
    assert_eq!(height_of(&h, "Save"), 28.0);
}

#[test]
fn a_narrow_window_keeps_its_bars_inside_the_window() {
    let mut h = at(vec2(420.0, 800.0), dialog::fixture());
    h.get_by_label("Show the tools").click();
    h.run();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(420.0, 800.0));
    // The tool column scrolls and the bottom bar scrolls sideways, so only their first buttons are checked.
    for label in ["Undo", "Rectangle tool", "Open or close the main menu"] {
        let r = h.get_by_label(label).rect();
        assert!(screen.contains_rect(r), "{label} at {r:?}");
    }
}

#[test]
fn the_panels_do_not_cover_each_other_on_a_phone() {
    // Menu buttons at the top left, the properties bar under them, the tab bar above the bottom bar.
    let h = at(vec2(390.0, 800.0), dialog::fixture());
    let menu = h.get_by_label("Open or close the main menu").rect();
    let stroke = h.get_all_by_label("Stroke blue").next().unwrap().rect();
    assert!(stroke.min.y >= menu.max.y, "the colour bar sits under the menu: {stroke:?} vs {menu:?}");
    let tab = h.get_all_by_label("Untitled").last().unwrap().rect();
    let undo = h.get_by_label("Undo").rect();
    assert!(tab.max.y <= undo.min.y, "the tab bar sits above the bottom bar: {tab:?} vs {undo:?}");
}
