//! Tests of the crop editor: scripted pointer events and keys go through the real recognizer and the interaction
//! layer into the real command registry. `exception_E17_` is the row of `docs/exceptions.md`.

// `cargo xtask exceptions` looks for tests named `exception_E<n>_...`, which is not snake case.
#![allow(non_snake_case)]

use base64::Engine as _;
use drafftink_canvas::{Buttons, Cursor, InputEvent, Modifiers, PointerEvent, PointerId, PointerKind, PointerPhase};
use drafftink_geom::kurbo::Point;
use drafftink_scene::{Crop, ElementId};
use serde_json::json;

use super::Key;
use super::testutil::Rig;
use crate::cmd::image::tests::png;

const SHIFT: Modifiers = Modifiers { shift: true, ctrl: false, alt: false, meta: false };

/// A rig with a 200 by 100 picture at (300, 250): one unit per pixel, its middle at (400, 300), selected.
fn rig_with_image() -> (Rig, ElementId) {
    let mut rig = Rig::new();
    let b64 = base64::engine::general_purpose::STANDARD.encode(png(200, 100));
    let out = rig.run("image.insert", json!({ "images": [{ "base64": b64 }], "at": [400, 300] }));
    let id = ElementId::new(out["ids"][0].as_str().unwrap()).unwrap();
    assert_eq!(rig.bounds(&id), (300.0, 250.0, 200.0, 100.0));
    (rig, id)
}

fn crop_of(rig: &Rig, id: &ElementId) -> Option<Crop> {
    rig.element(id).image_data().and_then(|d| d.crop.clone())
}

fn near(got: (f64, f64, f64, f64), want: (f64, f64, f64, f64)) {
    let ok = [got.0 - want.0, got.1 - want.1, got.2 - want.2, got.3 - want.3].iter().all(|d| d.abs() < 1e-9);
    assert!(ok, "{got:?} != {want:?}");
}

fn with_mods(rig: &mut Rig, mods: Modifiers, f: impl FnOnce(&mut Rig)) {
    rig.mods = mods;
    f(rig);
    rig.mods = Modifiers::NONE;
}

fn crops(rig: &Rig) -> usize {
    rig.ring_ids().iter().filter(|i| *i == "image.crop").count()
}

fn editor(rig: &Rig) -> Option<ElementId> {
    rig.s.tab().observed.crop_editor.clone()
}

// ---- opening ----

#[test]
fn a_double_click_on_an_image_opens_the_crop_editor() {
    let (mut rig, id) = rig_with_image();
    rig.run("selection.clear", json!({}));
    rig.double_click(400.0, 300.0);
    assert_eq!(editor(&rig), Some(id.clone()));
    assert_eq!(rig.selected(), vec![id.as_str()]);
    let view = rig.i.overlay(&rig.s).crop.expect("the overlay shows the editor");
    assert_eq!(view.element, id);
    assert!(rig.i.overlay(&rig.s).canvas.transformer.is_none(), "the window carries the handles, not the selection");
    assert!(!rig.ring_ids().iter().any(|i| i == "text.edit_begin"));
}

#[test]
fn enter_with_one_image_selected_opens_the_editor_and_enter_again_closes_it_without_a_command() {
    let (mut rig, id) = rig_with_image();
    assert!(rig.key(Key::Enter).handled);
    assert_eq!(editor(&rig), Some(id.clone()));
    assert!(rig.key(Key::Enter).handled);
    assert_eq!(editor(&rig), None);
    assert_eq!(crops(&rig), 0, "nothing was changed, so nothing is committed: {:?}", rig.ring_ids());
    let ids = rig.ring_ids();
    assert_eq!(
        ids.iter().filter(|i| i.starts_with("image.crop")).collect::<Vec<_>>(),
        ["image.crop_begin", "image.crop_end"]
    );
}

#[test]
fn enter_does_nothing_for_other_selections() {
    let (mut rig, id) = rig_with_image();
    let r = rig.rect(0.0, 0.0, 50.0, 50.0);
    rig.run("selection.select", json!({ "ids": [r.as_str()] }));
    assert!(!rig.key(Key::Enter).handled);
    rig.run("selection.select", json!({ "ids": [r.as_str(), id.as_str()] }));
    assert!(!rig.key(Key::Enter).handled);
    assert_eq!(editor(&rig), None);
}

#[test]
fn a_button_that_opens_the_editor_shows_it_before_any_pointer_event() {
    let (mut rig, id) = rig_with_image();
    rig.run("image.crop_begin", json!({}));
    let view = rig.i.overlay(&rig.s).crop.expect("the overlay shows the editor at once");
    assert_eq!(view.element, id);
    assert_eq!(view.window.size.width, 200.0);
    rig.run("image.crop_end", json!({}));
    assert!(rig.i.overlay(&rig.s).crop.is_none());
}

// ---- resizing the window ----

#[test]
fn dragging_a_handle_and_pressing_enter_commits_exactly_one_image_crop() {
    let (mut rig, id) = rig_with_image();
    let undo = rig.undo_depth();
    rig.key(Key::Enter);
    // The right edge handle is in the middle of the right edge, (500, 300). Drag it 50 to the left.
    rig.drag((500.0, 300.0), (450.0, 300.0));
    assert_eq!(crop_of(&rig, &id), None, "nothing changes in the document while the editor is open");
    assert_eq!(rig.bounds(&id), (300.0, 250.0, 200.0, 100.0));
    rig.key(Key::Enter);
    assert_eq!(crops(&rig), 1, "{:?}", rig.ring_ids());
    assert_eq!(crop_of(&rig, &id), Some(Crop { x: 0.0, y: 0.0, width: 150.0, height: 100.0 }));
    near(rig.bounds(&id), (300.0, 250.0, 150.0, 100.0));
    assert_eq!(editor(&rig), None);
    assert_eq!(rig.undo_depth(), undo + 1, "one undo step");
    rig.run("history.undo", json!({}));
    assert_eq!(crop_of(&rig, &id), None);
    near(rig.bounds(&id), (300.0, 250.0, 200.0, 100.0));
}

#[test]
fn several_drags_are_still_one_image_crop() {
    let (mut rig, id) = rig_with_image();
    rig.key(Key::Enter);
    rig.drag((500.0, 300.0), (450.0, 300.0));
    rig.drag((300.0, 300.0), (330.0, 300.0));
    rig.drag((400.0, 350.0), (400.0, 330.0));
    rig.key(Key::Enter);
    assert_eq!(crops(&rig), 1, "{:?}", rig.ring_ids());
    // Left 30, right 50 in, bottom 20 up: 120 by 80 pixels from (30, 0).
    assert_eq!(crop_of(&rig, &id), Some(Crop { x: 30.0, y: 0.0, width: 120.0, height: 80.0 }));
    near(rig.bounds(&id), (330.0, 250.0, 120.0, 80.0));
}

#[test]
fn shift_keeps_the_aspect_ratio_of_the_window() {
    let (mut rig, id) = rig_with_image();
    rig.key(Key::Enter);
    with_mods(&mut rig, SHIFT, |r| r.drag((500.0, 350.0), (440.0, 340.0)));
    rig.key(Key::Enter);
    // The window was 2 : 1. The pointer asks for 140 by 90; the wider of the two ratios wins: 180 by 90.
    assert_eq!(crop_of(&rig, &id), Some(Crop { x: 0.0, y: 0.0, width: 180.0, height: 90.0 }));
}

#[test]
fn the_window_stays_inside_the_picture_and_snaps_to_its_edges() {
    let (mut rig, id) = rig_with_image();
    rig.key(Key::Enter);
    // Dragging the left edge outwards does nothing: the window is at the edge of the picture already.
    rig.drag((300.0, 300.0), (200.0, 300.0));
    // And a drag that ends 5 pixels inside the edge snaps back to the edge.
    rig.drag((300.0, 300.0), (305.0, 300.0));
    rig.key(Key::Enter);
    assert_eq!(crops(&rig), 0, "{:?}", rig.ring_ids());
    assert_eq!(crop_of(&rig, &id), None);
}

#[test]
fn the_window_does_not_get_smaller_than_ten_units() {
    let (mut rig, id) = rig_with_image();
    rig.key(Key::Enter);
    rig.drag((500.0, 300.0), (300.0, 300.0));
    rig.key(Key::Enter);
    assert_eq!(crop_of(&rig, &id), Some(Crop { x: 0.0, y: 0.0, width: 10.0, height: 100.0 }));
}

// ---- panning ----

#[test]
fn dragging_the_picture_slides_it_under_a_window_that_stays() {
    let (mut rig, id) = rig_with_image();
    rig.key(Key::Enter);
    rig.drag((300.0, 300.0), (400.0, 300.0));
    rig.drag((450.0, 300.0), (490.0, 300.0));
    rig.key(Key::Enter);
    // The window is the right half. The picture moved 40 right, so the window shows pixels 60 to 160.
    assert_eq!(crop_of(&rig, &id), Some(Crop { x: 60.0, y: 0.0, width: 100.0, height: 100.0 }));
    near(rig.bounds(&id), (400.0, 250.0, 100.0, 100.0));
}

#[test]
fn the_picture_cannot_be_slid_off_the_window() {
    let (mut rig, id) = rig_with_image();
    rig.key(Key::Enter);
    rig.drag((300.0, 300.0), (400.0, 300.0));
    rig.drag((450.0, 300.0), (1000.0, 300.0));
    rig.key(Key::Enter);
    assert_eq!(crop_of(&rig, &id), Some(Crop { x: 0.0, y: 0.0, width: 100.0, height: 100.0 }));
}

// ---- ending ----

#[test]
fn escape_keeps_the_crop_as_excalidraw_does() {
    let (mut rig, id) = rig_with_image();
    rig.key(Key::Enter);
    rig.drag((500.0, 300.0), (450.0, 300.0));
    let out = rig.key(Key::Escape);
    assert!(out.handled);
    assert_eq!(crops(&rig), 1);
    assert_eq!(crop_of(&rig, &id).map(|c| c.width), Some(150.0));
    assert_eq!(editor(&rig), None);
}

#[test]
fn escape_during_a_drag_puts_the_window_back_and_leaves_the_editor_open() {
    let (mut rig, id) = rig_with_image();
    rig.key(Key::Enter);
    rig.down(500.0, 300.0);
    rig.drag_to(470.0, 300.0);
    rig.drag_to(450.0, 300.0);
    assert!(rig.i.overlay(&rig.s).crop.unwrap().window.size.width < 200.0);
    assert!(rig.key(Key::Escape).handled);
    rig.up(450.0, 300.0);
    assert_eq!(editor(&rig), Some(id.clone()));
    assert_eq!(rig.i.overlay(&rig.s).crop.unwrap().window.size.width, 200.0);
    assert_eq!(crops(&rig), 0);
    // A second Escape ends the editor, and there is still nothing to commit.
    rig.key(Key::Escape);
    assert_eq!(editor(&rig), None);
    assert_eq!(crops(&rig), 0);
}

#[test]
fn a_click_outside_commits_and_selects_what_is_there() {
    let (mut rig, id) = rig_with_image();
    let other = rig.rect(600.0, 100.0, 80.0, 80.0);
    rig.key(Key::Enter);
    rig.drag((500.0, 300.0), (450.0, 300.0));
    rig.click(640.0, 140.0);
    assert_eq!(crops(&rig), 1, "{:?}", rig.ring_ids());
    assert_eq!(crop_of(&rig, &id).map(|c| c.width), Some(150.0));
    assert_eq!(editor(&rig), None);
    assert_eq!(rig.selected(), vec![other.as_str()]);
}

#[test]
fn a_click_inside_the_window_keeps_the_editor_open() {
    let (mut rig, id) = rig_with_image();
    rig.key(Key::Enter);
    rig.click(400.0, 300.0);
    assert_eq!(editor(&rig), Some(id));
}

#[test]
fn a_click_on_empty_canvas_with_nothing_changed_closes_the_editor_without_a_command() {
    let (mut rig, _id) = rig_with_image();
    rig.key(Key::Enter);
    rig.click(700.0, 500.0);
    assert_eq!(editor(&rig), None);
    assert_eq!(crops(&rig), 0);
    assert!(rig.selected().is_empty());
}

#[test]
fn a_tool_change_closes_the_editor_and_drops_the_draft() {
    let (mut rig, id) = rig_with_image();
    rig.key(Key::Enter);
    rig.drag((500.0, 300.0), (450.0, 300.0));
    rig.tool(crate::state::Tool::Rectangle);
    assert_eq!(editor(&rig), None);
    assert_eq!(crop_of(&rig, &id), None);
    assert!(rig.i.overlay(&rig.s).crop.is_none());
}

// ---- rotation and mirroring ----

#[test]
fn a_rotated_image_is_cropped_along_its_own_axes() {
    let (mut rig, id) = rig_with_image();
    rig.run("element.rotate", json!({ "ids": [id.as_str()], "angle": 90, "mode": "to" }));
    rig.key(Key::Enter);
    // Turned a quarter clockwise about (400, 300), the right edge of the picture is at the bottom: its middle is
    // (400, 400). Dragging that handle up by 50 takes 50 pixels off the right.
    rig.drag((400.0, 400.0), (400.0, 350.0));
    rig.key(Key::Enter);
    let crop = crop_of(&rig, &id).expect("cropped");
    assert!([crop.x, crop.y, crop.width - 150.0, crop.height - 100.0].iter().all(|d| d.abs() < 1e-9), "{crop:?}");
    let e = rig.element(&id);
    assert!((e.transform.angle - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
    near(rig.bounds(&id), (325.0, 225.0, 150.0, 100.0));
    // The pixels that stayed did not move on the canvas: the left edge of the picture is still at (400, 200).
    let left = e.transform.local_to_world(drafftink_geom::kurbo::Point::new(0.0, 50.0));
    assert!(left.distance(drafftink_geom::kurbo::Point::new(400.0, 200.0)) < 1e-9, "{left:?}");
}

#[test]
fn a_mirrored_image_is_cropped_as_it_is_seen() {
    let (mut rig, id) = rig_with_image();
    rig.run("element.flip", json!({ "ids": [id.as_str()], "axis": "horizontal" }));
    rig.key(Key::Enter);
    // The right edge of what is seen is the left edge of the picture.
    rig.drag((500.0, 300.0), (450.0, 300.0));
    rig.key(Key::Enter);
    assert_eq!(crop_of(&rig, &id), Some(Crop { x: 50.0, y: 0.0, width: 150.0, height: 100.0 }));
    near(rig.bounds(&id), (300.0, 250.0, 150.0, 100.0));
}

// ---- cursors and the grid ----

/// The eight handles of the window of `rig_with_image` (a 200 by 100 picture centred on (400, 300)) as offsets from
/// the centre, with the cursor an unrotated window shows over each. Corners first, as the picker ranks them.
const HANDLE_CURSORS: [((f64, f64), Cursor); 8] = [
    ((-100.0, -50.0), Cursor::ResizeNwse),
    ((100.0, -50.0), Cursor::ResizeNesw),
    ((100.0, 50.0), Cursor::ResizeNwse),
    ((-100.0, 50.0), Cursor::ResizeNesw),
    ((0.0, -50.0), Cursor::ResizeNs),
    ((100.0, 0.0), Cursor::ResizeEw),
    ((0.0, 50.0), Cursor::ResizeNs),
    ((-100.0, 0.0), Cursor::ResizeEw),
];

/// Where a handle at `offset` from the centre (400, 300) lies once the window is turned by `degrees`.
fn turned(offset: (f64, f64), degrees: f64) -> (f64, f64) {
    let (s, c) = degrees.to_radians().sin_cos();
    (400.0 + offset.0 * c - offset.1 * s, 300.0 + offset.0 * s + offset.1 * c)
}

/// The cursor over every handle of the open editor, in the order of [`HANDLE_CURSORS`], with the window turned by
/// `degrees`. The pointer visits an empty spot between two handles, so that every visit is a change of target.
fn cursors_over_handles(rig: &mut Rig, degrees: f64) -> Vec<Cursor> {
    HANDLE_CURSORS
        .iter()
        .map(|(offset, _)| {
            rig.hover(5.0, 5.0);
            let (x, y) = turned(*offset, degrees);
            rig.hover(x, y);
            rig.i.cursor()
        })
        .collect()
}

#[test]
fn every_handle_of_the_window_shows_its_own_resize_cursor() {
    let (mut rig, _) = rig_with_image();
    rig.key(Key::Enter);
    let want: Vec<Cursor> = HANDLE_CURSORS.iter().map(|(_, c)| *c).collect();
    assert_eq!(cursors_over_handles(&mut rig, 0.0), want);
}

#[test]
fn the_resize_cursors_turn_with_a_rotated_image() {
    // A quarter turn swaps the diagonals and the axes; an eighth turn puts the sides on the diagonals.
    let quarter = [
        Cursor::ResizeNesw,
        Cursor::ResizeNwse,
        Cursor::ResizeNesw,
        Cursor::ResizeNwse,
        Cursor::ResizeEw,
        Cursor::ResizeNs,
        Cursor::ResizeEw,
        Cursor::ResizeNs,
    ];
    let eighth = [
        Cursor::ResizeNs,
        Cursor::ResizeEw,
        Cursor::ResizeNs,
        Cursor::ResizeEw,
        Cursor::ResizeNesw,
        Cursor::ResizeNwse,
        Cursor::ResizeNesw,
        Cursor::ResizeNwse,
    ];
    for (degrees, want) in [(90.0, quarter), (45.0, eighth)] {
        let (mut rig, id) = rig_with_image();
        rig.run("element.rotate", json!({ "ids": [id.as_str()], "angle": degrees, "mode": "to" }));
        rig.key(Key::Enter);
        assert_eq!(cursors_over_handles(&mut rig, degrees), want, "turned by {degrees} degrees");
    }
}

#[test]
fn a_mirrored_image_shows_the_cursors_of_its_window_whatever_the_flip() {
    // The handles sit on the window, not on the mirrored picture. A flip turns the element's angle (a mirror image
    // of a turned box), so the expected cursors are those of an unmirrored image turned to the same angle.
    for (degrees, axis) in [(0.0, "horizontal"), (0.0, "vertical"), (90.0, "horizontal"), (30.0, "vertical")] {
        let (mut mirrored, id) = rig_with_image();
        mirrored.run("element.rotate", json!({ "ids": [id.as_str()], "angle": degrees, "mode": "to" }));
        mirrored.run("element.flip", json!({ "ids": [id.as_str()], "axis": axis }));
        assert!(
            mirrored.element(&id).transform.flip_x || mirrored.element(&id).transform.flip_y,
            "the image is mirrored"
        );
        let turned_to = mirrored.element(&id).transform.angle.to_degrees();
        mirrored.key(Key::Enter);
        let got = cursors_over_handles(&mut mirrored, turned_to);

        let (mut plain, id) = rig_with_image();
        plain.run("element.rotate", json!({ "ids": [id.as_str()], "angle": turned_to, "mode": "to" }));
        plain.key(Key::Enter);
        assert_eq!(got, cursors_over_handles(&mut plain, turned_to), "{axis} flip, turned by {degrees}");
        assert!(got.iter().all(|c| *c != Cursor::Default), "every handle answers: {got:?}");
    }
}

#[test]
fn the_crop_overlay_names_the_pointer_used_last_and_a_pen_grabs_from_as_far_as_a_finger() {
    let (mut rig, _) = rig_with_image();
    rig.key(Key::Enter);
    let pointer = |rig: &Rig| rig.i.overlay(&rig.s).crop.map(|c| c.pointer);
    rig.hover(5.0, 5.0);
    assert_eq!(pointer(&rig), Some(PointerKind::Mouse));
    // A pen 20 px from the corner (300, 250) is out of reach of a mouse (12 px) but within the large handle's.
    for (kind, reaches) in [(PointerKind::Mouse, false), (PointerKind::Pen, true), (PointerKind::Touch, true)] {
        let mut at = |phase, x: f64, y: f64| {
            rig.now += 16.0;
            let ev = PointerEvent::new(PointerId(1), kind, phase, Point::new(x, y), rig.now);
            rig.input(InputEvent::Pointer(ev));
        };
        at(PointerPhase::Move, 5.0, 5.0);
        at(PointerPhase::Move, 285.0, 235.0);
        let over_handle = matches!(rig.i.cursor(), Cursor::ResizeNwse);
        assert_eq!(over_handle, reaches, "{kind:?}");
        assert_eq!(pointer(&rig), Some(kind));
    }
}

#[test]
fn the_picture_shows_an_open_hand_and_the_hand_closes_while_it_is_slid() {
    let (mut rig, _) = rig_with_image();
    rig.key(Key::Enter);
    // Make the window smaller than the picture, so that part of the picture is dimmed.
    rig.drag((500.0, 300.0), (400.0, 300.0));
    rig.hover(5.0, 5.0);
    assert_eq!(rig.i.cursor(), Cursor::Default, "off the picture");
    rig.hover(350.0, 280.0);
    assert_eq!(rig.i.cursor(), Cursor::Grab, "inside the window");
    rig.down(350.0, 280.0);
    let out = rig.drag_to(360.0, 280.0);
    assert!(out.redraw.is_some());
    assert_eq!(rig.i.cursor(), Cursor::Grabbing, "while the picture moves");
    rig.up(360.0, 280.0);
    assert_eq!(rig.i.cursor(), Cursor::Grab, "the hand opens again: the pointer is still over the picture");
}

#[test]
fn the_cursor_over_a_handle_stays_a_resize_cursor_during_its_drag_and_afterwards() {
    let (mut rig, _) = rig_with_image();
    rig.key(Key::Enter);
    rig.hover(500.0, 300.0);
    assert_eq!(rig.i.cursor(), Cursor::ResizeEw);
    rig.down(500.0, 300.0);
    rig.drag_to(470.0, 300.0);
    assert_eq!(rig.i.cursor(), Cursor::ResizeEw);
    rig.up(470.0, 300.0);
    assert_eq!(rig.i.cursor(), Cursor::ResizeEw, "the handle followed the pointer and is still under it");
    rig.hover(400.0, 300.0);
    assert_eq!(rig.i.cursor(), Cursor::Grab, "away from the handle, over the picture");
}

#[test]
fn the_picture_outside_the_cropped_box_is_grabbed_too() {
    let (mut rig, id) = rig_with_image();
    rig.key(Key::Enter);
    rig.drag((500.0, 300.0), (400.0, 300.0));
    rig.key(Key::Enter);
    // The image now shows its left half. Open the editor again: the right half is outside the element's box.
    rig.run("selection.select", json!({ "ids": [id.as_str()] }));
    rig.key(Key::Enter);
    rig.hover(5.0, 5.0);
    rig.hover(450.0, 300.0);
    assert_eq!(rig.i.cursor(), Cursor::Grab, "on the part of the picture that is cropped away");
}

#[test]
fn the_overlay_shows_the_thirds_grid_only_while_a_drag_runs() {
    let (mut rig, _) = rig_with_image();
    rig.key(Key::Enter);
    let grid = |rig: &Rig| rig.i.overlay(&rig.s).crop.map(|c| c.grid);
    assert_eq!(grid(&rig), Some(false), "at rest");
    rig.down(500.0, 300.0);
    rig.drag_to(470.0, 300.0);
    assert_eq!(grid(&rig), Some(true), "while a handle is dragged");
    rig.up(470.0, 300.0);
    assert_eq!(grid(&rig), Some(false), "after the drag");
    rig.down(350.0, 300.0);
    rig.drag_to(360.0, 300.0);
    assert_eq!(grid(&rig), Some(true), "while the picture is slid");
    rig.up(360.0, 300.0);
    assert_eq!(grid(&rig), Some(false));
}

// ---- touch ----

/// A finger drag from `from` to `to`, in a few steps.
fn finger_drag(rig: &mut Rig, from: (f64, f64), to: (f64, f64)) {
    let mut at = |phase, x: f64, y: f64, buttons| {
        rig.now += 16.0;
        let ev = PointerEvent::new(PointerId(1), PointerKind::Touch, phase, Point::new(x, y), rig.now);
        rig.input(InputEvent::Pointer(ev.with_button(drafftink_canvas::Button::Primary, buttons)));
    };
    at(PointerPhase::Down, from.0, from.1, Buttons::PRIMARY);
    for k in 1..=5 {
        let t = f64::from(k) / 5.0;
        at(PointerPhase::Move, from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t, Buttons::PRIMARY);
    }
    at(PointerPhase::Up, to.0, to.1, Buttons::NONE);
}

#[test]
fn a_finger_grabs_a_handle_from_further_away_than_a_mouse_does() {
    // 20 pixels outside the right edge handle: beyond the mouse's reach (12), within the finger's (24).
    let (mut rig, id) = rig_with_image();
    rig.key(Key::Enter);
    rig.drag((520.0, 300.0), (480.0, 300.0));
    rig.key(Key::Enter);
    assert_eq!(crops(&rig), 0, "the mouse missed the handle, so nothing was cropped");
    assert_eq!(editor(&rig), None);
    rig.key(Key::Enter);
    finger_drag(&mut rig, (520.0, 300.0), (470.0, 300.0));
    rig.key(Key::Enter);
    assert_eq!(crop_of(&rig, &id), Some(Crop { x: 0.0, y: 0.0, width: 150.0, height: 100.0 }), "{:?}", rig.ring_ids());
}

// ---- E17 ----

/// The same drags on two rigs, one ending with the keys and one with the plain command.
#[test]
fn exception_E17_crop_equals_plain_command() {
    let (mut rig, id) = rig_with_image();
    rig.key(Key::Enter);
    rig.drag((300.0, 300.0), (400.0, 300.0));
    rig.drag((450.0, 300.0), (490.0, 300.0));
    rig.key(Key::Enter);

    let (mut twin, tid) = rig_with_image();
    twin.run(
        "image.crop",
        json!({
            "id": tid.as_str(),
            "crop": { "x": 60, "y": 0, "w": 100, "h": 100 },
            "bounds": { "x": 400, "y": 250, "w": 100, "h": 100 },
        }),
    );
    assert_eq!(rig.normalized_doc(), twin.normalized_doc());
    assert_eq!(crop_of(&rig, &id), crop_of(&twin, &tid));
    assert_eq!(rig.undo_depth(), twin.undo_depth());
}

#[test]
fn exception_E17_a_rotated_mirrored_crop_equals_plain_command() {
    let prepare = |rig: &mut Rig, id: &ElementId| {
        rig.run("element.rotate", json!({ "ids": [id.as_str()], "angle": 30, "mode": "to" }));
        rig.run("element.flip", json!({ "ids": [id.as_str()], "axis": "vertical" }));
    };
    let (mut rig, id) = rig_with_image();
    prepare(&mut rig, &id);
    rig.key(Key::Enter);
    let before = rig.i.overlay(&rig.s).crop.unwrap();
    // Take the top-left corner of the window 40 pixels inwards along each of the image's own axes.
    let corner = before.window.corners()[0];
    let along = |a: f64, d: f64| drafftink_geom::kurbo::Vec2::new(a.cos() * d, a.sin() * d);
    let angle = 30.0_f64.to_radians();
    let to = corner + along(angle, 40.0) + along(angle + std::f64::consts::FRAC_PI_2, 40.0);
    rig.drag((corner.x, corner.y), (to.x, to.y));
    rig.key(Key::Enter);

    let crop = crop_of(&rig, &id).expect("cropped");
    let e = rig.element(&id);
    let (mut twin, tid) = rig_with_image();
    prepare(&mut twin, &tid);
    twin.run(
        "image.crop",
        json!({
            "id": tid.as_str(),
            "crop": { "x": crop.x, "y": crop.y, "w": crop.width, "h": crop.height },
            "bounds": { "x": e.transform.x, "y": e.transform.y, "w": e.transform.w, "h": e.transform.h },
        }),
    );
    assert_eq!(rig.normalized_doc(), twin.normalized_doc());
    // And the plain command with only the crop (the default box rule) gives the same box.
    let (mut plain, pid) = rig_with_image();
    prepare(&mut plain, &pid);
    plain.run(
        "image.crop",
        json!({ "id": pid.as_str(), "crop": { "x": crop.x, "y": crop.y, "w": crop.width, "h": crop.height } }),
    );
    let (a, b) = (rig.element(&id).transform, plain.element(&pid).transform);
    assert!(
        [a.x - b.x, a.y - b.y, a.w - b.w, a.h - b.h].iter().all(|d| d.abs() < 1e-6),
        "the resize-only crop is what `follow_crop` gives: {a:?} vs {b:?}"
    );
}
