//! Tests of the highlighter tool: a drag highlights what the path touches, characters of a text included, with one
//! `highlight.apply` and so one undo step.

use drafftink_geom::kurbo::Point;
use drafftink_scene::{Element, ElementId, ElementKind, MarkerColor, SpanHighlight};
use drafftink_typeset::Affinity;
use serde_json::{Value, json};

use crate::interact::Key;
use crate::interact::testutil::Rig;
use crate::services::Services;
use crate::state::Tool;

fn marker_rig() -> Rig {
    let mut rig = Rig::new();
    rig.tool(Tool::Highlighter);
    rig
}

/// The highlight of the whole element.
fn band(rig: &Rig, id: &ElementId) -> Option<MarkerColor> {
    rig.element(id).style.highlight
}

/// The characters of a text that have a highlight of their own: `[start, end, colour]`.
fn chars(rig: &Rig, id: &ElementId) -> Vec<(u32, u32, MarkerColor)> {
    let ElementKind::Text(t) = rig.element(id).kind else { panic!("a text") };
    t.runs
        .iter()
        .filter_map(|r| match r.highlight {
            SpanHighlight::On(c) => Some((r.start, r.end, c)),
            _ => None,
        })
        .collect()
}

/// A text with the real text engine, at (100, 100).
fn text_rig(content: &str) -> (Rig, ElementId) {
    let mut rig = Rig::new();
    rig.s.services = Services::new();
    let made = rig.run("element.create", json!({ "kind": "text", "text": content, "at": [100, 100] }));
    let id = ElementId::new(made["id"].as_str().expect("an id")).expect("a valid id");
    rig.tool(Tool::Highlighter);
    (rig, id)
}

/// The point at the left edge of character `n` of the text, in the middle of its line.
fn edge(rig: &Rig, id: &ElementId, n: usize) -> Point {
    let e: Element = rig.element(id);
    let ElementKind::Text(t) = &e.kind else { panic!("a text") };
    let layout = rig.s.services.text_layout(t, e.transform.w).expect("a layout");
    let byte = t.content.char_indices().nth(n).map_or(t.content.len(), |(i, _)| i);
    let r = layout.caret_rect(layout.selection_at_index(byte, Affinity::Downstream).focus(), 0.0);
    Point::new(e.transform.x + r.x0, e.transform.y + (r.y0 + r.y1) / 2.0)
}

// ---- elements ---------------------------------------------------------------------------------------------------

#[test]
fn a_drag_across_elements_highlights_them_with_one_command_and_creates_nothing() {
    let mut rig = marker_rig();
    let a = rig.rect(100.0, 100.0, 80.0, 80.0);
    let b = rig.rect(300.0, 100.0, 80.0, 80.0);
    let far = rig.rect(600.0, 400.0, 80.0, 80.0);
    let (ring, undo) = (rig.ring_ids().len(), rig.undo_depth());
    rig.drag((50.0, 140.0), (450.0, 140.0));

    assert_eq!(rig.commands_since(ring), ["highlight.apply"]);
    assert_eq!(rig.undo_depth(), undo + 1, "the whole drag is one undo step");
    assert_eq!(band(&rig, &a), Some(MarkerColor::YELLOW));
    assert_eq!(band(&rig, &b), Some(MarkerColor::YELLOW));
    assert_eq!(band(&rig, &far), None, "what the path did not touch");
    assert_eq!(rig.elements().len(), 3, "the highlighter creates no element");
    assert_eq!(rig.s.tool.active, Tool::Highlighter, "and the tool stays");
    assert_eq!(rig.s.tool.marker, Some(MarkerColor::YELLOW));

    rig.run("history.undo", json!({}));
    assert_eq!((band(&rig, &a), band(&rig, &b)), (None, None), "one undo takes both back");
}

#[test]
#[allow(non_snake_case)]
fn exception_E19_a_highlighter_drag_equals_the_plain_command() {
    let mut drag = marker_rig();
    let (a, b) = (drag.rect(100.0, 100.0, 80.0, 80.0), drag.rect(300.0, 100.0, 80.0, 80.0));
    drag.drag((50.0, 140.0), (450.0, 140.0));
    let mut plain = Rig::new();
    let (pa, pb) = (plain.rect(100.0, 100.0, 80.0, 80.0), plain.rect(300.0, 100.0, 80.0, 80.0));
    plain.run("highlight.apply", json!({ "ids": [pa.as_str(), pb.as_str()], "color": "yellow" }));
    assert_eq!(drag.normalized_doc(), plain.normalized_doc());
    assert_eq!((band(&drag, &a), band(&drag, &b)), (Some(MarkerColor::YELLOW), Some(MarkerColor::YELLOW)));
}

#[test]
fn the_marker_colour_is_the_colour_of_the_drag() {
    let mut rig = marker_rig();
    let a = rig.rect(100.0, 100.0, 80.0, 80.0);
    rig.run("tool.set_marker", json!({ "color": "pink" }));
    rig.drag((50.0, 140.0), (150.0, 140.0));
    assert_eq!(band(&rig, &a), Some(MarkerColor::from_name("pink-400").unwrap()));
}

#[test]
fn a_drag_that_touches_nothing_runs_no_command() {
    let mut rig = marker_rig();
    rig.rect(100.0, 100.0, 80.0, 80.0);
    let (ring, undo) = (rig.ring_ids().len(), rig.undo_depth());
    rig.drag((300.0, 300.0), (500.0, 320.0));
    assert_eq!(rig.commands_since(ring), Vec::<String>::new());
    assert_eq!(rig.undo_depth(), undo);
    assert!(rig.parts().marker.is_none());
}

#[test]
fn the_drag_shows_a_trail_and_the_elements_it_would_highlight_and_takes_them_back() {
    let mut rig = marker_rig();
    let a = rig.rect(100.0, 100.0, 80.0, 80.0);
    let undo = rig.undo_depth();
    rig.down(50.0, 140.0);
    rig.drag_to(150.0, 140.0);
    rig.drag_to(250.0, 140.0);
    let shown = rig.parts().marker.clone().expect("a preview");
    assert!(shown.trail.len() >= 3 && shown.trail.first() == Some(&Point::new(50.0, 140.0)), "{:?}", shown.trail);
    assert_eq!(shown.color, Some(MarkerColor::YELLOW));
    assert_eq!(rig.parts().highlight, std::slice::from_ref(&a), "the rectangle is outlined before the release");
    assert_eq!(band(&rig, &a), None, "nothing is written while the pointer is down");
    assert_eq!(rig.undo_depth(), undo);
    rig.up(250.0, 140.0);
    assert!(rig.parts().marker.is_none() && rig.parts().highlight.is_empty());
}

#[test]
fn escape_cancels_the_drag_without_a_trace() {
    let mut rig = marker_rig();
    let a = rig.rect(100.0, 100.0, 80.0, 80.0);
    let undo = rig.undo_depth();
    rig.down(50.0, 140.0);
    rig.drag_to(150.0, 140.0);
    assert!(rig.key(Key::Escape).handled);
    rig.up(150.0, 140.0);
    assert_eq!((band(&rig, &a), rig.undo_depth()), (None, undo));
}

// ---- removing ---------------------------------------------------------------------------------------------------

#[test]
fn with_none_selected_the_drag_removes_the_highlight() {
    let mut rig = marker_rig();
    let a = rig.rect(100.0, 100.0, 80.0, 80.0);
    let b = rig.rect(300.0, 100.0, 80.0, 80.0);
    rig.drag((50.0, 140.0), (450.0, 140.0));
    rig.run("tool.set_marker", json!({ "color": null }));
    let undo = rig.undo_depth();
    rig.drag((50.0, 140.0), (200.0, 140.0));
    assert_eq!((band(&rig, &a), band(&rig, &b)), (None, Some(MarkerColor::YELLOW)));
    assert_eq!(rig.undo_depth(), undo + 1, "removing is one undo step too");
    rig.run("history.undo", json!({}));
    assert_eq!(band(&rig, &a), Some(MarkerColor::YELLOW), "and undo puts it back");
}

#[test]
fn alt_removes_for_the_length_of_the_drag_whatever_the_colour() {
    let mut rig = marker_rig();
    let a = rig.rect(100.0, 100.0, 80.0, 80.0);
    rig.drag((50.0, 140.0), (150.0, 140.0));
    assert_eq!(band(&rig, &a), Some(MarkerColor::YELLOW));
    rig.mods.alt = true;
    rig.down(50.0, 140.0);
    rig.drag_to(150.0, 140.0);
    assert_eq!(rig.parts().marker.as_ref().map(|m| m.color), Some(None), "the preview shows a removal");
    rig.up(150.0, 140.0);
    rig.mods.alt = false;
    assert_eq!(band(&rig, &a), None);
    assert_eq!(rig.s.tool.marker, Some(MarkerColor::YELLOW), "the colour of the tool is not touched");
}

// ---- text -------------------------------------------------------------------------------------------------------

#[test]
fn a_drag_along_a_text_highlights_only_the_characters_it_crosses() {
    let (mut rig, id) = text_rig("hello big world");
    let (from, to) = (edge(&rig, &id, 6), edge(&rig, &id, 9));
    let ring = rig.ring_ids().len();
    rig.drag((from.x, from.y), (to.x, to.y));
    assert_eq!(rig.commands_since(ring), ["highlight.apply"]);
    let args = rig.ring().last().expect("a ring entry").args.clone();
    let args: Value = serde_json::from_str(&args).expect("json");
    assert_eq!(
        args["ranges"],
        json!([{ "id": id.as_str(), "start": 6, "end": 9 }]),
        "`big`, and not the rest of the text"
    );
    assert!(args.get("ids").is_none(), "no element is named, so the command does not fall back to the selection");
    assert_eq!(band(&rig, &id), None, "the text as a whole is not highlighted");
    assert_eq!(chars(&rig, &id), [(6, 9, MarkerColor::YELLOW)]);
}

#[test]
fn a_drag_shows_bands_over_the_characters_before_it_ends() {
    let (mut rig, id) = text_rig("hello big world");
    let (from, to) = (edge(&rig, &id, 6), edge(&rig, &id, 9));
    rig.down(from.x, from.y);
    rig.drag_to(to.x, to.y);
    let shown = rig.parts().marker.clone().expect("a preview");
    assert_eq!(shown.bands.len(), 1, "one line, one band");
    let xs: Vec<f64> = shown.bands[0].iter().map(|p| p.x).collect();
    let (lo, hi) = (xs.iter().copied().fold(f64::MAX, f64::min), xs.iter().copied().fold(f64::MIN, f64::max));
    assert!(
        (lo - from.x).abs() < 1.0 && (hi - to.x).abs() < 1.0,
        "the band is over `big`: {lo}..{hi} vs {}..{}",
        from.x,
        to.x
    );
    assert!(chars(&rig, &id).is_empty(), "still only shown");
    rig.up(to.x, to.y);
}

#[test]
fn removing_from_a_text_takes_off_only_the_crossed_characters() {
    let (mut rig, id) = text_rig("hello big world");
    let (a, b, c) = (edge(&rig, &id, 0), edge(&rig, &id, 5), edge(&rig, &id, 9));
    rig.drag((a.x, a.y), (b.x, b.y));
    rig.drag((b.x + 0.5, b.y), (c.x, c.y));
    assert_eq!(chars(&rig, &id), [(0, 9, MarkerColor::YELLOW)], "two drags that meet are one highlight");
    rig.mods.alt = true;
    let undo = rig.undo_depth();
    rig.drag((a.x, a.y), (b.x, b.y));
    rig.mods.alt = false;
    assert_eq!(rig.undo_depth(), undo + 1);
    assert_eq!(chars(&rig, &id), [(5, 9, MarkerColor::YELLOW)], "the first five characters lost theirs");
}

#[test]
fn a_drag_over_a_text_and_a_shape_is_one_command_for_both() {
    let (mut rig, id) = text_rig("hello big world");
    let shape = rig.rect(100.0, 200.0, 200.0, 80.0);
    let (from, to) = (edge(&rig, &id, 2), edge(&rig, &id, 4));
    let ring = rig.ring_ids().len();
    let undo = rig.undo_depth();
    rig.trace(&[(from.x, from.y), (to.x, to.y), (to.x, 240.0), (200.0, 240.0)]);
    assert_eq!(rig.commands_since(ring), ["highlight.apply"]);
    assert_eq!(rig.undo_depth(), undo + 1);
    assert_eq!(band(&rig, &shape), Some(MarkerColor::YELLOW));
    assert_eq!(chars(&rig, &id), [(2, 4, MarkerColor::YELLOW)]);
}

#[test]
fn a_path_just_beside_a_text_crosses_nothing() {
    let (mut rig, id) = text_rig("hello big world");
    let (from, to) = (edge(&rig, &id, 0), edge(&rig, &id, 9));
    let ring = rig.ring_ids().len();
    rig.drag((from.x, from.y + 60.0), (to.x, to.y + 60.0));
    assert_eq!(rig.commands_since(ring), Vec::<String>::new());
}
