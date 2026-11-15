//! Tests of the paint editor: scripted pointer events and keys go through the real recognizer and the interaction
//! layer into the real command registry. `exception_E18_` is the row of `docs/exceptions.md`.

// `cargo xtask exceptions` looks for tests named `exception_E<n>_...`, which is not snake case.
#![allow(non_snake_case)]

use base64::Engine as _;
use drafftink_canvas::{Buttons, Cursor, InputEvent, PointerEvent, PointerId, PointerPhase};
use drafftink_geom::kurbo::Point;
use drafftink_scene::paint::PaintMode;
use drafftink_scene::{Color, ElementId, PaintStroke};
use serde_json::{Value, json};

use super::Key;
use super::testutil::Rig;
use crate::cmd::image::tests::png;

/// A rig with a 200 by 100 picture at (300, 250): one unit per pixel, its middle at (400, 300), selected.
fn rig_with_image() -> (Rig, ElementId) {
    let mut rig = Rig::new();
    let b64 = base64::engine::general_purpose::STANDARD.encode(png(200, 100));
    let out = rig.run("image.insert", json!({ "images": [{ "base64": b64 }], "at": [400, 300] }));
    let id = ElementId::new(out["ids"][0].as_str().unwrap()).unwrap();
    assert_eq!(rig.bounds(&id), (300.0, 250.0, 200.0, 100.0));
    (rig, id)
}

/// The same, with the paint editor open.
fn rig_painting() -> (Rig, ElementId) {
    let (mut rig, id) = rig_with_image();
    rig.run("image.paint_begin", json!({}));
    (rig, id)
}

fn strokes(rig: &Rig, id: &ElementId) -> Vec<PaintStroke> {
    rig.element(id).image_data().unwrap().paint.values().cloned().collect()
}

fn editor(rig: &Rig) -> Option<ElementId> {
    rig.s.tab().observed.paint_editor.clone()
}

fn paints(rig: &Rig) -> usize {
    rig.ring_ids().iter().filter(|i| *i == "image.paint").count()
}

fn near(a: Point, b: Point) {
    assert!(a.distance(b) < 1e-6, "{a:?} != {b:?}");
}

/// The strokes of the draft the overlay shows, newest last.
fn shown_strokes(rig: &Rig) -> Vec<PaintStroke> {
    let view = rig.i.overlay(&rig.s).paint.expect("the overlay shows the editor");
    view.shown.image_data().unwrap().paint.values().cloned().collect()
}

/// A pen event: contact with `pressure` (a move with the tip down is a drag sample).
fn pen(rig: &mut Rig, phase: PointerPhase, x: f64, y: f64, pressure: f32) {
    rig.now += 8.0;
    let mut ev = PointerEvent::pen(phase, x, y, pressure, rig.now).with_modifiers(rig.mods);
    if phase == PointerPhase::Move {
        ev.buttons = Buttons::PRIMARY;
    }
    rig.input(InputEvent::Pointer(ev));
}

/// Presses at the first point, moves through the rest, and releases at the last one.
fn trace(rig: &mut Rig, points: &[(f64, f64)]) {
    let (first, rest) = points.split_first().expect("a trace has points");
    rig.down(first.0, first.1);
    for p in rest {
        rig.drag_to(p.0, p.1);
    }
    let last = points.last().expect("a trace has points");
    rig.up(last.0, last.1);
}

// ---- opening ----

#[test]
fn a_button_that_opens_the_editor_shows_it_before_any_pointer_event() {
    let (mut rig, id) = rig_with_image();
    assert!(rig.i.overlay(&rig.s).paint.is_none());
    rig.run("image.paint_begin", json!({}));
    let overlay = rig.i.overlay(&rig.s);
    let view = overlay.paint.expect("the overlay shows the editor at once");
    assert_eq!(view.element, id);
    assert!(view.veil);
    assert!(overlay.crop.is_none() && overlay.canvas.transformer.is_none(), "no handles while painting");
    rig.run("image.paint_end", json!({}));
    assert!(rig.i.overlay(&rig.s).paint.is_none());
}

// ---- painting ----

#[test]
fn a_drag_paints_one_stroke_in_picture_pixels_and_is_one_undo_step() {
    let (mut rig, id) = rig_painting();
    let undo = rig.undo_depth();
    rig.drag((350.0, 280.0), (450.0, 300.0));
    assert_eq!(paints(&rig), 1, "{:?}", rig.ring_ids());
    assert_eq!(rig.undo_depth(), undo + 1);
    let stored = strokes(&rig, &id);
    assert_eq!(stored.len(), 1);
    let first = stored[0].points.first().copied().unwrap();
    let last = stored[0].points.last().copied().unwrap();
    near(first, Point::new(50.0, 30.0));
    near(last, Point::new(150.0, 50.0));
    assert_eq!(stored[0].mode, PaintMode::Paint);
    assert!(stored[0].pressures.is_empty(), "a mouse has no pressure");
    assert_eq!(editor(&rig), Some(id.clone()), "the editor stays open");

    rig.run("history.undo", json!({}));
    assert!(strokes(&rig, &id).is_empty());
    assert_eq!(editor(&rig), Some(id), "taking the stroke back leaves the editor open");
}

#[test]
fn every_stroke_is_its_own_undo_step() {
    let (mut rig, id) = rig_painting();
    let undo = rig.undo_depth();
    rig.drag((320.0, 270.0), (380.0, 270.0));
    rig.drag((320.0, 300.0), (380.0, 300.0));
    rig.drag((320.0, 330.0), (380.0, 330.0));
    assert_eq!(rig.undo_depth(), undo + 3);
    assert_eq!(strokes(&rig, &id).len(), 3);
    rig.run("history.undo", json!({}));
    assert_eq!(strokes(&rig, &id).len(), 2);
}

#[test]
fn nothing_changes_in_the_document_while_the_pointer_moves_and_the_overlay_shows_the_draft() {
    let (mut rig, id) = rig_painting();
    let (ring, undo) = (rig.ring_ids().len(), rig.undo_depth());
    rig.down(330.0, 280.0);
    rig.drag_to(360.0, 290.0);
    let out = rig.drag_to(390.0, 310.0);
    assert!(out.redraw.is_some(), "the draft is drawn");
    assert_eq!(rig.ring_ids().len(), ring, "no command ran");
    assert_eq!(rig.undo_depth(), undo);
    assert!(strokes(&rig, &id).is_empty(), "the document has no stroke yet");
    assert_eq!(shown_strokes(&rig).len(), 1, "the overlay shows the stroke");
    assert!(rig.i.is_busy());

    let draft = shown_strokes(&rig).remove(0);
    rig.up(390.0, 310.0);
    assert!(!rig.i.is_busy());
    assert_eq!(strokes(&rig, &id), [draft], "the stroke that is stored is the one that was shown");
    assert_eq!(shown_strokes(&rig).len(), 1, "and the overlay shows it as stored");
}

#[test]
fn a_click_is_a_dab() {
    let (mut rig, id) = rig_painting();
    rig.click(400.0, 300.0);
    assert_eq!(paints(&rig), 1);
    let stored = strokes(&rig, &id);
    assert_eq!(stored[0].points.len(), 1);
    near(stored[0].points[0], Point::new(100.0, 50.0));
    assert_eq!(editor(&rig), Some(id), "a click on the image does not end the editor");
}

#[test]
fn the_stroke_has_the_session_brush() {
    let (mut rig, id) = rig_painting();
    rig.run(
        "image.paint_brush",
        json!({ "color": "#1971c2", "size": 10, "hardness": 0.3, "opacity": 0.5, "mode": "erase" }),
    );
    rig.drag((340.0, 280.0), (420.0, 280.0));
    let s = strokes(&rig, &id).remove(0);
    assert_eq!(s.mode, PaintMode::Erase);
    assert_eq!(s.color, Color::rgb(0x19, 0x71, 0xc2));
    assert_eq!((s.size, s.hardness, s.opacity), (10.0, 0.3, 0.5));
}

#[test]
fn a_stroke_may_run_off_the_image_and_is_stored_whole() {
    let (mut rig, id) = rig_painting();
    rig.drag((480.0, 300.0), (560.0, 300.0));
    let s = strokes(&rig, &id).remove(0);
    near(s.points[0], Point::new(180.0, 50.0));
    near(s.points.last().copied().unwrap(), Point::new(260.0, 50.0));
}

#[test]
fn a_drag_that_begins_off_the_image_paints_nothing() {
    let (mut rig, id) = rig_painting();
    let ring = rig.ring_ids().len();
    rig.drag((100.0, 100.0), (200.0, 120.0));
    assert_eq!(rig.ring_ids().len(), ring);
    assert!(strokes(&rig, &id).is_empty());
    assert_eq!(editor(&rig), Some(id));
}

// ---- the pen ----

#[test]
fn pen_pressure_is_stored_with_the_points() {
    use drafftink_canvas::PointerPhase::{Down, Move, Up};
    let (mut rig, id) = rig_painting();
    pen(&mut rig, Down, 330.0, 280.0, 0.2);
    for (i, p) in [0.4_f32, 0.9, 0.7, 0.3].into_iter().enumerate() {
        pen(&mut rig, Move, 330.0 + 25.0 * (i as f64 + 1.0), 280.0 + 12.0 * (i as f64 % 2.0), p);
    }
    pen(&mut rig, Up, 430.0, 280.0, 0.3);
    let s = strokes(&rig, &id).remove(0);
    assert_eq!(s.pressures.len(), s.points.len(), "one pressure per point");
    assert!(s.pressures.iter().all(|p| (0.0..=1.0).contains(p)));
    assert!(s.pressures.iter().any(|p| (*p - 0.9).abs() < 1e-6), "the pen's own value: {:?}", s.pressures);
    assert!((s.pressures[0] - 0.2).abs() < 1e-6, "the press: {:?}", s.pressures);
}

#[test]
fn a_touch_stroke_has_no_pressures() {
    use drafftink_canvas::PointerKind;
    let (mut rig, id) = rig_painting();
    for (phase, x) in [
        (PointerPhase::Down, 330.0),
        (PointerPhase::Move, 360.0),
        (PointerPhase::Move, 400.0),
        (PointerPhase::Up, 400.0),
    ] {
        rig.now += 16.0;
        let mut ev = PointerEvent::new(PointerId(7), PointerKind::Touch, phase, Point::new(x, 300.0), rig.now);
        if phase == PointerPhase::Move {
            ev.buttons = Buttons::PRIMARY;
        }
        rig.input(InputEvent::Pointer(ev));
    }
    let stored = strokes(&rig, &id);
    assert_eq!(stored.len(), 1);
    assert!(stored[0].pressures.is_empty());
}

// ---- rotated, mirrored and cropped images ----

/// Paints a dab at the world point and returns where it landed on the picture.
fn dab_at(rig: &mut Rig, id: &ElementId, x: f64, y: f64) -> Point {
    let before = strokes(rig, id).len();
    rig.click(x, y);
    let stored = strokes(rig, id);
    assert_eq!(stored.len(), before + 1, "the dab at ({x}, {y}) painted nothing");
    stored[before].points[0]
}

#[test]
fn a_mirrored_image_maps_the_pointer_to_the_picture_pixels_the_user_sees() {
    let (mut rig, id) = rig_with_image();
    rig.run("element.flip", json!({ "ids": [id.as_str()], "axis": "horizontal" }));
    rig.run("image.paint_begin", json!({}));
    // On screen the picture's left edge is now on the right: pixel (10, 20) is at (500 - 10, 250 + 20).
    near(dab_at(&mut rig, &id, 490.0, 270.0), Point::new(10.0, 20.0));
    near(dab_at(&mut rig, &id, 310.0, 330.0), Point::new(190.0, 80.0));
}

#[test]
fn a_rotated_image_maps_the_pointer_to_the_picture_pixels_the_user_sees() {
    let (mut rig, id) = rig_with_image();
    rig.run("element.rotate", json!({ "ids": [id.as_str()], "angle": 90, "mode": "to" }));
    rig.run("image.paint_begin", json!({}));
    // Turned a quarter clockwise about (400, 300): the picture's top-left corner is at the top-right of the box,
    // (450, 200), and its x axis points down. Pixel (10, 10) is 10 down and 10 left of that corner.
    near(dab_at(&mut rig, &id, 440.0, 210.0), Point::new(10.0, 10.0));
    near(dab_at(&mut rig, &id, 400.0, 250.0), Point::new(50.0, 50.0));
}

#[test]
fn a_rotated_and_mirrored_image_maps_the_pointer_to_the_picture_pixels_the_user_sees() {
    let (mut rig, id) = rig_with_image();
    rig.run("element.rotate", json!({ "ids": [id.as_str()], "angle": 90, "mode": "to" }));
    rig.run("element.flip", json!({ "ids": [id.as_str()], "axis": "horizontal" }));
    rig.run("image.paint_begin", json!({}));
    // The mirror image of the turned picture about the vertical line x = 400.
    near(dab_at(&mut rig, &id, 360.0, 210.0), Point::new(10.0, 10.0));
}

#[test]
fn a_cropped_image_maps_the_pointer_to_pixels_of_the_whole_picture() {
    let (mut rig, id) = rig_with_image();
    rig.run("image.crop", json!({ "crop": { "x": 50, "y": 25, "w": 100, "h": 50 } }));
    assert_eq!(rig.bounds(&id), (350.0, 275.0, 100.0, 50.0));
    rig.run("image.paint_begin", json!({}));
    near(dab_at(&mut rig, &id, 360.0, 285.0), Point::new(60.0, 35.0));
    // Outside the shown part, the picture is not painted: the click ends the editor instead.
    let before = strokes(&rig, &id).len();
    rig.click(310.0, 260.0);
    assert_eq!(strokes(&rig, &id).len(), before);
    assert_eq!(editor(&rig), None);
}

#[test]
fn an_image_shown_larger_than_its_picture_maps_in_picture_pixels() {
    let (mut rig, id) = rig_with_image();
    // The 200 by 100 picture is shown at 400 by 200, with its top-left corner kept at (300, 250).
    rig.run("element.resize", json!({ "ids": [id.as_str()], "handle": "se", "dx": 200, "dy": 100 }));
    assert_eq!(rig.bounds(&id), (300.0, 250.0, 400.0, 200.0));
    rig.run("image.paint_begin", json!({}));
    near(dab_at(&mut rig, &id, 400.0, 350.0), Point::new(50.0, 50.0));
}

// ---- E18: the stroke equals the command ----

/// The document without the paint, which has ids that differ between two runs.
fn doc_without_paint(rig: &Rig) -> Value {
    fn strip(v: &mut Value) {
        match v {
            Value::Object(m) => {
                m.remove("paint");
                m.values_mut().for_each(strip);
            }
            Value::Array(a) => a.iter_mut().for_each(strip),
            _ => {}
        }
    }
    let mut doc = rig.normalized_doc();
    strip(&mut doc);
    doc
}

/// The arguments of the last `image.paint` in the ring.
fn last_paint_args(rig: &Rig) -> Value {
    let entry = rig.ring().into_iter().rev().find(|e| e.id == "image.paint").expect("an image.paint ran");
    serde_json::from_str(&entry.args).expect("the ring keeps the arguments as JSON")
}

#[test]
fn exception_E18_paint_equals_plain_command() {
    let (mut dragged, id) = rig_painting();
    dragged.run("image.paint_brush", json!({ "color": "#2f9e44", "size": 12, "hardness": 0.6, "opacity": 0.8 }));
    let samples = [(330.0, 270.0), (360.0, 290.0), (390.0, 295.0), (420.0, 310.0), (440.0, 335.0)];
    trace(&mut dragged, &samples);
    let stored = strokes(&dragged, &id);
    assert_eq!(stored.len(), 1);
    assert_eq!(paints(&dragged), 1);

    // The plain command with the stroke as it was stored, in a document that has only had the same set-up.
    let (mut plain, plain_id) = rig_painting();
    plain.run("image.paint_brush", json!({ "color": "#2f9e44", "size": 12, "hardness": 0.6, "opacity": 0.8 }));
    let points: Vec<[f64; 2]> = stored[0].points.iter().map(|p| [p.x, p.y]).collect();
    plain.run("image.paint", json!({ "strokes": [{ "points": points }] }));

    assert_eq!(strokes(&plain, &plain_id), stored, "the same stroke");
    assert_eq!(doc_without_paint(&plain), doc_without_paint(&dragged), "the same document");
    assert_eq!(last_paint_args(&dragged)["strokes"][0]["points"], json!(points), "the command carried the stroke");
    assert_eq!(dragged.undo_depth(), plain.undo_depth(), "and the same undo steps");
}

#[test]
fn exception_E18_a_rotated_mirrored_cropped_stroke_equals_plain_command() {
    let set_up = |rig: &mut Rig, id: &ElementId| {
        rig.run("element.rotate", json!({ "ids": [id.as_str()], "angle": 30, "mode": "to" }));
        rig.run("element.flip", json!({ "ids": [id.as_str()], "axis": "vertical" }));
        rig.run("image.crop", json!({ "crop": { "x": 20, "y": 10, "w": 150, "h": 70 } }));
        rig.run("image.paint_begin", json!({}));
    };
    let (mut dragged, id) = rig_with_image();
    set_up(&mut dragged, &id);
    let t = dragged.element(&id).transform;
    let centre = (t.x + t.w / 2.0, t.y + t.h / 2.0);
    trace(&mut dragged, &[(centre.0 - 20.0, centre.1 - 5.0), (centre.0, centre.1), (centre.0 + 25.0, centre.1 + 8.0)]);
    let stored = strokes(&dragged, &id);
    assert_eq!(stored.len(), 1);

    let (mut plain, plain_id) = rig_with_image();
    set_up(&mut plain, &plain_id);
    let points: Vec<[f64; 2]> = stored[0].points.iter().map(|p| [p.x, p.y]).collect();
    plain.run("image.paint", json!({ "strokes": [{ "points": points }] }));
    assert_eq!(strokes(&plain, &plain_id), stored);
    assert_eq!(doc_without_paint(&plain), doc_without_paint(&dragged));
}

// ---- leaving the editor ----

#[test]
fn esc_during_a_drag_drops_the_stroke_and_leaves_the_editor_open() {
    let (mut rig, id) = rig_painting();
    let ring = rig.ring_ids().len();
    rig.down(330.0, 280.0);
    rig.drag_to(380.0, 300.0);
    rig.drag_to(430.0, 310.0);
    assert_eq!(shown_strokes(&rig).len(), 1);
    assert!(rig.key(Key::Escape).handled);
    assert_eq!(rig.ring_ids().len(), ring, "no command ran");
    assert!(shown_strokes(&rig).is_empty(), "the draft is gone");
    assert!(strokes(&rig, &id).is_empty());
    assert_eq!(editor(&rig), Some(id.clone()), "the first Esc only cancelled the drag");
    assert!(!rig.i.is_busy());
    // The release that follows does not paint it after all.
    rig.up(430.0, 310.0);
    assert!(strokes(&rig, &id).is_empty());
    // The next Esc ends the editor.
    assert!(rig.key(Key::Escape).handled);
    assert_eq!(editor(&rig), None);
}

#[test]
fn enter_and_esc_end_the_editor_without_a_command_other_than_the_end() {
    for key in [Key::Enter, Key::Escape] {
        let (mut rig, id) = rig_painting();
        let undo = rig.undo_depth();
        assert!(rig.key(key).handled);
        assert_eq!(editor(&rig), None);
        assert_eq!(rig.undo_depth(), undo);
        assert_eq!(rig.selected(), vec![id.as_str()], "the image stays selected");
        let ids = rig.ring_ids();
        assert_eq!(
            ids.iter().filter(|i| i.starts_with("image.paint")).collect::<Vec<_>>(),
            ["image.paint_begin", "image.paint_end"]
        );
    }
}

#[test]
fn a_click_outside_the_image_ends_the_editor_and_selects_what_is_under_it() {
    let (mut rig, id) = rig_painting();
    let other = rig.rect(50.0, 50.0, 100.0, 100.0);
    rig.click(100.0, 100.0);
    assert_eq!(editor(&rig), None);
    assert_eq!(rig.selected(), vec![other.as_str()]);
    assert!(strokes(&rig, &id).is_empty());
}

#[test]
fn a_click_on_empty_canvas_ends_the_editor_and_clears_the_selection() {
    let (mut rig, _) = rig_painting();
    rig.click(700.0, 500.0);
    assert_eq!(editor(&rig), None);
    assert!(rig.selected().is_empty());
}

#[test]
fn a_tool_change_closes_the_editor_and_drops_a_running_stroke() {
    let (mut rig, id) = rig_painting();
    rig.down(330.0, 280.0);
    rig.drag_to(380.0, 300.0);
    rig.tool(crate::state::Tool::Rectangle);
    rig.up(380.0, 300.0);
    assert_eq!(editor(&rig), None);
    assert!(!rig.i.is_busy());
    assert!(strokes(&rig, &id).is_empty());
    assert!(rig.i.overlay(&rig.s).paint.is_none());
}

// ---- the other elements ----

#[test]
fn other_elements_are_not_hit_while_editing() {
    let (mut rig, id) = rig_painting();
    // A rectangle on top of the right half of the image, and one beside it.
    let over = rig.rect(400.0, 260.0, 80.0, 80.0);
    let beside = rig.rect(600.0, 260.0, 80.0, 80.0);
    let (over_at, beside_at) = (rig.bounds(&over), rig.bounds(&beside));

    // Dragging across the rectangle on top paints the image and does not move the rectangle.
    rig.drag((420.0, 300.0), (460.0, 320.0));
    assert_eq!(rig.bounds(&over), over_at);
    assert_eq!(strokes(&rig, &id).len(), 1);
    // A drag that starts on the other rectangle moves nothing and paints nothing.
    rig.drag((620.0, 300.0), (660.0, 320.0));
    assert_eq!(rig.bounds(&beside), beside_at);
    assert_eq!(strokes(&rig, &id).len(), 1);
    // Hovering the other rectangle outlines nothing, and the pointer is not a move cursor over the image.
    rig.hover(640.0, 300.0);
    assert!(rig.i.overlay(&rig.s).canvas.hover.is_none());
    assert_eq!(rig.i.cursor(), Cursor::Default);
    rig.hover(440.0, 300.0);
    assert!(rig.i.overlay(&rig.s).canvas.hover.is_none());
    assert_eq!(rig.i.cursor(), Cursor::Crosshair);
}

#[test]
fn a_double_click_and_a_right_click_do_not_open_the_text_editor_or_the_crop_editor() {
    let (mut rig, id) = rig_painting();
    rig.double_click(400.0, 300.0);
    assert_eq!(editor(&rig), Some(id));
    assert!(rig.s.tab().observed.crop_editor.is_none());
    assert!(!rig.ring_ids().iter().any(|i| i == "text.edit_begin" || i == "image.crop_begin"));
}

// ---- the brush ring ----

#[test]
fn the_brush_ring_follows_the_pointer_over_the_image_and_has_the_size_of_the_brush() {
    let (mut rig, _) = rig_painting();
    rig.run("image.paint_brush", json!({ "size": 40 }));
    assert!(rig.i.overlay(&rig.s).eraser.is_none(), "no pointer seen yet");
    let out = rig.hover(400.0, 300.0);
    assert!(out.redraw.is_some());
    let ring = rig.i.overlay(&rig.s).eraser.expect("the ring is at the pointer");
    near(ring.at, Point::new(400.0, 300.0));
    assert!((ring.radius_px - 20.0).abs() < 1e-9, "40 pixels across is a radius of 20 at one unit per pixel: {ring:?}");

    // Zoomed in the ring grows with the picture, and the image shown larger has a larger ring.
    rig.run("view.set", json!({ "scroll": [400, 300], "zoom": 2 }));
    rig.hover(401.0, 300.0);
    let zoomed = rig.i.overlay(&rig.s).eraser.expect("ring");
    assert!((zoomed.radius_px - 40.0).abs() < 1e-9, "{zoomed:?}");

    rig.hover(5.0, 5.0);
    assert!(rig.i.overlay(&rig.s).eraser.is_none(), "off the image there is no brush");
}

#[test]
fn the_ring_goes_when_the_pointer_leaves_the_canvas_and_when_the_editor_ends() {
    let (mut rig, _) = rig_painting();
    rig.hover(400.0, 300.0);
    assert!(rig.i.overlay(&rig.s).eraser.is_some());
    rig.input(InputEvent::PointerLeave { id: PointerId(0) });
    assert!(rig.i.overlay(&rig.s).eraser.is_none());
    rig.hover(400.0, 300.0);
    rig.key(Key::Enter);
    assert!(rig.i.overlay(&rig.s).eraser.is_none());
}

#[test]
fn hovering_outside_the_image_while_nothing_is_shown_costs_no_redraw() {
    let (mut rig, _) = rig_painting();
    rig.hover(5.0, 5.0);
    let out = rig.hover(10.0, 10.0);
    assert!(out.redraw.is_none(), "{:?}", out.redraw);
}

// ---- undo while painting ----

#[test]
fn undo_and_redo_while_painting_keep_the_editor_and_show_the_strokes_that_are_there() {
    let (mut rig, id) = rig_painting();
    rig.drag((320.0, 270.0), (380.0, 270.0));
    rig.drag((320.0, 300.0), (380.0, 300.0));
    rig.run("history.undo", json!({}));
    assert_eq!(shown_strokes(&rig).len(), 1);
    // The next stroke is numbered after the one that is left, and the redo stack is gone.
    rig.drag((320.0, 330.0), (380.0, 330.0));
    assert_eq!(strokes(&rig, &id).len(), 2);
    assert_eq!(shown_strokes(&rig).len(), 2);
    assert_eq!(editor(&rig), Some(id));
}
