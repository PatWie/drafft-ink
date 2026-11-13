#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The element model: construction, serde forms, defaults, point-box maintenance.

mod common;

use common::*;
use drafftink_scene::*;
use kurbo::Point;

fn every_kind(z: &[ZIndex]) -> Vec<Element> {
    let mut v = Vec::new();
    v.push(rect("rect", &z[0]));
    v.push(Element::new(id("dia"), ElementKind::Diamond, z[1].clone()).with_bounds(0.0, 0.0, 5.0, 5.0));
    v.push(Element::new(id("ell"), ElementKind::Ellipse, z[2].clone()).with_bounds(1.0, 2.0, 30.0, 40.0));
    let mut line = Element::linear(
        id("line"),
        LinearKind::Line,
        z[3].clone(),
        &[Point::new(0.0, 0.0), Point::new(10.0, 5.0), Point::new(20.0, 0.0)],
    )
    .unwrap();
    if let ElementKind::Line(d) = &mut line.kind {
        d.closed = true;
        d.path = PathType::Round;
    }
    v.push(line);
    let mut ar = arrow("arrow", &z[4], &[(0.0, 0.0), (40.0, 0.0), (40.0, 30.0)]);
    if let ElementKind::Arrow(d) = &mut ar.kind {
        d.path = PathType::Elbow;
        d.start_head = Head::CardinalityOneOrMany;
        d.end_head = Head::TriangleOutline;
        d.head_size = Some(15.0);
        d.fixed_segments = vec![FixedSegment { index: 1, start: Point::new(0.0, 0.0), end: Point::new(40.0, 0.0) }];
        d.start_is_special = true;
        d.start_binding = Some(Binding { target: id("rect"), mode: BindMode::Inside, fixed_point: [0.25, 0.75] });
    }
    v.push(ar);
    let mut fh = Element::new(
        id("free"),
        ElementKind::Freehand(FreehandData::new(vec![Point::new(0.0, 0.0), Point::new(3.0, 4.0)])),
        z[5].clone(),
    );
    if let ElementKind::Freehand(d) = &mut fh.kind {
        d.pressures = vec![0.2, 0.9];
        d.pressure_mode = PressureMode::Device;
        d.brush = Brush::Calligraphy;
        d.highlighter = true;
    }
    fh.refit_points().unwrap();
    v.push(fh);
    let mut t = TextData::new("héllo\nworld");
    t.font.family = FontFamily::VanillaExtract;
    t.font.align = TextAlign::Center;
    t.auto_resize = false;
    t.runs = vec![TextRun { start: 0, end: 2, color: Some(Color::rgb(255, 0, 0)), ..Default::default() }];
    v.push(Element::new(id("text"), ElementKind::Text(t), z[6].clone()).with_bounds(0.0, 0.0, 50.0, 40.0));
    v.push(
        Element::new(id("math"), ElementKind::Math(MathData::new("x^2")), z[7].clone())
            .with_bounds(0.0, 0.0, 30.0, 20.0),
    );
    let img = ImageData {
        file_id: Some(FileId::of(b"png")),
        natural_width: 640,
        natural_height: 480,
        crop: Some(Crop { x: 1.0, y: 2.0, width: 3.0, height: 4.0 }),
    };
    let mut im = Element::new(id("img"), ElementKind::Image(img), z[8].clone()).with_bounds(0.0, 0.0, 64.0, 48.0);
    im.transform.flip_x = true;
    v.push(im);
    v.push(frame("frame", &z[9]));
    v
}

#[test]
fn every_kind_round_trips_through_json() {
    let z = zs(10);
    for mut e in every_kind(&z) {
        e.name = Some(format!("n_{}", e.id));
        e.link = Some("https://example.org/a".into());
        e.locked = true;
        e.group_ids = smallvec::smallvec![gid("inner"), gid("outer")];
        e.custom = Some([("k".to_owned(), serde_json::json!({"a": [1, 2]}))].into());
        e.transform.angle = 0.5;
        if e.tag() == Kind::Frame {
            e.transform.angle = 0.0;
        }
        e.version = 7;
        let json = serde_json::to_string(&e).unwrap();
        let back: Element = serde_json::from_str(&json).unwrap();
        assert_eq!(back, e, "{json}");
    }
}

#[test]
fn json_shape_is_the_documented_one() {
    let z = zs(10);
    let a = &every_kind(&z)[4];
    let v = serde_json::to_value(a).unwrap();
    assert_eq!(v["id"], "arrow");
    assert_eq!(v["kind"]["type"], "arrow");
    assert_eq!(v["kind"]["path"], "elbow");
    assert_eq!(v["kind"]["start_head"], "cardinality_one_or_many");
    assert_eq!(v["kind"]["points"], serde_json::json!([[0.0, 0.0], [40.0, 0.0], [40.0, 30.0]]));
    assert_eq!(v["kind"]["start_binding"]["mode"], "inside");
    assert_eq!(v["transform"]["w"], 40.0);
    assert_eq!(v["style"]["stroke_color"], "#6366f1");
    assert_eq!(v["style"]["sloppiness"], "artist");
    assert!(v.get("locked").is_none(), "defaults are not written");
    assert!(v.get("group_ids").is_none());
    assert!(v["transform"].get("angle").is_none());
}

#[test]
fn inline_math_is_a_font_setting_that_is_written_only_when_off() {
    let z = zs(2);
    let mut t = TextData::new("a $x$ b");
    let on = Element::new(id("on"), ElementKind::Text(t.clone()), z[0].clone());
    let v = serde_json::to_value(&on).unwrap();
    assert!(v["kind"]["font"].get("inline_math").is_none(), "auto is the default and is not written: {v}");
    t.font.inline_math = InlineMath::Off;
    let off = Element::new(id("off"), ElementKind::Text(t), z[1].clone());
    let v = serde_json::to_value(&off).unwrap();
    assert_eq!(v["kind"]["font"]["inline_math"], "off");
    let back: Element = serde_json::from_value(v).unwrap();
    assert_eq!(back.text_data().unwrap().font.inline_math, InlineMath::Off);
    // The content is the source, dollar signs included.
    assert_eq!(back.text_data().unwrap().content, "a $x$ b");
    // A file from before the setting reads as auto, and a wrong value is an error.
    let old: Element = serde_json::from_str(
        r#"{"id":"t","kind":{"type":"text","content":"x","font":{"size":20}},"transform":{"x":0,"y":0,"w":1,"h":1},"index":"a0"}"#,
    )
    .unwrap();
    assert_eq!(old.text_data().unwrap().font.inline_math, InlineMath::Auto);
    let bad = serde_json::from_str::<Element>(
        r#"{"id":"t","kind":{"type":"text","content":"x","font":{"inline_math":"maybe"}},"transform":{"x":0,"y":0,"w":1,"h":1},"index":"a0"}"#,
    );
    assert!(bad.is_err());
}

#[test]
fn defaults_apply_when_fields_are_missing() {
    let e: Element = serde_json::from_str(
        r#"{"id":"r","kind":{"type":"rectangle"},"transform":{"x":1,"y":2,"w":3,"h":4},"index":"a0"}"#,
    )
    .unwrap();
    assert_eq!(e.style, Style::default());
    assert!(!e.locked && !e.deleted && e.group_ids.is_empty() && e.frame_id.is_none() && e.version == 0);
    assert_eq!(e.transform.angle, 0.0);

    let t: Element = serde_json::from_str(
        r#"{"id":"t","kind":{"type":"text","content":"x"},"transform":{"x":0,"y":0,"w":1,"h":1},"index":"a0"}"#,
    )
    .unwrap();
    let td = t.text_data().unwrap();
    assert!(td.auto_resize && td.runs.is_empty() && td.container_id.is_none());
    assert_eq!(td.font, TextStyle::default());

    let f: Element = serde_json::from_str(
        r#"{"id":"f","kind":{"type":"frame"},"transform":{"x":0,"y":0,"w":1,"h":1},"index":"a0"}"#,
    )
    .unwrap();
    assert!(matches!(f.kind, ElementKind::Frame(FrameData { clip: true, name: None })));
}

#[test]
fn points_accept_both_pair_and_object_forms() {
    let e: Element = serde_json::from_str(
        r#"{"id":"l","kind":{"type":"line","points":[{"x":0,"y":0},[10,5]]},"transform":{"x":0,"y":0,"w":10,"h":5},"index":"a0"}"#,
    )
    .unwrap();
    assert_eq!(e.points().unwrap(), &[Point::new(0.0, 0.0), Point::new(10.0, 5.0)]);
}

#[test]
fn unknown_fields_and_kinds_are_rejected() {
    let bad = [
        r#"{"id":"r","kind":{"type":"rectangle"},"transform":{"x":1,"y":2,"w":3,"h":4},"index":"a0","oops":1}"#,
        r#"{"id":"r","kind":{"type":"hexagon"},"transform":{"x":1,"y":2,"w":3,"h":4},"index":"a0"}"#,
        r#"{"id":"r","kind":{"type":"rectangle"},"transform":{"x":1,"y":2,"w":3,"h":4,"skew":1},"index":"a0"}"#,
        r#"{"id":"r","kind":{"type":"rectangle"},"transform":{"x":1,"y":2},"index":"a0"}"#,
        r#"{"id":"r","kind":{"type":"rectangle"},"transform":{"x":1,"y":2,"w":3,"h":4}}"#,
        r#"{"id":"r r","kind":{"type":"rectangle"},"transform":{"x":1,"y":2,"w":3,"h":4},"index":"a0"}"#,
        r#"{"id":"r","kind":{"type":"rectangle"},"transform":{"x":1,"y":2,"w":3,"h":4},"index":"nope!"}"#,
        r#"{"id":"r","kind":{"type":"text","content":"x","font":{"family":"arial"}},"transform":{"x":0,"y":0,"w":1,"h":1},"index":"a0"}"#,
    ];
    for b in bad {
        assert!(serde_json::from_str::<Element>(b).is_err(), "{b}");
    }
}

#[test]
fn enum_names_are_the_catalog_names() {
    let names: Vec<String> = [
        FillPattern::Solid,
        FillPattern::Hachure,
        FillPattern::ZigZag,
        FillPattern::CrossHatch,
        FillPattern::Dots,
        FillPattern::Dashed,
        FillPattern::ZigZagLine,
    ]
    .iter()
    .map(|p| serde_json::to_value(p).unwrap().as_str().unwrap().to_owned())
    .collect();
    assert_eq!(names, ["solid", "hachure", "zigzag", "cross_hatch", "dots", "dashed", "zigzag_line"]);
    let fonts: Vec<String> =
        [FontFamily::GelPen, FontFamily::NotoSans, FontFamily::GelPenSerif, FontFamily::VanillaExtract]
            .iter()
            .map(|p| serde_json::to_value(p).unwrap().as_str().unwrap().to_owned())
            .collect();
    assert_eq!(fonts, ["gelpen", "noto_sans", "gelpen_serif", "vanilla_extract"]);
    assert_eq!(Sloppiness::Drunk.roughness(), 3.5);
    assert_eq!(FontFamily::GelPenSerif.display_name(), "GelPen Serif");
}

#[test]
fn kind_facts() {
    for k in [Kind::Rectangle, Kind::Diamond, Kind::Ellipse, Kind::Text, Kind::Image, Kind::Frame] {
        assert!(k.is_bindable(), "{k:?}");
    }
    for k in [Kind::Line, Kind::Arrow, Kind::Freehand, Kind::Math] {
        assert!(!k.is_bindable(), "{k:?}");
    }
    assert!(Kind::Arrow.is_label_container() && Kind::Rectangle.is_label_container());
    assert!(!Kind::Text.is_label_container() && !Kind::Line.is_label_container());
    let z = zs(10);
    for e in every_kind(&z) {
        assert_eq!(serde_json::to_value(e.tag()).unwrap(), e.tag().name());
    }
}

#[test]
fn linear_constructor_normalises_the_point_box() {
    let e = arrow("a", &ZIndex::first(), &[(100.0, 50.0), (160.0, 20.0), (130.0, 90.0)]);
    assert_eq!((e.transform.x, e.transform.y, e.transform.w, e.transform.h), (100.0, 20.0, 60.0, 70.0));
    assert_eq!(e.points().unwrap()[0], Point::new(0.0, 30.0));
    assert_eq!(validate_element(&e), Ok(()));
    assert_eq!(LinearKind::try_from(Kind::Rectangle), Err(Kind::Rectangle));
    assert_eq!(LinearKind::try_from(Kind::Arrow), Ok(LinearKind::Arrow));
    assert!(Element::linear(id("x"), LinearKind::Line, ZIndex::first(), &[]).is_none());
    assert!(Element::linear(id("x"), LinearKind::Line, ZIndex::first(), &[Point::new(f64::NAN, 0.0)]).is_none());
}

#[test]
fn refit_keeps_world_positions_also_when_rotated() {
    let mut e = arrow("a", &ZIndex::first(), &[(0.0, 0.0), (100.0, 40.0)]);
    e.transform.angle = 0.7;
    let world_before: Vec<Point> = e.points().unwrap().iter().map(|p| e.transform.local_to_world(*p)).collect();
    // Drag the first point to the left and up by editing local coordinates, then refit.
    if let ElementKind::Arrow(d) = &mut e.kind {
        d.points[0] = Point::new(-30.0, -10.0);
    }
    let moved_world_first = e.transform.local_to_world(Point::new(-30.0, -10.0));
    let second_world = world_before[1];
    e.refit_points().unwrap();
    let after: Vec<Point> = e.points().unwrap().iter().map(|p| e.transform.local_to_world(*p)).collect();
    assert!((after[0] - moved_world_first).hypot() < 1e-9, "{:?} vs {:?}", after[0], moved_world_first);
    assert!((after[1] - second_world).hypot() < 1e-9);
    assert_eq!(validate_element(&e), Ok(()));
    // Idempotent.
    let t = e.transform;
    e.refit_points().unwrap();
    assert!((e.transform.x - t.x).abs() < 1e-9 && (e.transform.w - t.w).abs() < 1e-9);
}

#[test]
fn refit_shifts_fixed_segments_with_the_points() {
    let mut e = arrow("a", &ZIndex::first(), &[(0.0, 0.0), (40.0, 0.0), (40.0, 30.0)]);
    if let ElementKind::Arrow(d) = &mut e.kind {
        d.path = PathType::Elbow;
        d.fixed_segments = vec![FixedSegment { index: 1, start: Point::new(0.0, 0.0), end: Point::new(40.0, 0.0) }];
        d.points[2] = Point::new(40.0, -30.0);
    }
    e.refit_points().unwrap();
    let d = e.linear_data().unwrap();
    assert_eq!(d.fixed_segments[0].start, Point::new(0.0, 30.0));
    assert_eq!(validate_element(&e), Ok(()));
}

#[test]
fn text_runs_and_old_char_colors_convert_both_ways() {
    let red = Color::rgb(255, 0, 0);
    let blue = Color::rgb(0, 0, 255);
    // Old arrays may be shorter or longer than the text, and contain nulls.
    let old = [None, Some(red), Some(red), None, Some(blue), Some(blue), Some(blue), Some(red)];
    let runs = TextData::runs_from_char_colors(&old, 6);
    assert_eq!(
        runs,
        vec![
            TextRun { start: 1, end: 3, color: Some(red), ..Default::default() },
            TextRun { start: 4, end: 6, color: Some(blue), ..Default::default() },
        ]
    );
    let mut t = TextData::new("ab😀def");
    assert_eq!(t.char_count(), 6, "characters are scalar values, not bytes");
    t.runs = runs;
    assert_eq!(t.char_colors(), vec![None, Some(red), Some(red), None, Some(blue), Some(blue)]);
    assert_eq!(TextData::runs_from_char_colors(&[], 3), vec![]);
    assert_eq!(t.char_colors().len(), 6);
}

#[test]
fn transform_geometry() {
    let t = Transform { angle: std::f64::consts::FRAC_PI_2, ..Transform::new(0.0, 0.0, 20.0, 10.0) };
    assert_eq!(t.center(), Point::new(10.0, 5.0));
    // The top-left corner of a 20x10 box turned a quarter clockwise about its centre lands at the top-right.
    let p = t.local_to_world(Point::ZERO);
    assert!((p - Point::new(15.0, -5.0)).hypot() < 1e-9, "{p:?}");
    assert_eq!(Transform::new(1.0, 2.0, 3.0, 4.0).rect(), kurbo::Rect::new(1.0, 2.0, 4.0, 6.0));
}

#[test]
fn json_schema_describes_the_document() {
    let schema = json_schema();
    let text = schema.to_string();
    for needle in [
        "elements",
        "ElementKind",
        "\"rectangle\"",
        "cardinality_one_or_many",
        "Color",
        "ZIndex",
        "fixed_point",
        "drafftink",
    ] {
        assert!(text.contains(needle), "schema lacks {needle}");
    }
}

#[test]
fn plex_sans_is_a_family_with_its_own_word_and_every_old_word_still_parses() {
    assert_eq!(serde_json::to_value(FontFamily::PlexSans).unwrap(), serde_json::json!("plex_sans"));
    assert_eq!(FontFamily::PlexSans.display_name(), "IBM Plex Sans");
    for (word, family) in [
        ("gelpen", FontFamily::GelPen),
        ("noto_sans", FontFamily::NotoSans),
        ("gelpen_serif", FontFamily::GelPenSerif),
        ("vanilla_extract", FontFamily::VanillaExtract),
        ("plex_sans", FontFamily::PlexSans),
    ] {
        let parsed: FontFamily = serde_json::from_value(serde_json::json!(word)).unwrap();
        assert_eq!(parsed, family, "{word}");
    }
    assert!(serde_json::from_value::<FontFamily>(serde_json::json!("plex")).is_err());
}

#[test]
fn an_image_written_before_crops_existed_has_none_and_an_uncropped_one_writes_no_key() {
    let old = r#"{"natural_width":64,"natural_height":48}"#;
    let data: ImageData = serde_json::from_str(old).unwrap();
    assert_eq!(data.crop, None);
    assert_eq!(serde_json::to_string(&data).unwrap(), old, "no crop is no key, so old readers see the same file");
    // A crop is four numbers and nothing else.
    let with = r#"{"natural_width":64,"natural_height":48,"crop":{"x":1.0,"y":2.0,"width":3.0,"height":4.0}}"#;
    let cropped: ImageData = serde_json::from_str(with).unwrap();
    assert_eq!(cropped.crop, Some(Crop { x: 1.0, y: 2.0, width: 3.0, height: 4.0 }));
    assert_eq!(serde_json::to_string(&cropped).unwrap(), with);
    assert!(serde_json::from_str::<ImageData>(&with.replace("\"height\":4.0", "\"height\":4.0,\"extra\":1")).is_err());
    // The helpers the crop editor uses agree with the fields.
    let c = cropped.crop.unwrap();
    assert_eq!(c.rect(), kurbo::Rect::new(1.0, 2.0, 4.0, 6.0));
    assert_eq!(Crop::from_rect(c.rect()), c);
}
