//! The debug panel: it draws what the app hands it, floats at the top right under the strip, closes with its own
//! button, costs nothing while closed, and is in the menu.

mod common;
mod dialog;
mod support_a;

use common::Fixture;
use drafftink_profile::FrameReading;
use drafftink_ui::{DebugRow, DebugSection, DebugView, Verb};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use support_a::menu;

fn view(frames: FrameReading) -> DebugView {
    DebugView {
        frames: Some(frames),
        sections: vec![
            DebugSection {
                title: "Renderer",
                rows: vec![
                    DebugRow::new("Adapter", "NVIDIA L40S"),
                    DebugRow::new("Draws on", "CPU (software driver)").flagged(),
                ],
            },
            DebugSection { title: "Scene", rows: vec![DebugRow::new("Elements", "12,345")] },
        ],
    }
}

fn busy() -> FrameReading {
    FrameReading {
        frames: 58,
        avg_ms: 2.1,
        worst_ms: 6.3,
        history_ms: vec![2.0, 3.0, 2.5, 6.3],
        ..FrameReading::default()
    }
}

fn open(debug: Option<DebugView>) -> Harness<'static, Fixture> {
    let mut f = dialog::fixture();
    f.state.debug_open = true;
    f.model.debug = debug;
    let mut h = dialog::harness(f);
    h.run();
    h
}

#[test]
fn it_shows_the_rate_the_frame_time_and_every_row_it_is_given() {
    let h = open(Some(view(busy())));
    for text in [
        "Debug",
        "Frames",
        "Rate",
        "58 fps",
        "Frame time",
        "avg 2.10 ms, worst 6.30 ms",
        "Renderer",
        "Adapter",
        "NVIDIA L40S",
        "Draws on",
        "CPU (software driver)",
        "Scene",
        "Elements",
        "12,345",
    ] {
        h.get_by_label(text);
    }
    h.get_by_label("Frame time history");
}

#[test]
fn a_quiet_canvas_reads_idle_and_not_zero() {
    let quiet = FrameReading {
        idle: true,
        last_ms: Some(3.2),
        since_last_ms: Some(4_200.0),
        history_ms: vec![3.0, 3.2],
        ..FrameReading::default()
    };
    let h = open(Some(view(quiet)));
    h.get_by_label("idle (draws on demand)");
    h.get_by_label("3.20 ms, 4.2 s ago");
    assert!(h.query_by_label("0 fps").is_none());
}

#[test]
fn before_the_first_measurement_it_says_so() {
    let h = open(None);
    h.get_by_label("Measuring…");
}

#[test]
fn it_floats_at_the_top_right_under_the_strip() {
    let h = open(Some(view(busy())));
    let r = h.get_by_label("Debug").rect();
    assert!(r.top() > 72.0, "under the two-row strip: {r:?}");
    let rate = h.get_by_label("Rate").rect();
    assert!(rate.left() > 1280.0 - 300.0, "at the right edge: {rate:?}");
}

#[test]
fn it_is_closed_by_default_and_draws_nothing() {
    let mut f = dialog::fixture();
    f.model.debug = Some(view(busy()));
    let mut h = dialog::harness(f);
    h.run();
    assert!(h.query_by_label("Rate").is_none());
    assert!(h.query_by_label("Debug").is_none());
}

#[test]
fn its_close_button_runs_the_toggle_verb() {
    let mut h = open(Some(view(busy())));
    h.get_by_label("Show or hide the debug panel").click();
    h.run();
    assert_eq!(dialog::verbs(h.state()), vec![Verb::ToggleDebug]);
}

#[test]
fn the_verb_toggles_the_state_and_the_key_is_alt_d() {
    let mut f = dialog::fixture();
    assert!(!f.state.debug_open);
    f.state.apply_verb(Verb::ToggleDebug, &mut Vec::new());
    assert!(f.state.debug_open);
    f.state.apply_verb(Verb::ToggleDebug, &mut Vec::new());
    assert!(!f.state.debug_open);
    assert_eq!(Verb::ToggleDebug.id(), "ui.toggle_debug");
    assert_eq!(drafftink_ui::keymap::verb_key_label(Verb::ToggleDebug, false).as_deref(), Some("Alt+D"));
}

#[test]
fn the_main_menu_has_a_checked_row_under_view() {
    let mut f = Fixture::new();
    f.state.menu_open = true;
    f.state.debug_open = true;
    let mut h = support_a::themed_lenient(f, false);
    h.run();
    let rows = menu::rows_in(&h, "Main menu");
    let at =
        |text: &str| rows.iter().position(|r| r.starts_with(text)).unwrap_or_else(|| panic!("no row {text}: {rows:?}"));
    assert!(at("Debug panel") > at("Library"), "{rows:?}");
}
