//! The colour popover and the font family popover.

mod common;
mod synth;

use common::Fixture;
use drafftink_engine::Origin;
use drafftink_ui::surfaces::color::{PopoverKey, remember_anchor};
use drafftink_ui::{ColorTarget, Routed, UiAction, Verb, route};
use egui::{Key, Rect, pos2};
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::{Value, json};

fn open(fixture: Fixture, target: ColorTarget) -> egui_kittest::Harness<'static, Fixture> {
    let mut h = synth::harness(fixture);
    h.state_mut().state.color_popover = Some(target);
    h.run();
    h
}

fn on(h: &egui_kittest::Harness<'static, Fixture>, label: &str) -> bool {
    h.get_by_label(label).accesskit_node().toggled() == Some(egui::accesskit::Toggled::True)
}

fn calls(h: &egui_kittest::Harness<'static, Fixture>) -> Vec<(String, Value, Origin)> {
    h.state().calls().into_iter().map(|c| (c.id.clone(), c.args.clone(), c.origin)).collect()
}

#[test]
fn stroke_pick_is_the_exact_style_set_call_and_closes() {
    let mut h = open(synth::fixture(), ColorTarget::Stroke);
    h.get_by_label("Blue 500").click();
    h.run();
    assert_eq!(calls(&h), [("style.set".to_owned(), json!({"props": {"stroke_color": "blue-500"}}), Origin::Ui)]);
    assert_eq!(synth::verbs(h.state()), [Verb::CloseOverlays]);
    // The calls run on the real engine and the popover closes.
    h.state_mut().dispatch();
    assert_eq!(h.state().model.style.str_value("stroke_color"), Some("#3b82f6"));
    assert_eq!(h.state().state.color_popover, None);
}

#[test]
fn the_quick_column_and_the_shade_rows() {
    let mut h = open(synth::fixture(), ColorTarget::Stroke);
    h.get_by_label("Emerald").click();
    h.run();
    assert_eq!(calls(&h)[0].1, json!({"props": {"stroke_color": "emerald-500"}}));
    h.state_mut().clear();
    h.get_by_label("Black").click();
    h.run();
    assert_eq!(calls(&h)[0].1, json!({"props": {"stroke_color": "#000000"}}));
    h.state_mut().clear();
    h.get_by_label("White").click();
    h.run();
    assert_eq!(calls(&h)[0].1, json!({"props": {"stroke_color": "#ffffff"}}));
    h.state_mut().clear();
    h.get_by_label("None").click();
    h.run();
    assert_eq!(calls(&h)[0].1, json!({"props": {"stroke_color": "transparent"}}));
}

#[test]
fn the_grid_has_22_families_by_11_shades_and_a_quick_column_of_9() {
    let h = open(synth::fixture(), ColorTarget::Stroke);
    let labels: Vec<String> =
        h.get_all_by_role(egui::accesskit::Role::Button).filter_map(|n| n.accesskit_node().label()).collect();
    let shaded = labels.iter().filter(|l| l.rsplit(' ').next().is_some_and(|s| s.parse::<u16>().is_ok())).count();
    assert_eq!(shaded, 22 * 11);
    for quick in ["None", "Black", "White", "Blue", "Red", "Emerald", "Amber", "Purple", "Slate"] {
        assert_eq!(labels.iter().filter(|l| *l == quick).count(), 1, "{quick}");
    }
    h.get_by_label("Red 50");
    h.get_by_label("Slate 950");
    h.get_by_label("Stone 500");
    assert!(h.query_by_label("Red 25").is_none(), "shade 25 is not in the grid");
}

#[test]
fn fill_none_removes_the_fill_with_null() {
    let mut h = open(synth::fixture(), ColorTarget::Fill);
    h.get_by_label("None").click();
    h.run();
    assert_eq!(calls(&h), [("style.set".to_owned(), json!({"props": {"fill_color": null}}), Origin::Ui)]);
    h.state_mut().dispatch();
    let fill = h.state().model.style.prop("fill_color").cloned();
    assert_eq!(fill.and_then(|p| p.value), Some(Value::Null), "the engine now has no fill");
}

#[test]
fn fill_pick_with_a_palette_name() {
    let mut h = open(synth::fixture(), ColorTarget::Fill);
    h.get_by_label("Amber 100").click();
    h.run();
    assert_eq!(calls(&h)[0].1, json!({"props": {"fill_color": "amber-100"}}));
}

#[test]
fn the_current_colour_is_ringed_and_none_is_not_black() {
    let mut f = synth::fixture();
    f.session.execute("style.set", json!({"props": {"stroke_color": "#00000000"}})).unwrap();
    f.refresh();
    let h = open(f, ColorTarget::Stroke);
    let selected = |label: &str| on(&h, label);
    assert!(selected("None"));
    assert!(!selected("Black"), "a clear stroke is not black");

    let mut f = synth::fixture();
    f.session.execute("style.set", json!({"props": {"stroke_color": "#000000"}})).unwrap();
    f.refresh();
    let h = open(f, ColorTarget::Stroke);
    let selected = |label: &str| on(&h, label);
    assert!(selected("Black"));
    assert!(!selected("None"));
}

#[test]
fn fill_without_a_fill_rings_none_only() {
    let mut f = synth::fixture();
    f.session.execute("style.set", json!({"props": {"fill_color": null}})).unwrap();
    f.refresh();
    let h = open(f, ColorTarget::Fill);
    assert!(on(&h, "None"));
    assert!(!on(&h, "Black"));
}

#[test]
fn the_ring_follows_a_palette_colour() {
    let mut f = synth::fixture();
    f.session.execute("style.set", json!({"props": {"stroke_color": "red-500"}})).unwrap();
    f.refresh();
    let h = open(f, ColorTarget::Stroke);
    assert!(on(&h, "Red 500"));
    assert!(on(&h, "Red"), "the quick cell too");
    assert!(!on(&h, "Red 400"));
}

#[test]
fn background_pick_produces_set_background() {
    let mut h = open(synth::fixture(), ColorTarget::Background);
    h.get_by_label("Slate 100").click();
    h.run();
    let got = calls(&h);
    assert_eq!(got, [("doc.set_background".to_owned(), json!({"color": "slate-100"}), Origin::Ui)]);
    // route() sends it to the command.
    let action = UiAction::run("doc.set_background", json!({"color": "slate-100"}));
    assert!(matches!(route(&action, &h.state().state), Routed::Command(c) if c.id == "doc.set_background"));
    h.state_mut().dispatch();
    assert_eq!(h.state().report.refusals().count(), 0);
    assert_eq!(h.state().state.color_popover, None);
}

#[test]
fn background_cell_is_greyed_when_the_command_is_not_registered() {
    // The real engine has no doc.set_background yet. When it gets one, this test must use another
    // reason to grey (the view-only test below does).
    let f = Fixture::new();
    let text = f.model.refusal_text("doc.set_background").expect("not registered in this build");
    let mut h = common_harness_with_fonts(f);
    h.state_mut().state.color_popover = Some(ColorTarget::Background);
    h.run();
    let cell = h.get_by_label("Blue 500");
    assert!(cell.accesskit_node().is_disabled());
    cell.click();
    h.run();
    assert!(h.state().calls().is_empty(), "a greyed cell pushes nothing");
    assert_eq!(text, "`doc.set_background` is not available in this build.");
}

fn common_harness_with_fonts(f: Fixture) -> egui_kittest::Harness<'static, Fixture> {
    synth::harness(f)
}

#[test]
fn a_refusal_greys_the_cells_with_its_text_as_the_tooltip() {
    let mut f = synth::fixture();
    f.session.view_only = true;
    f.refresh();
    let text = f.model.refusal_text("style.set").expect("style.set refuses in a view-only document");
    let mut h = open(f, ColorTarget::Stroke);
    let cell = h.get_by_label("Blue 500");
    assert!(cell.accesskit_node().is_disabled());
    cell.hover();
    for _ in 0..10 {
        h.step();
    }
    h.get_by_label_contains(&text);
    h.get_by_label("Blue 500").click();
    h.run();
    assert!(h.state().calls().is_empty());
}

#[test]
fn hex_field_applies_on_enter() {
    let mut h = open(synth::fixture(), ColorTarget::Stroke);
    let field = h.get_by_label("Hex colour");
    field.focus();
    h.run();
    h.get_by_label("Hex colour").type_text("#3b82f6");
    h.run();
    assert!(h.state().calls().is_empty(), "nothing before Enter");
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(calls(&h), [("style.set".to_owned(), json!({"props": {"stroke_color": "#3b82f6"}}), Origin::Ui)]);
    assert_eq!(synth::verbs(h.state()), [Verb::CloseOverlays]);
}

#[test]
fn hex_field_takes_short_and_alpha_forms_for_the_background() {
    let mut h = open(synth::fixture(), ColorTarget::Background);
    h.get_by_label("Hex colour").focus();
    h.run();
    h.get_by_label("Hex colour").type_text("abc");
    h.run();
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(calls(&h)[0].1, json!({"color": "#aabbcc"}));
}

#[test]
fn a_bad_hex_value_shows_an_error_and_pushes_nothing() {
    let mut h = open(synth::fixture(), ColorTarget::Fill);
    h.get_by_label("Hex colour").focus();
    h.run();
    h.get_by_label("Hex colour").type_text("#12");
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert!(h.state().calls().is_empty());
    h.get_by_label_contains("is not a colour");
    assert!(synth::verbs(h.state()).is_empty(), "the popover stays open");
}

#[test]
fn an_outside_click_closes_and_an_inside_click_does_not() {
    let mut h = open(synth::fixture(), ColorTarget::Stroke);
    let title = h.get_by_label("Stroke colour").rect().center();
    synth::click_at(&mut h, title);
    assert!(synth::verbs(h.state()).is_empty(), "a click inside the popover keeps it");
    synth::click_canvas(&mut h);
    assert_eq!(synth::verbs(h.state()), [Verb::CloseOverlays]);
    assert!(h.state().calls().is_empty());
}

#[test]
fn the_popover_stays_on_the_screen() {
    // A button in the bottom right corner: the popover would run off the edges.
    let size = egui::vec2(700.0, 500.0);
    let corner = Rect::from_min_size(pos2(660.0, 470.0), egui::vec2(28.0, 24.0));
    let hook = move |ctx: &egui::Context| remember_anchor(ctx, PopoverKey::Fill, corner);
    let mut h = synth::with_frame_hook(synth::fixture(), size, egui::Theme::Light, hook);
    h.state_mut().state.color_popover = Some(ColorTarget::Fill);
    h.run();
    let screen = Rect::from_min_size(pos2(0.0, 0.0), size);
    let hex = h.get_by_label("Hex colour").rect();
    assert!(screen.contains_rect(hex), "{hex:?} is not inside {screen:?}");
    let title = h.get_by_label("Fill colour").rect();
    assert!(screen.contains_rect(title), "{title:?}");
}

#[test]
fn a_short_screen_gets_a_scrolling_grid_inside_the_screen() {
    let size = egui::vec2(420.0, 300.0);
    let mut h = synth::sized(synth::fixture(), size, egui::Theme::Light);
    h.state_mut().state.color_popover = Some(ColorTarget::Stroke);
    h.run();
    let screen = Rect::from_min_size(pos2(0.0, 0.0), size);
    let hex = h.get_by_label("Hex colour").rect();
    assert!(screen.contains_rect(hex), "{hex:?}");
}

#[test]
fn both_themes_draw() {
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        let mut h = synth::sized(synth::fixture(), egui::vec2(1280.0, 800.0), theme);
        h.state_mut().state.color_popover = Some(ColorTarget::Fill);
        h.state_mut().state.font_picker_open = true;
        h.run();
        h.get_by_label("Blue 500");
        h.get_by_label("GelPen");
    }
}

// ---- fonts ----

#[test]
fn the_font_picker_lists_four_families_and_marks_the_current_one() {
    let mut f = synth::fixture();
    f.session.execute("style.set", json!({"props": {"font_family": "gelpen_serif"}})).unwrap();
    f.refresh();
    let mut h = synth::harness(f);
    h.state_mut().state.font_picker_open = true;
    h.run();
    for (name, current) in [("GelPen", false), ("GelPen Serif", true), ("Vanilla Extract", false), ("Noto Sans", false)]
    {
        assert_eq!(on(&h, name), current, "{name}");
    }
}

#[test]
fn picking_a_font_runs_style_set_and_closes() {
    let mut h = synth::harness(synth::fixture());
    h.state_mut().state.font_picker_open = true;
    h.run();
    h.get_by_label("Vanilla Extract").click();
    h.run();
    assert_eq!(calls(&h), [("style.set".to_owned(), json!({"props": {"font_family": "vanilla_extract"}}), Origin::Ui)]);
    assert_eq!(synth::verbs(h.state()), [Verb::CloseOverlays]);
    h.state_mut().dispatch();
    assert_eq!(h.state().model.style.str_value("font_family"), Some("vanilla_extract"));
    assert!(!h.state().state.font_picker_open);
}

#[test]
fn every_font_family_has_its_setting_value() {
    for (label, value) in [
        ("GelPen", "gelpen"),
        ("GelPen Serif", "gelpen_serif"),
        ("Vanilla Extract", "vanilla_extract"),
        ("Noto Sans", "noto_sans"),
    ] {
        let mut h = synth::harness(synth::fixture());
        h.state_mut().state.font_picker_open = true;
        h.run();
        h.get_by_label(label).click();
        h.run();
        assert_eq!(calls(&h)[0].1, json!({"props": {"font_family": value}}), "{label}");
    }
}

#[test]
fn an_outside_click_closes_the_font_picker() {
    let mut h = synth::harness(synth::fixture());
    h.state_mut().state.font_picker_open = true;
    h.run();
    synth::click_canvas(&mut h);
    assert_eq!(synth::verbs(h.state()), [Verb::CloseOverlays]);
}
