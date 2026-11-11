//! The shortcuts dialog: built from the registry and the verb table, grouped, searchable, readable in
//! both themes, and spelled `Cmd+` on a Mac.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;
mod support_a;

use std::collections::BTreeSet;

use common::Fixture;
use drafftink_engine::Registry;
use drafftink_ui::keymap::{VERB_BINDINGS, help_rows, parse_chord};
use drafftink_ui::surfaces::shortcuts::{all_rows, grouped};
use drafftink_ui::theme::Palette;
use drafftink_ui::{UiAction, Verb};
use egui::Key;
use egui::accesskit::Role;
use egui_kittest::kittest::Queryable;
use support_a::themed;

type H = egui_kittest::Harness<'static, Fixture>;

fn open(query: &str, mac: bool, dark: bool) -> H {
    let mut f = Fixture::new();
    f.model.mac = mac;
    f.refresh();
    let mut host = Vec::new();
    f.state.apply_verb(Verb::OpenShortcuts, &mut host);
    assert!(f.state.shortcuts_open, "the verb opens the dialog");
    let mut h = themed(f, dark);
    h.run();
    if !query.is_empty() {
        h.get_by_role(Role::TextInput).type_text(query);
        h.run();
    }
    h
}

fn count(h: &H, label: &str) -> usize {
    h.query_all_by_label(label).count()
}

#[test]
fn every_bound_command_and_titled_binding_of_the_registry_has_a_row() {
    let f = Fixture::new();
    let rows = help_rows(&f.model);
    let listed: BTreeSet<(String, String, String)> =
        rows.iter().map(|r| (r.group.clone(), r.title.clone(), r.keys.join(" / "))).collect();
    let mut expected = BTreeSet::new();
    let mut bindings = 0;
    for spec in Registry::global().all() {
        for b in spec.shortcuts {
            let keys: Vec<String> =
                b.chords(false).iter().filter_map(|c| parse_chord(c)).map(|c| c.label(false)).collect();
            if keys.is_empty() {
                continue;
            }
            bindings += 1;
            expected.insert((spec.family.to_owned(), b.title.unwrap_or(spec.title).to_owned(), keys.join(" / ")));
        }
    }
    assert!(bindings > 20, "the registry has many bindings: {bindings}");
    let missing: Vec<_> = expected.difference(&listed).collect();
    assert!(missing.is_empty(), "bindings without a help row: {missing:?}");
}

#[test]
fn every_ui_verb_key_has_a_row() {
    let f = Fixture::new();
    let rows = help_rows(&f.model);
    for vb in VERB_BINDINGS {
        let want: Vec<String> = vb.keys.iter().filter_map(|k| parse_chord(k)).map(|c| c.label(false)).collect();
        assert!(
            rows.iter().any(|r| r.group == "ui" && r.title == vb.verb.title() && r.keys == want),
            "no row for {:?}",
            vb.verb
        );
    }
}

#[test]
fn the_dialog_opens_from_the_verb_and_shows_undo_and_its_key() {
    let h = open("undo", false, false);
    assert!(count(&h, "Undo") >= 1, "the Undo row");
    assert!(count(&h, "Ctrl+Z") >= 1, "its key");
    h.get_by_label("Keyboard shortcuts");
}

#[test]
fn searching_filters_the_rows() {
    let h = open("undo", false, false);
    let all = open("", false, false);
    // The bottom bar has a Redo button too, so compare with the unfiltered list instead of with zero.
    assert!(count(&h, "Redo") < count(&all, "Redo"), "Redo does not match `undo`");
    assert!(count(&h, "Undo") >= 1);
    let first = {
        let f = all.state();
        let rows = all_rows(&f.model);
        grouped(&rows, "").first().map(|(h, _)| h.clone()).expect("a first group")
    };
    assert!(count(&all, &first) >= 1, "the unfiltered list starts with the group {first}");
    let none = open("qqqzzz", false, false);
    none.get_by_label("No shortcut matches.");
}

#[test]
fn grouping_puts_a_heading_per_family_with_capitalised_names() {
    let f = Fixture::new();
    let rows = all_rows(&f.model);
    let groups = grouped(&rows, "");
    let headings: Vec<&str> = groups.iter().map(|(h, _)| h.as_str()).collect();
    for h in &headings {
        assert!(h.chars().next().is_some_and(char::is_uppercase), "{h} is capitalised");
    }
    for h in ["History", "Tool", "Interface", "Text editing", "Mouse and touch"] {
        assert!(headings.contains(&h), "no group {h}: {headings:?}");
    }
    assert_eq!(headings.iter().collect::<BTreeSet<_>>().len(), headings.len(), "one heading per group");
    let total: usize = groups.iter().map(|(_, l)| l.len()).sum();
    assert_eq!(total, rows.len(), "an empty query keeps every row");
}

#[test]
fn the_search_matches_titles_groups_and_keys() {
    let f = Fixture::new();
    let rows = all_rows(&f.model);
    let titles = |q: &str| -> Vec<String> {
        grouped(&rows, q).into_iter().flat_map(|(_, l)| l.into_iter().map(|r| r.title.clone())).collect()
    };
    assert!(titles("undo").iter().any(|t| t == "Undo"));
    assert!(!titles("undo").iter().any(|t| t == "Redo"));
    assert!(titles("ctrl+z").iter().any(|t| t == "Undo"), "by key");
    assert!(!titles("history").is_empty(), "by group");
}

#[test]
fn the_fixed_sections_list_text_editing_and_pointer_gestures() {
    let h = open("wheel", false, false);
    h.get_by_label("Mouse and touch");
    h.get_by_label("Pan");
    h.get_by_label("Zoom");
    let h = open("by word", false, false);
    h.get_by_label("Text editing");
    h.get_by_label("Move by word");
}

#[test]
fn mac_labels_use_the_command_symbol() {
    let mut f = Fixture::new();
    f.model.mac = true;
    f.refresh();
    let rows = help_rows(&f.model);
    let undo = rows.iter().find(|r| r.title == "Undo").expect("Undo row");
    assert_eq!(undo.keys, ["Cmd+Z"]);
    assert!(rows.iter().filter(|r| r.group != "ui").flat_map(|r| &r.keys).any(|k| k.contains("Cmd+")));
    assert!(!rows.iter().flat_map(|r| &r.keys).any(|k| k.contains("Ctrl+")), "no Windows-style Mod chord on a Mac");
    let fixed = all_rows(&f.model);
    let text_undo = fixed.iter().find(|r| r.title == "Undo and redo typing").expect("row");
    assert_eq!(text_undo.keys[0], "Cmd+Z");
}

#[test]
fn on_a_mac_the_caps_are_drawn_without_glyphs_the_fonts_lack() {
    // `themed` fails the test on a missing glyph, so this proves the dialog never asks for ⌘.
    let h = open("undo", true, false);
    assert!(count(&h, "Cmd+Z") >= 1, "the chord is one named node");
}

#[test]
fn arrow_keys_are_spelled_as_words_because_the_fonts_have_no_arrow_glyphs() {
    let h = open("by word", false, false);
    assert!(count(&h, "Ctrl+Left") >= 1);
}

#[test]
fn the_search_field_is_focused_and_esc_closes() {
    let mut h = open("", false, false);
    assert!(h.get_by_role(Role::TextInput).is_focused());
    h.state_mut().clear();
    h.key_press(Key::Escape);
    h.run();
    assert!(h.state().actions.contains(&UiAction::Verb(Verb::CloseOverlays)), "{:?}", h.state().actions);
}

#[test]
fn the_dialog_draws_in_the_dark_theme() {
    let h = open("undo", false, true);
    assert!(count(&h, "Undo") >= 1);
}

fn luminance(c: egui::Color32) -> f64 {
    let lin = |v: u8| {
        let s = f64::from(v) / 255.0;
        if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

fn contrast(a: egui::Color32, b: egui::Color32) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

#[test]
fn text_has_enough_contrast_in_both_themes() {
    for (name, pal) in [("light", Palette::LIGHT), ("dark", Palette::DARK)] {
        assert!(contrast(pal.text, pal.panel) >= 7.0, "{name}: text on panel");
        assert!(contrast(pal.text, pal.button) >= 7.0, "{name}: text on a key cap");
        assert!(contrast(pal.muted, pal.panel) >= 4.5, "{name}: muted text on panel");
        assert!(contrast(pal.muted, pal.selected) >= 3.0, "{name}: muted text on the highlighted row");
        assert!(contrast(pal.text, pal.selected) >= 4.5, "{name}: text on the highlighted row");
    }
}
