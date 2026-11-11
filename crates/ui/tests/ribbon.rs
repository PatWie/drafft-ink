#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The ribbon: which groups show for what is selected or drawn, where the card sits, how it behaves in a small
//! window and in the dark theme. The controls are found by their accessible names; what a click runs is in
//! `surfaces.rs`, `text_props.rs` and `refusals.rs`.

mod common;
mod dialog;

use common::Fixture;
use drafftink_engine::testing::{add_rect, add_text_label, select};
use drafftink_scene::ElementId;
use drafftink_ui::Verb;
use drafftink_ui::chrome::ChromeRects;
use drafftink_ui::state::LayoutPreference;
use egui::{Rect, Vec2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;

type H = Harness<'static, Fixture>;

fn at(size: Vec2, theme: egui::Theme, f: Fixture) -> H {
    let mut h = dialog::sized(f, size, theme);
    h.run();
    h
}

fn desk(f: Fixture) -> H {
    at(vec2(1280.0, 800.0), egui::Theme::Light, f)
}

fn chrome(h: &H) -> ChromeRects {
    h.state().state.chrome
}

fn has(h: &H, label: &str) -> bool {
    h.query_by_label(label).is_some()
}

fn rect(h: &H, label: &str) -> Rect {
    h.get_by_label(label).rect()
}

fn create(f: &mut Fixture, args: serde_json::Value) -> ElementId {
    let made = dialog::run(f, "element.create", args);
    ElementId::new(made["id"].as_str().unwrap()).unwrap()
}

fn with_tool(tool: &str) -> Fixture {
    let mut f = dialog::fixture();
    dialog::run(&mut f, "tool.set", json!({ "tool": tool }));
    f
}

fn selecting(f: &mut Fixture, ids: &[ElementId]) {
    select(&mut f.session, ids);
    f.refresh();
}

fn one_rect() -> Fixture {
    let mut f = dialog::fixture();
    let a = add_rect(&mut f.session, 0.0, 0.0, 100.0, 60.0).unwrap();
    selecting(&mut f, &[a]);
    f
}

fn rects(n: usize) -> Fixture {
    let mut f = dialog::fixture();
    let ids: Vec<ElementId> =
        (0..n).map(|i| add_rect(&mut f.session, 150.0 * i as f64, 0.0, 100.0, 60.0).unwrap()).collect();
    selecting(&mut f, &ids);
    f
}

fn free_text() -> Fixture {
    let mut f = dialog::fixture();
    let t = create(&mut f, json!({ "kind": "text", "text": "hello", "at": [0, 0] }));
    selecting(&mut f, &[t]);
    f
}

fn arrow() -> Fixture {
    let mut f = dialog::fixture();
    let a = create(&mut f, json!({ "kind": "arrow", "points": [[0, 0], [80, 30]] }));
    selecting(&mut f, &[a]);
    f
}

fn freehand() -> Fixture {
    let mut f = dialog::fixture();
    let s = create(&mut f, json!({ "kind": "freehand", "points": [[0, 0], [10, 14], [30, 5], [50, 20]] }));
    selecting(&mut f, &[s]);
    f
}

/// The text and a rectangle together.
fn text_and_shape() -> Fixture {
    let mut f = dialog::fixture();
    let r = add_rect(&mut f.session, 0.0, 200.0, 100.0, 60.0).unwrap();
    let t = create(&mut f, json!({ "kind": "text", "text": "hello", "at": [0, 0] }));
    selecting(&mut f, &[r, t]);
    f
}

/// Opens a ribbon menu or dropdown by its accessible name.
fn open(h: &mut H, label: &str) {
    h.get_by_label(label).click();
    h.run();
    h.state_mut().dispatch();
    h.run();
}

const FONT: &str = "Pick the font";
const TEXT_SIZE: &str = "Text size M (20 px)";
const CORNERS: &str = "Corners: Round";
const PATTERN: &str = "Fill pattern: Hatch";
const BRING_FRONT: &str = "Bring to front";
const ALIGN_LEFT: &str = "Align left";

// ---- nothing selected ----

#[test]
fn with_nothing_selected_and_a_tool_that_draws_nothing_there_is_no_card() {
    for tool in ["select", "lasso", "hand", "eraser", "laser"] {
        let h = desk(with_tool(tool));
        assert!(chrome(&h).ribbon.is_none(), "{tool}: no card");
        assert!(!has(&h, "More options") && !has(&h, "Dashed"), "{tool}");
        // The strip still holds the defaults for new elements.
        h.get_by_label("Stroke blue");
        h.get_by_label("Thin");
    }
}

#[test]
fn each_drawing_tool_shows_the_groups_of_its_new_elements() {
    let h = desk(with_tool("rectangle"));
    assert!(has(&h, "Dashed") && has(&h, CORNERS) && has(&h, "More options") && has(&h, "Opacity"));
    assert!(
        !has(&h, FONT) && !has(&h, "Brush: Normal") && !has(&h, BRING_FRONT),
        "no text, no brush, nothing to layer"
    );
    let h = desk(with_tool("diamond"));
    assert!(has(&h, CORNERS));
    let h = desk(with_tool("ellipse"));
    assert!(has(&h, "Dashed") && !has(&h, CORNERS), "an ellipse has no corners");
    let h = desk(with_tool("line"));
    assert!(has(&h, "Dashed") && !has(&h, FONT) && !has(&h, "New arrows: Elbow"));
    let h = desk(with_tool("freehand"));
    assert!(has(&h, "Brush: Calligraphy") && has(&h, "More options") && !has(&h, "Dashed"));
    let h = desk(with_tool("highlighter"));
    assert!(has(&h, "Brush: Normal"));
    for tool in ["text", "math"] {
        let h = desk(with_tool(tool));
        assert!(has(&h, FONT) && has(&h, TEXT_SIZE) && has(&h, "Text align: Left"), "{tool}");
        assert!(has(&h, "Heavy") && has(&h, "More options"), "{tool}: weight, line height");
        assert!(!has(&h, "Dashed") && !has(&h, CORNERS) && !has(&h, BRING_FRONT), "{tool}");
    }
}

#[test]
fn the_arrow_tool_sets_new_arrows_and_keeps_the_corners_in_the_more_menu() {
    let mut h = desk(with_tool("arrow"));
    assert!(has(&h, "New arrows: Sharp") && has(&h, "New arrows: Round") && has(&h, "New arrows: Elbow"));
    assert!(has(&h, "Start head") && has(&h, "End head") && has(&h, "Dashed"));
    assert!(!has(&h, CORNERS), "the path chips are next to it, the corners wait in the menu");
    assert!(!has(&h, "Path: Elbow"), "that is for arrows that exist");
    open(&mut h, "More options");
    h.get_by_label(CORNERS);
}

// ---- a selection ----

#[test]
fn a_rectangle_gets_stroke_corners_opacity_layers_and_actions_but_no_text_align_or_pattern() {
    let h = desk(one_rect());
    for label in ["Solid", "Dashed", "Dotted", CORNERS, "More options", BRING_FRONT, "Flip horizontally", "Duplicate"] {
        assert!(has(&h, label), "{label}");
    }
    h.get_by_role_and_label(egui::accesskit::Role::Slider, "Opacity");
    for label in [FONT, TEXT_SIZE, ALIGN_LEFT, "Path: Elbow", "Start head", "Brush: Normal", PATTERN] {
        assert!(!has(&h, label), "{label}");
    }
}

#[test]
fn the_fill_patterns_come_with_a_fill() {
    let mut f = one_rect();
    dialog::run(&mut f, "style.set", json!({ "props": { "fill_color": "blue-100" } }));
    let mut h = desk(f);
    for label in [
        "Fill pattern: Solid",
        PATTERN,
        "Fill pattern: Cross-hatch",
        "Fill pattern: Dots",
        "Fill pattern: Zigzag",
        "Fill pattern: Dashed hatch",
        "Fill pattern: Zigzag lines",
    ] {
        assert!(has(&h, label), "{label}");
    }
    common::click(&mut h, PATTERN);
    let calls = h.state().calls().into_iter().cloned().collect::<Vec<_>>();
    assert_eq!(
        calls.last().map(|c| (c.id.as_str(), &c.args)),
        Some(("style.set", &json!({ "props": { "fill_pattern": "hachure" } })))
    );
    let mut f = one_rect();
    dialog::run(&mut f, "style.set", json!({ "props": { "fill_color": null } }));
    let h = desk(f);
    assert!(!has(&h, PATTERN), "no fill, no pattern");
}

#[test]
fn text_gets_font_weight_size_emphasis_and_alignment() {
    let mut h = desk(free_text());
    for label in [
        FONT,
        "Light",
        "Regular",
        "Heavy",
        TEXT_SIZE,
        "Text size XL (36 px)",
        "Bold text",
        "Italic text",
        "Text align: Center",
    ] {
        assert!(has(&h, label), "{label}");
    }
    assert!(has(&h, BRING_FRONT) && has(&h, "Duplicate") && has(&h, "More options"));
    for label in ["Dashed", CORNERS, "Path: Sharp"] {
        assert!(!has(&h, label), "{label}");
    }
    open(&mut h, "More options");
    h.get_by_label("Fixed width");
    h.get_by_label("Line height 1.5");
}

#[test]
fn an_arrow_gets_path_heads_and_a_swap() {
    let mut h = desk(arrow());
    for label in ["Path: Sharp", "Path: Round", "Path: Elbow", "Start head", "End head", "Dashed", BRING_FRONT] {
        assert!(has(&h, label), "{label}");
    }
    assert!(!has(&h, CORNERS) && !has(&h, FONT) && !has(&h, "New arrows: Elbow"));
    open(&mut h, "End head");
    h.get_by_label("End head: Triangle");
    h.get_by_label("Swap heads");
}

#[test]
fn a_freehand_stroke_gets_brush_and_pressure() {
    let mut h = desk(freehand());
    assert!(has(&h, "Brush: Normal") && has(&h, "Brush: Calligraphy") && has(&h, BRING_FRONT));
    assert!(!has(&h, FONT) && !has(&h, CORNERS));
    open(&mut h, "More options");
    h.get_by_label("Uniform");
    h.get_by_label("Pen");
}

#[test]
fn align_needs_two_elements_and_distribute_three() {
    let distribute = |h: &H| {
        let title = h.state().model.label("arrange.distribute", &json!({ "axis": "horizontal" }));
        has(h, &title)
    };
    let one = desk(rects(1));
    assert!(!has(&one, ALIGN_LEFT) && !distribute(&one));
    let two = desk(rects(2));
    assert!(has(&two, ALIGN_LEFT), "two can be aligned");
    assert!(!distribute(&two), "{:?}", two.state().model.refusal_text("arrange.distribute"));
    let three = desk(rects(3));
    assert!(has(&three, ALIGN_LEFT) && distribute(&three), "three can be distributed");
}

#[test]
fn a_text_and_a_shape_together_get_both_sets_of_groups() {
    let h = desk(text_and_shape());
    for label in [FONT, TEXT_SIZE, "Text align: Left", "Dashed", CORNERS, BRING_FRONT, ALIGN_LEFT, "Bold text"] {
        assert!(has(&h, label), "{label}");
    }
    assert!(!has(&h, "Path: Elbow"));
}

#[test]
fn a_labelled_shape_offers_the_vertical_align_and_bold_of_its_label() {
    let mut f = dialog::fixture();
    let r = add_rect(&mut f.session, 0.0, 0.0, 200.0, 120.0).unwrap();
    add_text_label(&mut f.session, &r, "Hi").unwrap();
    selecting(&mut f, &[r]);
    let mut h = desk(f);
    assert!(has(&h, "Bold text") && has(&h, FONT));
    open(&mut h, "More options");
    h.get_by_label("Middle");
}

// ---- the card ----

#[test]
fn the_groups_keep_their_order_from_left_to_right() {
    let h = desk(rects(3));
    let labels = [
        "Dashed",
        CORNERS,
        "More options",
        "Opacity",
        ALIGN_LEFT,
        BRING_FRONT,
        "Flip horizontally",
        "Duplicate",
        "Delete",
    ];
    let xs: Vec<f32> = labels.iter().map(|l| rect(&h, l).min.x).collect();
    assert!(xs.windows(2).all(|w| w[0] < w[1]), "{labels:?} at {xs:?}");
    let ys: Vec<f32> = labels.iter().map(|l| rect(&h, l).center().y).collect();
    assert!(ys.iter().all(|y| (y - ys[0]).abs() < 1.0), "one row at 1280: {ys:?}");
}

#[test]
fn the_card_hangs_under_the_strip_and_is_centred_on_the_window() {
    for (name, f) in [("rect", one_rect()), ("three", rects(3)), ("arrow", arrow())] {
        let h = desk(f);
        let (strip, card) = (chrome(&h).strip.unwrap(), chrome(&h).ribbon.unwrap());
        assert!(
            card.min.y > strip.max.y && card.min.y - strip.max.y < 12.0,
            "{name}: attached under the strip: {card:?}"
        );
        assert!((card.center().x - 640.0).abs() < 1.5, "{name}: centred, at {:?}", card.center());
    }
}

#[test]
fn the_card_stays_inside_the_window_and_clear_of_the_other_chrome_at_every_size() {
    let sizes = [(1280.0, 800.0), (820.0, 1180.0), (390.0, 844.0), (1180.0, 820.0)];
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        for (w, height) in sizes {
            for (name, mut f) in [("text and shape", text_and_shape()), ("three", rects(3)), ("arrow", arrow())] {
                // The drawer is open, the biggest thing the card must keep out of.
                f.state.apply_verb(Verb::ToggleLibrary, &mut Vec::new());
                f.model.wants_library = true;
                f.refresh();
                let h = at(vec2(w, height), theme, f);
                let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(w, height));
                let c = chrome(&h);
                let card = c.ribbon.unwrap_or_else(|| panic!("{name} at {w}x{height}: no card"));
                let here = format!("{name} {theme:?} at {w}x{height}: card {card:?}");
                assert!(screen.contains_rect(card), "{here} is inside the window");
                assert!(card.min.y >= c.strip.unwrap().max.y, "{here} is under the strip");
                for (other, r) in
                    [("tool column", c.tools), ("bottom bar", c.bottom_bar), ("tabs", c.tabs), ("library", c.library)]
                {
                    if let Some(r) = r {
                        assert!(!card.intersects(r), "{here} overlaps the {other} {r:?}");
                    }
                }
            }
        }
    }
}

#[test]
fn a_wide_card_wraps_into_rows_and_a_narrow_window_scrolls_one_row() {
    let wide = desk(free_text());
    let card = chrome(&wide).ribbon.unwrap();
    assert!(card.height() > 60.0, "text at 1280 needs two rows: {card:?}");
    let narrow = at(vec2(390.0, 844.0), egui::Theme::Light, free_text());
    let card = chrome(&narrow).ribbon.unwrap();
    assert!(card.height() < 40.0 + 2.0 * 12.0, "one row on a phone: {card:?}");
    assert!(card.width() <= 390.0 - 2.0 * 12.0 + 0.5, "{card:?}");
}

#[test]
fn the_card_follows_the_selection_without_moving_the_strip_row() {
    let mut h = desk(dialog::fixture());
    let before = (rect(&h, "Stroke blue"), rect(&h, "Thin"));
    assert!(chrome(&h).ribbon.is_none());
    let a = add_rect(&mut h.state_mut().session, 0.0, 0.0, 50.0, 50.0).unwrap();
    select(&mut h.state_mut().session, &[a]);
    h.state_mut().refresh();
    h.run();
    assert!(chrome(&h).ribbon.is_some());
    assert_eq!((rect(&h, "Stroke blue"), rect(&h, "Thin")), before, "stroke, fill and width stay where they were");
    select(&mut h.state_mut().session, &[]);
    h.state_mut().refresh();
    h.run();
    assert!(chrome(&h).ribbon.is_none(), "and the card goes");
}

#[test]
fn the_buttons_of_the_card_have_the_height_of_their_layout() {
    let h = desk(one_rect());
    for label in ["Dashed", BRING_FRONT, "Duplicate"] {
        assert_eq!(rect(&h, label).height(), 28.0, "{label}");
    }
    let mut f = one_rect();
    f.state.layout = LayoutPreference::Touch;
    let h = desk(f);
    for label in ["Dashed", BRING_FRONT, "Duplicate"] {
        assert_eq!(rect(&h, label).height(), 40.0, "{label}");
    }
}

// ---- menus ----

#[test]
fn a_ribbon_menu_opens_under_its_button_and_closes_on_a_pick_a_click_outside_and_escape() {
    let mut h = desk(one_rect());
    open(&mut h, "More options");
    let button = rect(&h, "More options");
    let row = rect(&h, "Cartoonist");
    assert!(row.min.y >= button.max.y && row.min.x >= button.min.x - 1.0, "{row:?} under {button:?}");
    assert!(h.state().state.ribbon_menu.is_some());
    common::click(&mut h, "Cartoonist");
    h.run();
    assert!(h.state().state.ribbon_menu.is_none(), "closed by a pick");
    open(&mut h, "More options");
    dialog::click_canvas(&mut h);
    assert!(h.state().state.ribbon_menu.is_none(), "closed by a click outside");
    open(&mut h, "More options");
    assert!(h.state_mut().state.close_overlays(), "Esc closes it like any overlay");
    assert!(h.state().state.ribbon_menu.is_none());
}

#[test]
fn the_current_sloppiness_is_checked_in_the_menu() {
    let mut f = one_rect();
    dialog::run(&mut f, "style.set", json!({ "props": { "sloppiness": "drunk" } }));
    let mut h = desk(f);
    open(&mut h, "More options");
    let on =
        |h: &H, label: &str| h.get_by_label(label).accesskit_node().toggled() == Some(egui::accesskit::Toggled::True);
    assert!(on(&h, "Drunk") && !on(&h, "Artist"));
}

#[test]
fn the_font_popover_opens_under_the_font_button() {
    let mut h = desk(free_text());
    open(&mut h, FONT);
    assert!(h.state().state.font_picker_open);
    let button = rect(&h, FONT);
    let row = rect(&h, "GelPen Serif");
    assert!(row.min.y >= button.max.y, "{row:?} under {button:?}");
}

#[test]
fn the_dropdown_shows_the_family_in_use() {
    let mut f = free_text();
    dialog::run(&mut f, "style.set", json!({ "props": { "font_family": "vanilla_extract" } }));
    let h = desk(f);
    h.get_by_label(FONT);
    assert_eq!(h.state().model.style.str_value("font_family"), Some("vanilla_extract"));
}

// ---- themes ----

#[test]
fn the_card_is_drawn_in_the_dark_theme_with_the_same_controls() {
    let light = desk(rects(3));
    let dark = at(vec2(1280.0, 800.0), egui::Theme::Dark, rects(3));
    assert_eq!(chrome(&light).ribbon, chrome(&dark).ribbon, "the same layout");
    for label in [ALIGN_LEFT, BRING_FRONT, "Dashed"] {
        assert!(has(&dark, label), "{label}");
    }
}
