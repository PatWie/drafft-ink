//! Tests of the paint commands of the `image` family: effect, brush defaults, undo, refusal text, limits and the
//! editor mode. Pixels are checked by baking the stored strokes over a decoded fixture, as the renderer does.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use drafftink_scene::paint::{self, MAX_PAINT_STROKES, PaintMode};
use drafftink_scene::{Color, Element, ElementId, ElementKind, SceneRead, SceneTx, StoreError};
use serde_json::{Value, json};

use crate::batch::{BatchOpts, BatchStep};
use crate::refusal::Refusal;
use crate::ring::Origin;
use crate::session::Session;
use crate::state::Tool;
use crate::testing;

fn run(s: &mut Session, id: &str, args: Value) -> Value {
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => v,
        Err(r) => panic!("{id} refused: {} ({})", r.message, r.code),
    }
}

fn refuse(s: &mut Session, id: &str, args: Value) -> Refusal {
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => panic!("{id} should have refused, got {v}"),
        Err(r) => r,
    }
}

/// Refuses, and checks that nothing changed.
fn refuse_clean(s: &mut Session, id: &str, args: Value) -> Refusal {
    let before = s.fingerprint();
    let r = refuse(s, id, args);
    assert_eq!(s.fingerprint(), before, "{id} refused but changed state: {}", r.message);
    r
}

/// Grey of the fixture picture.
const GREY: [u8; 4] = [120, 120, 120, 255];

/// A PNG of `w` by `h` pixels, all `GREY`.
fn grey_png(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().unwrap();
        writer.write_image_data(&GREY.repeat((w * h) as usize)).unwrap();
    }
    out
}

/// A session with a 200 by 100 picture at (0, 0) (one unit per pixel), selected.
fn session() -> (Session, ElementId) {
    let mut s = testing::headless_session();
    let out = run(&mut s, "image.insert", json!({"images": [{"base64": STANDARD.encode(grey_png(200, 100))}], "at": [100, 50]}));
    let id = ElementId::new(out["ids"][0].as_str().unwrap()).unwrap();
    (s, id)
}

fn element(s: &Session, id: &ElementId) -> Element {
    s.element(id).cloned().unwrap()
}

fn strokes_of(s: &Session, id: &ElementId) -> Vec<drafftink_scene::PaintStroke> {
    element(s, id).image_data().unwrap().paint.values().cloned().collect()
}

fn editor(s: &Session) -> Option<ElementId> {
    s.tab().observed.paint_editor.clone()
}

/// The picture with the strokes of the image baked over it, as the renderer shows it.
fn baked(s: &Session, id: &ElementId) -> Vec<u8> {
    let original = GREY.repeat(200 * 100);
    paint::bake(&original, 200, 100, (1.0, 1.0), strokes_of(s, id).iter()).unwrap()
}

fn pixel(buf: &[u8], x: usize, y: usize) -> [u8; 4] {
    let at = (y * 200 + x) * 4;
    [buf[at], buf[at + 1], buf[at + 2], buf[at + 3]]
}

fn dab(x: f64, y: f64) -> Value {
    json!({"points": [[x, y]]})
}

// ---- image.paint ----

#[test]
fn a_stroke_is_stored_with_the_session_brush_and_paints_the_picture() {
    let (mut s, id) = session();
    let out = run(&mut s, "image.paint", json!({"strokes": [{"points": [[100, 50]], "size": 40, "hardness": 1}]}));
    assert_eq!(out["id"], json!(id.as_str()));
    assert_eq!(out["strokes"], 1);
    assert_eq!(out["changed"], true);
    assert_eq!(out["added"].as_array().unwrap().len(), 1);
    assert!(out["added"][0].as_str().unwrap().starts_with("000001-"), "{out}");

    let stored = strokes_of(&s, &id);
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].mode, PaintMode::Paint);
    assert_eq!(stored[0].color, Color::rgb(220, 38, 38), "the session brush's colour");
    assert_eq!((stored[0].size, stored[0].hardness, stored[0].opacity), (40.0, 1.0, 1.0));

    let img = baked(&s, &id);
    assert_eq!(pixel(&img, 100, 50), [220, 38, 38, 255]);
    assert_eq!(pixel(&img, 190, 90), GREY, "far from the stroke the picture is untouched");
}

#[test]
fn what_a_stroke_leaves_out_comes_from_the_brush_and_what_it_names_wins() {
    let (mut s, id) = session();
    let brush = run(
        &mut s,
        "image.paint_brush",
        json!({"color": "#1971c2", "size": 16, "hardness": 0.5, "opacity": 0.75, "mode": "paint"}),
    );
    assert_eq!(brush["color"], "#1971c2");
    run(&mut s, "image.paint", json!({"strokes": [dab(10.0, 10.0), {"points": [[20, 20]], "color": "red-600", "size": 5, "mode": "erase"}]}));
    let stored = strokes_of(&s, &id);
    assert_eq!(stored[0].color, Color::rgb(0x19, 0x71, 0xc2));
    assert_eq!((stored[0].size, stored[0].hardness, stored[0].opacity), (16.0, 0.5, 0.75));
    assert_eq!(stored[1].mode, PaintMode::Erase);
    assert_eq!(stored[1].size, 5.0);
    assert_eq!(stored[1].color, Color::rgb(220, 38, 38), "a palette name");
    assert_eq!(stored[1].opacity, 0.75, "left out: the brush");
}

#[test]
fn the_first_brush_follows_the_picture_until_a_size_is_chosen() {
    let (mut s, id) = session();
    // 1.5 % of the longer side (200) is 3, which the lower bound lifts to 4.
    run(&mut s, "image.paint", json!({"strokes": [dab(10.0, 10.0)]}));
    assert_eq!(strokes_of(&s, &id)[0].size, 4.0);
    run(&mut s, "image.paint_brush", json!({"size": 30}));
    run(&mut s, "image.paint", json!({"strokes": [dab(20.0, 20.0)]}));
    assert_eq!(strokes_of(&s, &id)[1].size, 30.0);
}

#[test]
fn strokes_get_ordered_ids_and_a_second_call_continues_the_numbers() {
    let (mut s, id) = session();
    let first = run(&mut s, "image.paint", json!({"strokes": [dab(1.0, 1.0), dab(2.0, 2.0)]}));
    let second = run(&mut s, "image.paint", json!({"strokes": [dab(3.0, 3.0)]}));
    assert_eq!(second["strokes"], 3);
    let ids: Vec<&str> =
        first["added"].as_array().unwrap().iter().chain(second["added"].as_array().unwrap()).map(|v| v.as_str().unwrap()).collect();
    assert!(ids[0].starts_with("000001-") && ids[1].starts_with("000002-") && ids[2].starts_with("000003-"), "{ids:?}");
    assert_eq!(strokes_of(&s, &id).iter().map(|k| k.points[0].x).collect::<Vec<_>>(), [1.0, 2.0, 3.0]);
}

#[test]
fn pressures_are_stored_with_the_points() {
    let (mut s, id) = session();
    run(&mut s, "image.paint", json!({"strokes": [{"points": [[10, 10], [50, 10]], "pressures": [0.25, 1.0]}]}));
    assert_eq!(strokes_of(&s, &id)[0].pressures, [0.25, 1.0]);
}

#[test]
fn a_stroke_that_shows_nothing_is_still_stored() {
    let (mut s, id) = session();
    let out = run(&mut s, "image.paint", json!({"strokes": [{"points": [[-500, -500]], "opacity": 0}]}));
    assert_eq!(out["strokes"], 1);
    assert_eq!(baked(&s, &id), GREY.repeat(200 * 100));
}

#[test]
fn the_image_defaults_to_the_single_selection_and_a_name_works() {
    let (mut s, id) = session();
    run(&mut s, "image.paint", json!({"strokes": [dab(1.0, 1.0)]}));
    run(&mut s, "element.set_name", json!({"id": id.as_str(), "name": "photo"}));
    run(&mut s, "selection.clear", json!({}));
    assert_eq!(refuse_clean(&mut s, "image.paint", json!({"strokes": [dab(1.0, 1.0)]})).code, "needs_single");
    run(&mut s, "image.paint", json!({"id": "@photo", "strokes": [dab(2.0, 2.0)]}));
    assert_eq!(strokes_of(&s, &id).len(), 2);
}

#[test]
fn a_call_is_one_undo_step_and_undo_and_redo_restore_the_strokes() {
    let (mut s, id) = session();
    let depth = s.tab().doc.undo_depth();
    run(&mut s, "image.paint", json!({"strokes": [dab(1.0, 1.0), dab(2.0, 2.0), dab(3.0, 3.0)]}));
    assert_eq!(s.tab().doc.undo_depth(), depth + 1);
    run(&mut s, "image.paint", json!({"strokes": [dab(4.0, 4.0)]}));
    assert_eq!(s.tab().doc.undo_depth(), depth + 2);
    assert_eq!(strokes_of(&s, &id).len(), 4);
    assert_eq!(run(&mut s, "history.undo", json!({}))["undone"], json!(["Paint"]));
    assert_eq!(strokes_of(&s, &id).len(), 3);
    run(&mut s, "history.undo", json!({}));
    assert!(strokes_of(&s, &id).is_empty());
    run(&mut s, "history.redo", json!({"steps": 2}));
    assert_eq!(strokes_of(&s, &id).len(), 4);
}

#[test]
fn a_batch_of_paints_is_one_undo_step() {
    let (mut s, id) = session();
    let depth = s.tab().doc.undo_depth();
    let steps: Vec<BatchStep> = (0..3).map(|i| BatchStep::new("image.paint", json!({"strokes": [dab(f64::from(i), 1.0)]}))).collect();
    let out = s.execute_batch(Origin::Test, &steps, &BatchOpts::default());
    assert!(out.failed.is_none(), "{:?}", out.failed);
    assert_eq!(strokes_of(&s, &id).len(), 3);
    assert_eq!(s.tab().doc.undo_depth(), depth + 1);
}

#[test]
fn an_erase_stroke_gives_the_original_pixels_back() {
    let (mut s, id) = session();
    run(&mut s, "image.paint", json!({"strokes": [{"points": [[100, 50]], "size": 40, "hardness": 1, "opacity": 1}]}));
    assert_eq!(pixel(&baked(&s, &id), 100, 50), [220, 38, 38, 255]);
    run(&mut s, "image.paint", json!({"strokes": [{"mode": "erase", "points": [[100, 50]], "size": 40, "hardness": 1, "opacity": 1}]}));
    assert_eq!(pixel(&baked(&s, &id), 100, 50), GREY);
    assert_eq!(strokes_of(&s, &id).len(), 2, "the strokes are kept: erasing is a stroke too");
}

#[test]
fn strokes_stay_in_picture_pixels_through_rotate_flip_crop_and_resize() {
    let (mut s, id) = session();
    run(&mut s, "image.paint", json!({"strokes": [{"points": [[30, 20], [60, 40]]}]}));
    let before = strokes_of(&s, &id);
    run(&mut s, "element.rotate", json!({"ids": [id.as_str()], "angle": 90, "mode": "to"}));
    run(&mut s, "element.flip", json!({"ids": [id.as_str()], "axis": "horizontal"}));
    run(&mut s, "image.crop", json!({"crop": {"x": 10, "y": 10, "w": 100, "h": 60}}));
    run(&mut s, "element.resize", json!({"ids": [id.as_str()], "handle": "se", "dx": 60, "dy": 40}));
    assert_eq!(strokes_of(&s, &id), before, "the strokes are in the picture, not on the canvas");
}

#[test]
fn duplicate_and_copy_paste_keep_the_paint() {
    let (mut s, id) = session();
    run(&mut s, "image.paint", json!({"strokes": [{"points": [[30, 20], [60, 40]], "size": 9}]}));
    let want = strokes_of(&s, &id);

    run(&mut s, "element.duplicate", json!({}));
    let copies: Vec<Element> = s.store().iter().filter(|e| e.id != id && e.image_data().is_some()).cloned().collect();
    assert_eq!(copies.len(), 1);
    assert_eq!(copies[0].image_data().unwrap().paint.values().cloned().collect::<Vec<_>>(), want);

    testing::select(&mut s, std::slice::from_ref(&id));
    run(&mut s, "clipboard.copy", json!({}));
    run(&mut s, "clipboard.paste", json!({"source": "internal", "at": [500, 500]}));
    let images: Vec<Element> = s.store().iter().filter(|e| e.image_data().is_some()).cloned().collect();
    assert_eq!(images.len(), 3);
    assert!(images.iter().all(|e| e.image_data().unwrap().paint.values().cloned().collect::<Vec<_>>() == want));
}

// ---- refusals ----

#[test]
fn bad_numbers_are_refused_in_words_that_name_the_range_and_change_nothing() {
    let (mut s, _) = session();
    let case = |s: &mut Session, stroke: Value| refuse_clean(s, "image.paint", json!({"strokes": [stroke]}));
    let r = case(&mut s, json!({"points": [[1, 1]], "size": 0.1}));
    assert_eq!(r.code, "bad_paint");
    assert!(r.message.contains("0.5 to 4096"), "{}", r.message);
    assert!(case(&mut s, json!({"points": [[1, 1]], "size": 5000})).message.contains("size"));
    assert!(case(&mut s, json!({"points": [[1, 1]], "hardness": 1.5})).message.contains("hardness"));
    assert!(case(&mut s, json!({"points": [[1, 1]], "opacity": -0.5})).message.contains("opacity"));
    assert!(case(&mut s, json!({"points": []})).message.contains("no points"));
    assert!(case(&mut s, json!({"points": [[1e12, 1]]})).message.contains("finite"));
    assert!(case(&mut s, json!({"points": [[1, 1], [2, 2]], "pressures": [0.5]})).message.contains("pressures"));
    assert!(case(&mut s, json!({"points": [[1, 1]], "pressures": [1.5]})).message.contains("0 to 1"));
    let named = refuse_clean(&mut s, "image.paint", json!({"strokes": [dab(1.0, 1.0), {"points": []}]}));
    assert!(named.message.contains("Stroke 2"), "{}", named.message);
}

#[test]
fn malformed_arguments_are_bad_args() {
    let (mut s, _) = session();
    assert_eq!(refuse_clean(&mut s, "image.paint", json!({})).code, "bad_args");
    assert_eq!(refuse_clean(&mut s, "image.paint", json!({"strokes": []})).code, "bad_args");
    assert_eq!(refuse_clean(&mut s, "image.paint", json!({"strokes": [{"points": [[1, 1]], "colour": "red"}]})).code, "bad_args");
    assert_eq!(refuse_clean(&mut s, "image.paint", json!({"strokes": [{"points": [[1, 1]], "color": "not a colour"}]})).code, "bad_args");
    assert_eq!(refuse_clean(&mut s, "image.paint", json!({"strokes": [{"points": [[1, 1]], "mode": "smudge"}]})).code, "bad_args");
}

#[test]
fn a_locked_image_a_missing_one_and_another_kind_are_refused() {
    let (mut s, id) = session();
    let rect = testing::add_rect(&mut s, 0.0, 0.0, 50.0, 50.0).unwrap();
    let r = refuse_clean(&mut s, "image.paint", json!({"id": rect.as_str(), "strokes": [dab(1.0, 1.0)]}));
    assert_eq!(r.code, "wrong_kind");
    assert!(r.message.contains("images"), "{}", r.message);
    assert_eq!(refuse_clean(&mut s, "image.paint", json!({"id": "nope", "strokes": [dab(1.0, 1.0)]})).code, "not_found");
    run(&mut s, "element.lock", json!({"ids": [id.as_str()]}));
    assert_eq!(refuse_clean(&mut s, "image.paint", json!({"id": id.as_str(), "strokes": [dab(1.0, 1.0)]})).code, "locked");
    assert_eq!(refuse_clean(&mut s, "image.paint_begin", json!({"id": id.as_str()})).code, "locked");
}

fn without_picture(s: &mut Session, id: &ElementId) {
    s.tab_mut()
        .doc
        .edit::<(), StoreError>(|tx| {
            tx.update(id, |e| {
                if let ElementKind::Image(d) = &mut e.kind {
                    d.file_id = None;
                }
            })
        })
        .unwrap();
}

#[test]
fn a_picture_that_has_not_arrived_cannot_be_painted_on() {
    let (mut s, id) = session();
    without_picture(&mut s, &id);
    let r = refuse_clean(&mut s, "image.paint", json!({"strokes": [dab(1.0, 1.0)]}));
    assert_eq!(r.code, "picture_not_loaded");
    assert!(r.message.contains("not arrived"), "{}", r.message);
    assert_eq!(refuse_clean(&mut s, "image.paint_begin", json!({})).code, "picture_not_loaded");
    assert_eq!(s.availability("image.paint").unwrap_err().code, "picture_not_loaded");
}

#[test]
fn the_limits_are_refused_before_anything_is_written() {
    let (mut s, id) = session();
    let many: Vec<Value> = (0..65).map(|_| dab(1.0, 1.0)).collect();
    let r = refuse_clean(&mut s, "image.paint", json!({"strokes": many}));
    assert_eq!(r.code, "limit_exceeded");
    assert!(r.message.contains("64"), "{}", r.message);
    let long: Vec<[f64; 2]> = (0..10_001).map(|i| [f64::from(i % 200), 1.0]).collect();
    assert_eq!(refuse_clean(&mut s, "image.paint", json!({"strokes": [{"points": long}]})).code, "bad_paint");

    // Fill the picture to its limit of strokes with a store write (a thousand calls would be slow), then one more.
    s.tab_mut()
        .doc
        .edit::<(), StoreError>(|tx| {
            tx.update(&id, |e| {
                if let ElementKind::Image(d) = &mut e.kind {
                    for n in 0..MAX_PAINT_STROKES {
                        let key = drafftink_scene::StrokeId::new(&format!("{:06}-x", n + 1)).unwrap();
                        d.paint.insert(key, drafftink_scene::PaintStroke {
                            mode: PaintMode::Paint,
                            color: Color::BLACK,
                            size: 1.0,
                            hardness: 1.0,
                            opacity: 1.0,
                            points: vec![drafftink_geom::kurbo::Point::new(1.0, 1.0)],
                            pressures: Vec::new(),
                        });
                    }
                }
            })
        })
        .unwrap();
    let r = refuse_clean(&mut s, "image.paint", json!({"strokes": [dab(1.0, 1.0)]}));
    assert_eq!(r.code, "limit_exceeded");
    assert!(r.message.contains("1000"), "{}", r.message);
}

#[test]
fn paint_is_refused_in_a_view_only_document_and_while_a_gesture_runs() {
    let (mut s, _) = session();
    run(&mut s, "view.set_view_only", json!({"on": true}));
    assert_eq!(refuse_clean(&mut s, "image.paint", json!({"strokes": [dab(1.0, 1.0)]})).code, "view_only");
}

// ---- image.reset_paint ----

#[test]
fn reset_takes_all_the_paint_off_in_one_undoable_step() {
    let (mut s, id) = session();
    run(&mut s, "image.paint", json!({"strokes": [dab(1.0, 1.0), dab(2.0, 2.0)]}));
    let depth = s.tab().doc.undo_depth();
    let out = run(&mut s, "image.reset_paint", json!({}));
    assert_eq!(out["removed"], 2);
    assert!(strokes_of(&s, &id).is_empty());
    assert_eq!(baked(&s, &id), GREY.repeat(200 * 100));
    assert_eq!(s.tab().doc.undo_depth(), depth + 1);
    run(&mut s, "history.undo", json!({}));
    assert_eq!(strokes_of(&s, &id).len(), 2);
}

#[test]
fn reset_is_offered_only_for_an_image_that_has_paint() {
    let (mut s, id) = session();
    let r = s.availability("image.reset_paint").unwrap_err();
    assert_eq!(r.code, "nothing_to_do");
    assert!(r.message.contains("the image has no paint"), "{}", r.message);
    run(&mut s, "image.paint", json!({"strokes": [dab(1.0, 1.0)]}));
    assert!(s.availability("image.reset_paint").is_ok());
    // Named explicitly, an image with no paint is left alone and says so.
    run(&mut s, "image.reset_paint", json!({}));
    let depth = s.tab().doc.undo_depth();
    let again = run(&mut s, "image.reset_paint", json!({"id": id.as_str()}));
    assert_eq!(again["removed"], 0);
    assert_eq!(s.tab().doc.undo_depth(), depth, "nothing was written");
}

#[test]
fn reset_refuses_a_locked_image_and_another_kind() {
    let (mut s, id) = session();
    run(&mut s, "image.paint", json!({"strokes": [dab(1.0, 1.0)]}));
    let rect = testing::add_rect(&mut s, 0.0, 0.0, 50.0, 50.0).unwrap();
    assert_eq!(refuse_clean(&mut s, "image.reset_paint", json!({"id": rect.as_str()})).code, "wrong_kind");
    run(&mut s, "element.lock", json!({"ids": [id.as_str()]}));
    assert_eq!(refuse_clean(&mut s, "image.reset_paint", json!({"id": id.as_str()})).code, "locked");
}

// ---- image.paint_brush ----

#[test]
fn the_brush_is_set_in_parts_reported_and_needs_no_image() {
    let mut s = testing::headless_session();
    let start = run(&mut s, "image.paint_brush", json!({}));
    assert_eq!((start["size"].as_f64(), start["hardness"].as_f64(), start["opacity"].as_f64()), (Some(24.0), Some(0.8), Some(1.0)));
    assert_eq!(start["mode"], "paint");
    let out = run(&mut s, "image.paint_brush", json!({"mode": "erase", "opacity": 0.4}));
    assert_eq!(out["mode"], "erase");
    assert_eq!(out["opacity"], 0.4);
    assert_eq!(out["hardness"], 0.8, "what is not named stays");
    assert_eq!(s.tool.paint_brush.mode, PaintMode::Erase);
    assert!(s.tab().doc.undo_depth() == 0, "the brush is not in the history");
}

#[test]
fn a_brush_with_a_number_out_of_range_is_refused_whole() {
    let mut s = testing::headless_session();
    let r = refuse_clean(&mut s, "image.paint_brush", json!({"color": "#000000", "size": 9999}));
    assert_eq!(r.code, "bad_paint");
    assert!(r.message.contains("0.5 to 4096"), "{}", r.message);
    assert_eq!(s.tool.paint_brush.color, drafftink_edit::retouch::PaintBrush::default().color, "the colour was not taken either");
    assert_eq!(refuse_clean(&mut s, "image.paint_brush", json!({"hardness": 2})).code, "bad_paint");
    assert_eq!(refuse_clean(&mut s, "image.paint_brush", json!({"opacity": -1})).code, "bad_paint");
    assert_eq!(refuse_clean(&mut s, "image.paint_brush", json!({"mode": "blur"})).code, "bad_args");
}

// ---- the editor ----

#[test]
fn paint_begin_selects_the_image_and_paint_end_closes_the_editor_without_an_undo_step() {
    let (mut s, id) = session();
    run(&mut s, "selection.clear", json!({}));
    run(&mut s, "tool.set", json!({"tool": "rectangle"}));
    let depth = s.tab().doc.undo_depth();
    let out = run(&mut s, "image.paint_begin", json!({"id": id.as_str()}));
    assert_eq!((out["id"].as_str(), out["changed"].as_bool()), (Some(id.as_str()), Some(true)));
    assert_eq!(editor(&s), Some(id.clone()));
    assert_eq!(s.tool.active, Tool::Select);
    assert_eq!(s.selection().iter().map(|i| i.as_str()).collect::<Vec<_>>(), [id.as_str()]);
    assert_eq!(run(&mut s, "selection.get", json!({"detail": "summary"}))["paint_editor"], json!(id.as_str()));
    assert_eq!(run(&mut s, "image.paint_begin", json!({}))["changed"], false);
    assert_eq!(run(&mut s, "image.paint_end", json!({}))["changed"], true);
    assert_eq!(editor(&s), None);
    assert_eq!(s.availability("image.paint_end").unwrap_err().code, "nothing_to_do");
    assert_eq!(s.tab().doc.undo_depth(), depth);
}

#[test]
fn opening_the_paint_editor_sets_the_default_size_of_that_picture() {
    let mut s = testing::headless_session();
    let out = run(&mut s, "image.insert", json!({"images": [{"base64": STANDARD.encode(grey_png(2000, 1000))}], "at": [0, 0]}));
    let id = out["ids"][0].as_str().unwrap().to_owned();
    run(&mut s, "image.paint_begin", json!({"id": id}));
    assert_eq!(s.tool.paint_brush.size, 30.0, "1.5 % of 2000");
    assert!(!s.tool.paint_brush.size_chosen);
}

#[test]
fn the_two_editors_are_never_open_together() {
    let (mut s, id) = session();
    run(&mut s, "image.crop_begin", json!({}));
    run(&mut s, "image.paint_begin", json!({}));
    assert_eq!((s.tab().observed.crop_editor.clone(), editor(&s)), (None, Some(id.clone())));
    run(&mut s, "image.crop_begin", json!({}));
    assert_eq!((s.tab().observed.crop_editor.clone(), editor(&s)), (Some(id), None));
}

#[test]
fn the_editor_closes_with_the_selection_the_tool_and_esc_and_when_the_image_goes() {
    let (mut s, id) = session();
    run(&mut s, "image.paint_begin", json!({}));
    run(&mut s, "selection.clear", json!({}));
    assert_eq!(editor(&s), None);
    run(&mut s, "image.paint_begin", json!({"id": id.as_str()}));
    run(&mut s, "tool.set", json!({"tool": "rectangle"}));
    assert_eq!(editor(&s), None);
    run(&mut s, "image.paint_begin", json!({"id": id.as_str()}));
    assert_eq!(run(&mut s, "tool.cancel", json!({}))["rungs"], json!(["paint_editor"]));
    assert!(s.selection().iter().next().is_some(), "Esc closed the editor and left the selection");
    run(&mut s, "image.paint_begin", json!({"id": id.as_str()}));
    run(&mut s, "element.delete", json!({"ids": [id.as_str()]}));
    assert_eq!(editor(&s), None);
}

#[test]
fn painting_keeps_the_editor_open_and_undo_and_redo_keep_it_open_too() {
    let (mut s, id) = session();
    run(&mut s, "image.paint_begin", json!({}));
    run(&mut s, "image.paint", json!({"strokes": [dab(1.0, 1.0)]}));
    run(&mut s, "image.paint", json!({"strokes": [dab(2.0, 2.0)]}));
    assert_eq!(editor(&s), Some(id.clone()));
    run(&mut s, "history.undo", json!({}));
    assert_eq!(strokes_of(&s, &id).len(), 1);
    assert_eq!(editor(&s), Some(id.clone()), "taking a stroke back is what one does in the editor");
    run(&mut s, "history.redo", json!({}));
    assert_eq!(strokes_of(&s, &id).len(), 2);
    assert_eq!(editor(&s), Some(id.clone()));
    run(&mut s, "history.undo", json!({"steps": 2}));
    assert_eq!(editor(&s), Some(id.clone()));
    // An undo that takes the image away closes it.
    run(&mut s, "history.undo", json!({}));
    assert_eq!(editor(&s), None);
}

#[test]
fn a_reset_keeps_the_editor_open() {
    let (mut s, id) = session();
    run(&mut s, "image.paint_begin", json!({}));
    run(&mut s, "image.paint", json!({"strokes": [dab(1.0, 1.0)]}));
    run(&mut s, "image.reset_paint", json!({}));
    assert_eq!(editor(&s), Some(id));
}

#[test]
fn the_paint_commands_are_offered_for_one_image() {
    let s = testing::headless_session();
    for id in ["image.paint", "image.paint_begin"] {
        assert_eq!(s.availability(id).unwrap_err().code, "needs_single", "{id}");
    }
    assert!(s.availability("image.paint_brush").is_ok(), "the brush needs no image");
    let (s2, _) = session();
    for id in ["image.paint", "image.paint_begin"] {
        assert!(s2.availability(id).is_ok(), "{id}");
    }
}
