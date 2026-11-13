//! The colour popover and the font family popover.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;
mod dialog;

use common::Fixture;
use drafftink_engine::Origin;
use drafftink_ui::surfaces::color::{PopoverKey, remember_anchor};
use drafftink_ui::{ColorTarget, Routed, UiAction, Verb, route};
use egui::{Key, Rect, pos2};
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::{Value, json};

fn open(fixture: Fixture, target: ColorTarget) -> egui_kittest::Harness<'static, Fixture> {
    let mut h = dialog::harness(fixture);
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
    let mut h = open(dialog::fixture(), ColorTarget::Stroke);
    h.get_by_label("Blue 500").click();
    h.run();
    assert_eq!(calls(&h), [("style.set".to_owned(), json!({"props": {"stroke_color": "blue-500"}}), Origin::Ui)]);
    assert_eq!(dialog::verbs(h.state()), [Verb::CloseOverlays]);
    // The calls run on the real engine and the popover closes.
    h.state_mut().dispatch();
    assert_eq!(h.state().model.style.str_value("stroke_color"), Some("#3b82f6"));
    assert_eq!(h.state().state.color_popover, None);
}

#[test]
fn the_quick_column_and_the_shade_rows() {
    let mut h = open(dialog::fixture(), ColorTarget::Stroke);
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
    // With nothing selected the defaults keep their stroke: "None" is greyed with the engine's words.
    h.state_mut().clear();
    assert!(h.get_by_label("None").accesskit_node().is_disabled());
    h.get_by_label("None").click();
    h.run();
    assert!(calls(&h).is_empty());
    // A shape with a fill can lose its stroke.
    let mut h = open(filled_rect(None), ColorTarget::Stroke);
    h.get_by_label("None").click();
    h.run();
    assert_eq!(calls(&h)[0].1, json!({"props": {"stroke_color": "transparent"}}));
}

/// A selected rectangle with a fill, and a stroke when `stroke` is given.
fn filled_rect(stroke: Option<&str>) -> Fixture {
    let mut f = dialog::fixture();
    let id = drafftink_engine::testing::add_rect(&mut f.session, 0.0, 0.0, 50.0, 50.0).unwrap();
    drafftink_engine::testing::select(&mut f.session, std::slice::from_ref(&id));
    let mut props = json!({"fill_color": "blue-100"});
    if let Some(stroke) = stroke {
        props["stroke_color"] = json!(stroke);
    }
    f.session.execute("style.set", json!({"props": props})).unwrap();
    f.refresh();
    f
}

#[test]
fn the_grid_has_22_families_by_11_shades_and_a_quick_column_of_9() {
    let h = open(dialog::fixture(), ColorTarget::Stroke);
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
    let mut h = open(dialog::fixture(), ColorTarget::Fill);
    h.get_by_label("None").click();
    h.run();
    assert_eq!(calls(&h), [("style.set".to_owned(), json!({"props": {"fill_color": null}}), Origin::Ui)]);
    h.state_mut().dispatch();
    let fill = h.state().model.style.prop("fill_color").cloned();
    assert_eq!(fill.and_then(|p| p.value), Some(Value::Null), "the engine now has no fill");
}

#[test]
fn fill_pick_with_a_palette_name() {
    let mut h = open(dialog::fixture(), ColorTarget::Fill);
    h.get_by_label("Amber 100").click();
    h.run();
    assert_eq!(calls(&h)[0].1, json!({"props": {"fill_color": "amber-100"}}));
}

#[test]
fn the_current_colour_is_ringed_and_none_is_not_black() {
    let h = open(filled_rect(Some("transparent")), ColorTarget::Stroke);
    let selected = |label: &str| on(&h, label);
    assert!(selected("None"));
    assert!(!selected("Black"), "a clear stroke is not black");

    let h = open(filled_rect(Some("#000000")), ColorTarget::Stroke);
    let selected = |label: &str| on(&h, label);
    assert!(selected("Black"));
    assert!(!selected("None"));
}

#[test]
fn fill_without_a_fill_rings_none_only() {
    let mut f = dialog::fixture();
    f.session.execute("style.set", json!({"props": {"fill_color": null}})).unwrap();
    f.refresh();
    let h = open(f, ColorTarget::Fill);
    assert!(on(&h, "None"));
    assert!(!on(&h, "Black"));
}

#[test]
fn the_ring_follows_a_palette_colour() {
    let mut f = dialog::fixture();
    f.session.execute("style.set", json!({"props": {"stroke_color": "red-500"}})).unwrap();
    f.refresh();
    let h = open(f, ColorTarget::Stroke);
    assert!(on(&h, "Red 500"));
    assert!(on(&h, "Red"), "the quick cell too");
    assert!(!on(&h, "Red 400"));
}

#[test]
fn background_pick_produces_set_background() {
    let mut h = open(dialog::fixture(), ColorTarget::Background);
    h.get_by_label("Slate 100").click();
    h.run();
    let got = calls(&h);
    assert_eq!(got, [("doc.set_background".to_owned(), json!({"color": "slate-100"}), Origin::Ui)]);
    // route() sends it to the command.
    let action = UiAction::run("doc.set_background", json!({"color": "slate-100"}));
    assert!(matches!(route(&action, &h.state().state), Routed::Command(c) if c.id == "doc.set_background"));
    assert_eq!(h.state().model.background, "#ffffff");
    let report = h.state_mut().dispatch();
    assert_eq!(report.refusals().count(), 0, "{:?}", report.ran);
    assert_eq!(h.state().state.color_popover, None);
    // The real command changed the page of the document.
    assert_eq!(h.state().model.background, "#f1f5f9");
}

#[test]
fn background_cell_is_greyed_when_the_command_is_not_registered() {
    let f = Fixture::without(&["doc.set_background"]);
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
    dialog::harness(f)
}

#[test]
fn a_refusal_greys_the_cells_with_its_text_as_the_tooltip() {
    let mut f = dialog::fixture();
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
    let mut h = open(dialog::fixture(), ColorTarget::Stroke);
    let field = h.get_by_label("Hex colour");
    field.focus();
    h.run();
    h.get_by_label("Hex colour").type_text("#3b82f6");
    h.run();
    assert!(h.state().calls().is_empty(), "nothing before Enter");
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(calls(&h), [("style.set".to_owned(), json!({"props": {"stroke_color": "#3b82f6"}}), Origin::Ui)]);
    assert_eq!(dialog::verbs(h.state()), [Verb::CloseOverlays]);
}

#[test]
fn hex_field_takes_short_and_alpha_forms_for_the_background() {
    let mut h = open(dialog::fixture(), ColorTarget::Background);
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
    let mut h = open(dialog::fixture(), ColorTarget::Fill);
    h.get_by_label("Hex colour").focus();
    h.run();
    h.get_by_label("Hex colour").type_text("#12");
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert!(h.state().calls().is_empty());
    h.get_by_label_contains("is not a colour");
    assert!(dialog::verbs(h.state()).is_empty(), "the popover stays open");
}

#[test]
fn an_outside_click_closes_and_an_inside_click_does_not() {
    let mut h = open(dialog::fixture(), ColorTarget::Stroke);
    let title = h.get_by_label("Stroke colour").rect().center();
    dialog::click_at(&mut h, title);
    assert!(dialog::verbs(h.state()).is_empty(), "a click inside the popover keeps it");
    dialog::click_canvas(&mut h);
    assert_eq!(dialog::verbs(h.state()), [Verb::CloseOverlays]);
    assert!(h.state().calls().is_empty());
}

#[test]
fn the_popover_stays_on_the_screen() {
    // A button in the bottom right corner: the popover would run off the edges.
    let size = egui::vec2(700.0, 500.0);
    let corner = Rect::from_min_size(pos2(660.0, 470.0), egui::vec2(28.0, 24.0));
    let hook = move |ctx: &egui::Context| remember_anchor(ctx, PopoverKey::Fill, corner);
    let mut h = dialog::with_frame_hook(dialog::fixture(), size, egui::Theme::Light, hook);
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
    let mut h = dialog::sized(dialog::fixture(), size, egui::Theme::Light);
    h.state_mut().state.color_popover = Some(ColorTarget::Stroke);
    h.run();
    let screen = Rect::from_min_size(pos2(0.0, 0.0), size);
    let hex = h.get_by_label("Hex colour").rect();
    assert!(screen.contains_rect(hex), "{hex:?}");
}

#[test]
fn both_themes_draw() {
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        let mut h = dialog::sized(dialog::fixture(), egui::vec2(1280.0, 800.0), theme);
        h.state_mut().state.color_popover = Some(ColorTarget::Fill);
        h.state_mut().state.font_picker_open = true;
        h.run();
        h.get_by_label("Blue 500");
        h.get_by_label("GelPen");
    }
}

// ---- fonts ----

#[test]
fn the_font_picker_lists_five_families_and_marks_the_current_one() {
    let mut f = dialog::fixture();
    f.session.execute("style.set", json!({"props": {"font_family": "gelpen_serif"}})).unwrap();
    f.refresh();
    let mut h = dialog::harness(f);
    h.state_mut().state.font_picker_open = true;
    h.run();
    for (name, current) in [
        ("GelPen", false),
        ("GelPen Serif", true),
        ("Vanilla Extract", false),
        ("Noto Sans", false),
        ("IBM Plex Sans", false),
    ] {
        assert_eq!(on(&h, name), current, "{name}");
    }
}

#[test]
fn picking_a_font_runs_style_set_and_closes() {
    let mut h = dialog::harness(dialog::fixture());
    h.state_mut().state.font_picker_open = true;
    h.run();
    h.get_by_label("Vanilla Extract").click();
    h.run();
    assert_eq!(calls(&h), [("style.set".to_owned(), json!({"props": {"font_family": "vanilla_extract"}}), Origin::Ui)]);
    assert_eq!(dialog::verbs(h.state()), [Verb::CloseOverlays]);
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
        ("IBM Plex Sans", "plex_sans"),
    ] {
        let mut h = dialog::harness(dialog::fixture());
        h.state_mut().state.font_picker_open = true;
        h.run();
        h.get_by_label(label).click();
        h.run();
        assert_eq!(calls(&h)[0].1, json!({"props": {"font_family": value}}), "{label}");
    }
}

#[test]
fn an_outside_click_closes_the_font_picker() {
    let mut h = dialog::harness(dialog::fixture());
    h.state_mut().state.font_picker_open = true;
    h.run();
    dialog::click_canvas(&mut h);
    assert_eq!(dialog::verbs(h.state()), [Verb::CloseOverlays]);
}

// ---- the last picked colour --------------------------------------------------------------------------------------

#[test]
fn a_colour_picked_in_the_popover_becomes_the_last_picked_swatch_next_to_the_wheel() {
    let mut h = open(dialog::fixture(), ColorTarget::Stroke);
    assert!(h.query_by_label("Last picked stroke").is_none(), "no swatch before a pick");
    h.get_by_label("Stone 500").click();
    h.run();
    assert_eq!(h.state().state.last_picked.of(ColorTarget::Stroke), Some("stone-500"));
    assert_eq!(h.state().state.last_picked.of(ColorTarget::Fill), None, "the fill has its own");
    h.state_mut().dispatch();
    h.run();
    h.state_mut().clear();
    // The popover is closed, the top bar shows the swatch and a click on it is the same style.set.
    h.get_by_label("Last picked stroke").click();
    h.run();
    assert_eq!(calls(&h), [("style.set".to_owned(), json!({"props": {"stroke_color": "stone-500"}}), Origin::Ui)]);
}

#[test]
fn the_fill_remembers_its_own_pick_and_a_hex_value_counts() {
    let mut h = open(dialog::fixture(), ColorTarget::Fill);
    h.get_by_label("Teal 300").click();
    h.run();
    assert_eq!(h.state().state.last_picked.of(ColorTarget::Fill), Some("teal-300"));
    assert_eq!(h.state().state.last_picked.of(ColorTarget::Stroke), None);
    h.state_mut().dispatch();
    h.run();
    h.get_by_label("Last picked fill");
    assert!(h.query_by_label("Last picked stroke").is_none());
}

#[test]
fn none_and_the_background_are_not_remembered() {
    let mut h = open(dialog::fixture(), ColorTarget::Fill);
    h.get_by_label("None").click();
    h.run();
    assert_eq!(h.state().state.last_picked.of(ColorTarget::Fill), None, "no fill is not a colour");
    let mut h = open(dialog::fixture(), ColorTarget::Background);
    h.get_by_label("Blue 500").click();
    h.run();
    assert_eq!(h.state().state.last_picked, drafftink_ui::LastPicked::default());
}

// ---- colours in the dark theme: the swatch is the colour the canvas draws ----

/// The fill of the circle painted at the centre of the control labelled `label`.
fn swatch_fill(h: &egui_kittest::Harness<'static, Fixture>, label: &str) -> egui::Color32 {
    fn circles(shape: &egui::Shape, out: &mut Vec<(egui::Pos2, egui::Color32)>) {
        match shape {
            egui::Shape::Circle(c) if c.fill != egui::Color32::TRANSPARENT => out.push((c.center, c.fill)),
            egui::Shape::Vec(v) => v.iter().for_each(|s| circles(s, out)),
            _ => {}
        }
    }
    let centre = h.get_by_label(label).rect().center();
    let mut all = Vec::new();
    h.output().shapes.iter().for_each(|s| circles(&s.shape, &mut all));
    all.into_iter()
        .rev()
        .find(|(c, _)| (c.x - centre.x).abs() < 0.5 && (c.y - centre.y).abs() < 0.5)
        .unwrap_or_else(|| panic!("no swatch is painted for {label:?}"))
        .1
}

fn open_dark(target: ColorTarget) -> egui_kittest::Harness<'static, Fixture> {
    let mut h = dialog::sized(dialog::fixture(), egui::vec2(1280.0, 800.0), egui::Theme::Dark);
    h.state_mut().state.color_popover = Some(target);
    h.run();
    h
}

#[test]
fn in_the_dark_theme_a_swatch_shows_what_the_canvas_draws_for_its_colour() {
    use drafftink_scene::Color;
    let h = open_dark(ColorTarget::Stroke);
    // Blue 500 is stored as blue-500; the dark canvas draws it through the filter, and so does the swatch.
    let want = Color::parse("#3b82f6").unwrap().for_dark_theme();
    assert_eq!(swatch_fill(&h, "Blue 500"), egui::Color32::from_rgb(want.r, want.g, want.b));
    assert_ne!(swatch_fill(&h, "Blue 500"), egui::Color32::from_rgb(0x3b, 0x82, 0xf6), "not the stored colour");
    // Every cell of the grid agrees with the shared mapping, white and black included.
    for fam in drafftink_scene::palette::FAMILIES {
        for shade in drafftink_scene::palette::SHADES.iter().skip(1) {
            let name = format!("{}-{shade}", fam.name);
            let stored = drafftink_scene::palette::lookup(&name).unwrap().for_dark_theme();
            let label = format!("{}{} {shade}", fam.name[..1].to_uppercase(), &fam.name[1..]);
            assert_eq!(swatch_fill(&h, &label), egui::Color32::from_rgb(stored.r, stored.g, stored.b), "{label}");
        }
    }
    // Black and White are named by how they look: the dark canvas draws white as its lightest grey, and black as
    // its own background.
    let white = swatch_fill(&h, "White");
    let black = swatch_fill(&h, "Black");
    assert!(white.r() >= 236 && black.r() <= 19, "{white:?} {black:?}");
}

#[test]
fn in_the_dark_theme_a_palette_pick_stores_the_palette_colour_and_a_look_pick_stores_the_inverse() {
    use drafftink_scene::Color;
    let mut h = open_dark(ColorTarget::Stroke);
    h.get_by_label("Blue 500").click();
    h.run();
    assert_eq!(
        calls(&h)[0].1,
        json!({"props": {"stroke_color": "blue-500"}}),
        "the document holds the same colour in both themes"
    );
    h.state_mut().clear();
    let mut h = open_dark(ColorTarget::Stroke);
    let shown = swatch_fill(&h, "White");
    h.get_by_label("White").click();
    h.run();
    let stored = calls(&h)[0].1["props"]["stroke_color"].as_str().unwrap().to_owned();
    let stored = Color::parse(&stored).unwrap();
    assert_eq!(stored, Color::WHITE.from_dark_theme(), "the inverse of the display");
    let drawn = stored.for_dark_theme();
    assert_eq!(egui::Color32::from_rgb(drawn.r, drawn.g, drawn.b), shown, "and the canvas draws the swatch");
    // The custom hex is read as the colour to see.
    let mut h = open_dark(ColorTarget::Stroke);
    h.get_by_label("Hex colour").focus();
    h.run();
    h.get_by_label("Hex colour").type_text("#808080");
    h.run();
    h.key_press(Key::Enter);
    h.run();
    let typed = calls(&h)[0].1["props"]["stroke_color"].as_str().unwrap().to_owned();
    assert_eq!(Color::parse(&typed).unwrap().for_dark_theme(), Color::rgb(128, 128, 128));
    // And on the light theme nothing changes: the typed colour is stored as it is.
    let mut h = open(dialog::fixture(), ColorTarget::Stroke);
    h.get_by_label("Hex colour").focus();
    h.run();
    h.get_by_label("Hex colour").type_text("#808080");
    h.run();
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(calls(&h)[0].1, json!({"props": {"stroke_color": "#808080"}}));
}

#[test]
fn in_the_light_theme_a_swatch_is_the_stored_colour() {
    use drafftink_scene::Color;
    let h = open(dialog::fixture(), ColorTarget::Stroke);
    let stored = Color::parse("#3b82f6").unwrap();
    assert_eq!(swatch_fill(&h, "Blue 500"), egui::Color32::from_rgb(stored.r, stored.g, stored.b));
    assert_eq!(swatch_fill(&h, "White"), egui::Color32::WHITE);
}

#[test]
fn the_top_bar_the_last_picked_swatch_and_the_page_button_show_the_dark_display_too() {
    use drafftink_scene::Color;
    let shown = |name: &str| {
        let c = drafftink_scene::palette::lookup(name).unwrap().for_dark_theme();
        egui::Color32::from_rgb(c.r, c.g, c.b)
    };
    let mut h = dialog::sized(dialog::fixture(), egui::vec2(1280.0, 800.0), egui::Theme::Dark);
    h.state_mut().state.last_picked.remember(ColorTarget::Stroke, "rose-600".to_owned());
    h.run();
    assert_eq!(swatch_fill(&h, "Stroke blue"), shown("blue-500"));
    assert_eq!(swatch_fill(&h, "Fill red"), shown("red-100"));
    assert_eq!(swatch_fill(&h, "Last picked stroke"), shown("rose-600"));
    // The page is stored white; the canvas draws it as the dark page colour, and so does its button.
    let page = Color::WHITE.for_dark_theme();
    assert_eq!(swatch_fill(&h, "Background colour"), egui::Color32::from_rgb(page.r, page.g, page.b));
}

// ---- the eyedropper ----

const PICK_STROKE: &str = "Pick the stroke colour from the canvas";
const PICK_FILL: &str = "Pick the fill colour from the canvas";

#[test]
fn each_colour_row_has_an_eyedropper_next_to_its_wheel_that_starts_pick_mode_for_its_colour() {
    let mut h = dialog::harness(dialog::fixture());
    let (stroke, fill) = (h.get_by_label(PICK_STROKE).rect(), h.get_by_label(PICK_FILL).rect());
    let (wheel, fill_wheel) =
        (h.get_by_label("Pick the stroke colour").rect(), h.get_by_label("Pick the fill colour").rect());
    assert!(stroke.max.x <= wheel.min.x && (stroke.center().y - wheel.center().y).abs() < 1.0, "left of its wheel");
    assert!(fill.max.x <= fill_wheel.min.x && fill.min.y > stroke.max.y - 1.0, "and the fill row has its own");
    assert!(!on(&h, PICK_STROKE), "not on until it is pressed");

    h.get_by_label(PICK_FILL).click();
    h.run();
    assert_eq!(dialog::verbs(h.state()), [Verb::PickColorFill]);
    h.state_mut().dispatch();
    h.run();
    assert_eq!(h.state().state.eyedropper_target(), Some(ColorTarget::Fill));
    assert!(on(&h, PICK_FILL) && !on(&h, PICK_STROKE), "the button of the row that picks is on");
    h.get_by_label("Click a colour on the canvas to use it for the fill. Esc cancels.");

    // The other row switches the target; pressing the same one again leaves the mode.
    h.get_by_label(PICK_STROKE).click();
    h.run();
    h.state_mut().dispatch();
    assert_eq!(h.state().state.eyedropper_target(), Some(ColorTarget::Stroke));
    h.get_by_label(PICK_STROKE).click();
    h.run();
    h.state_mut().dispatch();
    h.run();
    assert_eq!(h.state().state.eyedropper_target(), None);
    assert!(h.query_by_label("Click a colour on the canvas to use it for the stroke. Esc cancels.").is_none());
}

#[test]
fn the_swatch_beside_the_pointer_shows_the_colour_it_would_pick() {
    let mut h = dialog::harness(dialog::fixture());
    h.state_mut().state.apply_verb(Verb::PickColorStroke, &mut Vec::new());
    h.state_mut().state.set_eyedropper_preview(pos2(500.0, 400.0), Some([0x3b, 0x82, 0xf6, 255]));
    h.run();
    let hex = h.get_by_label("#3b82f6").rect();
    assert!(hex.min.x > 500.0 && hex.min.y > 400.0, "it hangs off the pointer: {hex:?}");
    h.state_mut().state.set_eyedropper_preview(pos2(1270.0, 790.0), Some([0, 0, 0, 255]));
    h.run();
    let hex = h.get_by_label("#000000").rect();
    assert!(hex.max.x <= 1280.0 && hex.max.y <= 800.0, "at the corner it flips to stay on the screen: {hex:?}");
}

#[test]
fn a_platform_with_its_own_screen_picker_gets_a_request_instead_of_pick_mode() {
    let mut f = dialog::fixture();
    f.state.set_screen_color_picker(true);
    let mut h = dialog::harness(f);
    h.get_by_label(PICK_FILL).click();
    h.run();
    h.state_mut().dispatch();
    assert_eq!(h.state().report.host, [drafftink_ui::HostRequest::PickScreenColor(ColorTarget::Fill)]);
    assert!(h.state().state.eyedropper().is_none(), "the platform shows its own loupe");
}

#[test]
fn a_pick_sets_the_colour_on_the_selection_in_one_undo_step() {
    let mut f = dialog::fixture();
    let a = drafftink_engine::testing::add_rect(&mut f.session, 0.0, 0.0, 100.0, 60.0).unwrap();
    drafftink_engine::testing::select(&mut f.session, std::slice::from_ref(&a));
    f.refresh();
    let before = f.session.tab().doc.undo_depth();
    f.state.apply_verb(Verb::PickColorFill, &mut Vec::new());
    let call = f.state.finish_eyedropper(ColorTarget::Fill, [0x10, 0xb9, 0x81]).unwrap();
    f.actions.push(UiAction::Command(call));
    f.dispatch();
    assert_eq!(f.model.style.str_value("fill_color"), Some("#10b981"));
    assert_eq!(f.session.tab().doc.undo_depth(), before + 1, "one undo step");
    assert!(f.state.eyedropper().is_none());
    assert_eq!(f.state.last_picked.of(ColorTarget::Fill), Some("#10b981"), "the strip keeps it as the last picked");
}
