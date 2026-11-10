//! Tests of the `image` family: availability, refusal text, effect, undo and negative cases.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use drafftink_scene::{ElementKind, FileId, SceneRead};
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
    let r = refuse_clean(&mut s, "image.insert", json!({"images": [named(b"GIF89a\x01\x00\x01\x00", "cat.gif")]}));
    assert_eq!(r.code, "unsupported_format");
    assert_eq!(r.message, "`cat.gif` is not a supported image (it is a GIF). Use PNG, JPEG or WebP.");
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
