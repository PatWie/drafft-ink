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
fn the_tools_start_collapsed_on_a_narrow_window_and_the_ribbon_scrolls_in_one_row() {
    let mut h = at(vec2(600.0, 900.0), with_selection());
    assert!(h.query_by_label("Rectangle tool").is_none(), "tools are collapsed");
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(600.0, 900.0));
    let ribbon = h.state().state.chrome.ribbon.expect("a ribbon for the selection");
    assert!(screen.contains_rect(ribbon), "the ribbon {ribbon:?} lies inside the window");
    assert!(ribbon.height() < 40.0 + 2.0 * 12.0, "one row of 40 point targets: {ribbon:?}");
    assert!(ribbon.contains_rect(h.get_by_label("Dashed").rect()), "the first group is in view");
    let delete = h.get_by_label("Delete").rect();
    assert!(!ribbon.contains_rect(delete), "the last group is scrolled out: {delete:?} vs {ribbon:?}");
    h.event(Event::PointerMoved(ribbon.center()));
    h.event(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(-3000.0, 0.0),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
    });
    for _ in 0..10 {
        h.step();
    }
    let delete = h.get_by_label("Delete").rect();
    assert!(ribbon.contains_rect(delete), "the sideways scroll brings it into view: {delete:?} vs {ribbon:?}");
}

#[test]
fn the_ribbon_has_finger_sized_targets_on_a_touch_device() {
    let mut f = dialog::fixture();
    f.state.set_touch_first(true);
    let mut h = at(vec2(1180.0, 820.0), f);
    h.state_mut().session.execute("tool.set", serde_json::json!({ "tool": "rectangle" })).unwrap();
    h.state_mut().refresh();
    h.run();
    assert!(h.query_by_label("Bring to front").is_none(), "nothing is selected, so nothing to layer");
    for label in ["Dashed", "Corners: Round", "More options"] {
        assert!(height_of(&h, label) >= 36.0, "{label}");
    }
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
    // One strip across the top (menu, rooms, title, and one style button), the properties as a popover under it, the
    // tab bar above the bottom bar.
    let mut h = at(vec2(390.0, 800.0), dialog::fixture());
    let strip = h.state().state.chrome.strip.unwrap();
    let menu = h.get_by_label("Open or close the main menu").rect();
    let style = h.get_by_label("Stroke, fill and width").rect();
    assert!(strip.contains_rect(menu) && strip.contains_rect(style), "{menu:?} and {style:?} in {strip:?}");
    assert!(menu.max.x < style.min.x, "the style button is at the right end: {style:?} vs {menu:?}");
    assert!(h.query_by_label("Stroke blue").is_none(), "the swatches wait in the popover");
    h.get_by_label("Stroke, fill and width").click();
    h.run();
    let stroke = h.get_all_by_label("Stroke blue").next().unwrap().rect();
    assert!(stroke.min.y >= strip.max.y, "the swatches sit under the strip: {stroke:?} vs {strip:?}");
    let tab = h.get_all_by_label("Untitled").last().unwrap().rect();
    let undo = h.get_by_label("Undo").rect();
    assert!(tab.max.y <= undo.min.y, "the tab bar sits above the bottom bar: {tab:?} vs {undo:?}");
}

/// Windows of an iPad Pro 11" in portrait and landscape and of an iPhone 14, in points.
const TABLET_AND_PHONE: [(f32, f32); 3] = [(820.0, 1180.0), (1180.0, 820.0), (390.0, 844.0)];

/// The device is touch-first from the start (`SetupCx::touch_first`, from `(pointer: coarse)` on the web).
fn on_a_touch_device(size: Vec2) -> H {
    let mut f = dialog::fixture();
    f.state.set_touch_first(true);
    at(size, f)
}

#[test]
fn the_bottom_bar_lies_inside_the_screen_on_an_ipad_and_an_iphone() {
    for (w, h) in TABLET_AND_PHONE {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(w, h));
        let harness = on_a_touch_device(vec2(w, h));
        let bar = harness.state().state.chrome.bottom_bar.unwrap_or_else(|| panic!("{w}x{h}: no bottom bar"));
        assert!(screen.contains_rect(bar), "{w}x{h}: the bottom bar {bar:?} is outside the screen {screen:?}");
        assert!(bar.width() > 150.0 && bar.height() >= 40.0, "{w}x{h}: a bar you can use with a finger: {bar:?}");
        // Its buttons are on the screen too, the first and the last that fit.
        for label in ["Undo", "Redo"] {
            let r = harness.get_by_label(label).rect();
            assert!(screen.contains_rect(r) && bar.contains_rect(r), "{w}x{h}: {label} at {r:?}");
        }
    }
}

#[test]
fn the_bottom_bar_stays_clear_of_the_tab_bar_and_the_tools() {
    for (w, h) in TABLET_AND_PHONE {
        let harness = on_a_touch_device(vec2(w, h));
        let chrome = harness.state().state.chrome;
        let bar = chrome.bottom_bar.expect("a bottom bar");
        for (name, other) in [("tabs", chrome.tabs), ("tools", chrome.tools)] {
            if let Some(other) = other {
                assert!(!bar.intersects(other), "{w}x{h}: the bottom bar {bar:?} overlaps the {name} {other:?}");
            }
        }
    }
}

#[test]
fn the_chrome_keeps_out_of_the_safe_area_insets_a_host_reports() {
    // A home indicator at the bottom, a notch at the top, rounded corners at the sides (an iPhone in landscape).
    let insets = egui::SafeAreaInsets(egui::epaint::MarginF32 { left: 47.0, right: 47.0, top: 24.0, bottom: 21.0 });
    for (w, h) in TABLET_AND_PHONE {
        let mut harness = on_a_touch_device(vec2(w, h));
        harness.input_mut().safe_area_insets = Some(insets);
        harness.run();
        let safe = egui::Rect::from_min_max(egui::pos2(47.0, 24.0), egui::pos2(w - 47.0, h - 21.0));
        let chrome = harness.state().state.chrome;
        let bar = chrome.bottom_bar.expect("a bottom bar");
        let strip = chrome.strip.expect("a top strip");
        assert!(
            bar.bottom() <= safe.bottom() + 0.5 && bar.left() >= safe.left() - 0.5,
            "{w}x{h}: bar {bar:?} in {safe:?}"
        );
        assert!(strip.top() >= safe.top() - 0.5, "{w}x{h}: strip {strip:?} in {safe:?}");
    }
}

#[test]
fn a_touch_device_starts_in_the_touch_layout_before_any_touch() {
    let h = on_a_touch_device(vec2(1180.0, 820.0));
    assert!(height_of(&h, "Rectangle tool") >= 44.0 && height_of(&h, "Undo") >= 40.0);
    let mouse = at(vec2(1180.0, 820.0), dialog::fixture());
    assert_eq!(height_of(&mouse, "Undo"), 28.0, "a desktop window keeps the desktop sizes");
}
