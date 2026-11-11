//! What the menus have in common: one width, one row height, headings in the hamburger menu, the rows of the tab
//! menu, and scrolling in a window that is too short.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;
mod dialog;
mod support_a;

use common::Fixture;
use drafftink_engine::testing::{add_rect, select};
use drafftink_ui::state::LayoutPreference;
use egui::accesskit::Role;
use egui::pos2;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use support_a::menu::{self, H};

const MAIN: &str = "Main menu";
const TABS: &str = "Tab menu";

fn hamburger(f: Fixture) -> H {
    let mut f = f;
    f.state.menu_open = true;
    let mut h = support_a::themed_lenient(f, false);
    h.run();
    h
}

fn context(layout: LayoutPreference, size: egui::Vec2) -> H {
    let mut f = Fixture::new();
    let id = add_rect(&mut f.session, 0.0, 0.0, 30.0, 30.0).unwrap();
    select(&mut f.session, &[id]);
    f.refresh();
    f.state.layout = layout;
    f.state.open_context_menu(pos2(300.0, 60.0), None);
    let mut h = support_a::themed_lenient_sized(f, size);
    h.run();
    h
}

fn tab_menu() -> H {
    let mut f = Fixture::new();
    dialog::run(&mut f, "doc.rename", json!({ "name": "First" }));
    dialog::run(&mut f, "tab.new", json!({ "name": "Second" }));
    dialog::run(&mut f, "tab.switch", json!({ "tab": "First" }));
    let mut h = dialog::harness(f);
    h.run();
    h.get_by_label("First").click_secondary();
    h.run();
    h
}

#[test]
fn the_hamburger_menu_groups_its_rows_under_headings() {
    let h = hamburger(Fixture::new());
    let headings: Vec<String> =
        menu::menu(&h, MAIN).query_all_by_role(Role::Heading).filter_map(|n| n.accesskit_node().label()).collect();
    assert_eq!(headings, ["File", "Export", "Insert", "View", "Help"]);
    let rows = menu::rows_in(&h, MAIN);
    let at = |text: &str| rows.iter().position(|r| r == text).unwrap_or_else(|| panic!("no row {text}: {rows:?}"));
    // Rows keep to their group: the order is File, Export, Insert, View, Help.
    let order = [
        "New tab",
        "Save as…",
        "Clear document",
        "Export as PNG",
        "Copy as PNG",
        "Insert image…",
        "Import Mermaid from clipboard",
        "View-only mode",
        "Preferences…",
        "Command palette…",
        "Show intro document",
        "About Drafft.ink…",
    ];
    let positions: Vec<usize> = order.iter().map(|t| at(t)).collect();
    assert!(positions.windows(2).all(|w| w[0] < w[1]), "{positions:?} for {order:?}");
}

#[test]
fn every_menu_has_the_same_width_and_the_row_height_of_the_layout() {
    let wide = egui::vec2(1280.0, 800.0);
    let c = context(LayoutPreference::Desktop, wide);
    let m = hamburger(Fixture::new());
    let t = tab_menu();
    let widths = [
        menu::row(&c, "Cut").rect().width(),
        menu::row_in(&m, MAIN, "Save").rect().width(),
        menu::row_in(&t, TABS, "Duplicate tab").rect().width(),
    ];
    assert!(widths.iter().all(|w| (w - widths[0]).abs() < 0.5), "one width: {widths:?}");
    // A flyout has it too.
    let mut c = c;
    menu::open_flyout(&mut c, "Copy as");
    assert!((menu::row_in(&c, "Copy as", "Copy as PNG").rect().width() - widths[0]).abs() < 0.5);
    // Rows are 28 points high on the desktop and 40 on a touch screen.
    assert_eq!(menu::row(&c, "Cut").rect().height(), 28.0);
    let touch = context(LayoutPreference::Touch, wide);
    assert_eq!(menu::row(&touch, "Cut").rect().height(), 40.0);
}

#[test]
fn the_tab_menu_has_the_kit_rows() {
    let h = tab_menu();
    assert_eq!(menu::rows_in(&h, TABS), ["Rename tab…", "Duplicate tab", "Move left", "Move right", "Close tab"]);
}

#[test]
fn a_menu_taller_than_the_window_scrolls_and_its_last_row_can_be_reached() {
    let mut h = context(LayoutPreference::Desktop, egui::vec2(1280.0, 260.0));
    h.state_mut().clear();
    let last = menu::row(&h, "Copy style");
    last.scroll_to_me();
    h.run();
    menu::row(&h, "Copy style").click();
    h.run();
    let calls: Vec<_> = h.state().calls().into_iter().map(|c| c.id.clone()).collect();
    assert_eq!(calls, ["clipboard.copy_style"]);
}
