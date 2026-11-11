#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The top strip and the geometry that hangs off it: one bar across the window with the menu, rooms and title at
//! the left, the style controls centred, an empty right slot; side panels below it; popovers, toasts, the stats
//! panel and the tab bar placed from the rectangles of the surfaces (`UiState::chrome`), not from numbers.

mod common;
mod dialog;

use common::Fixture;
use drafftink_engine::testing::{add_rect, select};
use drafftink_ui::chrome::ChromeRects;
use drafftink_ui::state::LayoutPreference;
use drafftink_ui::{CollabStatus, Toast};
use egui::{Rect, Vec2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

type H = Harness<'static, Fixture>;

const MENU: &str = "Open or close the main menu";
const STYLE: &str = "Stroke, fill and width";

fn at(size: Vec2, f: Fixture) -> H {
    let mut h = dialog::sized(f, size, egui::Theme::Light);
    h.run();
    h
}

fn rect(h: &H, label: &str) -> Rect {
    h.get_all_by_label(label).next().unwrap_or_else(|| panic!("no {label}")).rect()
}

fn chrome(h: &H) -> ChromeRects {
    h.state().state.chrome
}

fn with_selection() -> Fixture {
    let mut f = dialog::fixture();
    let id = add_rect(&mut f.session, 0.0, 0.0, 50.0, 50.0).unwrap();
    select(&mut f.session, std::slice::from_ref(&id));
    f.refresh();
    f
}

#[test]
fn the_strip_spans_the_window_and_holds_menu_rooms_title_and_style_controls() {
    let h = at(vec2(1280.0, 800.0), dialog::fixture());
    let strip = chrome(&h).strip.unwrap();
    assert_eq!(strip, Rect::from_min_size(egui::Pos2::ZERO, vec2(1280.0, 40.0)), "flush with the top edge, full width");
    let menu = rect(&h, MENU);
    let rooms = rect(&h, "Collaboration");
    let title = rect(&h, "Document name: Untitled");
    let stroke = rect(&h, "Stroke blue");
    let fill = rect(&h, "Fill blue");
    let thin = rect(&h, "Thin");
    for (name, r) in
        [("menu", menu), ("rooms", rooms), ("title", title), ("stroke", stroke), ("fill", fill), ("width", thin)]
    {
        assert!(strip.contains_rect(r), "{name} {r:?} is in the strip {strip:?}");
    }
    assert!(menu.max.x < rooms.min.x && rooms.max.x < title.min.x, "left group order");
    assert!(title.max.x < stroke.min.x && stroke.max.x < fill.min.x && fill.max.x < thin.min.x, "then the style group");
    assert_eq!(menu.min.x, 12.0, "the margin of the window");
}

#[test]
fn every_button_of_the_strip_has_the_same_height() {
    for (size, layout) in
        [(vec2(1280.0, 800.0), LayoutPreference::Desktop), (vec2(1280.0, 800.0), LayoutPreference::Touch)]
    {
        let mut f = dialog::fixture();
        f.state.layout = layout;
        let h = at(size, f);
        let heights: Vec<(&str, f32)> =
            [MENU, "Collaboration", "Document name: Untitled", "Thin", "Extra bold", "Pick the stroke colour"]
                .into_iter()
                .map(|label| (label, rect(&h, label).height()))
                .collect();
        let first = heights[0].1;
        assert!(heights.iter().all(|(_, height)| (*height - first).abs() < 0.01), "{layout:?}: {heights:?}");
    }
}

#[test]
fn the_style_group_is_centred_on_the_window_and_the_right_slot_is_empty() {
    for width in [1280.0, 1100.0] {
        let h = at(vec2(width, 800.0), dialog::fixture());
        let strip = chrome(&h).strip.unwrap();
        let first = rect(&h, "Stroke");
        let last = rect(&h, "Extra bold");
        let middle = (first.min.x + last.max.x) / 2.0;
        assert!((middle - width / 2.0).abs() < 1.5, "centre {middle} vs {} at {width}", width / 2.0);
        // Nothing in the strip lies right of the style group.
        let past = h
            .query_all_by_role(egui::accesskit::Role::Button)
            .filter(|n| n.rect().max.y <= strip.max.y && n.rect().min.x > last.max.x + 1.0)
            .count();
        assert_eq!(past, 0, "the right slot holds nothing yet");
    }
}

#[test]
fn the_menu_opens_under_the_menu_button() {
    let mut f = dialog::fixture();
    f.state.menu_open = true;
    let h = at(vec2(1280.0, 800.0), f);
    let strip = chrome(&h).strip.unwrap();
    let menu = rect(&h, MENU);
    let row = h.get_all_by_label("New tab").map(|n| n.rect()).find(|r| r.min.x < 100.0).unwrap();
    assert!(row.min.y >= strip.max.y, "{row:?} under {strip:?}");
    assert!(row.min.x >= menu.min.x && row.min.x < menu.min.x + 20.0, "left-aligned with the button: {row:?}");
    // The numbers the Playwright specs click (`e2e/web/specs/input/pointer.spec.ts`, `shell/collab.spec.ts`).
    assert_eq!((menu.center().x, menu.center().y), (26.0, 20.0));
    assert_eq!((rect(&h, "Collaboration").center().x, rect(&h, "Collaboration").center().y), (58.0, 20.0));
    // The first row sits under the "File" heading. "Export as PNG" is the first row under the "Export" heading.
    assert!((row.center().y - 92.0).abs() < 1.0 && row.min.x < 50.0 && row.max.x > 50.0, "{row:?}");
    let png = h.get_all_by_label("Export as PNG").map(|n| n.rect()).find(|r| r.min.x < 100.0).unwrap();
    assert!((png.center().y - 322.0).abs() < 1.0 && png.min.x < 50.0 && png.max.x > 50.0, "{png:?}");
}

#[test]
fn the_side_panels_start_under_the_strip_even_in_a_short_window() {
    for height in [800.0, 420.0, 300.0] {
        let h = at(vec2(1280.0, height), with_selection());
        let strip = chrome(&h).strip.unwrap();
        let tools = chrome(&h).tools.unwrap();
        let ribbon = chrome(&h).ribbon.unwrap();
        assert!(tools.min.y >= strip.max.y, "tools {tools:?} under {strip:?} at {height}");
        assert!(ribbon.min.y >= strip.max.y, "ribbon {ribbon:?} under {strip:?} at {height}");
        assert!(tools.max.y <= height && ribbon.max.y <= height, "inside the window");
    }
}

#[test]
fn a_tall_window_keeps_the_tool_column_whole_and_a_short_one_scrolls_it() {
    let tall = at(vec2(1280.0, 800.0), dialog::fixture());
    let first = rect(&tall, "Select tool");
    let last = rect(&tall, "Keep tool active");
    let tools = chrome(&tall).tools.unwrap();
    assert!(tools.contains_rect(first) && tools.contains_rect(last), "all tools fit at 800 high");
    let short = at(vec2(1280.0, 400.0), dialog::fixture());
    let tools = chrome(&short).tools.unwrap();
    let last = rect(&short, "Keep tool active");
    assert!(!tools.contains_rect(last), "the last button is scrolled out of the short column: {last:?} vs {tools:?}");
}

#[test]
fn the_colour_wheels_open_their_popover_under_the_strip_whatever_the_window() {
    let mut f = dialog::fixture();
    f.state.color_popover = Some(drafftink_ui::ColorTarget::Fill);
    let h = at(vec2(1280.0, 800.0), f);
    let strip = chrome(&h).strip.unwrap();
    let title = rect(&h, "Fill colour");
    let wheel = rect(&h, "Pick the fill colour");
    assert!(title.min.y >= wheel.max.y && title.min.y >= strip.max.y, "{title:?} under the wheel {wheel:?}");
}

// ---- the narrow layout ----

#[test]
fn a_narrow_strip_collapses_the_style_controls_into_one_button_that_opens_a_popover() {
    for width in [390.0, 420.0] {
        let mut h = at(vec2(width, 800.0), dialog::fixture());
        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(width, 800.0));
        let strip = chrome(&h).strip.unwrap();
        // In the window, in the strip, side by side.
        let mut row: Vec<Rect> =
            [MENU, "Collaboration", "Document name: Untitled", STYLE].iter().map(|l| rect(&h, l)).collect();
        for r in &row {
            assert!(screen.contains_rect(*r) && strip.contains_rect(*r), "{r:?} in {strip:?} at {width}");
        }
        row.sort_by(|a, b| a.min.x.total_cmp(&b.min.x));
        assert!(row.windows(2).all(|pair| pair[0].max.x <= pair[1].min.x), "no overlap: {row:?}");
        assert!(h.query_by_label("Stroke blue").is_none());

        h.get_by_label(STYLE).click();
        h.run();
        assert!(h.state().state.style_open);
        let popover = ["Stroke blue", "Fill blue", "Extra bold", "Pick the stroke colour", "Pick the fill colour"];
        for label in popover {
            let r = rect(&h, label);
            assert!(screen.contains_rect(r) && r.min.y >= strip.max.y, "{label} {r:?} at {width}");
        }
        // A colour pick keeps the popover; a press on the canvas closes it.
        let calls = common::click(&mut h, "Stroke blue");
        assert_eq!(calls.len(), 1);
        assert!(h.state().state.style_open);
        dialog::click_at(&mut h, egui::pos2(width / 2.0, 500.0));
        assert!(!h.state().state.style_open, "closed by a press on the canvas");
    }
}

#[test]
fn the_colour_popover_opened_from_the_style_popover_does_not_close_it() {
    let mut h = at(vec2(390.0, 800.0), dialog::fixture());
    h.get_by_label(STYLE).click();
    h.run();
    h.get_by_label("Pick the stroke colour").click();
    h.run();
    h.state_mut().dispatch();
    h.run();
    assert!(h.state().state.color_popover.is_some());
    let anchor = rect(&h, "Pick the stroke colour");
    let colours = rect(&h, "Stroke colour");
    assert!(colours.min.y >= anchor.max.y, "the colour popover opens under the wheel in the style popover");
    assert!(h.state().state.style_open, "and the style popover stays");
}

#[test]
fn a_wide_window_that_gets_narrower_collapses_and_a_wider_one_expands_again() {
    // 800 points is not a touch layout, yet the three groups do not fit side by side.
    let h = at(vec2(800.0, 600.0), dialog::fixture());
    assert!(h.query_by_label(STYLE).is_some() && h.query_by_label("Stroke blue").is_none());
    let h = at(vec2(1280.0, 600.0), dialog::fixture());
    assert!(h.query_by_label(STYLE).is_none() && h.query_by_label("Stroke blue").is_some());
}

// ---- surfaces placed from the rectangles ----

#[test]
fn the_tab_bar_sits_above_the_bottom_bar_when_it_would_touch_it() {
    for width in [390.0, 800.0] {
        let h = at(vec2(width, 800.0), dialog::fixture());
        let bar = chrome(&h).bottom_bar.unwrap();
        let tabs = chrome(&h).tabs.unwrap();
        assert!(tabs.max.y <= bar.min.y, "{tabs:?} above {bar:?} at {width}");
    }
    let h = at(vec2(1600.0, 800.0), dialog::fixture());
    let bar = chrome(&h).bottom_bar.unwrap();
    let tabs = chrome(&h).tabs.unwrap();
    assert!(tabs.min.x > bar.max.x && tabs.max.y > bar.min.y, "beside the bar when there is room: {tabs:?} / {bar:?}");
}

#[test]
fn toasts_and_the_stats_panel_sit_above_the_bottom_chrome() {
    let mut f = dialog::fixture();
    f.state.stats_open = true;
    f.state.push_toast(Toast {
        text: "Nothing to undo.".to_owned(),
        code: "nothing_to_do".to_owned(),
        born: 0.0,
        suggestion: None,
        command: String::new(),
        args: String::new(),
    });
    for width in [1280.0, 390.0] {
        let mut f2 = dialog::fixture();
        f2.state.stats_open = true;
        f2.state.push_toast(Toast {
            text: "Nothing to undo.".to_owned(),
            code: "nothing_to_do".to_owned(),
            born: 0.0,
            suggestion: None,
            command: String::new(),
            args: String::new(),
        });
        let h = at(vec2(width, 800.0), f2);
        let c = chrome(&h);
        let floor = c.bottom_bar.unwrap().min.y.min(c.tabs.unwrap().min.y);
        let toast = rect(&h, "Nothing to undo.");
        assert!(toast.max.y <= floor, "toast {toast:?} above {floor} at {width}");
        let stats = rect(&h, "Stats");
        assert!(stats.max.y <= c.bottom_bar.unwrap().min.y, "stats {stats:?} above the bar at {width}");
    }
    drop(f);
}

#[test]
fn zen_mode_draws_no_strip() {
    let mut f = dialog::fixture();
    f.state.zen = true;
    let h = at(vec2(1280.0, 800.0), f);
    assert!(chrome(&h).strip.is_none());
    assert!(h.query_by_label(MENU).is_none());
}

#[test]
fn the_collaboration_dot_reports_the_status_in_the_strip() {
    let mut f = dialog::fixture();
    f.model.collab.status = CollabStatus::Connected;
    let h = at(vec2(1280.0, 800.0), f);
    let strip = chrome(&h).strip.unwrap();
    assert!(strip.contains_rect(rect(&h, "Collaboration")));
}
