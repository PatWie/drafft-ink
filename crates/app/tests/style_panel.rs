//! The style controls of the top strip over a windowless shell with a real GPU: the pictures the pixel checks and the
//! screenshots look at. Needs a Vulkan, Metal or DX12 adapter; without one the tests skip
//! (`DRAFFTINK_REQUIRE_GPU=1` makes that a failure). `DRAFFTINK_SCREENSHOT_DIR=<dir>` writes the pictures.
#![cfg(all(feature = "wgpu", not(target_arch = "wasm32")))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use drafftink_app::{App, AppConfig, NoHost, Platform, SharedClipboard, WakeCell};
use drafftink_engine::ports::fake::Fakes;
use drafftink_engine::services::LazyTypesetter;
use drafftink_gpu::headless::{Headless, HeadlessOptions};
use drafftink_gpu::{Image, PointerKind, PointerPhase, PointerPoint, PointerSample, ShellEvent};
use egui::{Event, Modifiers, PointerButton, Pos2};
use serde_json::{Value, json};

const SIZE: [f32; 2] = [1100.0, 700.0];

fn platform(fakes: &Fakes) -> Platform {
    Platform {
        ports: fakes.ports(),
        host: Box::new(NoHost),
        wake: WakeCell::new(),
        clipboard: SharedClipboard::new(),
        typesetter: LazyTypesetter::shared(),
        bridge: None,
        mcp_http: None,
        is_mac: false,
    }
}

/// A windowless shell over an app, or `None` (with the skip line) on a machine without a GPU adapter.
fn shell() -> Option<Headless<App>> {
    shell_of(SIZE, false)
}

/// The same at another size, as a touch-first device when `touch_first`.
fn shell_of(size_points: [f32; 2], touch_first: bool) -> Option<Headless<App>> {
    let fakes = Fakes::new();
    let app = App::new(platform(&fakes), AppConfig::default());
    let options = HeadlessOptions { size_points, scale: 1.0, touch_first };
    match Headless::new(app, options) {
        Ok(shell) => Some(shell),
        Err(drafftink_gpu::GpuError::NoAdapter(why)) => {
            if let Err(message) = drafftink_profile::skip_without_gpu(&why) {
                panic!("{message}");
            }
            None
        }
        Err(other) => panic!("a headless shell: {other}"),
    }
}

fn run(shell: &mut Headless<App>, id: &str, args: Value) -> Value {
    shell.app_mut().session_mut().execute(id, args).unwrap_or_else(|r| panic!("{id}: {}", r.message))
}

/// A click at `at`: the pointer moves there, presses and releases, each in a frame of its own.
fn click(shell: &mut Headless<App>, at: Pos2) {
    let button =
        |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    shell.frame(vec![Event::PointerMoved(at)]);
    shell.frame(vec![button(true)]);
    shell.frame(vec![button(false)]);
    shell.settle(3);
}

fn save(image: &Image, name: &str) {
    let Ok(dir) = std::env::var("DRAFFTINK_SCREENSHOT_DIR") else { return };
    std::fs::create_dir_all(&dir).unwrap();
    let file = std::fs::File::create(std::path::Path::new(&dir).join(name)).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), image.width, image.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&image.rgba).unwrap();
}

/// The part of `image` at `x`, `y` of `w` by `h` pixels, magnified `zoom` times (nearest pixel), for a picture a
/// reader can look at in a report.
fn crop(image: &Image, [x, y, w, h]: [u32; 4], zoom: u32) -> Image {
    let mut rgba = Vec::with_capacity((w * h * zoom * zoom * 4) as usize);
    for row in 0..h * zoom {
        for col in 0..w * zoom {
            let pixel = image.pixel(x + col / zoom, y + row / zoom).unwrap_or([0, 0, 0, 255]);
            rgba.extend_from_slice(&pixel);
        }
    }
    Image { width: w * zoom, height: h * zoom, rgba }
}

/// Where the End head button of the strip is, in the intro document's dark theme at `SIZE`.
const END_HEAD_BUTTON: Pos2 = Pos2::new(742.0, 51.0);

#[test]
fn the_head_picker_opens_over_the_canvas_with_a_picture_in_every_cell() {
    let Some(mut shell) = shell() else { return };
    run(&mut shell, "element.create", json!({ "kind": "arrow", "points": [[200, 300], [500, 400]] }));
    shell.settle(3);
    let closed = shell.screenshot().unwrap();
    click(&mut shell, END_HEAD_BUTTON);
    let open = shell.screenshot().unwrap();
    let area = [592, 60, 260, 150];
    let [x, y, w, h] = area;
    let changed = (y..y + h)
        .flat_map(|row| (x..x + w).map(move |col| (col, row)))
        .filter(|&(col, row)| closed.pixel(col, row) != open.pixel(col, row))
        .count();
    assert!(changed > 2_000, "the picker covers the canvas under the End button ({changed} pixels changed)");
    save(&crop(&open, [552, 0, 380, 260], 2), "head-picker.png");
}

/// Where the corner radius button of the strip is, with a rectangle selected, in the same window.
const RADIUS_BUTTON: Pos2 = Pos2::new(641.0, 19.0);

#[test]
fn the_corner_radius_menu_opens_under_its_button_with_the_slider_and_the_value() {
    let Some(mut shell) = shell() else { return };
    run(
        &mut shell,
        "element.create",
        json!({ "kind": "rectangle", "bounds": { "x": 300, "y": 300, "w": 200, "h": 120 } }),
    );
    run(&mut shell, "style.set", json!({ "props": { "corner_radius": 20 } }));
    shell.settle(3);
    let closed = shell.screenshot().unwrap();
    click(&mut shell, RADIUS_BUTTON);
    let open = shell.screenshot().unwrap();
    let [x, y, w, h] = [612, 40, 260, 80];
    let changed = (y..y + h)
        .flat_map(|row| (x..x + w).map(move |col| (col, row)))
        .filter(|&(col, row)| closed.pixel(col, row) != open.pixel(col, row))
        .count();
    assert!(changed > 1_500, "the menu covers the canvas under the button ({changed} pixels changed)");
    save(&crop(&open, [552, 0, 380, 140], 2), "corner-radius.png");
}

/// The eyedropper buttons of the strip in the light theme at `SIZE`: the stroke row over the fill row.
const PICK_STROKE: Pos2 = Pos2::new(376.0, 19.0);
const PICK_FILL: Pos2 = Pos2::new(376.0, 51.0);
/// A point on the middle of the rectangle `emerald_rectangle` makes, and one on empty canvas.
const ON_RECTANGLE: Pos2 = Pos2::new(550.0, 350.0);
const ON_NOTHING: Pos2 = Pos2::new(900.0, 560.0);

/// A light window with a solid emerald rectangle (`#10b981`) in the middle, selected, at zoom 1.
fn emerald_rectangle() -> Option<Headless<App>> {
    let mut shell = shell()?;
    run(&mut shell, "settings.set", json!({ "key": "theme", "value": "light" }));
    run(&mut shell, "view.set", json!({ "scroll": [400, 300], "zoom": 1 }));
    run(
        &mut shell,
        "element.create",
        json!({
            "kind": "rectangle",
            "bounds": { "x": 300, "y": 250, "w": 200, "h": 100 },
            "style": { "fill_color": "#10b981", "fill_pattern": "solid" }
        }),
    );
    shell.settle(4);
    Some(shell)
}

fn pointer(shell: &mut Headless<App>, phase: PointerPhase, pos: Pos2) {
    let point = PointerPoint { pos, pressure: None, time_ms: 0.0 };
    let sample = PointerSample::new(phase, PointerKind::Mouse, point, Modifiers::NONE);
    shell.event(ShellEvent::Pointer(sample));
}

/// A press and release of the mouse on the canvas at `pos`.
fn canvas_click(shell: &mut Headless<App>, pos: Pos2) {
    pointer(shell, PointerPhase::Hover, pos);
    pointer(shell, PointerPhase::Down, pos);
    pointer(shell, PointerPhase::Up, pos);
    shell.settle(3);
}

fn style_of(shell: &mut Headless<App>, prop: &str) -> Value {
    run(shell, "style.get", json!({ "props": [prop] }))["props"][prop]["value"].clone()
}

fn undo_depth(shell: &mut Headless<App>) -> usize {
    shell.app_mut().session_mut().tab().doc.undo_depth()
}

#[test]
fn a_click_on_the_canvas_in_pick_mode_sets_the_stroke_to_the_pixel_under_it_in_one_undo_step() {
    let Some(mut shell) = emerald_rectangle() else { return };
    assert_ne!(style_of(&mut shell, "stroke_color"), json!("#10b981"));
    let depth = undo_depth(&mut shell);
    click(&mut shell, PICK_STROKE);
    pointer(&mut shell, PointerPhase::Hover, ON_RECTANGLE);
    shell.settle(6);
    let hovering = shell.screenshot().unwrap();
    save(&crop(&hovering, [150, 0, 560, 100], 2), "eyedropper-strip.png");
    save(&crop(&hovering, [440, 280, 360, 200], 2), "eyedropper-preview.png");
    assert_eq!(undo_depth(&mut shell), depth, "looking changes nothing");
    pointer(&mut shell, PointerPhase::Down, ON_RECTANGLE);
    assert_eq!(undo_depth(&mut shell), depth, "nor does the press: the release picks");
    pointer(&mut shell, PointerPhase::Up, ON_RECTANGLE);
    shell.settle(3);
    assert_eq!(style_of(&mut shell, "stroke_color"), json!("#10b981"), "the colour of the canvas under the pointer");
    assert_eq!(undo_depth(&mut shell), depth + 1, "one undo step");
    run(&mut shell, "history.undo", json!({}));
    assert_ne!(style_of(&mut shell, "stroke_color"), json!("#10b981"));
}

#[test]
fn the_fill_button_picks_for_the_fill_and_the_pick_is_what_the_canvas_shows_not_what_is_stored() {
    let Some(mut shell) = emerald_rectangle() else { return };
    // Empty canvas: the page colour, as the light theme draws it.
    click(&mut shell, PICK_FILL);
    canvas_click(&mut shell, ON_NOTHING);
    assert_eq!(style_of(&mut shell, "fill_color"), json!("#ffffff"));
    assert_ne!(style_of(&mut shell, "stroke_color"), json!("#ffffff"), "the stroke is left alone");
}

#[test]
fn pick_mode_keeps_the_press_from_the_canvas_and_ends_after_a_pick() {
    let Some(mut shell) = emerald_rectangle() else { return };
    let selected = run(&mut shell, "selection.get", json!({}))["ids"].clone();
    assert!(selected.as_array().is_some_and(|a| a.len() == 1), "the rectangle is selected: {selected}");
    click(&mut shell, PICK_STROKE);
    canvas_click(&mut shell, ON_NOTHING);
    assert_eq!(
        run(&mut shell, "selection.get", json!({}))["ids"],
        selected,
        "a click on empty canvas did not deselect"
    );
    // The mode is over: the next click on empty canvas is an ordinary one.
    canvas_click(&mut shell, ON_NOTHING);
    let after = run(&mut shell, "selection.get", json!({}))["ids"].clone();
    assert!(after.as_array().is_some_and(Vec::is_empty), "now it deselects: {after}");
}

#[test]
fn esc_ends_pick_mode_and_the_next_click_is_an_ordinary_one() {
    let Some(mut shell) = emerald_rectangle() else { return };
    let before = run(&mut shell, "style.get", json!({}))["props"].clone();
    click(&mut shell, PICK_STROKE);
    let esc = |pressed| Event::Key {
        key: egui::Key::Escape,
        physical_key: Some(egui::Key::Escape),
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    shell.frame(vec![esc(true), esc(false)]);
    shell.settle(2);
    assert_eq!(run(&mut shell, "style.get", json!({}))["props"], before, "nothing was picked");
    canvas_click(&mut shell, ON_NOTHING);
    let ids = run(&mut shell, "selection.get", json!({}))["ids"].clone();
    assert!(ids.as_array().is_some_and(Vec::is_empty), "the click deselected as usual: {ids}");
}

#[test]
fn a_pick_in_the_dark_theme_stores_the_colour_that_draws_as_the_pixel() {
    let Some(mut shell) = emerald_rectangle() else { return };
    run(&mut shell, "settings.set", json!({ "key": "theme", "value": "dark" }));
    shell.settle(4);
    click(&mut shell, PICK_STROKE);
    canvas_click(&mut shell, ON_RECTANGLE);
    let stored = style_of(&mut shell, "stroke_color");
    let stored = stored.as_str().and_then(|s| drafftink_scene::Color::parse(s).ok()).expect("a colour");
    // The rectangle's fill is stored as #10b981 and drawn through the dark filter: the pick undoes that filter.
    let drawn = drafftink_scene::Color::parse("#10b981").unwrap().for_dark_theme();
    assert_eq!(stored, drawn.from_dark_theme(), "stored so that it draws as the fill did");
}

// ---- the paint editor ----

/// A solid picture as a base64 PNG.
fn solid_picture(w: u32, h: u32, [r, g, b]: [u8; 3]) -> String {
    use base64::Engine as _;
    let mut bytes = Vec::new();
    let mut enc = png::Encoder::new(&mut bytes, w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let pixels: Vec<u8> = (0..w * h).flat_map(|_| [r, g, b]).collect();
    enc.write_header().unwrap().write_image_data(&pixels).unwrap();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The blue of the picture `painting_window` makes.
const PICTURE_BLUE: [u8; 3] = [0x3b, 0x82, 0xf6];
/// The middle of that picture and a point near its top-left corner, on the canvas, at zoom 1.
const ON_PICTURE: Pos2 = Pos2::new(550.0, 350.0);
const NEAR_CORNER: Pos2 = Pos2::new(470.0, 310.0);
/// The middle of the emerald rectangle that stands apart from the picture.
const ON_APART: Pos2 = Pos2::new(300.0, 210.0);

/// A light window with a 200 by 100 blue picture (selected, in the middle of the canvas at zoom 1) and an emerald
/// rectangle apart from it.
fn painting_window() -> Option<Headless<App>> {
    painting_window_in(shell()?)
}

/// `painting_window` in a shell the caller made (another size, touch).
fn painting_window_in(mut shell: Headless<App>) -> Option<Headless<App>> {
    run(&mut shell, "settings.set", json!({ "key": "theme", "value": "light" }));
    run(&mut shell, "view.set", json!({ "scroll": [400, 300], "zoom": 1 }));
    run(
        &mut shell,
        "element.create",
        json!({
            "kind": "rectangle",
            "bounds": { "x": 100, "y": 130, "w": 100, "h": 60 },
            "style": { "fill_color": "#10b981", "fill_pattern": "solid" }
        }),
    );
    let png = solid_picture(200, 100, PICTURE_BLUE);
    let out = run(&mut shell, "image.insert", json!({ "images": [{ "base64": png }], "at": [450.0, 350.0] }));
    let id = out["ids"][0].as_str().unwrap().to_owned();
    run(&mut shell, "element.set_bounds", json!({ "ids": [id], "x": 300, "y": 250, "w": 200, "h": 100 }));
    run(&mut shell, "selection.select", json!({ "ids": [id] }));
    shell.settle(4);
    Some(shell)
}

/// The eyedropper button of the brush row in the strip while the paint editor is open (any theme, `SIZE`).
const PICK_BRUSH: Pos2 = Pos2::new(343.0, 19.0);

/// A drag of the mouse over the canvas from `from` to `to` in eight steps.
fn drag(shell: &mut Headless<App>, from: Pos2, to: Pos2) {
    pointer(shell, PointerPhase::Hover, from);
    pointer(shell, PointerPhase::Down, from);
    for step in 1..=8 {
        pointer(shell, PointerPhase::Move, from + (to - from) * (step as f32 / 8.0));
    }
    pointer(shell, PointerPhase::Up, to);
    shell.settle(3);
}

fn rgb(image: &Image, at: Pos2) -> [u8; 3] {
    let [r, g, b, _] = image.pixel(at.x as u32, at.y as u32).unwrap();
    [r, g, b]
}

/// How far two colours are apart in their most different channel.
fn apart(a: [u8; 3], b: [u8; 3]) -> u8 {
    a.iter().zip(b).map(|(x, y)| x.abs_diff(y)).max().unwrap_or(0)
}

#[test]
fn a_drag_in_the_paint_editor_paints_the_picture_in_one_undo_step_and_the_rest_of_the_canvas_is_veiled() {
    let Some(mut shell) = painting_window() else { return };
    let before = shell.screenshot().unwrap();
    assert_eq!(rgb(&before, ON_PICTURE), PICTURE_BLUE);
    let emerald = rgb(&before, ON_APART);
    let depth = undo_depth(&mut shell);

    run(&mut shell, "image.paint_begin", json!({}));
    run(
        &mut shell,
        "image.paint_brush",
        json!({ "color": "#ff0000", "size": 20, "hardness": 1, "opacity": 1, "mode": "paint" }),
    );
    shell.settle(4);
    let editing = shell.screenshot().unwrap();
    // The page colour at 60 %: the emerald rectangle is set back, the picture is not.
    let veiled = emerald.map(|c| (f32::from(c) * 0.4 + 255.0 * 0.6).round() as u8);
    assert!(apart(rgb(&editing, ON_APART), veiled) <= 4, "{:?} should be near {veiled:?}", rgb(&editing, ON_APART));
    assert_eq!(rgb(&editing, ON_PICTURE), PICTURE_BLUE, "the picture stays as it is under the veil");
    assert_eq!(undo_depth(&mut shell), depth, "opening the editor and setting the brush change no document");

    drag(&mut shell, Pos2::new(480.0, 350.0), Pos2::new(620.0, 350.0));
    assert_eq!(undo_depth(&mut shell), depth + 1, "one stroke is one undo step");
    let painted = shell.screenshot().unwrap();
    save(&crop(&painted, [0, 0, 760, 420], 1), "paint-editor-stroke.png");
    assert!(apart(rgb(&painted, ON_PICTURE), [255, 0, 0]) <= 12, "red on the stroke: {:?}", rgb(&painted, ON_PICTURE));
    assert_eq!(rgb(&painted, NEAR_CORNER), PICTURE_BLUE, "and the picture is as it was away from it");
    assert_eq!(rgb(&painted, Pos2::new(550.0, 330.0)), PICTURE_BLUE, "the stroke is as wide as the brush: 20 pixels");
    assert!(apart(rgb(&painted, ON_APART), veiled) <= 4, "the veil stays while the editor is open");

    // Done: the document shows the same stroke and the veil is gone.
    run(&mut shell, "image.paint_end", json!({}));
    shell.settle(4);
    let done = shell.screenshot().unwrap();
    assert!(apart(rgb(&done, ON_PICTURE), [255, 0, 0]) <= 12, "the stroke is in the document");
    assert_eq!(rgb(&done, ON_APART), emerald, "no veil any more");
    run(&mut shell, "history.undo", json!({}));
    shell.settle(4);
    assert_eq!(rgb(&shell.screenshot().unwrap(), ON_PICTURE), PICTURE_BLUE, "undo takes the stroke away");
}

#[test]
fn the_erase_mode_gives_the_picture_back_where_the_brush_passes() {
    let Some(mut shell) = painting_window() else { return };
    run(&mut shell, "image.paint_begin", json!({}));
    run(&mut shell, "image.paint_brush", json!({ "color": "#ff0000", "size": 30, "hardness": 1, "opacity": 1 }));
    drag(&mut shell, Pos2::new(480.0, 350.0), Pos2::new(620.0, 350.0));
    assert!(apart(rgb(&shell.screenshot().unwrap(), ON_PICTURE), [255, 0, 0]) <= 12);
    run(&mut shell, "image.paint_brush", json!({ "mode": "erase", "size": 12 }));
    drag(&mut shell, Pos2::new(520.0, 350.0), Pos2::new(580.0, 350.0));
    let after = shell.screenshot().unwrap();
    assert_eq!(rgb(&after, ON_PICTURE), PICTURE_BLUE, "erased down to the picture");
    assert!(apart(rgb(&after, Pos2::new(550.0, 340.0)), [255, 0, 0]) <= 12, "the paint beside the erased stripe stays");
}

#[test]
fn the_brush_eyedropper_reads_the_picture_itself_not_the_veil_in_both_themes() {
    for theme in ["light", "dark"] {
        let Some(mut shell) = painting_window() else { return };
        run(&mut shell, "settings.set", json!({ "key": "theme", "value": theme }));
        run(&mut shell, "image.paint_begin", json!({}));
        run(&mut shell, "image.paint_brush", json!({ "color": "#ff0000" }));
        shell.settle(4);
        click(&mut shell, PICK_BRUSH);
        canvas_click(&mut shell, NEAR_CORNER);
        let brush = run(&mut shell, "image.paint_brush", json!({}));
        assert_eq!(brush["color"], json!("#3b82f6"), "{theme}: the colour of the picture, untinted");
        let depth = undo_depth(&mut shell);
        // A second pick on the veiled rectangle gives the veiled colour: the sample is the canvas as drawn.
        click(&mut shell, PICK_BRUSH);
        pointer(&mut shell, PointerPhase::Hover, ON_APART);
        pointer(&mut shell, PointerPhase::Down, ON_APART);
        pointer(&mut shell, PointerPhase::Up, ON_APART);
        shell.settle(3);
        let veiled = run(&mut shell, "image.paint_brush", json!({}))["color"].as_str().unwrap().to_owned();
        assert_ne!(veiled, "#3b82f6", "{theme}: another colour was read");
        assert_eq!(undo_depth(&mut shell), depth, "{theme}: the clicks went to the eyedropper, not to the picture");
    }
}

/// The first quick colour of the brush row (blue 500), in the strip at `SIZE`.
const BRUSH_BLUE: Pos2 = Pos2::new(146.0, 19.0);

#[test]
fn the_brush_row_shows_the_colour_as_it_is_in_the_dark_theme_too() {
    for (theme, name) in [("light", "paint-strip-light.png"), ("dark", "paint-strip-dark.png")] {
        let Some(mut shell) = painting_window() else { return };
        run(&mut shell, "settings.set", json!({ "key": "theme", "value": theme }));
        run(&mut shell, "image.paint_begin", json!({}));
        shell.settle(4);
        let shot = shell.screenshot().unwrap();
        // A picture is not recoloured by the dark theme, so neither is the brush: blue 500 is #3b82f6 in both.
        assert!(apart(rgb(&shot, BRUSH_BLUE), PICTURE_BLUE) <= 3, "{theme}: {:?}", rgb(&shot, BRUSH_BLUE));
        save(&crop(&shot, [0, 0, 760, 420], 1), name);
    }
}

#[test]
fn a_selected_picture_has_edit_picture_next_to_crop_in_the_strip() {
    let Some(mut shell) = painting_window() else { return };
    let shot = shell.screenshot().unwrap();
    save(&crop(&shot, [0, 0, 1100, 100], 1), "edit-picture-strip.png");
    // Both chips are on the first row of the image group: the strip has pixels of the chip's fill there.
    let strip = (0..shot.width).filter(|&x| shot.pixel(x, 19) != shot.pixel(x, 60)).count();
    assert!(strip > 300, "the strip is drawn ({strip})");
}

/// The More button and the colour button of the one-row strip of a phone.
const PHONE_MORE: Pos2 = Pos2::new(360.0, 25.0);
const PHONE_COLOUR: Pos2 = Pos2::new(206.0, 25.0);

#[test]
fn on_a_phone_the_brush_waits_behind_the_colour_button_and_more() {
    let Some(shell) = shell_of([390.0, 844.0], true) else { return };
    let Some(mut shell) = painting_window_in(shell) else { return };
    run(&mut shell, "image.paint_begin", json!({}));
    shell.settle(4);
    let closed = shell.screenshot().unwrap();
    click(&mut shell, PHONE_MORE);
    let more = shell.screenshot().unwrap();
    let changed = |a: &Image, b: &Image, [x, y, w, h]: [u32; 4]| {
        (y..y + h)
            .flat_map(|row| (x..x + w).map(move |col| (col, row)))
            .filter(|&(c, r)| a.pixel(c, r) != b.pixel(c, r))
            .count()
    };
    assert!(changed(&closed, &more, [60, 55, 320, 130]) > 3_000, "the menu with the sliders opens under the strip");
    save(&crop(&more, [0, 0, 390, 260], 2), "paint-phone-more.png");
    // The menu stays inside the window: its right edge is not cut off.
    assert_eq!(more.pixel(389, 120), more.pixel(389, 300), "nothing of the menu reaches the window edge");
    click(&mut shell, PHONE_MORE);
    click(&mut shell, PHONE_COLOUR);
    let colours = shell.screenshot().unwrap();
    assert!(changed(&closed, &colours, [60, 55, 320, 100]) > 2_000, "the brush colours open from the colour button");
    save(&crop(&colours, [0, 0, 390, 260], 2), "paint-phone-colours.png");
}
