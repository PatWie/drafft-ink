use drafftink_scene::{FileEntry, ImageData, MemoryStore, SceneStore, Transform, ZIndex};

use super::*;

fn id(s: &str) -> ElementId {
    ElementId::new(s).unwrap()
}

/// A 200 by 100 picture shown in a 100 by 50 box at (10, 20): one canvas unit per two pixels.
fn store_with(edit: impl FnOnce(&mut Element)) -> MemoryStore {
    let mut store = MemoryStore::new();
    let mut tx = store.begin();
    let (file, entry) = FileEntry::new("image/png", vec![1u8, 2, 3]);
    tx.put_file(entry).unwrap();
    let data = ImageData { file_id: Some(file), natural_width: 200, natural_height: 100, crop: None };
    let mut e = Element::new(id("a"), ElementKind::Image(data), ZIndex::first());
    e.transform = Transform::new(10.0, 20.0, 100.0, 50.0);
    edit(&mut e);
    tx.insert(e).unwrap();
    tx.commit().unwrap();
    store
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect::new(x, y, x + w, y + h)
}

fn near(a: Rect, b: Rect) {
    let d = [a.x0 - b.x0, a.y0 - b.y0, a.x1 - b.x1, a.y1 - b.y1];
    assert!(d.iter().all(|v| v.abs() < 1e-9), "{a:?} != {b:?}");
}

fn crop_of(store: &MemoryStore) -> Option<Crop> {
    store.get(&id("a")).unwrap().image_data().unwrap().crop.clone()
}

fn bounds_of(store: &MemoryStore) -> Rect {
    store.get(&id("a")).unwrap().transform.rect()
}

fn run(store: &mut MemoryStore, source: Option<Rect>, rule: BoxRule) -> Result<Cropped, CropError> {
    let mut tx = store.begin();
    let out = crop_image(&mut tx, &id("a"), source, rule)?;
    tx.commit().unwrap();
    Ok(out)
}

#[test]
fn following_the_crop_keeps_the_scale_and_the_place_of_the_picture() {
    let mut store = store_with(|_| {});
    let out = run(&mut store, Some(rect(40.0, 20.0, 80.0, 40.0)), BoxRule::FollowCrop).unwrap();
    assert!(out.changed);
    // Two pixels per unit: the box shrinks to 40 by 20 and moves 20 right and 10 down, so the pixels stay put.
    near(bounds_of(&store), rect(30.0, 30.0, 40.0, 20.0));
    near(out.bounds, bounds_of(&store));
    assert_eq!(crop_of(&store), Some(Crop { x: 40.0, y: 20.0, width: 80.0, height: 40.0 }));
}

#[test]
fn keeping_the_box_stretches_the_crop_over_it() {
    let mut store = store_with(|_| {});
    run(&mut store, Some(rect(0.0, 0.0, 100.0, 100.0)), BoxRule::Keep).unwrap();
    near(bounds_of(&store), rect(10.0, 20.0, 100.0, 50.0));
    assert_eq!(crop_of(&store), Some(Crop { x: 0.0, y: 0.0, width: 100.0, height: 100.0 }));
}

#[test]
fn a_second_crop_is_relative_to_the_picture_not_to_the_first_crop() {
    let mut store = store_with(|_| {});
    run(&mut store, Some(rect(40.0, 20.0, 80.0, 40.0)), BoxRule::FollowCrop).unwrap();
    // Widen the crop to the left: the box grows to the left and the pixels that were shown do not move.
    run(&mut store, Some(rect(20.0, 20.0, 100.0, 40.0)), BoxRule::FollowCrop).unwrap();
    near(bounds_of(&store), rect(20.0, 30.0, 50.0, 20.0));
    assert_eq!(crop_of(&store), Some(Crop { x: 20.0, y: 20.0, width: 100.0, height: 40.0 }));
}

#[test]
fn resetting_gives_back_the_whole_picture_where_it_was() {
    let mut store = store_with(|_| {});
    run(&mut store, Some(rect(40.0, 20.0, 80.0, 40.0)), BoxRule::FollowCrop).unwrap();
    let out = run(&mut store, None, BoxRule::FollowCrop).unwrap();
    assert!(out.changed);
    near(bounds_of(&store), rect(10.0, 20.0, 100.0, 50.0));
    assert_eq!(crop_of(&store), None);
}

#[test]
fn a_crop_of_the_whole_picture_is_no_crop() {
    let mut store = store_with(|_| {});
    let out = run(&mut store, Some(rect(0.0, 0.0, 200.0, 100.0)), BoxRule::FollowCrop).unwrap();
    assert!(!out.changed);
    assert_eq!(out.crop, None);
    assert_eq!(crop_of(&store), None);
}

#[test]
fn the_same_crop_twice_writes_nothing_the_second_time() {
    let mut store = store_with(|_| {});
    assert!(run(&mut store, Some(rect(0.0, 0.0, 100.0, 50.0)), BoxRule::FollowCrop).unwrap().changed);
    assert!(!run(&mut store, Some(rect(0.0, 0.0, 100.0, 50.0)), BoxRule::FollowCrop).unwrap().changed);
}

#[test]
fn an_exact_box_is_taken_as_given() {
    let mut store = store_with(|_| {});
    run(&mut store, Some(rect(0.0, 0.0, 100.0, 100.0)), BoxRule::Exactly(rect(5.0, 6.0, 7.0, 8.0))).unwrap();
    near(bounds_of(&store), rect(5.0, 6.0, 7.0, 8.0));
    let bad = run(&mut store, None, BoxRule::Exactly(rect(0.0, 0.0, 0.0, 5.0)));
    assert_eq!(bad, Err(CropError::BadBounds));
}

#[test]
fn a_flipped_image_crops_what_the_user_sees() {
    let mut store = store_with(|e| e.transform.flip_x = true);
    // Mirrored left to right: the left edge of the box shows the right edge of the picture. Crop the right
    // 80 pixels of the picture, which are the left 40 units of the box.
    run(&mut store, Some(rect(120.0, 0.0, 80.0, 100.0)), BoxRule::FollowCrop).unwrap();
    near(bounds_of(&store), rect(10.0, 20.0, 40.0, 50.0));
    assert_eq!(crop_of(&store), Some(Crop { x: 120.0, y: 0.0, width: 80.0, height: 100.0 }));
}

#[test]
fn a_rotated_image_keeps_its_pixels_on_the_canvas() {
    let mut store = store_with(|e| e.transform.angle = std::f64::consts::FRAC_PI_2);
    let before = |store: &MemoryStore, local: Point| store.get(&id("a")).unwrap().transform.local_to_world(local);
    // The picture pixel (100, 50) is the middle of the box now: remember where it is on the canvas.
    let middle = before(&store, Point::new(50.0, 25.0));
    run(&mut store, Some(rect(100.0, 0.0, 100.0, 100.0)), BoxRule::FollowCrop).unwrap();
    // The cropped part is the right half: its left edge is at the old middle, in the rotated frame.
    let e = store.get(&id("a")).unwrap();
    assert_eq!(e.transform.angle, std::f64::consts::FRAC_PI_2);
    assert!((e.transform.w - 50.0).abs() < 1e-9 && (e.transform.h - 50.0).abs() < 1e-9);
    let left_middle = e.transform.local_to_world(Point::new(0.0, 25.0));
    assert!(left_middle.distance(middle) < 1e-9, "{left_middle:?} vs {middle:?}");
}

#[test]
fn the_window_and_the_source_are_inverse_with_flips() {
    for (fx, fy) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut store = store_with(|e| {
            e.transform.flip_x = fx;
            e.transform.flip_y = fy;
            if let ElementKind::Image(d) = &mut e.kind {
                d.crop = Some(Crop { x: 30.0, y: 10.0, width: 100.0, height: 60.0 });
            }
        });
        let e = store.begin().get(&id("a")).unwrap().clone();
        let picture = Picture::of(&e).unwrap();
        let source = rect(50.0, 20.0, 40.0, 30.0);
        near(picture.source_of(picture.window_of(source)), source);
        // The window of the shown part is the box.
        near(
            picture.window_of(Crop { x: 30.0, y: 10.0, width: 100.0, height: 60.0 }.rect()),
            rect(0.0, 0.0, 100.0, 50.0),
        );
    }
}

#[test]
fn what_cannot_be_cropped_is_refused_in_words_and_changes_nothing() {
    let mut store = store_with(|_| {});
    let whole = Some(rect(0.0, 0.0, 10.0, 10.0));
    let r = |source| {
        let mut s = store_with(|_| {});
        run(&mut s, source, BoxRule::FollowCrop)
    };
    assert!(matches!(r(Some(rect(0.0, 0.0, 0.0, 10.0))), Err(CropError::TooSmall { .. })));
    assert!(matches!(r(Some(rect(0.0, 0.0, 10.0, 0.5))), Err(CropError::TooSmall { .. })));
    assert!(matches!(r(Some(rect(150.0, 0.0, 60.0, 10.0))), Err(CropError::OutsidePicture { .. })));
    assert!(matches!(r(Some(rect(-1.0, 0.0, 10.0, 10.0))), Err(CropError::OutsidePicture { .. })));
    assert!(matches!(r(Some(rect(f64::NAN, 0.0, 10.0, 10.0))), Err(CropError::NotFinite)));
    let _ = run(&mut store, whole, BoxRule::FollowCrop).unwrap();

    let mut locked = store_with(|e| e.locked = true);
    assert!(matches!(run(&mut locked, whole, BoxRule::FollowCrop), Err(CropError::Target(TargetError::Locked(_)))));
    assert_eq!(crop_of(&locked), None);

    let mut missing = store_with(|e| {
        if let ElementKind::Image(d) = &mut e.kind {
            d.file_id = None;
        }
    });
    assert!(matches!(run(&mut missing, whole, BoxRule::FollowCrop), Err(CropError::NotLoaded { .. })));

    let mut tx = store.begin();
    assert!(matches!(
        crop_image(&mut tx, &id("nope"), whole, BoxRule::FollowCrop),
        Err(CropError::Target(TargetError::NotFound(_)))
    ));
}

#[test]
fn a_rectangle_is_not_an_image() {
    let mut store = MemoryStore::new();
    let mut tx = store.begin();
    tx.insert(Element::new(id("a"), ElementKind::Rectangle, ZIndex::first()).with_bounds(0.0, 0.0, 10.0, 10.0))
        .unwrap();
    tx.commit().unwrap();
    let err = run(&mut store, None, BoxRule::FollowCrop).unwrap_err();
    assert!(matches!(err, CropError::NotAnImage { kind: "rectangle", .. }), "{err:?}");
    assert!(err.to_string().contains("Select an image"));
}
