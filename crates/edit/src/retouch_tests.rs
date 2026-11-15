use drafftink_scene::{Crop, FileEntry, ImageData, MemoryStore, SceneStore, Transform, ZIndex};

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
    let mut e = Element::new(id("a"), ElementKind::Image(ImageData::new(Some(file), 200, 100)), ZIndex::first());
    e.transform = Transform::new(10.0, 20.0, 100.0, 50.0);
    edit(&mut e);
    tx.insert(e).unwrap();
    tx.commit().unwrap();
    store
}

fn stroke(points: &[(f64, f64)]) -> PaintStroke {
    PaintBrush::default().stroke(10.0, points.iter().map(|(x, y)| Point::new(*x, *y)).collect(), Vec::new())
}

fn paint(store: &mut MemoryStore, strokes: Vec<PaintStroke>) -> Result<Painted, PaintError> {
    let mut tx = store.begin();
    let out = paint_image(&mut tx, &id("a"), strokes, "t1")?;
    tx.commit().unwrap();
    Ok(out)
}

fn paint_of(store: &MemoryStore) -> Paint {
    store.get(&id("a")).unwrap().image_data().unwrap().paint.clone()
}

fn near(a: Point, b: Point) {
    assert!(a.distance(b) < 1e-9, "{a:?} != {b:?}");
}

// ---- adding strokes ----

#[test]
fn strokes_are_stored_in_the_order_given_and_numbered_on_from_the_ones_there() {
    let mut store = store_with(|_| {});
    let first = paint(&mut store, vec![stroke(&[(1.0, 1.0)]), stroke(&[(2.0, 2.0), (3.0, 3.0)])]).unwrap();
    assert_eq!(first.total, 2);
    assert_eq!(first.added.iter().map(StrokeId::as_str).collect::<Vec<_>>(), ["000001-t1", "000002-t1"]);
    let second = paint(&mut store, vec![stroke(&[(4.0, 4.0)])]).unwrap();
    assert_eq!(second.added.iter().map(StrokeId::as_str).collect::<Vec<_>>(), ["000003-t1"]);
    assert_eq!(second.total, 3);
    let kept = paint_of(&store);
    assert_eq!(kept.values().map(|s| s.points.len()).collect::<Vec<_>>(), [1, 2, 1]);
}

#[test]
fn a_stroke_that_shows_nothing_is_still_stored() {
    let mut store = store_with(|_| {});
    let clear =
        PaintBrush { opacity: 0.0, ..PaintBrush::default() }.stroke(10.0, vec![Point::new(5.0, 5.0)], Vec::new());
    paint(&mut store, vec![clear]).unwrap();
    assert_eq!(paint_of(&store).len(), 1);
}

#[test]
fn a_call_is_one_write() {
    let mut store = store_with(|_| {});
    let before = store.revision();
    paint(&mut store, vec![stroke(&[(1.0, 1.0)]), stroke(&[(2.0, 2.0)]), stroke(&[(3.0, 3.0)])]).unwrap();
    assert_eq!(store.revision(), before + 1);
}

#[test]
fn a_refused_call_changes_nothing() {
    let mut store = store_with(|_| {});
    let before = store.revision();
    let bad = PaintBrush::default().stroke(10.0, Vec::new(), Vec::new());
    let err = paint(&mut store, vec![stroke(&[(1.0, 1.0)]), bad]).unwrap_err();
    assert_eq!(err, PaintError::BadStroke { number: 2, problem: PaintProblem::NoPoints });
    assert_eq!(store.revision(), before);
    assert!(paint_of(&store).is_empty());
}

// ---- the target ----

#[test]
fn a_locked_a_missing_and_a_non_image_target_are_refused() {
    let mut locked = store_with(|e| e.locked = true);
    assert_eq!(paint(&mut locked, vec![stroke(&[(1.0, 1.0)])]), Err(TargetError::Locked(vec![id("a")]).into()));
    assert!(paint_of(&locked).is_empty());

    let mut gone = store_with(|e| e.deleted = true);
    assert_eq!(paint(&mut gone, vec![stroke(&[(1.0, 1.0)])]), Err(TargetError::NotFound(id("a")).into()));

    let mut rect = MemoryStore::new();
    let mut tx = rect.begin();
    tx.insert(Element::new(id("a"), ElementKind::Rectangle, ZIndex::first())).unwrap();
    tx.commit().unwrap();
    let err = paint(&mut rect, vec![stroke(&[(1.0, 1.0)])]).unwrap_err();
    assert!(matches!(err, PaintError::NotAnImage { kind: "rectangle", .. }), "{err:?}");
}

#[test]
fn a_picture_that_has_not_arrived_is_refused() {
    let mut store = store_with(|e| {
        if let ElementKind::Image(d) = &mut e.kind {
            d.file_id = None;
        }
    });
    let err = paint(&mut store, vec![stroke(&[(1.0, 1.0)])]).unwrap_err();
    assert_eq!(err, PaintError::NotLoaded { id: id("a") });
    assert!(err.to_string().contains("has not arrived"));
}

// ---- what a stroke may be ----

#[test]
fn the_numbers_of_a_stroke_are_checked() {
    let with = |f: fn(&mut PaintStroke)| {
        let mut s = stroke(&[(1.0, 1.0), (2.0, 2.0)]);
        f(&mut s);
        validate_stroke(&s)
    };
    assert_eq!(with(|_| {}), Ok(()));
    assert_eq!(with(|s| s.size = 0.1), Err(PaintProblem::Size(0.1)));
    assert_eq!(with(|s| s.size = 5000.0), Err(PaintProblem::Size(5000.0)));
    assert!(matches!(with(|s| s.size = f64::NAN), Err(PaintProblem::Size(v)) if v.is_nan()));
    assert_eq!(with(|s| s.hardness = 1.5), Err(PaintProblem::Hardness(1.5)));
    assert_eq!(with(|s| s.opacity = -0.1), Err(PaintProblem::Opacity(-0.1)));
    assert_eq!(with(|s| s.points.clear()), Err(PaintProblem::NoPoints));
    assert_eq!(with(|s| s.points[0].x = f64::INFINITY), Err(PaintProblem::Point));
    assert_eq!(with(|s| s.points[1].y = 2.0e9), Err(PaintProblem::Point));
    assert_eq!(with(|s| s.pressures = vec![0.5]), Err(PaintProblem::PressureCount { points: 2, pressures: 1 }));
    assert_eq!(with(|s| s.pressures = vec![0.5, 1.5]), Err(PaintProblem::Pressure(1.5)));
    assert_eq!(with(|s| s.pressures = vec![0.0, 1.0]), Ok(()));
    assert_eq!(
        with(|s| s.points = vec![Point::ZERO; MAX_STROKE_POINTS + 1]),
        Err(PaintProblem::TooManyPoints { have: MAX_STROKE_POINTS + 1 })
    );
}

#[test]
fn the_messages_name_the_range() {
    assert!(PaintProblem::Size(0.0).to_string().contains("0.5 to 4096"));
    assert!(PaintProblem::Hardness(2.0).to_string().contains("0 (soft edge) to 1"));
    assert!(PaintProblem::Opacity(2.0).to_string().contains("0 (clear) to 1"));
}

#[test]
fn too_many_strokes_in_a_call_or_on_the_picture_are_refused() {
    let mut store = store_with(|_| {});
    let many = (0..=MAX_STROKES_PER_CALL).map(|_| stroke(&[(1.0, 1.0)])).collect();
    let err = paint(&mut store, many).unwrap_err();
    assert!(matches!(err, PaintError::Limit { have: 65, max: 64, .. }), "{err:?}");

    // A picture that carries the most strokes takes no more.
    let mut full = store_with(|e| {
        if let ElementKind::Image(d) = &mut e.kind {
            for n in 0..MAX_PAINT_STROKES {
                let key = StrokeId::new(&format!("{:06}-x", n + 1)).unwrap();
                d.paint.insert(key, stroke(&[(1.0, 1.0)]));
            }
        }
    });
    let err = paint(&mut full, vec![stroke(&[(1.0, 1.0)])]).unwrap_err();
    assert!(matches!(err, PaintError::Limit { what: "Strokes on the picture", max: 1000, .. }), "{err:?}");
    assert_eq!(paint_of(&full).len(), MAX_PAINT_STROKES);
}

#[test]
fn too_many_points_on_the_picture_are_refused() {
    let mut store = store_with(|e| {
        if let ElementKind::Image(d) = &mut e.kind {
            for n in 0..10 {
                let key = StrokeId::new(&format!("{:06}-x", n + 1)).unwrap();
                d.paint
                    .insert(key, PaintBrush::default().stroke(5.0, vec![Point::ZERO; MAX_STROKE_POINTS], Vec::new()));
            }
        }
    });
    assert_eq!(paint_points(&paint_of(&store)), MAX_PAINT_POINTS);
    let err = paint(&mut store, vec![stroke(&[(1.0, 1.0)])]).unwrap_err();
    assert!(matches!(err, PaintError::Limit { what: "Points in the strokes of the picture", .. }), "{err:?}");
}

#[test]
fn a_tag_without_letters_or_digits_still_numbers_the_strokes() {
    let mut store = store_with(|_| {});
    let mut tx = store.begin();
    // Only letters and digits count; nothing is left of this tag, and the number alone is an id.
    let out = paint_image(&mut tx, &id("a"), vec![stroke(&[(1.0, 1.0)])], "...").unwrap();
    assert_eq!(out.added.iter().map(StrokeId::as_str).collect::<Vec<_>>(), ["000001-"]);
}

// ---- reset ----

#[test]
fn reset_takes_all_the_paint_off_in_one_write() {
    let mut store = store_with(|_| {});
    paint(&mut store, vec![stroke(&[(1.0, 1.0)]), stroke(&[(2.0, 2.0)])]).unwrap();
    let before = store.revision();
    let mut tx = store.begin();
    assert_eq!(reset_paint(&mut tx, &id("a")), Ok(2));
    tx.commit().unwrap();
    assert!(paint_of(&store).is_empty());
    assert_eq!(store.revision(), before + 1);
}

#[test]
fn resetting_an_image_with_no_paint_writes_nothing() {
    let mut store = store_with(|_| {});
    let before = store.revision();
    let mut tx = store.begin();
    assert_eq!(reset_paint(&mut tx, &id("a")), Ok(0));
    tx.commit().unwrap();
    assert_eq!(store.revision(), before);
}

#[test]
fn reset_refuses_a_locked_image() {
    let mut store = store_with(|_| {});
    paint(&mut store, vec![stroke(&[(1.0, 1.0)])]).unwrap();
    let mut tx = store.begin();
    tx.update(&id("a"), |e| e.locked = true).unwrap();
    assert_eq!(reset_paint(&mut tx, &id("a")), Err(TargetError::Locked(vec![id("a")]).into()));
}

// ---- the brush ----

#[test]
fn the_default_size_is_a_share_of_the_longer_side_within_bounds() {
    assert_eq!(default_brush_size((1000, 500)), 15.0);
    assert_eq!(default_brush_size((500, 1000)), 15.0);
    assert_eq!(default_brush_size((100, 80)), 4.0);
    assert_eq!(default_brush_size((0, 0)), 4.0);
    assert_eq!(default_brush_size((50_000, 100)), 200.0);
}

#[test]
fn an_unchosen_size_follows_the_picture_and_a_chosen_one_does_not() {
    let brush = PaintBrush::default();
    assert_eq!(brush.size_for((2000, 1000)), 30.0);
    let chosen = PaintBrush { size: 42.0, size_chosen: true, ..brush };
    assert_eq!(chosen.size_for((2000, 1000)), 42.0);
}

#[test]
fn a_brush_with_numbers_out_of_range_is_refused() {
    assert_eq!(PaintBrush::default().validate(), Ok(()));
    assert_eq!(PaintBrush { size: 0.0, ..PaintBrush::default() }.validate(), Err(PaintProblem::Size(0.0)));
    assert_eq!(PaintBrush { hardness: 2.0, ..PaintBrush::default() }.validate(), Err(PaintProblem::Hardness(2.0)));
    assert_eq!(PaintBrush { opacity: -1.0, ..PaintBrush::default() }.validate(), Err(PaintProblem::Opacity(-1.0)));
}

// ---- where a canvas point lies on the picture ----

fn space(store: &MemoryStore) -> PictureSpace {
    PictureSpace::of(store.get(&id("a")).unwrap()).unwrap()
}

#[test]
fn an_upright_image_maps_the_canvas_to_picture_pixels() {
    let store = store_with(|_| {});
    let s = space(&store);
    // Two pixels per unit: the box corner is pixel (0, 0), the middle of the box is the middle of the picture.
    near(s.pixel(Point::new(10.0, 20.0)), Point::new(0.0, 0.0));
    near(s.pixel(Point::new(60.0, 45.0)), Point::new(100.0, 50.0));
    near(s.pixel(Point::new(110.0, 70.0)), Point::new(200.0, 100.0));
    assert!((s.units_per_pixel() - 0.5).abs() < 1e-12);
    assert!(s.shows(Point::new(60.0, 45.0)));
    assert!(!s.shows(Point::new(9.0, 45.0)));
}

#[test]
fn a_mirrored_image_maps_to_the_mirrored_pixels() {
    let store = store_with(|e| e.transform.flip_x = true);
    let s = space(&store);
    near(s.pixel(Point::new(10.0, 20.0)), Point::new(200.0, 0.0));
    near(s.pixel(Point::new(110.0, 70.0)), Point::new(0.0, 100.0));
    let both = store_with(|e| (e.transform.flip_x, e.transform.flip_y) = (true, true));
    near(space(&both).pixel(Point::new(10.0, 20.0)), Point::new(200.0, 100.0));
}

#[test]
fn a_cropped_image_maps_to_pixels_of_the_whole_picture() {
    // The box shows pixels 40..120 across and 20..60 down, at one unit per two pixels.
    let store = store_with(|e| {
        e.transform = Transform::new(10.0, 20.0, 40.0, 20.0);
        if let ElementKind::Image(d) = &mut e.kind {
            d.crop = Some(Crop { x: 40.0, y: 20.0, width: 80.0, height: 40.0 });
        }
    });
    let s = space(&store);
    near(s.pixel(Point::new(10.0, 20.0)), Point::new(40.0, 20.0));
    near(s.pixel(Point::new(50.0, 40.0)), Point::new(120.0, 60.0));
    // A point beside the box is a pixel of the picture all the same.
    near(s.pixel(Point::new(0.0, 20.0)), Point::new(20.0, 20.0));
    assert!(!s.shows(Point::new(0.0, 20.0)));
}

#[test]
fn a_rotated_image_maps_what_the_user_sees() {
    // Turned a quarter clockwise about the middle of the box (60, 45): the top-left corner of the picture goes to
    // the top-right of the turned box.
    let store = store_with(|e| e.transform.angle = std::f64::consts::FRAC_PI_2);
    let s = space(&store);
    near(s.pixel(Point::new(85.0, -5.0)), Point::new(0.0, 0.0));
    near(s.pixel(Point::new(35.0, 95.0)), Point::new(200.0, 100.0));
    assert!(s.shows(Point::new(60.0, 10.0)));
    assert!(!s.shows(Point::new(10.0, 20.5)));
}

#[test]
fn only_images_have_a_picture_space() {
    let rect = Element::new(id("r"), ElementKind::Rectangle, ZIndex::first());
    assert!(matches!(PictureSpace::of(&rect), Err(PaintError::NotAnImage { kind: "rectangle", .. })));
}

#[test]
fn can_paint_follows_the_crop_rule() {
    let store = store_with(|_| {});
    let e = store.get(&id("a")).unwrap();
    assert!(can_paint(&store, e));
    let mut locked = e.clone();
    locked.locked = true;
    assert!(!can_paint(&store, &locked));
}
