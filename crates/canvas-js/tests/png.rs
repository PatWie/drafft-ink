//! PNG export on a real GPU adapter (`--features gpu-tests`; Mesa lavapipe is enough:
//! `VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json`).
#![cfg(feature = "gpu-tests")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use drafftink_canvas_js::{PngOptions, Renderer};
use serde_json::json;

fn decode(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    (info.width, info.height, buf)
}

fn pixel(img: &(u32, u32, Vec<u8>), x: u32, y: u32) -> [u8; 4] {
    let i = ((y * img.0 + x) * 4) as usize;
    [img.2[i], img.2[i + 1], img.2[i + 2], img.2[i + 3]]
}

#[test]
fn a_png_shows_the_scene_at_the_scale_and_holds_the_scene() {
    let mut renderer = Renderer::headless().unwrap();
    let mut s = stage();
    s.add(json!({"id": "a", "type": "rectangle", "x": 0, "y": 0, "w": 100, "h": 50,
                 "style": {"fill_color": "#ff0000", "sloppiness": "architect", "stroke_width": 2}}))
        .unwrap();
    let plan =
        s.plan_png(&PngOptions { scale: 2.0, padding: 10.0, ..Default::default() }, renderer.max_side()).unwrap();
    assert_eq!(plan.size(), [248, 148], "(100 + 2 * 10 + the stroke margin of 2 per side) world units at scale 2");
    let bytes = pollster::block_on(plan.render(&mut renderer)).unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    let img = decode(&bytes);
    assert_eq!((img.0, img.1), (248, 148));
    assert_eq!(pixel(&img, 124, 74), [255, 0, 0, 255], "the fill, in the middle");
    assert_eq!(pixel(&img, 2, 2), [255, 255, 255, 255], "the white canvas in the padding");
    // The scene is inside: loading the PNG brings the rectangle back.
    let mut t = stage();
    t.load_bytes(&bytes).unwrap();
    assert_eq!(t.len(), 1);
    assert_eq!(xywh(&t, "a"), [0.0, 0.0, 100.0, 50.0]);
}

#[test]
fn background_off_leaves_it_transparent_and_a_selection_exports_alone() {
    let mut renderer = Renderer::headless().unwrap();
    let mut s = stage();
    let fill = |id: &str, x: f64, color: &str| {
        json!({"id": id, "type": "rectangle", "x": x, "y": 0, "w": 50, "h": 50,
               "style": {"fill_color": color, "sloppiness": "architect"}})
    };
    s.add(fill("a", 0.0, "#ff0000")).unwrap();
    s.add(fill("b", 200.0, "#0000ff")).unwrap();
    let opts = PngOptions {
        background: false,
        selection: Some(vec!["b".to_owned()]),
        padding: 5.0,
        embed_scene: false,
        ..Default::default()
    };
    let plan = s.plan_png(&opts, renderer.max_side()).unwrap();
    let img = decode(&pollster::block_on(plan.render(&mut renderer)).unwrap());
    assert!(img.0 < 80, "only b is in the picture: {} wide", img.0);
    assert_eq!(pixel(&img, img.0 / 2, img.1 / 2), [0, 0, 255, 255]);
    assert_eq!(pixel(&img, 0, 0)[3], 0, "transparent padding");
}

#[test]
fn an_image_too_large_for_the_device_is_refused_with_advice() {
    let renderer = Renderer::headless().unwrap();
    let mut s = stage();
    s.add(rect("a", 0.0, 0.0, 1000.0, 1000.0)).unwrap();
    let err =
        s.plan_png(&PngOptions { scale: 1.0e6, ..Default::default() }, renderer.max_side()).err().unwrap().to_string();
    assert!(err.contains("smaller scale"), "{err}");
    assert!(stage().plan_png(&PngOptions::default(), 4096).is_err(), "nothing to export");
}
