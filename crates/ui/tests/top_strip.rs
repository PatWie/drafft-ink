#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The top strip and the geometry that hangs off it: one bar across the window, two control rows tall, with the
//! menu, rooms and title at the left, then the colour group (stroke over fill), then the groups of the selection;
//! side panels below it; popovers, toasts, the stats panel and the tab bar placed from the rectangles of the
//! surfaces (`UiState::chrome`), not from numbers.

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
const STYLE: &str = "Stroke and fill";

/// The height of the strip: two rows of 28, the gap between them and 6 above and below.
const STRIP_HEIGHT: f32 = 72.0;

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
fn the_strip_spans_the_window_and_holds_menu_rooms_title_and_the_colour_group() {
    let h = at(vec2(1280.0, 800.0), dialog::fixture());
    let strip = chrome(&h).strip.unwrap();
    assert_eq!(
        strip,
        Rect::from_min_size(egui::Pos2::ZERO, vec2(1280.0, STRIP_HEIGHT)),
        "flush with the top edge, full width, two rows"
    );
    let menu = rect(&h, MENU);
    let rooms = rect(&h, "Collaboration");
    let title = rect(&h, "Document name: Untitled");
    let stroke = rect(&h, "Stroke blue");
    let fill = rect(&h, "Fill blue");
    for (name, r) in [("menu", menu), ("rooms", rooms), ("title", title), ("stroke", stroke), ("fill", fill)] {
        assert!(strip.contains_rect(r), "{name} {r:?} is in the strip {strip:?}");
    }
    assert!(menu.max.x < rooms.min.x, "the menu and the rooms share the first row");
    assert!(title.min.y >= menu.max.y && title.min.x <= menu.min.x + 1.0, "the title is under them: {title:?}");
    assert!(rooms.max.x < stroke.min.x && title.max.x < stroke.min.x, "then the colour group");
    assert!(stroke.max.x < fill.min.x || fill.min.y > stroke.max.y, "fill is under stroke");
    assert_eq!(menu.min.x, 12.0, "the margin of the window");
    assert!(h.query_by_label("Thin").is_none(), "no width with nothing to give a width to");
}

#[test]
fn every_button_of_the_strip_has_the_same_height() {
    for (size, layout) in
        [(vec2(1280.0, 800.0), LayoutPreference::Desktop), (vec2(1280.0, 800.0), LayoutPreference::Touch)]
    {
        let mut f = with_selection();
        f.state.layout = layout;
        let h = at(size, f);
        let heights: Vec<(&str, f32)> = [
            MENU,
            "Collaboration",
            "Document name: Untitled",
            "Thin",
            "Extra bold",
            "Dashed",
            "Pick the stroke colour",
            "Pick the fill colour",
        ]
        .into_iter()
        .map(|label| (label, rect(&h, label).height()))
        .collect();
        let first = heights[0].1;
        assert!(heights.iter().all(|(_, height)| (*height - first).abs() < 0.01), "{layout:?}: {heights:?}");
    }
}

#[test]
fn the_colour_group_follows_the_left_cluster_and_nothing_else_is_in_the_strip_when_nothing_is_selected() {
    for width in [1280.0, 1100.0] {
        let h = at(vec2(width, 800.0), dialog::fixture());
        let strip = chrome(&h).strip.unwrap();
        let wheel = rect(&h, "Pick the fill colour").max.x.max(rect(&h, "Pick the stroke colour").max.x);
        assert!(wheel < width / 2.0, "the colour group is at the left, not centred: ends at {wheel}");
        let past = h
            .query_all_by_role(egui::accesskit::Role::Button)
            .filter(|n| n.rect().max.y <= strip.max.y && n.rect().min.x > wheel + 1.0)
            .count();
        assert_eq!(past, 0, "no groups and no More button: nothing right of the colour group");
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
    let shift = STRIP_HEIGHT - 40.0;
    assert!(row.min.y >= strip.max.y, "{row:?} under {strip:?}");
    assert!(row.min.x >= menu.min.x && row.min.x < menu.min.x + 20.0, "left-aligned with the button: {row:?}");
    // The numbers the Playwright specs click (`e2e/web/specs/input/pointer.spec.ts`, `shell/collab.spec.ts`).
    assert_eq!((menu.center().x, menu.center().y), (26.0, 20.0));
    assert_eq!((rect(&h, "Collaboration").center().x, rect(&h, "Collaboration").center().y), (58.0, 20.0));
    // The first row sits under the "File" heading. "Export as PNG" is the first row under the "Export" heading.
    // The menu hangs under the strip, which is 32 points taller than the one-row strip these numbers were first
    // taken for (the e2e specs click them).
    assert!((row.center().y - (92.0 + shift)).abs() < 1.0 && row.min.x < 50.0 && row.max.x > 50.0, "{row:?}");
    let png = h.get_all_by_label("Export as PNG").map(|n| n.rect()).find(|r| r.min.x < 100.0).unwrap();
    assert!((png.center().y - (322.0 + shift)).abs() < 1.0 && png.min.x < 50.0 && png.max.x > 50.0, "{png:?}");
}

#[test]
fn the_side_panels_start_under_the_strip_even_in_a_short_window() {
    for height in [800.0, 420.0, 300.0] {
        let h = at(vec2(1280.0, height), with_selection());
        let strip = chrome(&h).strip.unwrap();
        let tools = chrome(&h).tools.unwrap();
        assert!(tools.min.y >= strip.max.y, "tools {tools:?} under {strip:?} at {height}");
        assert!(tools.max.y <= height && strip.max.y <= height, "inside the window");
        let delete = rect(&h, "Delete");
        assert!(strip.contains_rect(delete), "the groups of the selection are in the strip: {delete:?}");
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
        let popover = ["Stroke blue", "Fill blue", "Pick the stroke colour", "Pick the fill colour"];
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
fn a_window_that_is_too_narrow_for_the_colour_group_collapses_it_and_a_wider_one_expands_again() {
    // The desktop layout at 340 points is not the phone layout, yet the colour group does not fit beside the
    // left cluster. The strip keeps its two rows.
    let mut f = dialog::fixture();
    f.state.layout = LayoutPreference::Desktop;
    let h = at(vec2(340.0, 600.0), f);
    assert!(h.query_by_label(STYLE).is_some() && h.query_by_label("Stroke blue").is_none());
    assert_eq!(chrome(&h).strip.unwrap().height(), STRIP_HEIGHT);
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
