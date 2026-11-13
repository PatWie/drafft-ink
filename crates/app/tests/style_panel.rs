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
    let fakes = Fakes::new();
    let app = App::new(platform(&fakes), AppConfig::default());
    let options = HeadlessOptions { size_points: SIZE, scale: 1.0, touch_first: false };
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
