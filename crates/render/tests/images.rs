#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Pictures: decode on demand, placeholders, deferred decoding, bounds.

mod common;

use std::io::Cursor;
use std::sync::Arc;

use common::*;
use drafftink_render::*;
use drafftink_scene::*;
use drafftink_typeset::Typesetter;
use kurbo::{Shape as _, Size, Vec2};
use vello::Scene;

fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255]));
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img).write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn picture(name: &str, file: Option<FileId>, w: u32, h: u32) -> Element {
    let data = ImageData { file_id: file, natural_width: w, natural_height: h, crop: None };
    let mut e = Element::new(id(name), ElementKind::Image(data), ZIndex::first());
    e.transform = Transform::new(10.0, 10.0, f64::from(w), f64::from(h));
    e.style.stroke_width = 0.0;
    e
}

struct Rig {
    store: MemoryStore,
    index: PaintIndex,
    ts: Typesetter,
    composer: SceneComposer,
    scene: Scene,
}

impl Rig {
    fn with(elements: Vec<Element>, blobs: Vec<Vec<u8>>) -> (Self, Vec<FileId>) {
        let mut store = MemoryStore::new();
        let mut tx = store.begin();
        let mut ids = Vec::new();
        for b in blobs {
            let (fid, entry) = FileEntry::new("image/png", b);
            tx.put_file(entry).unwrap();
            ids.push(fid);
        }
        for e in elements {
            tx.insert(e).unwrap();
        }
        tx.commit().unwrap();
        let index = build_paint_index(&store);
        (Self { store, index, ts: Typesetter::new(), composer: SceneComposer::new(), scene: Scene::new() }, ids)
    }

    fn compose(&mut self) -> FrameReport {
        let v = View::new(1.0, Vec2::ZERO, Size::new(800.0, 600.0));
        self.composer.compose(&self.store, &self.index, &v, &mut self.ts, &mut self.scene)
    }
}

#[test]
fn decoding_checks_limits_and_garbage() {
    let ok = decode_image(&png(7, 5)).unwrap();
    assert_eq!((ok.width, ok.height), (7, 5));
    assert_eq!(ok.data.data().len(), 7 * 5 * 4);
    assert!(matches!(decode_image(b"not a picture"), Err(ImageError::Decode(_))));
    assert!(matches!(decode_image(&[]), Err(ImageError::Decode(_))));
    assert_eq!(decode_image(&png(MAX_IMAGE_SIDE + 1, 1)), Err(ImageError::TooLarge));
    // A truncated file is an error, not a panic.
    let full = png(40, 40);
    assert!(decode_image(&full[..full.len() / 2]).is_err());
}

#[test]
fn inline_policy_decodes_on_first_sight_then_hits() {
    let bytes = png(20, 10);
    let fid = FileId::of(&bytes);
    let (mut rig, _) = Rig::with(vec![picture("p", Some(fid), 20, 10)], vec![bytes]);
    let first = rig.compose();
    assert_eq!((first.cache_misses, first.images_pending), (1, 0), "{first:?}");
    assert_eq!(rig.composer.painter().images().stats().decoded, 1);
    let second = rig.compose();
    assert_eq!((second.cache_misses, second.cache_hits), (0, 1));
    assert_eq!(rig.composer.painter().images().stats().decoded, 1, "decoded once");
}

#[test]
fn deferred_policy_shows_a_placeholder_until_the_host_completes_the_decode() {
    let bytes = png(20, 10);
    let fid = FileId::of(&bytes);
    let (mut rig, _) = Rig::with(vec![picture("p", Some(fid.clone()), 20, 10)], vec![bytes]);
    rig.composer.painter_mut().images_mut().set_policy(DecodePolicy::Deferred);

    let first = rig.compose();
    assert_eq!((first.cache_misses, first.images_pending), (1, 1), "{first:?}");
    let reqs = rig.composer.painter_mut().images_mut().take_requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].file, fid);
    assert_eq!(rig.composer.painter().images().stats().decoded, 0);

    // Still waiting: the placeholder fragment is served from the cache, not encoded again.
    let again = rig.compose();
    assert_eq!((again.cache_misses, again.cache_hits, again.images_pending), (0, 1, 1), "{again:?}");
    assert_eq!(rig.composer.painter().images().pending_requests(), 0, "no second request");

    // The host decodes (on any thread) and reports back.
    let result = decode_image(&reqs[0].bytes);
    rig.composer.painter_mut().images_mut().complete(&reqs[0].file, result);
    let done = rig.compose();
    assert_eq!((done.cache_misses, done.images_pending), (1, 0), "{done:?}");
    let after = rig.compose();
    assert_eq!((after.cache_misses, after.cache_hits), (0, 1));
}

#[test]
fn a_blob_that_arrives_later_replaces_the_placeholder() {
    let bytes = png(16, 16);
    let fid = FileId::of(&bytes);
    let (mut rig, _) = Rig::with(vec![picture("p", Some(fid), 16, 16)], vec![]);
    let first = rig.compose();
    assert_eq!(first.images_pending, 1);
    let second = rig.compose();
    assert_eq!((second.cache_misses, second.images_pending), (0, 1), "nothing changed, nothing re-encoded");
    // The blob shows up (collaboration order), the element is untouched.
    let mut tx = rig.store.begin();
    tx.put_file(FileEntry::new("image/png", bytes).1).unwrap();
    tx.commit().unwrap();
    let third = rig.compose();
    assert_eq!((third.cache_misses, third.images_pending), (1, 0), "{third:?}");
}

#[test]
fn a_broken_picture_is_remembered_and_not_retried() {
    let junk = b"definitely not an image".to_vec();
    let fid = FileId::of(&junk);
    let (mut rig, _) = Rig::with(vec![picture("p", Some(fid), 10, 10)], vec![junk]);
    rig.compose();
    rig.compose();
    rig.compose();
    let s = rig.composer.painter().images().stats();
    assert_eq!((s.decoded, s.failed), (0, 1), "{s:?}");
}

#[test]
fn an_image_without_a_file_is_a_placeholder_and_never_waits() {
    let (mut rig, _) = Rig::with(vec![picture("p", None, 10, 10)], vec![]);
    let rep = rig.compose();
    assert_eq!((rep.drawn, rep.images_pending), (1, 0));
}

#[test]
fn inline_decoding_is_spread_over_frames_by_the_pixel_budget() {
    let blobs: Vec<Vec<u8>> = (0..5).map(|i| png(100, 100 + i)).collect();
    let fids: Vec<FileId> = blobs.iter().map(|b| FileId::of(b)).collect();
    let els: Vec<Element> = fids
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let mut e = picture(&format!("p{i}"), Some(f.clone()), 100, 100 + i as u32);
            e.index = ZIndex::rebalance(5).unwrap()[i].clone();
            e.transform.x = i as f64 * 120.0;
            e
        })
        .collect();
    let (mut rig, _) = Rig::with(els, blobs);
    rig.composer.painter_mut().images_mut().set_policy(DecodePolicy::Inline { pixels_per_frame: 100 });
    let mut decoded = Vec::new();
    for _ in 0..6 {
        rig.compose();
        decoded.push(rig.composer.painter().images().stats().decoded);
    }
    assert_eq!(decoded, [1, 2, 3, 4, 5, 5], "one picture per frame while over budget");
    let last = rig.compose();
    assert_eq!((last.images_pending, last.cache_misses), (0, 0));
}

#[test]
fn decoded_pixels_are_bounded_by_the_cache_budget() {
    let mut cache = ImageCache::new(100 * 100 * 4 * 3);
    for i in 0..20u8 {
        let fid = FileId::of(&[i]);
        cache.complete(&fid, decode_image(&png(100, 100)));
        assert!(cache.stats().bytes <= 100 * 100 * 4 * 3 + 100 * 100 * 4, "{:?}", cache.stats());
    }
    let s = cache.stats();
    assert!(s.evictions >= 15, "{s:?}");
    // The newest picture is still there.
    assert!(matches!(cache.state(&FileId::of(&[19])), Some(ImageState::Ready(_))));
    assert!(cache.state(&FileId::of(&[0])).is_none());
}

#[test]
fn a_crop_changes_the_picture_but_not_the_box() {
    let bytes = png(40, 40);
    let fid = FileId::of(&bytes);
    let mut whole = picture("a", Some(fid.clone()), 40, 40);
    whole.transform.w = 80.0;
    whole.transform.h = 40.0;
    let mut cropped = whole.clone();
    if let ElementKind::Image(d) = &mut cropped.kind {
        d.crop = Some(Crop { x: 10.0, y: 0.0, width: 20.0, height: 40.0 });
    }
    let files = move |_: &FileId| Some(Arc::<[u8]>::from(bytes.clone()));
    let mut painter = ElementPainter::new();
    let mut ts = Typesetter::new();
    let a = painter.paint(&whole, &RenderOptions::default(), &mut ts, &files);
    let b = painter.paint(&cropped, &RenderOptions::default(), &mut ts, &files);
    assert_ne!(a.scene.encoding().transforms, b.scene.encoding().transforms);
    assert_eq!(a.bounds, b.bounds);
    assert_eq!((a.bounds.width(), a.bounds.height()), (80.0, 40.0));
    assert!(a.awaiting_image.is_none() && b.awaiting_image.is_none());
}

#[test]
fn flip_and_rotation_do_not_re_encode_a_picture() {
    let bytes = png(8, 8);
    let fid = FileId::of(&bytes);
    let (mut rig, _) = Rig::with(vec![picture("p", Some(fid), 8, 8)], vec![bytes]);
    rig.compose();
    let mut tx = rig.store.begin();
    tx.update(&id("p"), |e| {
        e.transform.flip_x = true;
        e.transform.angle = 1.0;
    })
    .unwrap();
    let cs = tx.commit().unwrap();
    update_paint_index(&mut rig.index, &rig.store, &cs);
    let rep = rig.compose();
    assert_eq!((rep.cache_misses, rep.cache_reuses), (0, 1), "{rep:?}");
}

// ---- rounded corners ----

/// Records the shapes a replayed scene fills with a picture and strokes, in the space of the scene.
#[derive(Default)]
struct Shapes {
    picture: Vec<kurbo::BezPath>,
    strokes: Vec<kurbo::BezPath>,
    layers: u32,
}

impl replay::Canvas for Shapes {
    fn fill(&mut self, transform: kurbo::Affine, path: &kurbo::BezPath, _: peniko::Fill, brush: replay::Brush<'_>) {
        if matches!(brush, replay::Brush::Image { .. }) {
            self.picture.push(transform * path.clone());
        }
    }

    fn stroke(&mut self, transform: kurbo::Affine, path: &kurbo::BezPath, _: &kurbo::Stroke, _: replay::Brush<'_>) {
        self.strokes.push(transform * path.clone());
    }

    fn push_layer(&mut self, _: kurbo::Affine, _: &kurbo::BezPath, _: replay::LayerKind) {
        self.layers += 1;
    }

    fn pop_layer(&mut self) {}

    fn glyph_run(&mut self, _: &replay::GlyphDraw<'_>) {}
}

/// `got` is `want` (x0, y0, x1, y1) up to rounding.
fn assert_rect(got: kurbo::Rect, want: [f64; 4], what: &str) {
    let ok = [got.x0 - want[0], got.y0 - want[1], got.x1 - want[2], got.y1 - want[3]].iter().all(|d| d.abs() < 1e-6);
    assert!(ok, "{what}: {got:?} is not {want:?}");
}

fn shapes_of(scene: &Scene) -> Shapes {
    let mut shapes = Shapes::default();
    replay::replay(scene.encoding(), &mut shapes, replay::Detail::Full);
    shapes
}

/// A 100 by 60 picture at (10, 10) with a 20 unit radius and a red stroke.
fn rounded_picture(bytes: &[u8]) -> Element {
    let mut e = picture("p", Some(FileId::of(bytes)), 100, 60);
    e.style.corner_radius = 20.0;
    e.style.stroke_width = 4.0;
    e.style.stroke_color = Color::rgb(255, 0, 0);
    e
}

#[test]
fn the_radius_clips_the_picture_and_the_outline_in_the_element_frame() {
    let bytes = png(100, 60);
    let e = rounded_picture(&bytes);
    let files = {
        let bytes = bytes.clone();
        move |_: &FileId| Some(Arc::<[u8]>::from(bytes.clone()))
    };
    let mut painter = ElementPainter::new();
    let mut ts = Typesetter::new();
    let fragment = painter.paint(&e, &RenderOptions::default(), &mut ts, &files);
    let shapes = shapes_of(&fragment.scene);
    let [fill] = &shapes.picture[..] else { panic!("one picture fill: {}", shapes.picture.len()) };
    let [outline] = &shapes.strokes[..] else { panic!("one outline: {}", shapes.strokes.len()) };
    for (name, path) in [("picture", fill), ("outline", outline)] {
        assert_rect(path.bounding_box(), [0.0, 0.0, 100.0, 60.0], name);
        assert!(!path.contains(kurbo::Point::new(1.0, 1.0)), "{name}: the top left corner is cut off");
        assert!(!path.contains(kurbo::Point::new(99.0, 59.0)), "{name}: the bottom right corner is cut off");
        assert!(path.contains(kurbo::Point::new(50.0, 30.0)), "{name}: the middle is inside");
        assert!(path.contains(kurbo::Point::new(30.0, 1.0)), "{name}: the straight part of the edge is inside");
        assert!(!path.contains(kurbo::Point::new(5.0, 5.0)), "{name}: outside the arc of radius 20");
        assert!(path.contains(kurbo::Point::new(6.0, 6.0)), "{name}: inside the arc of radius 20");
    }
    assert_rect(fragment.bounds, [-2.0, -2.0, 102.0, 62.0], "the box and half the stroke");
}

#[test]
fn a_radius_larger_than_half_the_shorter_side_is_held_to_it() {
    let bytes = png(100, 60);
    let mut e = rounded_picture(&bytes);
    e.style.corner_radius = 1.0e9;
    let files = {
        let bytes = bytes.clone();
        move |_: &FileId| Some(Arc::<[u8]>::from(bytes.clone()))
    };
    let fragment = ElementPainter::new().paint(&e, &RenderOptions::default(), &mut Typesetter::new(), &files);
    let shapes = shapes_of(&fragment.scene);
    let [fill] = &shapes.picture[..] else { panic!("one picture fill") };
    // The short sides are half circles of radius 30 and the long sides stay straight between them.
    assert!(!fill.contains(kurbo::Point::new(5.0, 5.0)) && !fill.contains(kurbo::Point::new(2.0, 30.0 - 29.0)));
    assert!(fill.contains(kurbo::Point::new(30.0, 1.0)) && fill.contains(kurbo::Point::new(1.0, 30.0)));
    assert_rect(fill.bounding_box(), [0.0, 0.0, 100.0, 60.0], "the box");
}

#[test]
fn rotation_and_mirroring_carry_the_clip_with_the_picture() {
    let bytes = png(100, 60);
    let mut e = rounded_picture(&bytes);
    e.transform.angle = std::f64::consts::FRAC_PI_2;
    e.transform.flip_x = true;
    let (mut rig, _) = Rig::with(vec![e], vec![bytes]);
    rig.compose();
    let shapes = shapes_of(&rig.scene);
    let [fill] = &shapes.picture[..] else { panic!("one picture fill: {}", shapes.picture.len()) };
    // The box (10, 10, 100, 60) turned a quarter about its centre (60, 40) covers x 30 to 90 and y -10 to 90.
    let b = fill.bounding_box();
    let near = |a: f64, b: f64| (a - b).abs() < 1e-6;
    assert!(near(b.x0, 30.0) && near(b.x1, 90.0) && near(b.y0, -10.0) && near(b.y1, 90.0), "{b:?}");
    for corner in [(31.0, -9.0), (89.0, -9.0), (31.0, 89.0), (89.0, 89.0)] {
        assert!(!fill.contains(kurbo::Point::new(corner.0, corner.1)), "{corner:?} is cut off");
    }
    assert!(fill.contains(kurbo::Point::new(60.0, 40.0)));
    assert!(fill.contains(kurbo::Point::new(60.0, -8.0)), "the straight part of the short edge");
}

#[test]
fn a_new_radius_re_encodes_the_picture_and_the_placeholder_is_not_rounded() {
    let bytes = png(100, 60);
    let (mut rig, _) = Rig::with(vec![rounded_picture(&bytes)], vec![bytes]);
    rig.compose();
    let mut tx = rig.store.begin();
    tx.update(&id("p"), |e| e.style.corner_radius = 5.0).unwrap();
    let cs = tx.commit().unwrap();
    update_paint_index(&mut rig.index, &rig.store, &cs);
    let rep = rig.compose();
    assert_eq!(rep.cache_misses, 1, "the radius is part of the fragment: {rep:?}");

    // A picture that is not there yet is the grey box, square whatever the radius.
    let mut waiting = rounded_picture(&[1, 2, 3]);
    waiting.style.stroke_width = 0.0;
    let fragment =
        ElementPainter::new().paint(&waiting, &RenderOptions::default(), &mut Typesetter::new(), &|_: &FileId| None);
    assert!(fragment.awaiting_image.is_some());
    assert!(shapes_of(&fragment.scene).picture.is_empty());
}
