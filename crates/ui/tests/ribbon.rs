#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The contextual groups of the top strip: which groups show for what is selected or drawn, how they are stacked
//! in the strip's two rows, how they overflow into "More" in a small window and how they look in the dark theme.
//! The controls are found by their accessible names; what a click runs is in `surfaces.rs`, `text_props.rs` and
//! `refusals.rs`.

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
fn with_nothing_selected_and_a_tool_that_draws_nothing_there_only_the_colour_group_shows() {
    // Decision: the colour group stays (the defaults for new elements are what people reach for first, and the
    // strip keeps its height); nothing else, in particular no width, which has no meaning without a stroke.
    for tool in ["select", "lasso", "hand", "eraser", "laser"] {
        let h = desk(with_tool(tool));
        assert!(!has(&h, "More options") && !has(&h, "Dashed") && !has(&h, "Thin"), "{tool}");
        h.get_by_label("Stroke blue");
        h.get_by_label("Fill blue");
        h.get_by_label("Pick the fill colour");
    }
}

#[test]
fn each_drawing_tool_shows_the_groups_of_its_new_elements() {
    let h = desk(with_tool("rectangle"));
    assert!(has(&h, "Dashed") && has(&h, CORNERS) && has(&h, "More options") && has(&h, "Opacity") && has(&h, "Thin"));
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
    assert!(has(&h, "Thin"), "a freehand stroke has a width");
    let h = desk(with_tool("highlighter"));
    assert!(has(&h, "Brush: Normal"));
    for tool in ["text", "math"] {
        let h = desk(with_tool(tool));
        assert!(has(&h, FONT) && has(&h, TEXT_SIZE) && has(&h, "Text align: Left"), "{tool}");
        assert!(has(&h, "Heavy") && has(&h, "More options"), "{tool}: weight, line height");
        assert!(!has(&h, "Dashed") && !has(&h, CORNERS) && !has(&h, BRING_FRONT) && !has(&h, "Thin"), "{tool}");
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
    for label in
        [FONT, TEXT_SIZE, ALIGN_LEFT, "Path: Elbow", "Start head", "Brush: Normal", PATTERN, "Vertical align: Top"]
    {
        assert!(!has(&h, label), "{label}");
    }
    assert!(has(&h, "Thin") && has(&h, "Extra bold"), "a rectangle has a width");
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
    for label in ["Dashed", CORNERS, "Path: Sharp", "Thin"] {
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
fn the_head_buttons_show_the_heads_of_the_selected_arrow() {
    let mut f = dialog::fixture();
    let a = create(
        &mut f,
        json!({ "kind": "arrow", "points": [[0, 0], [80, 30]], "start_head": "circle", "end_head": "triangle" }),
    );
    selecting(&mut f, &[a]);
    let mut h = desk(f);
    let heads = h.state().model.style.heads.clone().expect("a selected arrow has heads");
    assert_eq!((heads.start.as_str(), heads.end.as_str()), ("circle", "triangle"));
    let on =
        |h: &H, label: &str| h.get_by_label(label).accesskit_node().toggled() == Some(egui::accesskit::Toggled::True);
    open(&mut h, "Start head");
    assert!(on(&h, "Start head: Dot") && !on(&h, "Start head: None"), "the start head is checked in its menu");
    common::click(&mut h, "Start head: Dot");
    h.run();
    open(&mut h, "End head");
    assert!(on(&h, "End head: Triangle") && !on(&h, "End head: Arrow"), "and so is the end head");
}

#[test]
fn two_arrows_with_different_heads_say_mixed() {
    let mut f = dialog::fixture();
    let a = create(&mut f, json!({ "kind": "arrow", "points": [[0, 0], [80, 30]], "end_head": "triangle" }));
    let b = create(&mut f, json!({ "kind": "arrow", "points": [[0, 100], [80, 130]], "end_head": "bar" }));
    selecting(&mut f, &[a, b]);
    let h = desk(f);
    let heads = h.state().model.style.heads.clone().unwrap();
    assert_eq!((heads.start.as_str(), heads.end.as_str()), ("none", "mixed"));
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
    // Changed with the strip: at 1280 the layer and action groups of this selection do not fit beside the text
    // groups any more and wait behind "More" (see the overflow tests); they used to hang in the card's second row.
    let mut h = desk(text_and_shape());
    for label in [FONT, TEXT_SIZE, "Text align: Left", "Dashed", CORNERS, ALIGN_LEFT, "Bold text"] {
        assert!(has(&h, label), "{label}");
    }
    assert!(!has(&h, "Path: Elbow"));
    open(&mut h, "More options");
    h.get_by_label(BRING_FRONT);
}

#[test]
fn a_labelled_shape_offers_the_vertical_align_and_bold_of_its_label() {
    let mut f = dialog::fixture();
    let r = add_rect(&mut f.session, 0.0, 0.0, 200.0, 120.0).unwrap();
    add_text_label(&mut f.session, &r, "Hi").unwrap();
    selecting(&mut f, &[r]);
    let mut h = desk(f);
    assert!(has(&h, "Bold text") && has(&h, FONT));
    // The vertical align is a row of the text group now, no longer in "More".
    h.get_by_label("Vertical align: Middle");
    common::click(&mut h, "Vertical align: Bottom");
    let calls = h.state().calls().into_iter().cloned().collect::<Vec<_>>();
    assert_eq!(
        calls.last().map(|c| (c.id.as_str(), &c.args)),
        Some(("style.set", &json!({ "props": { "vertical_align": "bottom" } })))
    );
}

// ---- the strip ----

/// The height of the strip: two rows of 28 points, the gap between them, and 6 points above and below.
const STRIP_HEIGHT: f32 = 2.0 * 28.0 + 4.0 + 2.0 * 6.0;

#[test]
fn the_strip_has_the_same_height_whatever_is_selected() {
    let states: [(&str, Fixture); 6] = [
        ("empty", dialog::fixture()),
        ("rectangle", one_rect()),
        ("text", free_text()),
        ("arrow", arrow()),
        ("three shapes", rects(3)),
        ("freehand tool", with_tool("freehand")),
    ];
    for (name, f) in states {
        let h = desk(f);
        let strip = chrome(&h).strip.unwrap();
        assert_eq!(strip.height(), STRIP_HEIGHT, "{name}: {strip:?}");
        assert_eq!(strip.width(), 1280.0, "{name}");
    }
}

#[test]
fn the_width_group_comes_with_a_stroke_to_set() {
    let none = desk(dialog::fixture());
    assert!(!has(&none, "Thin") && !has(&none, "Solid"), "nothing selected and the select tool: no width");
    for (name, f) in
        [("rectangle", one_rect()), ("arrow", arrow()), ("freehand", freehand()), ("tool", with_tool("line"))]
    {
        let h = desk(f);
        assert!(has(&h, "Thin") && has(&h, "Extra bold"), "{name}");
    }
    assert!(!has(&desk(free_text()), "Thin"), "text has no stroke width");
}

#[test]
fn the_colour_rows_are_stacked_and_their_swatches_start_at_the_same_x() {
    let h = desk(one_rect());
    let (stroke, fill) = (rect(&h, "Stroke"), rect(&h, "Fill"));
    assert_eq!(stroke.min.x, fill.min.x, "the labels start together: {stroke:?} {fill:?}");
    assert!(fill.min.y >= stroke.max.y, "fill is the row under stroke: {stroke:?} over {fill:?}");
    // The first swatch of both rows: "no fill" under the first stroke colour.
    let first_stroke = rect(&h, "Stroke blue");
    let no_fill = rect(&h, "No fill");
    assert_eq!(first_stroke.min.x, no_fill.min.x, "swatch columns line up");
    assert!(no_fill.center().y > first_stroke.center().y + 20.0);
    let (stroke_wheel, fill_wheel) = (rect(&h, "Pick the stroke colour"), rect(&h, "Pick the fill colour"));
    assert!(fill_wheel.center().y > stroke_wheel.center().y + 20.0, "{stroke_wheel:?} over {fill_wheel:?}");
}

#[test]
fn the_groups_lie_right_of_the_colour_group_on_one_band_in_two_rows() {
    let h = desk(rects(3));
    let strip = chrome(&h).strip.unwrap();
    let colour_end = rect(&h, "Pick the stroke colour").max.x.max(rect(&h, "Pick the fill colour").max.x);
    // Row one: the first control of each group. Row two: the second row of each group.
    let first = ["Thin", CORNERS, "Opacity", BRING_FRONT, "Duplicate"];
    let second = ["Dashed", ALIGN_LEFT, "Flip horizontally", "Delete"];
    for label in first.iter().chain(&second) {
        let r = rect(&h, label);
        assert!(r.min.x > colour_end, "{label} {r:?} is right of the colour group (ends {colour_end})");
        assert!(strip.contains_rect(r), "{label} {r:?} is in the strip {strip:?}");
    }
    let ys = |labels: &[&str]| labels.iter().map(|l| rect(&h, l).center().y).collect::<Vec<_>>();
    let (top, bottom) = (ys(&first), ys(&second));
    assert!(top.iter().all(|y| (y - top[0]).abs() < 1.0), "one top row: {top:?}");
    assert!(bottom.iter().all(|y| (y - bottom[0]).abs() < 1.0), "one bottom row: {bottom:?}");
    assert!(bottom[0] - top[0] > 28.0, "the second row is under the first: {top:?} {bottom:?}");
    // The colour rows are the same two rows.
    assert!((rect(&h, "Stroke blue").center().y - top[0]).abs() < 1.0);
    assert!((rect(&h, "No fill").center().y - bottom[0]).abs() < 1.0);
}

#[test]
fn the_groups_keep_their_order_from_left_to_right() {
    let h = desk(rects(3));
    let labels = ["Thin", CORNERS, "Opacity", ALIGN_LEFT, BRING_FRONT, "Duplicate", "Delete"];
    let xs: Vec<f32> = labels.iter().map(|l| rect(&h, l).min.x).collect();
    assert!(xs.windows(2).all(|w| w[0] <= w[1]), "{labels:?} at {xs:?}");
    assert!(xs[0] < xs[1] && xs[1] < xs[2] && xs[3] < xs[5], "and the groups are apart: {xs:?}");
}

#[test]
fn text_selection_packs_its_groups_left_to_right_in_two_rows() {
    let h = desk(free_text());
    let strip = chrome(&h).strip.unwrap();
    let labels = [FONT, "Text size S (16 px)", "Text align: Left", "Opacity"];
    let xs: Vec<f32> = labels.iter().map(|l| rect(&h, l).min.x).collect();
    assert!(xs.windows(2).all(|w| w[0] < w[1]), "{labels:?} at {xs:?}");
    // Weight is under the family, bold under the size, on the same x.
    assert_eq!(rect(&h, "Light").min.x, rect(&h, FONT).min.x);
    assert_eq!(rect(&h, "Bold text").min.x, rect(&h, "Text size S (16 px)").min.x);
    assert!(rect(&h, "Light").min.y >= rect(&h, FONT).max.y - 1.0);
    for label in labels {
        assert!(strip.contains_rect(rect(&h, label)), "{label}");
    }
}

#[test]
fn corners_are_icons_with_the_same_height_as_the_other_buttons() {
    let h = desk(one_rect());
    for label in ["Corners: Sharp", CORNERS] {
        let r = rect(&h, label);
        assert_eq!((r.width(), r.height()), (28.0, 28.0), "{label} is a square icon button");
    }
}

#[test]
fn the_buttons_of_the_strip_have_the_height_of_their_layout() {
    let h = desk(one_rect());
    for label in ["Dashed", BRING_FRONT, "Duplicate", "Thin"] {
        assert_eq!(rect(&h, label).height(), 28.0, "{label}");
    }
    let mut f = one_rect();
    f.state.layout = LayoutPreference::Touch;
    let h = desk(f);
    for label in ["Dashed", "Thin", CORNERS] {
        assert_eq!(rect(&h, label).height(), 40.0, "{label}");
    }
}

#[test]
fn the_strip_stays_inside_the_window_and_clear_of_the_other_chrome_at_every_size() {
    let sizes = [(1280.0, 800.0), (820.0, 1180.0), (390.0, 844.0), (1180.0, 820.0)];
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        for (w, height) in sizes {
            for (name, mut f) in [("text and shape", text_and_shape()), ("three", rects(3)), ("arrow", arrow())] {
                // The drawer is open, the biggest thing the strip must keep out of.
                f.state.apply_verb(Verb::ToggleLibrary, &mut Vec::new());
                f.model.wants_library = true;
                f.refresh();
                let h = at(vec2(w, height), theme, f);
                let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(w, height));
                let c = chrome(&h);
                let strip = c.strip.unwrap();
                let here = format!("{name} {theme:?} at {w}x{height}: strip {strip:?}");
                assert!(screen.contains_rect(strip), "{here} is inside the window");
                for (other, r) in
                    [("tool column", c.tools), ("bottom bar", c.bottom_bar), ("tabs", c.tabs), ("library", c.library)]
                {
                    if let Some(r) = r {
                        assert!(!strip.intersects(r), "{here} overlaps the {other} {r:?}");
                    }
                }
            }
        }
    }
}

// ---- overflow ----

/// The controls of the strip that are in the window, by role: everything the person can press up there.
fn in_the_strip(h: &H) -> Vec<Rect> {
    let strip = chrome(h).strip.unwrap();
    h.query_all_by_role(egui::accesskit::Role::Button)
        .map(|n| n.rect())
        .filter(|r| r.is_positive() && r.max.y <= strip.max.y + 0.5)
        .collect()
}

#[test]
fn what_does_not_fit_goes_behind_more_from_the_right_end_and_is_reachable_there() {
    for width in [1024.0, 820.0] {
        let mut h = at(vec2(width, 800.0), egui::Theme::Light, text_and_shape());
        let strip = chrome(&h).strip.unwrap();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(width, 800.0));
        assert_eq!(strip.height(), STRIP_HEIGHT, "the strip does not grow at {width}");
        assert!(has(&h, "More options"), "{width}");
        assert!(!has(&h, "Delete"), "the actions are the last group, so they go first at {width}");
        assert!(has(&h, "Dashed") || width < 1000.0, "the first group stays at {width}");
        for r in in_the_strip(&h) {
            assert!(screen.contains_rect(r), "{r:?} is inside the window at {width}");
        }
        open(&mut h, "More options");
        let delete = rect(&h, "Delete");
        let button = rect(&h, "More options");
        assert!(delete.min.y >= strip.max.y, "the popover is under the strip: {delete:?} at {width}");
        assert!(delete.max.x <= button.max.x + 1.0 && screen.contains_rect(delete), "{delete:?} at {width}");
        let calls = common::click(&mut h, "Delete");
        assert_eq!(calls.last().map(|c| c.id.as_str()), Some("element.delete"), "{width}");
    }
}

#[test]
fn a_window_that_is_wide_enough_has_no_overflow_for_the_properties_of_one_shape() {
    let h = desk(one_rect());
    assert!(has(&h, "Delete") && has(&h, BRING_FRONT) && has(&h, "Duplicate"));
}

#[test]
fn the_overflow_keeps_the_groups_in_order_and_hides_only_a_tail() {
    let widths = [1280.0, 1100.0, 1024.0, 900.0, 820.0];
    let order = ["Thin", "Corners: Sharp", FONT, TEXT_SIZE, "Text align: Left", "Opacity", BRING_FRONT, "Delete"];
    let mut last_hidden = 0;
    for width in widths {
        let h = at(vec2(width, 800.0), egui::Theme::Light, text_and_shape());
        let shown: Vec<bool> = order.iter().map(|l| has(&h, l)).collect();
        let first_hidden = shown.iter().position(|s| !s).unwrap_or(shown.len());
        assert!(shown[first_hidden..].iter().all(|s| !s), "only a tail is hidden at {width}: {shown:?}");
        assert!(first_hidden <= order.len() && (width == 1280.0 || first_hidden >= 1), "{width}: {shown:?}");
        assert!(order.len() - first_hidden >= last_hidden, "a narrower window never shows more: {width}");
        last_hidden = order.len() - first_hidden;
    }
}

#[test]
fn a_phone_keeps_one_row_and_puts_the_groups_behind_more() {
    let mut h = at(vec2(390.0, 844.0), egui::Theme::Light, free_text());
    let strip = chrome(&h).strip.unwrap();
    assert_eq!(strip.height(), 40.0 + 2.0 * 6.0, "one row of finger-sized controls");
    assert!(!has(&h, FONT) && !has(&h, "Duplicate"), "nothing contextual in the strip");
    for r in in_the_strip(&h) {
        assert!(strip.contains_rect(r) && r.max.x <= 390.0, "{r:?}");
    }
    open(&mut h, "More options");
    h.get_by_label(FONT);
    h.get_by_label("Duplicate");
    assert!(rect(&h, FONT).min.y >= strip.max.y, "under the strip");
    assert!(rect(&h, "Duplicate").max.x <= 390.0);
}

// ---- menus ----

#[test]
fn a_ribbon_menu_opens_under_its_button_and_closes_on_a_pick_a_click_outside_and_escape() {
    let mut h = desk(one_rect());
    open(&mut h, "More options");
    let button = rect(&h, "More options");
    let strip = chrome(&h).strip.unwrap();
    let row = rect(&h, "Cartoonist");
    // The button is at the right end of the strip, so the menu hangs from its right edge, under the strip.
    assert!(row.min.y >= strip.max.y && row.max.x <= button.max.x + 1.0, "{row:?} under {strip:?}, {button:?}");
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
fn the_strip_is_drawn_in_the_dark_theme_with_the_same_layout() {
    let light = desk(rects(3));
    let dark = at(vec2(1280.0, 800.0), egui::Theme::Dark, rects(3));
    assert_eq!(chrome(&light).strip, chrome(&dark).strip, "the same layout");
    for label in [ALIGN_LEFT, BRING_FRONT, "Dashed", "Thin"] {
        assert_eq!(rect(&light, label), rect(&dark, label), "{label}");
    }
}
