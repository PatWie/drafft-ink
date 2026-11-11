//! Helpers for the tests of the popup menus (the context menu, the hamburger menu, the tab menu). A menu is told
//! apart from the chrome around it by its accessibility node (`Role::Menu` with a name), not by where it is drawn:
//! the chrome has buttons of the same names ("Undo", "Align left", ...), and the top bars may move.
#![allow(dead_code)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use egui::accesskit::Role;
use egui_kittest::kittest::{NodeT, Queryable};

use crate::common::Fixture;

/// The harness of the menu tests.
pub type H = egui_kittest::Harness<'static, Fixture>;

/// The name of the context menu.
pub const CONTEXT: &str = "Context menu";

/// The menu or flyout with this name.
pub fn menu<'a>(h: &'a H, name: &'a str) -> egui_kittest::Node<'a> {
    h.get_by_role_and_label(Role::Menu, name)
}

/// True when a menu or flyout with this name is on screen.
pub fn is_open(h: &H, name: &str) -> bool {
    h.query_by_role_and_label(Role::Menu, name).is_some()
}

/// The row with this text in the menu or flyout `of`.
pub fn row_in<'a>(h: &'a H, of: &'a str, text: &'a str) -> egui_kittest::Node<'a> {
    menu(h, of)
        .query_by_role_and_label(Role::Button, text)
        .unwrap_or_else(|| panic!("no row {text:?} in the menu {of:?}; rows: {:?}", rows_in(h, of)))
}

/// True when the menu or flyout `of` has a row with this text.
pub fn has_row_in(h: &H, of: &str, text: &str) -> bool {
    is_open(h, of) && menu(h, of).query_by_role_and_label(Role::Button, text).is_some()
}

/// The texts of the rows of the menu or flyout `of`, top to bottom.
pub fn rows_in(h: &H, of: &str) -> Vec<String> {
    let mut rows: Vec<_> = menu(h, of)
        .query_all_by_role(Role::Button)
        .filter_map(|n| n.accesskit_node().label().map(|l| (n.rect().min.y, l.to_owned())))
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    rows.into_iter().map(|(_, text)| text).collect()
}

/// A row of the context menu itself.
pub fn row<'a>(h: &'a H, text: &'a str) -> egui_kittest::Node<'a> {
    row_in(h, CONTEXT, text)
}

/// True when the context menu has this row.
pub fn has_row(h: &H, text: &str) -> bool {
    has_row_in(h, CONTEXT, text)
}

/// The rows of the context menu.
pub fn top_rows(h: &H) -> Vec<String> {
    rows_in(h, CONTEXT)
}

/// Opens the flyout of the context-menu row `name` by moving the pointer onto it.
pub fn open_flyout(h: &mut H, name: &str) {
    row(h, name).hover();
    h.run();
    assert!(is_open(h, name), "the flyout {name:?} did not open");
}

/// Clicks the row `text` in the flyout `flyout` (opened first when it is not).
pub fn choose(h: &mut H, flyout: &str, text: &str) {
    if !is_open(h, flyout) {
        open_flyout(h, flyout);
    }
    h.state_mut().clear();
    row_in(h, flyout, text).click();
    h.run();
}
