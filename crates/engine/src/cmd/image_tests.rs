//! Tests of the `image` family: availability, refusal text, effect, undo and negative cases.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use drafftink_scene::{ElementKind, FileId, SceneRead, SceneTx};
use serde_json::{Value, json};

use crate::batch::{BatchOpts, BatchStep};
use crate::ports::FileRef;
use crate::refusal::Refusal;
use crate::ring::Origin;
use crate::session::Session;
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

fn refuse_clean(s: &mut Session, id: &str, args: Value) -> Refusal {
    let before = s.fingerprint();
    let r = refuse(s, id, args);
    assert_eq!(s.fingerprint(), before, "{id} refused but changed state: {}", r.message);
    r
}

/// A PNG of `w` x `h` pixels (flat colour, so it is small).
pub(crate) fn png(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Grayscale);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().unwrap();
        writer.write_image_data(&vec![200u8; (w * h) as usize]).unwrap();
    }
    out
}

/// A PNG of noise, which does not compress: over 2 MiB at this size.
fn noisy_png(w: u32, h: u32) -> Vec<u8> {
    let mut data = Vec::with_capacity((w * h * 3) as usize);
    let mut x: u32 = 0x1234_5678;
    for _ in 0..w * h * 3 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        data.push((x >> 8) as u8);
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut writer = enc.write_header().unwrap();
        writer.write_image_data(&data).unwrap();
    }
    out
}

fn b64(bytes: &[u8]) -> Value {
    json!({ "base64": STANDARD.encode(bytes) })
}

fn named(bytes: &[u8], name: &str) -> Value {
    json!({ "base64": STANDARD.encode(bytes), "name": name })
}

fn images(s: &Session) -> Vec<drafftink_scene::Element> {
    s.store().iter().filter(|e| matches!(e.kind, ElementKind::Image(_))).cloned().collect()
}

fn session() -> Session {
    testing::headless_session()
}

// ---- GIF, BMP and ICO ----

/// A 1x1 GIF, written by hand.
pub(crate) const GIF_1X1: &[u8] = &[
    b'G', b'I', b'F', b'8', b'9', b'a', 1, 0, 1, 0, 0x80, 0, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0x21, 0xf9, 4, 0, 0, 0, 0, 0, 0x2c, 0, 0, 0,
    0, 1, 0, 1, 0, 0, 2, 2, 0x44, 1, 0, 0x3b,
];

/// A 2x1 BMP (24 bits), written by hand: a red and a blue pixel.
pub(crate) fn bmp_2x1() -> Vec<u8> {
    let mut out = b"BM".to_vec();
    out.extend_from_slice(&58u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&[2, 0, 0, 0, 1, 0, 0, 0, 1, 0, 24, 0]);
    out.extend_from_slice(&[0; 24]);
    out.extend_from_slice(&[0, 0, 255, 255, 0, 0, 0, 0]);
    out
}

/// A square RGBA PNG of one colour. An icon reads only PNGs of this kind.
fn rgba_png(side: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, side, side);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().unwrap();
        writer.write_image_data(&[200u8, 0, 0, 255].repeat((side * side) as usize)).unwrap();
    }
    out
}

/// An icon with a 4 px and a 20 px picture (PNG inside), both declared with 32 bits.
pub(crate) fn ico_4_and_20() -> Vec<u8> {
    let (small, large) = (rgba_png(4), rgba_png(20));
    let mut out = vec![0, 0, 1, 0, 2, 0];
    for (side, data, offset) in [(4u8, &small, 38usize), (20, &large, 38 + small.len())] {
        out.extend_from_slice(&[side, side, 0, 0, 1, 0, 32, 0]);
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
    }
    out.extend_from_slice(&small);
    out.extend_from_slice(&large);
    out
}

#[test]
fn gif_bmp_and_ico_become_png_pictures_of_the_size_they_have() {
    let mut s = session();
    let files = [(GIF_1X1.to_vec(), "dot.gif", (1, 1)), (bmp_2x1(), "pair.bmp", (2, 1)), (ico_4_and_20(), "app.ico", (20, 20))];
    for (bytes, name, (w, h)) in files {
        let out = run(&mut s, "image.insert", json!({"images": [named(&bytes, name)], "at": [0, 0]}));
        let one = &out["images"][0];
        assert_eq!((one["natural_width"].as_u64(), one["natural_height"].as_u64()), (Some(w), Some(h)), "{name}");
        assert_eq!((one["width"].as_f64(), one["height"].as_f64()), (Some(w as f64), Some(h as f64)), "{name}");
        // What is stored is a PNG, not the file as it came.
        assert!(s.store().file(&FileId::of(&bytes)).is_none(), "{name}: the original bytes are not stored");
        let id = FileId::new(one["file_id"].as_str().unwrap()).unwrap();
        let entry = s.store().file(&id).unwrap();
        assert_eq!(entry.mime, "image/png", "{name}");
        assert!(entry.bytes.starts_with(&[0x89, b'P', b'N', b'G']), "{name}");
    }
    assert_eq!(images(&s).len(), 3);
}

// ---- availability ----

#[test]
fn it_is_always_available_but_not_in_view_only_mode() {
    let mut s = session();
    assert!(s.availability("image.insert").is_ok());
    s.view_only = true;
    let r = s.availability("image.insert").unwrap_err();
    assert_eq!(r.code, "view_only");
    let e = refuse(&mut s, "image.insert", json!({"images": [b64(&png(4, 4))]}));
    assert_eq!((e.code, e.message.as_str()), (r.code, r.message.as_str()));
}

// ---- no outline ----

use drafftink_scene::Color;

#[test]
fn an_inserted_picture_has_no_outline_but_keeps_the_default_width() {
    let mut s = session();
    run(&mut s, "image.insert", json!({"images": [b64(&png(100, 50)), b64(&png(20, 20))], "at": [0, 0]}));
    let pictures = images(&s);
    assert_eq!(pictures.len(), 2);
    for e in &pictures {
        // The stroke is clear, so nothing is drawn; a colour picked later outlines at the usual width.
        assert_eq!(e.style.stroke_color, Color::TRANSPARENT, "{}", e.id);
        assert_eq!(e.style.stroke_width, drafftink_scene::Style::default().stroke_width);
    }
    assert_ne!(pictures[0].style.seed, pictures[1].style.seed, "each picture has its own seed");
}

#[test]
fn the_stroke_of_the_new_shapes_does_not_reach_a_picture() {
    let mut s = session();
    run(&mut s, "style.set", json!({"props": {"stroke_color": "red-500", "stroke_width": 8}, "scope": "defaults"}));
    run(&mut s, "image.insert", json!({"images": [b64(&png(10, 10))], "at": [0, 0]}));
    assert_eq!(images(&s)[0].style.stroke_color, Color::TRANSPARENT);
}

#[test]
fn an_outline_can_be_given_to_an_inserted_picture_and_taken_away_with_undo() {
    let mut s = session();
    let out = run(&mut s, "image.insert", json!({"images": [b64(&png(10, 10))], "at": [0, 0]}));
    let id = out["ids"][0].as_str().unwrap().to_owned();
    run(&mut s, "style.set", json!({"ids": [id], "props": {"stroke_color": "red-500"}}));
    assert_eq!(images(&s)[0].style.stroke_color, drafftink_scene::palette::lookup("red-500").unwrap());
    run(&mut s, "history.undo", json!({}));
    assert_eq!(images(&s)[0].style.stroke_color, Color::TRANSPARENT);
}

// ---- the sizing rule ----

#[test]
fn a_big_picture_is_scaled_to_fit_800_and_centred_on_at() {
    let mut s = session();
    let out = run(&mut s, "image.insert", json!({"images": [b64(&png(1000, 500))], "at": [100, 50]}));
    let ids = out["ids"].as_array().unwrap();
    assert_eq!(ids.len(), 1);
    let e = &images(&s)[0];
    assert!((e.transform.w - 800.0).abs() < 1e-9 && (e.transform.h - 400.0).abs() < 1e-9, "{:?}", e.transform);
    assert!((e.transform.x - (100.0 - 400.0)).abs() < 1e-9 && (e.transform.y - (50.0 - 200.0)).abs() < 1e-9);
    let ElementKind::Image(d) = &e.kind else { panic!() };
    assert_eq!((d.natural_width, d.natural_height), (1000, 500));
    assert!(d.file_id.is_some() && s.store().file(d.file_id.as_ref().unwrap()).is_some());
    assert_eq!(s.selection().iter().next(), Some(&e.id));
    assert_eq!(out["images"][0]["width"], 800.0);
    assert_eq!(out["created"].as_array().unwrap().len(), 1);
}

#[test]
fn a_small_picture_keeps_its_size() {
    let mut s = session();
    run(&mut s, "image.insert", json!({"images": [b64(&png(100, 50))], "at": [0, 0]}));
    let t = images(&s)[0].transform;
    assert_eq!((t.w, t.h), (100.0, 50.0));
}

#[test]
fn fit_changes_the_limit() {
    let mut s = session();
    run(&mut s, "image.insert", json!({"images": [b64(&png(400, 200))], "fit": 100, "at": [0, 0]}));
    let t = images(&s)[0].transform;
    assert_eq!((t.w, t.h), (100.0, 50.0));
}

#[test]
fn the_default_place_is_the_middle_of_the_view() {
    let mut s = session();
    run(&mut s, "view.set", json!({"scroll": [300, 200]}));
    run(&mut s, "image.insert", json!({"images": [b64(&png(100, 100))]}));
    let t = images(&s)[0].transform;
    assert_eq!((t.x + t.w / 2.0, t.y + t.h / 2.0), (300.0, 200.0));
}

#[test]
fn several_pictures_cascade_by_offset_and_the_same_bytes_are_stored_once() {
    let mut s = session();
    let p = png(100, 100);
    let out = run(&mut s, "image.insert", json!({"images": [b64(&p), b64(&p), b64(&png(10, 10))], "at": [0, 0], "offset": 30}));
    let es = images(&s);
    assert_eq!(es.len(), 3);
    let centre = |i: usize| (es[i].transform.x + es[i].transform.w / 2.0, es[i].transform.y + es[i].transform.h / 2.0);
    assert_eq!((centre(0), centre(1), centre(2)), ((0.0, 0.0), (30.0, 30.0), (60.0, 60.0)));
    assert_eq!(s.store().files().count(), 2, "two different pictures, three elements");
    assert_eq!(out["images"][0]["reused"], false);
    assert_eq!(out["images"][1]["reused"], true);
    assert_eq!(s.selection().len(), 3);
    // Inserting the same picture again stores nothing new.
    let out = run(&mut s, "image.insert", json!({"images": [b64(&p)]}));
    assert_eq!(out["images"][0]["reused"], true);
    assert_eq!(s.store().files().count(), 2);
}

#[test]
fn select_false_leaves_the_selection_alone() {
    let (mut s, ids) = testing::session_with_shapes().unwrap();
    run(&mut s, "image.insert", json!({"images": [b64(&png(8, 8))], "select": false}));
    assert_eq!(s.selection().iter().collect::<Vec<_>>(), vec![&ids[0]]);
}

// ---- undo ----

#[test]
fn one_call_is_one_undo_step_and_undo_removes_the_images() {
    let mut s = session();
    let p = png(20, 20);
    let out = run(&mut s, "image.insert", json!({"images": [b64(&p), b64(&png(30, 30))]}));
    assert_eq!(out["undo_label"], "Insert image");
    assert_eq!(s.tab().doc.undo_depth(), 1);
    s.undo_steps(1).unwrap();
    assert!(images(&s).is_empty());
    assert!(s.selection().is_empty(), "undo restores the selection from before");
    s.redo_steps(1).unwrap();
    assert_eq!(images(&s).len(), 2);
}

#[test]
fn a_batch_of_inserts_is_one_step() {
    let mut s = session();
    let step = || BatchStep::new("image.insert", json!({"images": [b64(&png(5, 5))]}));
    let out = s.execute_batch(Origin::Test, &[step(), step()], &BatchOpts::default());
    assert!(out.failed.is_none());
    assert_eq!(images(&s).len(), 2);
    assert_eq!(s.tab().doc.undo_depth(), 1);
}

// ---- refusals ----

#[test]
fn other_formats_are_unsupported_and_the_name_is_in_the_message() {
    let mut s = session();
    let r = refuse_clean(&mut s, "image.insert", json!({"images": [named(b"%PDF-1.4 ....", "paper.pdf")]}));
    assert_eq!(r.code, "unsupported_format");
    assert_eq!(r.message, "`paper.pdf` is not a supported image (it is a PDF). Use PNG, JPEG, WebP, GIF, BMP or ICO.");
    let r = refuse_clean(&mut s, "image.insert", json!({"images": [b64(b"hello there")]}));
    assert_eq!(r.code, "unsupported_format");
    assert!(r.message.starts_with("`image 1` is not a supported image"), "{}", r.message);
    let r = refuse_clean(&mut s, "image.insert", json!({"images": [b64(b"hello")], "name": "notes.txt"}));
    assert!(r.message.starts_with("`notes.txt` is not"), "{}", r.message);
}

#[test]
fn a_damaged_picture_is_parse_failed() {
    let mut s = session();
    let good = png(16, 16);
    let r = refuse_clean(&mut s, "image.insert", json!({"images": [named(&good[..14], "cut.png")]}));
    assert_eq!(r.code, "parse_failed");
    assert!(r.message.starts_with("cut.png could not be read as an image: "), "{}", r.message);
    assert!(r.message.ends_with("Try another file."), "{}", r.message);
    let r = refuse_clean(&mut s, "image.insert", json!({"images": [{"base64": "not base64 at all!!"}]}));
    assert_eq!(r.code, "parse_failed");
}

#[test]
fn a_picture_with_damaged_pixel_data_is_refused_when_inserted_not_when_drawn() {
    let mut s = session();
    let good = png(64, 64);
    // The header says 64 x 64 and is fine; the pixel data ends early.
    let r = refuse_clean(&mut s, "image.insert", json!({"images": [named(&good[..good.len() - 12], "half.png")]}));
    assert_eq!(r.code, "parse_failed");
    assert!(r.message.starts_with("half.png could not be read as an image: "), "{}", r.message);
    assert!(s.store().files().next().is_none(), "nothing was stored");
    // The same picture whole is inserted.
    run(&mut s, "image.insert", json!({"images": [named(&good, "whole.png")]}));
    assert_eq!(s.store().files().count(), 1);
}

#[test]
fn a_bad_picture_among_good_ones_changes_nothing() {
    let mut s = session();
    let r = refuse_clean(&mut s, "image.insert", json!({"images": [b64(&png(8, 8)), b64(b"junk"), b64(&png(9, 9))]}));
    assert_eq!(r.code, "unsupported_format");
    assert!(s.store().files().next().is_none());
    assert!(r.message.contains("image 2"), "the second picture is named: {}", r.message);
}

#[test]
fn the_count_and_the_size_are_limited() {
    let mut s = session();
    let p = b64(&png(2, 2));
    let r = refuse_clean(&mut s, "image.insert", json!({"images": vec![p; 21]}));
    assert_eq!(r.code, "limit_exceeded");
    assert_eq!(r.message, "`image.insert` was given 21 images, and the limit is 20. Split the work into several calls.");
    let r = refuse_clean(&mut s, "image.insert", json!({"images": []}));
    assert_eq!(r.code, "bad_args");
    assert!(r.message.contains("images"), "{}", r.message);
    let big = json!({"base64": "A".repeat(40 * 1024 * 1024)});
    let r = refuse_clean(&mut s, "image.insert", json!({"images": [big]}));
    assert_eq!(r.code, "limit_exceeded");
    assert!(r.message.contains("26214400"), "{}", r.message);
}

#[test]
fn numbers_are_checked() {
    let mut s = session();
    let img = b64(&png(2, 2));
    for bad in [json!({"fit": 0}), json!({"fit": -5}), json!({"offset": 1e300}), json!({"at": [1e300, 0]})] {
        let mut args = json!({"images": [img.clone()]});
        args.as_object_mut().unwrap().extend(bad.as_object().unwrap().clone());
        let r = refuse_clean(&mut s, "image.insert", args);
        assert_eq!(r.code, "bad_args");
    }
}

#[test]
fn a_picture_over_the_size_limit_is_refused_before_it_is_decoded() {
    let mut s = session();
    // A real PNG whose width is over the 32768 pixel limit; it is tiny, so only the header can refuse it.
    let p = png(32_769, 1);
    let r = refuse_clean(&mut s, "image.insert", json!({"images": [named(&p, "huge.png")]}));
    assert_eq!(r.code, "limit_exceeded");
    assert!(r.message.contains("huge.png"), "{}", r.message);
}

// ---- inputs ----

#[test]
fn a_data_url_with_line_breaks_is_accepted() {
    let mut s = session();
    let text = format!("data:image/png;base64,{}", STANDARD.encode(png(6, 6)).as_bytes().chunks(20).map(|c| std::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join("\n"));
    run(&mut s, "image.insert", json!({"images": [{"base64": text}]}));
    assert_eq!(images(&s).len(), 1);
}

#[test]
fn a_path_is_read_through_the_files_port() {
    let (mut s, fakes) = testing::session();
    fakes.files.insert(&FileRef::Path { path: "/tmp/pics/a.png".into() }, &png(40, 20), "image/png");
    run(&mut s, "image.insert", json!({"images": [{"path": "/tmp/pics/a.png"}], "at": [0, 0]}));
    assert_eq!(images(&s).len(), 1);
    let r = refuse_clean(&mut s, "image.insert", json!({"images": [{"path": "/tmp/pics/missing.png"}]}));
    assert_eq!(r.code, "io_failed");
    assert!(r.message.contains("/tmp/pics/missing.png"), "{}", r.message);
    let mut headless = session();
    let r = refuse_clean(&mut headless, "image.insert", json!({"images": [{"path": "/x.png"}]}));
    assert_eq!(r.code, "port_unavailable");
    assert!(r.message.contains("`base64`"), "{}", r.message);
}

#[test]
fn a_big_picture_is_stored_smaller_unless_downscale_is_off() {
    let big = noisy_png(2300, 1200);
    assert!(big.len() > 2 * 1024 * 1024, "{}", big.len());
    let mut s = session();
    let out = run(&mut s, "image.insert", json!({"images": [b64(&big)], "at": [0, 0]}));
    let i = &out["images"][0];
    assert_eq!(i["shrunk"], true);
    assert_eq!((i["natural_width"].as_u64(), i["natural_height"].as_u64()), (Some(2048), Some(1069)));
    // On the canvas it is still the 800 rule applied to the original size.
    assert_eq!(i["width"], 800.0);
    let stored = s.store().files().next().unwrap().1.bytes.len();
    assert!(stored < big.len());

    let mut s = session();
    let out = run(&mut s, "image.insert", json!({"images": [b64(&big)], "downscale": false}));
    assert_eq!(out["images"][0]["shrunk"], false);
    assert_eq!(out["images"][0]["natural_width"], 2300);
    let id: FileId = FileId::new(out["images"][0]["file_id"].as_str().unwrap()).unwrap();
    assert_eq!(s.store().file(&id).unwrap().bytes.len(), big.len());
}

#[test]
fn random_bytes_never_panic() {
    let mut s = session();
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    for round in 0..300 {
        let n = (round % 64) + 1;
        let mut bytes = Vec::new();
        for _ in 0..n {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            bytes.push(x as u8);
        }
        // Dress some of them as pictures.
        match round % 3 {
            0 => bytes.splice(0..0, [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]).for_each(drop),
            1 => bytes.splice(0..0, [0xff, 0xd8, 0xff, 0xe0]).for_each(drop),
            _ => bytes.splice(0..0, *b"RIFF\0\0\0\0WEBPVP8 ").for_each(drop),
        }
        let r = s.execute("image.insert", json!({"images": [b64(&bytes)]}));
        if let Err(r) = r {
            assert_ne!(r.code, "internal", "{}", r.message);
        }
    }
}

// ---- crop ----

/// A 200 by 100 picture shown at 200 by 100 with its top-left corner at the origin: one unit per pixel.
fn cropped_session() -> (Session, String) {
    let mut s = session();
    let out = run(&mut s, "image.insert", json!({"images": [b64(&png(200, 100))], "at": [100, 50]}));
    let id = out["ids"][0].as_str().unwrap().to_owned();
    (s, id)
}

fn crop_of(s: &Session, id: &str) -> Option<drafftink_scene::Crop> {
    let id = drafftink_scene::ElementId::new(id).unwrap();
    s.element(&id).and_then(|e| e.image_data()).and_then(|d| d.crop.clone())
}

fn box_of(s: &Session, id: &str) -> [f64; 4] {
    let t = s.element(&drafftink_scene::ElementId::new(id).unwrap()).unwrap().transform;
    [t.x, t.y, t.w, t.h]
}

fn assert_box(got: [f64; 4], want: [f64; 4]) {
    assert!(got.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-9), "{got:?} != {want:?}");
}

#[test]
fn crop_shrinks_the_box_around_the_cropped_pixels_by_default() {
    let (mut s, id) = cropped_session();
    let out = run(&mut s, "image.crop", json!({"id": id, "crop": {"x": 50, "y": 0, "w": 100, "h": 100}}));
    assert_eq!(out["changed"], true);
    assert_eq!(out["crop"], json!({"x": 50.0, "y": 0.0, "w": 100.0, "h": 100.0}));
    assert_eq!((out["natural_width"].as_u64(), out["natural_height"].as_u64()), (Some(200), Some(100)));
    // Same scale as before (one unit per pixel): a 100 by 100 box that starts 50 units right of the old corner.
    assert_box(box_of(&s, &id), [50.0, 0.0, 100.0, 100.0]);
    assert_eq!(crop_of(&s, &id).unwrap().width, 100.0);
}

#[test]
fn crop_can_keep_the_box_and_stretch_the_picture_into_it() {
    let (mut s, id) = cropped_session();
    run(&mut s, "image.crop", json!({"id": id, "crop": {"x": 50, "y": 0, "w": 100, "h": 100}, "element_box": "keep"}));
    assert_box(box_of(&s, &id), [0.0, 0.0, 200.0, 100.0]);
    assert_eq!(crop_of(&s, &id).unwrap().x, 50.0);
}

#[test]
fn crop_with_bounds_puts_the_box_exactly_there() {
    let (mut s, id) = cropped_session();
    run(&mut s, "image.crop", json!({"id": id, "crop": {"x": 0, "y": 0, "w": 100, "h": 100}, "bounds": {"x": 7, "y": 8, "w": 30, "h": 40}}));
    assert_box(box_of(&s, &id), [7.0, 8.0, 30.0, 40.0]);
    let r = refuse_clean(&mut s, "image.crop", json!({"id": id, "crop": null, "bounds": {"x": 0, "y": 0, "w": 0, "h": 5}}));
    assert_eq!(r.code, "bad_args");
}

#[test]
fn crop_defaults_to_the_selected_image_and_reset_gives_the_picture_back() {
    let (mut s, id) = cropped_session();
    run(&mut s, "image.crop", json!({"crop": {"x": 100, "y": 50, "w": 100, "h": 50}}));
    assert_box(box_of(&s, &id), [100.0, 50.0, 100.0, 50.0]);
    let out = run(&mut s, "image.reset_crop", json!({}));
    assert_eq!(out["crop"], Value::Null);
    assert_box(box_of(&s, &id), [0.0, 0.0, 200.0, 100.0]);
    assert!(crop_of(&s, &id).is_none());
    // Nothing left to reset: the control is greyed out with a reason.
    let r = s.availability("image.reset_crop").unwrap_err();
    assert_eq!(r.code, "nothing_to_do", "{}", r.message);
}

#[test]
fn crop_null_is_a_reset_and_a_whole_picture_crop_is_no_crop() {
    let (mut s, id) = cropped_session();
    run(&mut s, "image.crop", json!({"id": id, "crop": {"x": 10, "y": 10, "w": 50, "h": 50}}));
    run(&mut s, "image.crop", json!({"id": id, "crop": null}));
    assert!(crop_of(&s, &id).is_none());
    run(&mut s, "image.crop", json!({"id": id, "crop": {"x": 10, "y": 10, "w": 50, "h": 50}}));
    let out = run(&mut s, "image.crop", json!({"id": id, "crop": {"x": 0, "y": 0, "w": 200, "h": 100}}));
    assert_eq!(out["crop"], Value::Null);
    assert!(crop_of(&s, &id).is_none());
}

#[test]
fn crop_is_one_undo_step_and_undo_restores_the_crop_and_the_box() {
    let (mut s, id) = cropped_session();
    let before = box_of(&s, &id);
    run(&mut s, "image.crop", json!({"id": id, "crop": {"x": 50, "y": 20, "w": 80, "h": 40}}));
    assert!(crop_of(&s, &id).is_some());
    run(&mut s, "history.undo", json!({}));
    assert!(crop_of(&s, &id).is_none());
    assert_box(box_of(&s, &id), before);
    run(&mut s, "history.redo", json!({}));
    assert_eq!(crop_of(&s, &id).unwrap().x, 50.0);
}

#[test]
fn a_repeated_crop_changes_nothing_and_makes_no_undo_step() {
    let (mut s, id) = cropped_session();
    run(&mut s, "image.insert", json!({"images": [b64(&png(10, 10))], "at": [500, 500]}));
    run(&mut s, "image.crop", json!({"id": id, "crop": {"x": 50, "y": 20, "w": 80, "h": 40}}));
    let out = run(&mut s, "image.crop", json!({"id": id, "crop": {"x": 50, "y": 20, "w": 80, "h": 40}}));
    assert_eq!(out["changed"], false);
    run(&mut s, "history.undo", json!({}));
    assert!(crop_of(&s, &id).is_none(), "one undo removes the crop, so the repeat made no step of its own");
}

#[test]
fn crop_refusals_say_what_to_change_and_leave_everything_alone() {
    let (mut s, id) = cropped_session();
    let zero = refuse_clean(&mut s, "image.crop", json!({"id": id, "crop": {"x": 0, "y": 0, "w": 0, "h": 10}}));
    assert_eq!(zero.code, "bad_crop");
    assert!(zero.message.contains("at least 1 pixel"), "{}", zero.message);
    let outside = refuse_clean(&mut s, "image.crop", json!({"id": id, "crop": {"x": 150, "y": 0, "w": 100, "h": 10}}));
    assert_eq!(outside.code, "bad_crop");
    assert!(outside.message.contains("200 by 100"), "{}", outside.message);
    assert_eq!(refuse_clean(&mut s, "image.crop", json!({"id": id, "crop": {"x": -5, "y": 0, "w": 10, "h": 10}})).code, "bad_crop");
    assert_eq!(refuse_clean(&mut s, "image.crop", json!({"id": id, "crop": {"x": 0, "y": 0, "w": 10}})).code, "bad_args");
    assert_eq!(refuse_clean(&mut s, "image.crop", json!({"id": "nope", "crop": null})).code, "not_found");
    assert!(crop_of(&s, &id).is_none());
}

#[test]
fn crop_refuses_what_is_not_an_image_and_what_is_locked() {
    let (mut s, id) = cropped_session();
    let rect = testing::add_rect(&mut s, 0.0, 0.0, 50.0, 50.0).unwrap();
    let r = refuse_clean(&mut s, "image.crop", json!({"id": rect.as_str(), "crop": null}));
    assert_eq!(r.code, "wrong_kind");
    assert!(r.message.contains("images"), "{}", r.message);
    run(&mut s, "element.lock", json!({"ids": [id]}));
    let locked = refuse_clean(&mut s, "image.crop", json!({"id": id, "crop": {"x": 0, "y": 0, "w": 10, "h": 10}}));
    assert_eq!(locked.code, "locked");
    assert_eq!(refuse_clean(&mut s, "image.crop_begin", json!({"id": id})).code, "locked");
}

#[test]
fn crop_refuses_a_picture_that_is_not_in_the_document() {
    let (mut s, id) = cropped_session();
    let eid = drafftink_scene::ElementId::new(&id).unwrap();
    s.tab_mut()
        .doc
        .edit::<(), drafftink_scene::StoreError>(|tx| {
            tx.update(&eid, |e| {
                if let ElementKind::Image(d) = &mut e.kind {
                    d.file_id = None;
                }
            })
        })
        .unwrap();
    let r = refuse_clean(&mut s, "image.crop", json!({"id": id, "crop": {"x": 0, "y": 0, "w": 10, "h": 10}}));
    assert_eq!(r.code, "picture_not_loaded");
    assert!(r.message.contains("not arrived"), "{}", r.message);
    assert_eq!(s.availability("image.crop_begin").unwrap_err().code, "picture_not_loaded");
}

#[test]
fn crop_needs_one_selected_image_when_no_id_is_given() {
    let mut s = session();
    let r = refuse_clean(&mut s, "image.crop", json!({"crop": null}));
    assert_eq!(r.code, "needs_single");
    assert!(s.availability("image.crop").is_err());
}

#[test]
fn a_rotated_and_mirrored_image_crops_what_is_seen() {
    let (mut s, id) = cropped_session();
    run(&mut s, "element.rotate", json!({"ids": [id], "angle": 90, "mode": "to"}));
    run(&mut s, "element.flip", json!({"ids": [id], "axis": "horizontal"}));
    let before = s.element(&drafftink_scene::ElementId::new(&id).unwrap()).unwrap().clone();
    let (cx, cy) = (before.transform.x + before.transform.w / 2.0, before.transform.y + before.transform.h / 2.0);
    // The image is a 200 by 100 box turned a quarter: the middle of the box stays where it is for a crop of the
    // middle of the picture.
    run(&mut s, "image.crop", json!({"id": id, "crop": {"x": 50, "y": 25, "w": 100, "h": 50}}));
    let after = s.element(&drafftink_scene::ElementId::new(&id).unwrap()).unwrap();
    assert_eq!((after.transform.w, after.transform.h), (100.0, 50.0));
    assert!(((after.transform.x + 50.0) - cx).abs() < 1e-9 && ((after.transform.y + 25.0) - cy).abs() < 1e-9);
    assert_eq!(after.transform.angle, before.transform.angle);
    assert_eq!((after.transform.flip_x, after.transform.flip_y), (before.transform.flip_x, before.transform.flip_y));
}

#[test]
fn crop_begin_and_end_open_and_close_the_editor_without_an_undo_step() {
    let (mut s, id) = cropped_session();
    run(&mut s, "selection.clear", json!({}));
    let out = run(&mut s, "image.crop_begin", json!({"id": id}));
    assert_eq!(out["changed"], true);
    assert_eq!(s.selection().iter().map(|i| i.as_str()).collect::<Vec<_>>(), vec![id.as_str()]);
    let summary = run(&mut s, "selection.get", json!({"detail": "summary"}));
    assert_eq!(summary["crop_editor"], json!(id));
    assert_eq!(run(&mut s, "image.crop_begin", json!({"id": id}))["changed"], false);
    let end = run(&mut s, "image.crop_end", json!({}));
    assert_eq!(end["changed"], true);
    assert!(s.availability("image.crop_end").is_err());
    assert_eq!(run(&mut s, "history.undo", json!({}))["undone"], json!(["Insert image"]), "the insert is still the last step");
}

#[test]
fn the_crop_editor_closes_with_the_selection_the_tool_and_esc() {
    let (mut s, id) = cropped_session();
    run(&mut s, "image.crop_begin", json!({"id": id}));
    run(&mut s, "selection.clear", json!({}));
    assert!(s.tab().observed.crop_editor.is_none());
    run(&mut s, "image.crop_begin", json!({"id": id}));
    run(&mut s, "tool.set", json!({"tool": "rectangle"}));
    assert!(s.tab().observed.crop_editor.is_none());
    run(&mut s, "image.crop_begin", json!({"id": id}));
    assert_eq!(s.tool.active, crate::state::Tool::Select);
    let out = run(&mut s, "tool.cancel", json!({}));
    assert_eq!(out["rungs"], json!(["crop_editor"]));
    assert!(s.selection().iter().next().is_some(), "Esc closed the editor and left the selection");
}

#[test]
fn a_crop_closes_the_editor_of_its_image() {
    let (mut s, id) = cropped_session();
    run(&mut s, "image.crop_begin", json!({"id": id}));
    run(&mut s, "image.crop", json!({"id": id, "crop": {"x": 0, "y": 0, "w": 50, "h": 50}}));
    assert!(s.tab().observed.crop_editor.is_none());
}

#[test]
fn a_collaborator_deleting_the_image_closes_the_editor() {
    let (mut s, id) = cropped_session();
    run(&mut s, "image.crop_begin", json!({"id": id}));
    run(&mut s, "element.delete", json!({"ids": [id]}));
    assert!(s.tab().observed.crop_editor.is_none());
}
