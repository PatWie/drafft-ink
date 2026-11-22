//! Tests of `tool.set_marker`.

use drafftink_scene::MarkerColor;
use serde_json::{Value, json};

use crate::ring::Origin;
use crate::testing;

#[test]
fn the_marker_starts_yellow_and_none_is_a_colour_of_its_own() {
    let mut s = testing::headless_session();
    assert_eq!(s.tool.marker, Some(MarkerColor::YELLOW));
    let out = s.execute_from(Origin::Test, "tool.set_marker", json!({ "color": "pink" })).unwrap();
    assert_eq!((out["changed"].clone(), out["color"].clone()), (json!(true), json!("pink-400")));
    let out = s.execute_from(Origin::Test, "tool.set_marker", json!({ "color": null })).unwrap();
    assert_eq!((out["changed"].clone(), out["color"].clone()), (json!(true), Value::Null));
    assert_eq!(s.tool.marker, None);
    let again = s.execute_from(Origin::Test, "tool.set_marker", json!({})).unwrap();
    assert_eq!(again["changed"], json!(false));
}

#[test]
fn it_is_tool_state_and_makes_no_undo_step_and_an_unknown_colour_is_refused() {
    let mut s = testing::headless_session();
    let depth = s.tab().doc.undo_depth();
    s.execute_from(Origin::Test, "tool.set_marker", json!({ "color": "blue" })).unwrap();
    assert_eq!(s.tab().doc.undo_depth(), depth);
    let r = s.execute_from(Origin::Test, "tool.set_marker", json!({ "color": "purple" })).unwrap_err();
    assert_eq!(r.code, "bad_args", "{}", r.message);
    assert!(r.message.contains("purple"), "{}", r.message);
}

#[test]
fn any_palette_or_custom_colour_becomes_the_nearest_marker_and_the_answer_says_which() {
    let mut s = testing::headless_session();
    for (given, want) in [
        ("blue-50", "blue-300"),
        ("blue-300", "blue-300"),
        ("rose-400", "rose-400"),
        ("emerald-900", "emerald-500"),
        ("#ef4444", "red-500"),
    ] {
        let out = s.execute_from(Origin::Test, "tool.set_marker", json!({ "color": given })).unwrap();
        assert_eq!(out["color"], json!(want), "{given}");
        assert_eq!(s.tool.marker, MarkerColor::from_name(want), "{given}");
    }
    let r = s.execute_from(Origin::Test, "tool.set_marker", json!({ "color": "#12" })).unwrap_err();
    assert_eq!(r.code, "bad_args", "{}", r.message);
    assert_eq!(s.tool.marker, MarkerColor::from_name("red-500"), "a refused pick changes nothing");
}

#[test]
fn picking_a_highlighter_colour_never_changes_the_stroke_colour_of_new_shapes() {
    let mut s = testing::headless_session();
    let stroke = s.tool.style_defaults.stroke_color;
    s.execute_from(Origin::Test, "tool.set_marker", json!({ "color": "green-300" })).unwrap();
    s.execute_from(Origin::Test, "tool.set_marker", json!({ "color": null })).unwrap();
    assert_eq!(s.tool.style_defaults.stroke_color, stroke);
}
