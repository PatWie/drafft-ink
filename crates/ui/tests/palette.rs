//! The command palette: every command is listed, a refused row says why inline, a chosen row runs with
//! origin `Palette` and closes the palette, the filter narrows, a command with required arguments opens
//! its form.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;
mod support_a;

use std::collections::BTreeSet;

use common::Fixture;
use drafftink_engine::{Origin, Registry};
use drafftink_ui::state::PaletteState;
use drafftink_ui::surfaces::palette::{Row, RowAction, filter, rows};
use drafftink_ui::{Call, Verb};
use egui::Key;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use support_a::themed;

type H = egui_kittest::Harness<'static, Fixture>;

fn open_palette(query: &str) -> H {
    let mut f = Fixture::new();
    f.state.palette = Some(PaletteState { query: query.to_owned(), selected: 0 });
    let mut h = themed(f, false);
    h.run();
    h
}

/// The palette row with this title. The toolbar can have a button of the same name, so take the last
/// one: the palette is drawn on top of everything.
fn row<'a>(h: &'a H, title: &'a str) -> egui_kittest::Node<'a> {
    h.get_all_by_role_and_label(egui::accesskit::Role::Button, title)
        .last()
        .unwrap_or_else(|| panic!("no button named {title}"))
}

fn calls(h: &H) -> Vec<Call> {
    h.state().calls().into_iter().cloned().collect()
}

fn row_for<'a>(all: &'a [Row], title: &str) -> &'a Row {
    all.iter().find(|r| r.title == title).unwrap_or_else(|| panic!("no row titled {title}"))
}

#[test]
fn rows_cover_every_palette_command_and_every_titled_binding() {
    let f = Fixture::new();
    let all = rows(&f.model);
    let mut expected: BTreeSet<(String, String)> = BTreeSet::new();
    for spec in Registry::global().all().iter().filter(|s| s.palette) {
        let titled: Vec<&str> = spec.shortcuts.iter().filter_map(|b| b.title).collect();
        if titled.is_empty() {
            expected.insert((spec.id.to_owned(), spec.title.to_owned()));
        }
        for t in titled {
            expected.insert((spec.id.to_owned(), t.to_owned()));
        }
    }
    assert!(expected.len() > 40, "the registry has palette commands: {}", expected.len());
    let listed: BTreeSet<(String, String)> = all
        .iter()
        .filter(|r| !matches!(r.action, RowAction::Verb(_)))
        .map(|r| (r.id.clone(), r.title.clone()))
        .collect();
    assert_eq!(listed, expected, "palette rows and registry differ");
    let command_rows = all.iter().filter(|r| !matches!(r.action, RowAction::Verb(_))).count();
    assert_eq!(command_rows, expected.len(), "no row is listed twice");
    for verb in Verb::ALL {
        assert!(all.iter().any(|r| r.action == RowAction::Verb(verb)), "verb {verb:?} has no row");
    }
}

#[test]
fn a_titled_binding_row_carries_the_binding_arguments_and_its_key() {
    let f = Fixture::new();
    let all = rows(&f.model);
    let rect = row_for(&all, "Rectangle tool");
    assert_eq!(rect.action, RowAction::Command { id: "tool.set".into(), args: json!({ "tool": "rectangle" }) });
    assert_eq!(rect.family, "tool");
    assert!(rect.keys.is_some(), "the tool has a key");
    let undo = row_for(&all, "Undo");
    assert_eq!(undo.keys.as_deref(), Some("Ctrl+Z"));
}

#[test]
fn commands_with_required_arguments_open_a_form_unless_a_binding_fixes_them() {
    let f = Fixture::new();
    let all = rows(&f.model);
    for id in ["arrow.set_type", "style.apply_theme", "frame.rename", "tab.rename", "tab.move", "tab.switch"] {
        let row = all.iter().find(|r| r.id == id).unwrap_or_else(|| panic!("no row for {id}"));
        assert_eq!(row.action, RowAction::Form { id: id.to_owned() }, "{id}");
    }
    for r in all.iter().filter(|r| r.id == "arrange.z_order") {
        assert!(matches!(r.action, RowAction::Command { .. }), "{}: a titled binding fixes `op`", r.title);
    }
}

#[test]
fn palette_shows_titles_and_families_in_the_dialog() {
    let h = open_palette("");
    h.get_by_label("Up and Down to move, Enter to run, Esc to close");
    row(&h, "Undo");
    h.get_by_label("History");
    let buttons = h.get_all_by_role(egui::accesskit::Role::Button).count();
    assert!(buttons >= 20, "rows are drawn: {buttons}");
}

#[test]
fn a_refused_row_shows_the_refusal_inline_and_cannot_run() {
    let mut h = open_palette("undo");
    let f = h.state();
    let reason = f.model.refusal_text("history.undo").expect("nothing to undo in a fresh session");
    let node = row(&h, "Undo");
    assert!(node.accesskit_node().is_disabled(), "the row is disabled");
    // The text is a label of its own, not only a tooltip.
    h.get_by_label(&reason);
    h.state_mut().clear();
    row(&h, "Undo").click();
    h.run();
    assert!(calls(&h).is_empty(), "a refused row does not run: {:?}", calls(&h));
    assert!(h.state().state.palette.is_some(), "and the palette stays open");
}

#[test]
fn every_refused_palette_command_shows_its_own_refusal_text() {
    let f = Fixture::new();
    let all = rows(&f.model);
    let refused: Vec<&Row> = all.iter().filter(|r| !r.enabled()).collect();
    assert!(refused.len() >= 3, "a fresh session refuses several commands");
    for r in refused {
        assert_eq!(r.refusal, f.model.refusal_text(&r.id), "{}", r.id);
    }
    for id in ["history.undo", "group.create"] {
        let row = all.iter().find(|r| r.id == id).expect("row");
        assert_eq!(row.refusal, f.model.refusal_text(id));
        assert!(row.refusal.is_some(), "{id} is refused in a fresh session");
    }
}

#[test]
fn clicking_an_enabled_row_runs_it_with_origin_palette_and_closes() {
    let mut h = open_palette("rectangle tool");
    row(&h, "Rectangle tool").click();
    h.run();
    let calls = calls(&h);
    assert_eq!(calls, vec![Call::new(Origin::Palette, "tool.set", json!({ "tool": "rectangle" }))]);
    assert!(h.state().state.palette.is_none(), "the palette closed");
}

#[test]
fn enter_runs_the_highlighted_row() {
    let mut h = open_palette("ellipse tool");
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::Palette, "tool.set", json!({ "tool": "ellipse" }))]);
    assert!(h.state().state.palette.is_none());
}

#[test]
fn arrow_keys_move_the_highlight() {
    let mut h = open_palette("tool");
    let (second, third) = {
        let f = h.state();
        let all = rows(&f.model);
        let order = filter(&all, "tool");
        (all[order[1]].clone(), all[order[2]].clone())
    };
    h.key_press(Key::ArrowDown);
    h.run();
    assert_eq!(h.state().state.palette.as_ref().map(|p| p.selected), Some(1));
    h.key_press(Key::ArrowDown);
    h.run();
    assert_eq!(h.state().state.palette.as_ref().map(|p| p.selected), Some(2));
    h.key_press(Key::ArrowUp);
    h.run();
    assert_eq!(h.state().state.palette.as_ref().map(|p| p.selected), Some(1));
    h.key_press(Key::Enter);
    h.run();
    let RowAction::Command { id, args } = &second.action else {
        panic!("a command row: {second:?}");
    };
    assert_eq!(calls(&h), vec![Call::new(Origin::Palette, id.clone(), args.clone())], "{third:?} was not chosen");
}

#[test]
fn enter_on_a_refused_row_does_nothing() {
    let mut h = open_palette("undo");
    h.key_press(Key::Enter);
    h.run();
    assert!(calls(&h).is_empty());
    assert!(h.state().state.palette.is_some());
}

#[test]
fn escape_in_the_query_field_closes_the_palette() {
    let mut h = open_palette("");
    h.key_press(Key::Escape);
    h.run();
    let verbs: Vec<_> =
        h.state().actions.iter().filter(|a| matches!(a, drafftink_ui::UiAction::Verb(Verb::CloseOverlays))).collect();
    assert!(!verbs.is_empty(), "Esc asks to close the overlays: {:?}", h.state().actions);
}

#[test]
fn a_verb_row_applies_the_verb_and_closes() {
    let mut h = open_palette("keyboard shortcuts");
    row(&h, "Keyboard shortcuts").click();
    h.run();
    assert!(h.state().actions.contains(&drafftink_ui::UiAction::Verb(Verb::OpenShortcuts)), "{:?}", h.state().actions);
    assert!(calls(&h).is_empty(), "a verb is not a command");
    assert!(h.state().state.palette.is_none());
}

#[test]
fn filtering_narrows_the_rows() {
    let f = Fixture::new();
    let all = rows(&f.model);
    let everything = filter(&all, "").len();
    let some = filter(&all, "zoom");
    assert_eq!(everything, all.len());
    assert!(!some.is_empty() && some.len() < everything / 4, "{} of {everything}", some.len());
    assert!(some.iter().all(|&i| {
        let r = &all[i];
        [&r.title, &r.family, &r.id].iter().any(|t| t.to_lowercase().contains('z'))
    }));
    let wide = open_palette("").get_all_by_role(egui::accesskit::Role::Button).count();
    let narrow = open_palette("zoom").get_all_by_role(egui::accesskit::Role::Button).count();
    assert!(narrow < wide, "{narrow} < {wide}");
    assert!(narrow >= some.len().min(5));
    let none = open_palette("qqqzzz");
    none.get_by_label("No command matches.");
}

#[test]
fn typing_in_the_query_field_filters() {
    let mut h = open_palette("");
    h.get_by_role(egui::accesskit::Role::TextInput).type_text("zoom to fit");
    h.run();
    assert_eq!(h.state().state.palette.as_ref().map(|p| p.query.as_str()), Some("zoom to fit"));
    row(&h, "Zoom to fit everything");
    assert!(h.query_by_role_and_label(egui::accesskit::Role::Button, "Undo").is_none());
}

#[test]
fn the_query_field_is_focused_when_the_palette_opens() {
    let h = open_palette("");
    assert!(h.get_by_role(egui::accesskit::Role::TextInput).is_focused());
}

#[test]
fn a_required_args_command_opens_a_form_and_submitting_runs_it_with_the_built_args() {
    let mut h = open_palette("rename tab");
    row(&h, "Rename tab").click();
    h.run();
    assert!(calls(&h).is_empty(), "opening the form runs nothing");
    assert!(h.state().state.palette.is_some(), "the palette stays open for the form");
    h.get_by_label("name");
    let field = h.get_by_role(egui::accesskit::Role::TextInput);
    field.focus();
    h.run();
    h.get_by_role(egui::accesskit::Role::TextInput).type_text("!");
    h.run();
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Run").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::Palette, "tab.rename", json!({ "name": "Architecture!" }))]);
    assert!(h.state().state.palette.is_none(), "submitting closes the palette");
}

#[test]
fn the_form_is_prefilled_from_the_example_and_a_raw_json_field_reports_bad_json() {
    let mut h = open_palette("tab.switch");
    row(&h, "Switch tab").click();
    h.run();
    // `tab` is an integer-or-string reference: a raw JSON field, prefilled from the example.
    let field = h.get_by_role(egui::accesskit::Role::TextInput);
    assert_eq!(field.value().as_deref(), Some("\"t2\""));
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Run").click();
    h.run();
    assert_eq!(calls(&h), vec![Call::new(Origin::Palette, "tab.switch", json!({ "tab": "t2" }))]);
}

#[test]
fn invalid_json_in_the_form_is_shown_inline_and_blocks_the_run() {
    let mut h = open_palette("tab.switch");
    row(&h, "Switch tab").click();
    h.run();
    h.get_by_role(egui::accesskit::Role::TextInput).focus();
    h.run();
    h.get_by_role(egui::accesskit::Role::TextInput).type_text("x");
    h.run();
    assert!(h.query_by_label_contains("is not valid JSON").is_some());
    let run = h.get_by_role_and_label(egui::accesskit::Role::Button, "Run");
    assert!(run.accesskit_node().is_disabled());
    h.state_mut().clear();
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Run").click();
    h.run();
    assert!(calls(&h).is_empty());
}

#[test]
fn the_palette_works_in_the_dark_theme_too() {
    let mut f = Fixture::new();
    f.state.palette = Some(PaletteState::default());
    let mut h = themed(f, true);
    h.run();
    h.get_by_label("Up and Down to move, Enter to run, Esc to close");
}
