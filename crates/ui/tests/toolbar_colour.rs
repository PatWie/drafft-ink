#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The bar under the highlighter icon in the left toolbar: a neutral grey, or with "Colourful toolbar" the ink of the
//! marker that the tool has, on the theme in use.

mod common;
mod dialog;

use drafftink_scene::{MarkerColor, Tone};
use egui::{Color32, Rect, Shape};
use egui_kittest::Harness;
use serde_json::json;

use common::Fixture;

type H = Harness<'static, Fixture>;

fn toolbar(colourful: bool, marker: Option<&str>, theme: egui::Theme) -> H {
    let mut f = dialog::fixture();
    dialog::run(&mut f, "settings.set", json!({ "key": "ui.colourful_toolbar", "value": colourful }));
    dialog::run(&mut f, "tool.set_marker", json!({ "color": marker }));
    f.refresh();
    let mut h = dialog::sized(f, egui::vec2(1280.0, 800.0), theme);
    h.run();
    h
}

fn filled_rects(shape: &Shape, out: &mut Vec<(Rect, Color32)>) {
    match shape {
        Shape::Vec(v) => v.iter().for_each(|s| filled_rects(s, out)),
        Shape::Rect(r) if r.fill != Color32::TRANSPARENT => out.push((r.rect, r.fill)),
        _ => {}
    }
}

/// The fill of the bar under the marker in the button of the highlighter: the flat filled rectangle inside the
/// button that is wide and low.
fn bar(h: &H) -> Color32 {
    let button = egui_kittest::kittest::Queryable::get_by_label(h, "Highlighter tool").rect();
    let mut all = Vec::new();
    h.output().shapes.iter().for_each(|s| filled_rects(&s.shape, &mut all));
    all.into_iter()
        .filter(|(r, _)| button.contains_rect(*r) && r.width() > 2.0 * r.height() && r.height() < button.height() * 0.2)
        .map(|(_, c)| c)
        .next_back()
        .expect("the highlighter button paints a bar")
}

fn grey(c: Color32) -> bool {
    c.r() == c.g() && c.g() == c.b()
}

#[test]
fn off_the_bar_is_a_neutral_grey_whatever_the_marker_in_both_themes() {
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        for marker in [Some("yellow-400"), Some("pink-300"), None] {
            let c = bar(&toolbar(false, marker, theme));
            assert!(grey(c), "{theme:?} {marker:?}: {c:?}");
        }
    }
}

#[test]
fn colourful_the_bar_is_the_ink_of_the_marker_on_this_theme() {
    for (theme, tone) in [(egui::Theme::Light, Tone::Light), (egui::Theme::Dark, Tone::Dark)] {
        for name in ["yellow-400", "pink-300", "slate-500"] {
            let ink = MarkerColor::from_name(name).unwrap().rgb(tone);
            let c = bar(&toolbar(true, Some(name), theme));
            assert_eq!((c.r(), c.g(), c.b()), (ink.r, ink.g, ink.b), "{theme:?} {name}");
        }
    }
}

#[test]
fn colourful_with_none_chosen_the_bar_is_the_neutral_grey() {
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        let c = bar(&toolbar(true, None, theme));
        assert!(grey(c), "{theme:?}: {c:?}");
    }
}
