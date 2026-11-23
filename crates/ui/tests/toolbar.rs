//! The tool column: the 16 tools and nothing else, the lock badge, and the library button that left the column for
//! the top right corner. What the clicks and double-clicks call is in `surfaces.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;
mod dialog;

use common::Fixture;
use drafftink_engine::state::Tool;
use drafftink_ui::state::LayoutPreference;
use drafftink_ui::surfaces::toolbar::TOOLS;
use drafftink_ui::theme::Palette;
use egui::epaint::{CircleShape, Shape};
use egui::{Rect, Vec2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;

type H = Harness<'static, Fixture>;

const LIBRARY: &str = "Show or hide the library";

fn at(size: Vec2, theme: egui::Theme, f: Fixture) -> H {
    let mut h = dialog::sized(f, size, theme);
    h.run();
    h
}

fn rect(h: &H, label: &str) -> Rect {
    h.get_all_by_label(label).next().unwrap_or_else(|| panic!("no {label}")).rect()
}

/// The accessible name of the button of a tool.
fn name(tool: Tool) -> &'static str {
    match tool {
        Tool::Select => "Select tool",
        Tool::Lasso => "Lasso tool",
        Tool::Hand => "Hand tool",
        Tool::Rectangle => "Rectangle tool",
        Tool::Diamond => "Diamond tool",
        Tool::Ellipse => "Ellipse tool",
        Tool::Arrow => "Arrow tool",
        Tool::Line => "Line tool",
        Tool::Freehand => "Freehand tool",
        Tool::Highlighter => "Highlighter tool",
        Tool::Text => "Text tool",
        Tool::Sticky => "Sticky note tool",
        Tool::Math => "Math tool",
        Tool::Frame => "Frame tool",
        Tool::Eraser => "Eraser tool",
        Tool::Laser => "Laser tool",
    }
}

fn locked(tool: Tool) -> Fixture {
    let mut f = dialog::fixture();
    dialog::run(&mut f, "tool.set", json!({ "tool": tool.name() }));
    dialog::run(&mut f, "tool.lock", json!({ "on": true }));
    f
}

// ---- the column ----

#[test]
fn the_column_is_the_16_tools_in_order_and_nothing_else() {
    let h = at(vec2(1280.0, 800.0), egui::Theme::Light, dialog::fixture());
    assert_eq!(TOOLS.len(), 16);
    let tools = h.state().state.chrome.tools.expect("the column");
    let mut inside: Vec<(String, Rect)> = h
        .query_all_by_role(egui::accesskit::Role::Button)
        .filter(|n| tools.contains_rect(n.rect()))
        .map(|n| (n.accesskit_node().label().unwrap_or_default().to_owned(), n.rect()))
        .collect();
    inside.sort_by(|a, b| a.1.min.y.total_cmp(&b.1.min.y));
    let labels: Vec<&str> = inside.iter().map(|(l, _)| l.as_str()).collect();
    let expected: Vec<&str> = TOOLS.iter().map(|(t, _)| name(*t)).collect();
    assert_eq!(labels, expected);
    // The sticky note follows the text tool.
    let at_of = |t: Tool| expected.iter().position(|l| *l == name(t));
    assert_eq!(at_of(Tool::Sticky), at_of(Tool::Text).map(|i| i + 1));
    // No spacer: the buttons are 34 high and 3 apart, all the way down.
    for pair in inside.windows(2) {
        assert_eq!(pair[1].1.min.y - pair[0].1.max.y, 3.0, "{} and {}", pair[0].0, pair[1].0);
    }
    for gone in ["Sticky note", "Keep tool active", LIBRARY] {
        assert!(h.query_by_label(gone).is_none() || rect(&h, gone).min.y < 80.0, "{gone} is not in the column");
    }
}

#[test]
fn the_touch_column_has_the_collapse_button_and_the_16_tools() {
    let mut f = dialog::fixture();
    f.state.layout = LayoutPreference::Touch;
    let mut h = at(vec2(1280.0, 1300.0), egui::Theme::Light, f);
    let tools = h.state().state.chrome.tools.expect("the column");
    let buttons =
        |h: &H| h.query_all_by_role(egui::accesskit::Role::Button).filter(|n| tools.contains_rect(n.rect())).count();
    assert_eq!(buttons(&h), 17, "16 tools and the button that hides them");
    h.get_by_label("Hide the tools").click();
    h.run();
    assert_eq!(h.state().state.chrome.tools.map(|r| r.height() < 80.0), Some(true));
    assert!(h.query_by_label("Rectangle tool").is_none());
    assert!(h.query_by_label("Show the tools").is_some(), "the collapse button stays");
}

#[test]
fn the_column_keeps_the_place_the_browser_specs_click() {
    // `e2e/web/support/ui.ts`: x 40, and the first button centred 8 points inside the panel, which is centred on the
    // 800 point window and 16 tools tall.
    let h = at(vec2(1280.0, 800.0), egui::Theme::Light, dialog::fixture());
    let first = rect(&h, "Select tool").center();
    let panel_height = 16.0 * 34.0 + 15.0 * 3.0 + 2.0 * 8.0;
    // 122.5, which egui snaps to the pixel above it.
    assert_eq!((first.x, first.y), (40.0, (400.0 - panel_height / 2.0 + 8.0 + 17.0_f32).round()));
    let last = rect(&h, "Laser tool").center();
    assert_eq!(last.y - first.y, 15.0 * 37.0);
}

// ---- the lock badge ----

/// The circles of the last frame, with the clip of the shape that holds them.
fn circles(h: &H) -> Vec<CircleShape> {
    fn walk(shape: &Shape, out: &mut Vec<CircleShape>) {
        match shape {
            Shape::Circle(c) => out.push(*c),
            Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    h.output().shapes.iter().for_each(|clipped| walk(&clipped.shape, &mut out));
    out
}

/// The circles that lie in the corner of `button`, the way the badge does.
fn badge_in(h: &H, button: Rect) -> Vec<CircleShape> {
    circles(h)
        .into_iter()
        .filter(|c| button.contains(c.center) && c.center.x > button.center().x && c.center.y > button.center().y)
        .collect()
}

#[test]
fn the_locked_tool_wears_a_lock_badge_in_a_corner_in_both_themes_and_in_the_touch_layout() {
    for (theme, pal) in [(egui::Theme::Light, Palette::LIGHT), (egui::Theme::Dark, Palette::DARK)] {
        for touch in [false, true] {
            let mut f = locked(Tool::Rectangle);
            if touch {
                f.state.layout = LayoutPreference::Touch;
            }
            let h = at(vec2(1280.0, 1000.0), theme, f);
            let button = rect(&h, "Rectangle tool");
            let badge = badge_in(&h, button);
            assert_eq!(badge.len(), 1, "{theme:?} touch={touch}: {badge:?}");
            let disc = badge[0];
            // In the colour of the icon on the accent: white on the blue, as legible as the icon beside it.
            assert_eq!(disc.fill, pal.on_accent, "{theme:?}");
            assert!(button.contains_rect(Rect::from_center_size(disc.center, Vec2::splat(2.0 * disc.radius))));
            assert!(disc.radius >= 7.0, "big enough to read: {} at {theme:?} touch={touch}", disc.radius);
            // Only that button has one.
            for tool in TOOLS.iter().map(|(t, _)| *t).filter(|t| *t != Tool::Rectangle) {
                assert!(badge_in(&h, rect(&h, name(tool))).is_empty(), "{tool:?}");
            }
        }
    }
}

#[test]
fn no_badge_while_the_lock_is_off_and_it_follows_the_lock() {
    let h = at(vec2(1280.0, 800.0), egui::Theme::Light, dialog::fixture());
    for (tool, _) in TOOLS {
        assert!(badge_in(&h, rect(&h, name(*tool))).is_empty(), "{tool:?}");
    }
    // A tool that is active while the lock is on elsewhere wears no badge: the lock is on the tool it keeps.
    let mut f = locked(Tool::Ellipse);
    dialog::run(&mut f, "tool.set", json!({ "tool": "line" }));
    let h = at(vec2(1280.0, 800.0), egui::Theme::Light, f);
    assert_eq!(badge_in(&h, rect(&h, "Line tool")).len(), 1, "the lock keeps the tool that is active");
    assert!(badge_in(&h, rect(&h, "Ellipse tool")).is_empty());
}

// ---- the library button ----

#[test]
fn the_library_button_is_the_top_right_corner_of_the_strip_at_every_layout() {
    for size in [vec2(1280.0, 800.0), vec2(1100.0, 800.0), vec2(1180.0, 820.0), vec2(600.0, 900.0), vec2(390.0, 800.0)]
    {
        let mut f = dialog::fixture();
        // A selection gives the strip its contextual groups and the More button.
        let id = drafftink_engine::testing::add_rect(&mut f.session, 0.0, 0.0, 50.0, 50.0).unwrap();
        drafftink_engine::testing::select(&mut f.session, std::slice::from_ref(&id));
        f.refresh();
        let h = at(size, egui::Theme::Light, f);
        let strip = h.state().state.chrome.strip.expect("the strip");
        let library = rect(&h, LIBRARY);
        assert!(strip.contains_rect(library), "{size:?}: {library:?} in {strip:?}");
        assert_eq!(library.max.x, size.x - 12.0, "{size:?}: as far from the right edge as the menu is from the left");
        assert_eq!(library.min.y, rect(&h, "Open or close the main menu").min.y, "{size:?}: the first row");
        // It overlaps no other button of the strip.
        for node in h.query_all_by_role(egui::accesskit::Role::Button) {
            let other = node.rect();
            if other != library && strip.contains_rect(other) {
                assert!(
                    !other.intersects(library),
                    "{size:?}: {:?} {other:?} over {library:?}",
                    node.accesskit_node().label()
                );
            }
        }
    }
}

#[test]
fn the_more_button_stands_under_the_library_button_in_a_two_row_strip() {
    let mut f = dialog::fixture();
    let id = drafftink_engine::testing::add_rect(&mut f.session, 0.0, 0.0, 50.0, 50.0).unwrap();
    drafftink_engine::testing::select(&mut f.session, std::slice::from_ref(&id));
    f.refresh();
    let h = at(vec2(1280.0, 800.0), egui::Theme::Light, f);
    let (library, more) = (rect(&h, LIBRARY), rect(&h, "More options"));
    assert!(more.min.y >= library.max.y && more.max.x == library.max.x, "{more:?} under {library:?}");
}
