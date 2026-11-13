//! The style controls of the top strip over a windowless shell with a real GPU: the pictures the pixel checks and the
//! screenshots look at. Needs a Vulkan, Metal or DX12 adapter; without one the tests skip
//! (`DRAFFTINK_REQUIRE_GPU=1` makes that a failure). `DRAFFTINK_SCREENSHOT_DIR=<dir>` writes the pictures.
#![cfg(all(feature = "wgpu", not(target_arch = "wasm32")))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use drafftink_app::{App, AppConfig, NoHost, Platform, SharedClipboard, WakeCell};
use drafftink_engine::ports::fake::Fakes;
use drafftink_engine::services::LazyTypesetter;
use drafftink_gpu::Image;
use drafftink_gpu::headless::{Headless, HeadlessOptions};
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
const END_HEAD_BUTTON: Pos2 = Pos2::new(710.0, 51.0);

#[test]
fn the_head_picker_opens_over_the_canvas_with_a_picture_in_every_cell() {
    let Some(mut shell) = shell() else { return };
    run(&mut shell, "element.create", json!({ "kind": "arrow", "points": [[200, 300], [500, 400]] }));
    shell.settle(3);
    let closed = shell.screenshot().unwrap();
    click(&mut shell, END_HEAD_BUTTON);
    let open = shell.screenshot().unwrap();
    let area = [560, 60, 260, 150];
    let [x, y, w, h] = area;
    let changed = (y..y + h)
        .flat_map(|row| (x..x + w).map(move |col| (col, row)))
        .filter(|&(col, row)| closed.pixel(col, row) != open.pixel(col, row))
        .count();
    assert!(changed > 2_000, "the picker covers the canvas under the End button ({changed} pixels changed)");
    save(&crop(&open, [520, 0, 380, 260], 2), "head-picker.png");
}
