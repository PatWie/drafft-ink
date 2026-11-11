#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Excalidraw import and export against the fixtures in `tests/fixtures/excalidraw`, plus hostile input.

use std::collections::BTreeMap;
use std::path::PathBuf;

use drafftink_io::excalidraw::*;
use drafftink_io::{Imported, IoError};
use drafftink_scene::limits::{MAX_COORD, MAX_ELEMENTS, MAX_GROUP_DEPTH};
use drafftink_scene::{
    BindMode, Color, Document, Element, ElementKind, FillPattern, FontFamily, Head, Kind, PathType, PressureMode,
    SeqIds, Severity, Sloppiness, StrokeStyle,
};
use kurbo::Point;
use serde_json::{Value, json};

// ---------------------------------------------------------------------------------------------------------------
// Helpers

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/excalidraw")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(dir().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn json_file(rel: &str) -> Value {
    serde_json::from_str(&read(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn scene(name: &str) -> Imported {
    import_scene(&read(&format!("{name}.excalidraw")), &mut SeqIds::new()).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn import_str(text: &str) -> Result<Imported, IoError> {
    import_scene(text, &mut SeqIds::new())
}

/// Imports a scene made of the given elements and checks that it validates.
fn import_elements(elements: Value) -> Imported {
    let text = json!({"type": "excalidraw", "version": 2, "elements": elements}).to_string();
    let imported = import_str(&text).expect("scene imports");
    assert_no_errors(&imported.document);
    imported
}

fn assert_no_errors(doc: &Document) {
    let report = doc.validate();
    let errors: Vec<String> = report.errors().map(|i| i.to_string()).collect();
    assert!(errors.is_empty(), "document breaks rules: {errors:#?}");
}

fn assert_clean(doc: &Document) {
    let report = doc.validate();
    assert!(
        report.is_clean(),
        "document has findings: {:#?}",
        report.issues.iter().map(ToString::to_string).collect::<Vec<_>>()
    );
}

fn get<'a>(doc: &'a Document, id: &str) -> &'a Element {
    doc.elements.iter().find(|e| e.id.as_str() == id).unwrap_or_else(|| panic!("no element {id}"))
}

fn counts(doc: &Document) -> BTreeMap<Kind, usize> {
    let mut m = BTreeMap::new();
    for e in &doc.elements {
        *m.entry(e.tag()).or_insert(0) += 1;
    }
    m
}

fn names(e: &Element) -> Vec<&str> {
    e.group_ids.iter().map(|g| g.as_str()).collect()
}

fn linear(e: &Element) -> &drafftink_scene::LinearData {
    e.linear_data().unwrap_or_else(|| panic!("{} is not linear", e.id))
}

/// Compares two JSON values; numbers may differ by 1e-9 (relative), because points move between two coordinate
/// frames on the way through Excalidraw and f64 sums are not exact.
fn approx_eq(a: &Value, b: &Value, path: &str) -> Result<(), String> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap_or(f64::NAN), y.as_f64().unwrap_or(f64::NAN));
            if (x - y).abs() <= 1e-9 * (1.0 + x.abs().max(y.abs())) {
                Ok(())
            } else {
                Err(format!("{path}: {x} != {y}"))
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                return Err(format!("{path}: length {} != {}", x.len(), y.len()));
            }
            x.iter().zip(y).enumerate().try_for_each(|(i, (p, q))| approx_eq(p, q, &format!("{path}[{i}]")))
        }
        (Value::Object(x), Value::Object(y)) => {
            if x.keys().ne(y.keys()) {
                return Err(format!(
                    "{path}: keys {:?} != {:?}",
                    x.keys().collect::<Vec<_>>(),
                    y.keys().collect::<Vec<_>>()
                ));
            }
            x.iter().try_for_each(|(k, p)| approx_eq(p, y.get(k).unwrap_or(&Value::Null), &format!("{path}.{k}")))
        }
        _ => {
            if a == b {
                Ok(())
            } else {
                Err(format!("{path}: {a} != {b}"))
            }
        }
    }
}

fn assert_same_doc(a: &Document, b: &Document) {
    let (x, y) = (serde_json::to_value(a).unwrap(), serde_json::to_value(b).unwrap());
    if let Err(why) = approx_eq(&x, &y, "doc") {
        panic!("documents differ: {why}");
    }
}

const SCENE_FIXTURES: [&str; 9] = [
    "basic_shapes",
    "bindings",
    "closed_paths",
    "elbow_arrows",
    "frames",
    "groups",
    "images_and_unsupported",
    "minimal_fields",
    "text_containers",
];

// ---------------------------------------------------------------------------------------------------------------
// Fixtures

#[test]
fn every_scene_fixture_loads_and_validates() {
    let mut seen = 0;
    for entry in std::fs::read_dir(dir()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "excalidraw") {
            continue;
        }
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let result = import_str(&std::fs::read_to_string(&path).unwrap());
        match name.as_str() {
            "not_json" => assert!(matches!(result, Err(IoError::Parse { .. })), "{result:?}"),
            "no_elements" => assert!(matches!(result, Err(IoError::Invalid { .. })), "{result:?}"),
            _ => {
                let doc = result.unwrap_or_else(|e| panic!("{name}: {e}")).document;
                assert_no_errors(&doc);
            }
        }
        seen += 1;
    }
    assert!(seen >= 11, "found only {seen} fixtures");
}

#[test]
fn fixtures_the_old_importer_rejected_are_rejected_for_the_same_reason() {
    let summary = json_file("expected/summary.json");
    for f in summary["files"].as_array().unwrap() {
        if f["parsed"] == json!(false) {
            let name = f["name"].as_str().unwrap();
            assert!(import_str(&read(&format!("{name}.excalidraw"))).is_err(), "{name}");
        }
    }
    // not_json is a parse error, a scene without `elements` is invalid content.
    assert!(matches!(import_str(&read("not_json.excalidraw")), Err(IoError::Parse { .. })));
    let e = import_str(&read("no_elements.excalidraw")).unwrap_err();
    assert!(e.to_string().contains("elements"), "{e}");
}

/// The old importer's counts per shape type are the new ones, except where the new importer intentionally keeps more:
/// diamonds, frames, images and (with their container) bound texts all existed in the old output only as rectangles
/// or not at all.
#[test]
fn shape_counts_match_the_old_importer_where_behaviour_is_the_same() {
    for name in SCENE_FIXTURES {
        let facts = json_file(&format!("expected/{name}.facts.json"));
        let old = &facts["doc"]["counts_total"];
        let old_count = |k: &str| old[k].as_u64().unwrap_or(0) as usize;
        let doc = scene(name).document;
        let c = counts(&doc);
        let n = |k: Kind| c.get(&k).copied().unwrap_or(0);
        // Old: a diamond is a Rectangle. New: a Diamond of its own.
        assert_eq!(old_count("Rectangle"), n(Kind::Rectangle) + n(Kind::Diamond), "{name}: rectangles");
        for (old_name, kind) in [
            ("Ellipse", Kind::Ellipse),
            ("Line", Kind::Line),
            ("Arrow", Kind::Arrow),
            ("Freehand", Kind::Freehand),
            ("Text", Kind::Text),
        ] {
            assert_eq!(old_count(old_name), n(kind), "{name}: {old_name}");
        }
        // Everything else is new behaviour: frames and images were dropped by the old code.
        let extra: usize = [Kind::Frame, Kind::Image, Kind::Math].iter().map(|k| n(*k)).sum();
        assert_eq!(
            doc.elements.len(),
            old.as_object().map_or(0, |o| o.values().filter_map(Value::as_u64).sum::<u64>() as usize) + extra,
            "{name}"
        );
    }
}

#[test]
fn basic_shapes_map_every_style_field() {
    let i = scene("basic_shapes");
    let d = &i.document;
    assert_clean(d);
    // Document background.
    assert_eq!(d.background, Color::WHITE);
    // r1: adaptive round corners, hachure, red.
    let r1 = get(d, "r1");
    assert_eq!(r1.style.fill_pattern, FillPattern::Hachure);
    assert_eq!(r1.style.corner_radius, 25.0, "a quarter of the shorter side (100)");
    assert_eq!(r1.style.stroke_color, Color::rgb(0xe0, 0x31, 0x31));
    assert_eq!(r1.style.fill_color, Some(Color::rgb(0xff, 0xc9, 0xc9)));
    assert_eq!(get(d, "r6").style.corner_radius, 5.0, "50 x 20 box: a quarter of 20");
    assert_eq!(get(d, "r2").style.corner_radius, 0.0);
    // Fill patterns, roughness, opacity (50 percent, not "greater than one").
    assert_eq!(get(d, "r2").style.fill_pattern, FillPattern::CrossHatch);
    assert_eq!(get(d, "r2").style.sloppiness, Sloppiness::Architect);
    let r3 = get(d, "r3");
    assert_eq!(
        (r3.style.fill_pattern, r3.style.sloppiness, r3.style.opacity),
        (FillPattern::ZigZag, Sloppiness::Cartoonist, 0.5)
    );
    // Rotation and stroke style, which the old importer dropped.
    let r4 = get(d, "r4");
    assert_eq!((r4.transform.angle, r4.style.stroke_style), (0.4, StrokeStyle::Dashed));
    // Colours: #rgb, #rgba, and a string that is not a colour.
    let r5 = get(d, "r5");
    assert_eq!(r5.style.stroke_color, Color::WHITE);
    assert_eq!(r5.style.fill_color, Some(Color::rgba(0, 0, 0xff, 0x88)));
    assert_eq!(r5.style.stroke_style, StrokeStyle::Dotted);
    assert_eq!(get(d, "r6").style.fill_color, Some(Color::rgb(0xaa, 0xbb, 0xcc)));
    // A diamond is a diamond now.
    assert_eq!(get(d, "d1").tag(), Kind::Diamond);
    assert_eq!(get(d, "d1").style.fill_pattern, FillPattern::Hachure);
    // "red" is not a colour we read: default stroke and a warning, no panic.
    let e2 = get(d, "e2");
    assert_eq!(e2.style.stroke_color, Color::rgb(0x1e, 0x1e, 0x1e));
    assert_eq!(e2.style.fill_color, None);
    assert!(i.report.warnings.iter().any(|w| w.contains("\"red\"")), "{:?}", i.report.warnings);
    // Lines and arrows: roundness, heads (legacy "dot" is a circle).
    assert_eq!(linear(get(d, "l1")).path, PathType::Sharp);
    assert_eq!(linear(get(d, "l2")).path, PathType::Round);
    let a1 = linear(get(d, "a1"));
    assert_eq!((a1.start_head, a1.end_head), (Head::Circle, Head::Triangle));
    assert_eq!((linear(get(d, "a2")).path, linear(get(d, "a2")).end_head), (PathType::Round, Head::Arrow));
    // Deleted and embedded elements are gone; the rest is in array order.
    assert!(d.elements.iter().all(|e| e.id.as_str() != "gone" && e.id.as_str() != "img-ignored"));
    assert!(i.report.warnings.iter().any(|w| w.contains("embeddable")));
    assert_eq!(d.elements.len(), 17);
    assert_eq!(get(d, "f1").tag(), Kind::Freehand);
}

#[test]
fn bindings_keep_both_ends_with_their_targets() {
    let i = scene("bindings");
    let d = &i.document;
    assert_clean(d);
    let a1 = linear(get(d, "arr1"));
    let (s, e) = (a1.start_binding.as_ref().unwrap(), a1.end_binding.as_ref().unwrap());
    assert_eq!((s.target.as_str(), e.target.as_str()), ("A", "B"));
    // No `mode` in the file: a fixed point on the edge is "orbit".
    assert_eq!((s.mode, s.fixed_point), (BindMode::Orbit, [1.0, 0.5]));
    assert_eq!((e.mode, e.fixed_point), (BindMode::Orbit, [0.0, 0.5]));
    assert_eq!(a1.end_head, Head::Triangle);
    let a2 = linear(get(d, "arr2"));
    assert_eq!(a2.start_binding.as_ref().unwrap().target.as_str(), "A");
    assert_eq!(a2.end_binding.as_ref().unwrap().target.as_str(), "C");
    assert_eq!((a2.start_head, a2.end_head), (Head::Bar, Head::Arrow));
    // The arrow whose target does not exist is loose, with a warning.
    let dangling = linear(get(d, "dangling"));
    assert!(dangling.start_binding.is_none() && dangling.end_binding.is_none());
    assert!(i.report.warnings.iter().any(|w| w.contains("attached to something that is missing")));
    // The label of the ellipse.
    assert_eq!(get(d, "lbl").container_id().map(|c| c.as_str()), Some("C"));
    assert_eq!(get(d, "lbl").text_data().unwrap().font.family, FontFamily::GelPen);
}

#[test]
fn legacy_bindings_without_fixed_point_or_mode_are_worked_out_from_the_arrow() {
    let target = |id: &str, x: f64| json!({"id": id, "type": "rectangle", "x": x, "y": 0, "width": 100, "height": 50});
    let arrow = json!({"id": "a", "type": "arrow", "x": 50, "y": 25, "points": [[0, 0], [250, 0]],
        "startBinding": {"elementId": "t1", "focus": 0, "gap": 4},
        "endBinding": {"elementId": "t2", "focus": 0, "gap": 4}});
    let d = import_elements(json!([target("t1", 0.0), target("t2", 300.0), arrow])).document;
    let a = linear(get(&d, "a"));
    let (s, e) = (a.start_binding.as_ref().unwrap(), a.end_binding.as_ref().unwrap());
    // Start at the middle of t1: inside. End at (300, 25), on t2's left edge: orbit.
    assert_eq!((s.mode, s.fixed_point), (BindMode::Inside, [0.5, 0.5]));
    assert_eq!(e.mode, BindMode::Orbit);
    assert_eq!(e.fixed_point, [0.0, 0.5]);
}

#[test]
fn skip_mode_and_bad_targets_leave_the_end_loose() {
    let els = json!([
        {"id": "r", "type": "rectangle", "x": 0, "y": 0, "width": 10, "height": 10},
        {"id": "ln", "type": "line", "x": 0, "y": 0, "points": [[0, 0], [5, 5]]},
        {"id": "a", "type": "arrow", "x": 0, "y": 0, "points": [[0, 0], [5, 5]],
         "startBinding": {"elementId": "r", "mode": "skip", "fixedPoint": [0.5, 0.5]},
         "endBinding": {"elementId": "ln", "mode": "orbit", "fixedPoint": [0.5, 0.5]}},
        // Bound to itself.
        {"id": "me", "type": "arrow", "x": 0, "y": 0, "points": [[0, 0], [5, 5]],
         "startBinding": {"elementId": "me", "mode": "inside", "fixedPoint": [0.5, 0.5]}},
    ]);
    let d = import_elements(els).document;
    for id in ["a", "me"] {
        let a = linear(get(&d, id));
        assert!(a.start_binding.is_none() && a.end_binding.is_none(), "{id}");
    }
}

#[test]
fn groups_keep_their_stacks_and_single_member_groups_vanish() {
    let i = scene("groups");
    let d = &i.document;
    assert_clean(d);
    assert_eq!(names(get(d, "g1")), ["G_inner", "G_outer"]);
    assert_eq!(names(get(d, "g2")), ["G_inner", "G_outer"]);
    assert_eq!(names(get(d, "g3")), ["G_outer"]);
    assert_eq!(names(get(d, "s1")), ["G_other"]);
    assert_eq!(names(get(d, "s2")), ["G_other"]);
    assert!(get(d, "loose").group_ids.is_empty());
    // A group of one is no group.
    assert!(get(d, "single").group_ids.is_empty());
}

#[test]
fn frames_children_and_z_order() {
    let i = scene("frames");
    let d = &i.document;
    assert_clean(d);
    let f1 = get(d, "frame1");
    match &f1.kind {
        ElementKind::Frame(f) => assert_eq!((f.name.as_deref(), f.clip), (Some("Frame 1"), true)),
        k => panic!("{k:?}"),
    }
    assert!(matches!(&get(d, "mframe").kind, ElementKind::Frame(f) if f.name.as_deref() == Some("Magic")));
    for (child, frame) in [("in1", "frame1"), ("in2", "frame1"), ("in3", "frame1"), ("in4", "mframe")] {
        assert_eq!(get(d, child).frame_id.as_ref().map(|f| f.as_str()), Some(frame), "{child}");
        // Children sit below their frame.
        assert!(get(d, child).index < get(d, frame).index, "{child} must be below {frame}");
    }
    assert!(get(d, "outside").frame_id.is_none());
    assert!(get(d, "frame1").frame_id.is_none());
    assert!(i.report.warnings.iter().any(|w| w.contains("magic frame")));
}

#[test]
fn frames_cannot_rotate_or_nest() {
    let els = json!([
        {"id": "f", "type": "frame", "x": 0, "y": 0, "width": 100, "height": 100, "angle": 0.5},
        {"id": "g", "type": "frame", "x": 0, "y": 0, "width": 50, "height": 50, "frameId": "f"},
        {"id": "x", "type": "rectangle", "x": 0, "y": 0, "width": 5, "height": 5, "frameId": "nothing"},
        {"id": "y", "type": "rectangle", "x": 0, "y": 0, "width": 5, "height": 5, "frameId": "x"},
    ]);
    let i = import_elements(els);
    let d = &i.document;
    assert_eq!(get(d, "f").transform.angle, 0.0);
    assert!(get(d, "g").frame_id.is_none());
    assert!(get(d, "x").frame_id.is_none(), "missing frame");
    assert!(get(d, "y").frame_id.is_none(), "not a frame");
    assert_clean(d);
}

#[test]
fn text_containers_labels_and_fonts() {
    let i = scene("text_containers");
    let d = &i.document;
    assert_clean(d);
    for (text, container) in [("txt1", "box1"), ("txt2", "box2"), ("txt3", "box3"), ("txt4", "arrowL")] {
        assert_eq!(get(d, text).container_id().map(|c| c.as_str()), Some(container), "{text}");
        assert!(get(d, text).index > get(d, container).index, "{text} above {container}");
    }
    for free in ["free", "wrapped", "noattr"] {
        assert!(get(d, free).container_id().is_none());
    }
    let t1 = get(d, "txt1").text_data().unwrap();
    assert_eq!(t1.content, "Centered in box");
    assert_eq!(
        (t1.font.align, t1.font.vertical_align),
        (drafftink_scene::TextAlign::Center, drafftink_scene::VerticalAlign::Middle)
    );
    assert_eq!(t1.font.line_height, 1.25);
    // Font numbers: 5 and 1 hand-written, 6 plain.
    assert_eq!(get(d, "txt2").text_data().unwrap().font.family, FontFamily::GelPen);
    assert_eq!(get(d, "txt3").text_data().unwrap().font.family, FontFamily::NotoSans);
    assert_eq!(get(d, "free").text_data().unwrap().font.size, 36.0);
    // Fixed width text keeps its box and wraps.
    let w = get(d, "wrapped");
    assert!(!w.text_data().unwrap().auto_resize);
    assert_eq!((w.transform.w, w.transform.h), (240.0, 50.0));
    assert_eq!(w.text_data().unwrap().content, "A text with a fixed width that wraps\nover two lines");
    // The rounded rectangle.
    assert_eq!(get(d, "box1").style.corner_radius, 25.0);
}

#[test]
fn elbow_arrows_keep_segments_bindings_and_flags() {
    let i = scene("elbow_arrows");
    let d = &i.document;
    assert_clean(d);
    let el1 = linear(get(d, "el1"));
    assert_eq!(el1.path, PathType::Elbow);
    assert_eq!(el1.points.len(), 4);
    assert_eq!(el1.fixed_segments.len(), 1);
    let seg = &el1.fixed_segments[0];
    assert_eq!(seg.index, 2);
    assert_eq!((seg.start, seg.end), (Point::new(140.0, 0.0), Point::new(140.0, 240.0)));
    assert_eq!(el1.start_binding.as_ref().unwrap().target.as_str(), "E1");
    assert_eq!(el1.end_binding.as_ref().unwrap().target.as_str(), "E2");
    assert_eq!(el1.start_binding.as_ref().unwrap().mode, BindMode::Orbit);
    assert!(!el1.start_is_special && !el1.end_is_special);
    assert_eq!(linear(get(d, "el2")).path, PathType::Elbow);
    // `roundness` without `elbowed` is a round arrow.
    assert_eq!(linear(get(d, "round")).path, PathType::Round);
}

#[test]
fn closed_paths_use_the_polygon_flag_not_a_distance_guess() {
    let i = scene("closed_paths");
    let d = &i.document;
    assert_clean(d);
    // The old importer closed any path whose ends were within 10 units. Excalidraw says so with `polygon`.
    assert!(!linear(get(d, "tri")).closed);
    assert!(!linear(get(d, "nearclosed")).closed);
    assert_eq!(linear(get(d, "tri")).points.len(), 4);
    let els = json!([{"id": "p", "type": "line", "x": 5, "y": 5, "polygon": true,
        "points": [[0, 0], [100, 0], [50, 80], [0, 0]]}]);
    let p = import_elements(els).document;
    let l = linear(get(&p, "p"));
    assert!(l.closed);
    assert_eq!(l.points.len(), 3, "the repeated last point is dropped");
    // Freehand: the stroke is kept, simulated pressure.
    let fd = match &get(d, "fd-closed").kind {
        ElementKind::Freehand(f) => f,
        k => panic!("{k:?}"),
    };
    assert_eq!((fd.points.len(), fd.pressure_mode), (5, PressureMode::Simulated));
    assert!(d.elements.iter().all(|e| !["onepoint", "fd-empty", "one-point-arrow"].contains(&e.id.as_str())));
}

#[test]
fn images_come_with_their_picture_and_unsupported_embeds_are_reported() {
    let i = scene("images_and_unsupported");
    let d = &i.document;
    assert_clean(d);
    assert_eq!(d.elements.len(), 2);
    assert_eq!(d.files.len(), 1);
    let img = get(d, "img1");
    let ElementKind::Image(data) = &img.kind else { panic!("not an image") };
    let id = data.file_id.as_ref().expect("file id");
    let file = &d.files[id];
    assert_eq!(file.mime, "image/png");
    assert_eq!(drafftink_scene::FileId::of(&file.bytes), *id);
    assert!(data.natural_width > 0 && data.natural_height > 0);
    assert!(data.crop.is_none());
    assert!(!img.transform.flip_x && !img.transform.flip_y);
    assert_eq!(i.report.warnings.iter().filter(|w| w.contains("embedded web page")).count(), 2);
    assert_eq!(get(d, "after").tag(), Kind::Rectangle);
}

#[test]
fn image_scale_and_crop_and_bad_picture_data() {
    let png = {
        let mut b = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, b'I', b'H', b'D', b'R'];
        b.extend_from_slice(&40u32.to_be_bytes());
        b.extend_from_slice(&30u32.to_be_bytes());
        b
    };
    use base64::Engine as _;
    let url = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&png));
    let img = |id: &str, file: &str, extra: Value| {
        let mut v = json!({"id": id, "type": "image", "x": 0, "y": 0, "width": 40, "height": 30, "fileId": file});
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        v
    };
    let text = json!({
        "type": "excalidraw", "version": 2,
        "elements": [
            img("flipped", "f1", json!({"scale": [-1, 1], "crop": {"x": 5, "y": 6, "width": 20, "height": 10, "naturalWidth": 40, "naturalHeight": 30}})),
            img("junk", "f2", json!({})),
            img("svg", "f3", json!({})),
            img("badmime", "f4", json!({})),
            img("nofile", "gone", json!({})),
            img("badcrop", "f1", json!({"crop": {"x": 5, "y": 6, "width": 0, "height": 10}})),
        ],
        "files": {
            "f1": {"id": "f1", "mimeType": "image/png", "dataURL": url, "created": 1},
            "f2": {"id": "f2", "mimeType": "image/png", "dataURL": "data:image/png;base64,@@@@", "created": 1},
            "f3": {"id": "f3", "mimeType": "image/svg+xml", "dataURL": "data:image/svg+xml;base64,PHN2Zy8+", "created": 1},
            "f4": {"id": "f4", "mimeType": "text/html", "dataURL": "data:text/html;base64,PGI+", "created": 1},
        },
    })
    .to_string();
    let i = import_str(&text).unwrap();
    assert_no_errors(&i.document);
    let d = &i.document;
    let ids: Vec<&str> = d.elements.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["flipped", "svg", "badcrop"]);
    let flipped = get(d, "flipped");
    assert!(flipped.transform.flip_x && !flipped.transform.flip_y);
    let ElementKind::Image(data) = &flipped.kind else { panic!() };
    assert_eq!((data.natural_width, data.natural_height), (40, 30), "from the PNG header");
    let c = data.crop.as_ref().unwrap();
    assert_eq!((c.x, c.y, c.width, c.height), (5.0, 6.0, 20.0, 10.0));
    // The same picture is stored once.
    assert_eq!(d.files.len(), 2);
    // SVG has no pixel size: the box stands in.
    let ElementKind::Image(svg) = &get(d, "svg").kind else { panic!() };
    assert_eq!((svg.natural_width, svg.natural_height), (40, 30));
    assert!(d.files.values().any(|f| f.mime == "image/svg+xml"));
    // A crop with no area shows the whole picture.
    assert!(matches!(&get(d, "badcrop").kind, ElementKind::Image(i) if i.crop.is_none()));
    assert!(i.report.warnings.iter().any(|w| w.contains("damaged")));
    assert!(i.report.warnings.iter().any(|w| w.contains("not an image type")));
    assert!(i.report.warnings.iter().any(|w| w.contains("not in `files`")));
}

#[test]
fn minimal_fields_use_defaults_and_odd_input_is_skipped() {
    let i = scene("minimal_fields");
    let d = &i.document;
    assert_clean(d);
    assert_eq!(d.elements.len(), 6);
    // Missing ids get new ones; missing size is 100 x 100; missing colour is Excalidraw's own black.
    let first = &d.elements[0];
    assert_eq!((first.transform.w, first.transform.h), (100.0, 100.0));
    assert_eq!(first.style.stroke_color, Color::rgb(0x1e, 0x1e, 0x1e));
    assert_eq!(first.style.opacity, 1.0);
    // Text without a size is measured with the estimate.
    let t = d.elements.iter().find(|e| e.tag() == Kind::Text).unwrap();
    assert!(t.transform.w >= 20.0 && t.transform.h > 0.0);
    assert!(i.report.warnings.iter().any(|w| w.contains("unknown type")));
    assert!(i.report.warnings.iter().any(|w| w.contains("no `type`")));
    // Non-hex colour strings fall back with a warning.
    assert!(i.report.warnings.iter().any(|w| w.contains("rgb(1,2,3)")));
}

// ---------------------------------------------------------------------------------------------------------------
// Round trips

#[test]
fn every_fixture_survives_import_export_import() {
    for name in SCENE_FIXTURES {
        let first = scene(name);
        let (text, report) = export_scene_with_report(&first.document).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(report.is_clean(), "{name}: {:?}", report.warnings);
        let second = import_str(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(second.report.is_clean(), "{name}: re-import warns: {:?}", second.report.warnings);
        assert_same_doc(&first.document, &second.document);
        // And the second export is byte-identical to the first: the writer is deterministic and the model is stable.
        assert_eq!(export_scene(&second.document).unwrap(), text, "{name}");
    }
}

#[test]
fn inline_math_goes_out_as_its_source_and_the_off_setting_comes_back() {
    let mut d = Document::new();
    let z = drafftink_scene::ZIndex::rebalance(2).unwrap();
    let make = |name: &str, z: &drafftink_scene::ZIndex, off: bool| {
        let mut t = drafftink_scene::TextData::new("Energy $E = mc^2$ costs \\$5");
        if off {
            t.font.inline_math = drafftink_scene::InlineMath::Off;
        }
        let mut e = Element::new(drafftink_scene::ElementId::new(name).unwrap(), ElementKind::Text(t), z.clone());
        e.transform = drafftink_scene::Transform::new(0.0, 0.0, 200.0, 25.0);
        e
    };
    d.elements = vec![make("on", &z[0], false), make("off", &z[1], true)];
    let text = export_scene(&d).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    for e in v["elements"].as_array().unwrap() {
        // Excalidraw has no inline math: the text is the source, delimiters and escape included.
        assert_eq!(e["text"], "Energy $E = mc^2$ costs \\$5");
        assert_eq!(e["originalText"], e["text"]);
    }
    let note =
        |id: &str| v["elements"].as_array().unwrap().iter().find(|e| e["id"] == id).unwrap()["customData"].clone();
    assert!(note("on").is_null(), "the default needs no note: {}", note("on"));
    assert_eq!(note("off")["drafftink"]["inline_math"], "off");
    let back = import_str(&text).unwrap().document;
    for (id, mode) in [("on", drafftink_scene::InlineMath::Auto), ("off", drafftink_scene::InlineMath::Off)] {
        let t = get(&back, id).text_data().unwrap();
        assert_eq!((t.content.as_str(), t.font.inline_math), ("Energy $E = mc^2$ costs \\$5", mode), "{id}");
    }
}

#[test]
fn export_derives_bound_elements_on_both_sides() {
    let d = scene("bindings").document;
    let v: Value = serde_json::from_str(&export_scene(&d).unwrap()).unwrap();
    let el = |id: &str| v["elements"].as_array().unwrap().iter().find(|e| e["id"] == id).unwrap().clone();
    let bound = |id: &str| -> Vec<(String, String)> {
        el(id)["boundElements"].as_array().map_or_else(Vec::new, |a| {
            a.iter().map(|b| (b["id"].as_str().unwrap().to_owned(), b["type"].as_str().unwrap().to_owned())).collect()
        })
    };
    assert_eq!(bound("A"), [("arr1".to_owned(), "arrow".to_owned()), ("arr2".to_owned(), "arrow".to_owned())]);
    assert_eq!(bound("B"), [("arr1".to_owned(), "arrow".to_owned())]);
    // C: the second arrow's end and the label text.
    assert_eq!(bound("C"), [("arr2".to_owned(), "arrow".to_owned()), ("lbl".to_owned(), "text".to_owned())]);
    assert_eq!(el("lbl")["containerId"], "C");
    assert_eq!(el("arr1")["startBinding"], json!({"elementId": "A", "fixedPoint": [1.0, 0.5], "mode": "orbit"}));
    assert_eq!(el("arr1")["endArrowhead"], "triangle");
    assert_eq!(el("arr2")["startArrowhead"], "bar");
    // The envelope.
    assert_eq!(v["type"], "excalidraw");
    assert_eq!(v["version"], 2);
    assert_eq!(v["appState"]["viewBackgroundColor"], "#ffffff");
    assert!(v["source"].as_str().unwrap().starts_with("https://"));
    // Deterministic version fields, no clock.
    assert_eq!(el("A")["version"], 1);
    assert_eq!(el("A")["updated"], 0);
    assert_eq!(el("A")["versionNonce"], export_nonce_again(&d, "A"));
}

fn export_nonce_again(d: &Document, id: &str) -> Value {
    let v: Value = serde_json::from_str(&export_scene(d).unwrap()).unwrap();
    v["elements"].as_array().unwrap().iter().find(|e| e["id"] == id).unwrap()["versionNonce"].clone()
}

fn base_element(doc: &mut Document, kind: ElementKind, x: f64, y: f64, w: f64, h: f64) -> usize {
    let n = doc.elements.len();
    let keys = drafftink_scene::ZIndex::rebalance(n + 1).unwrap();
    let mut e = Element::new(drafftink_scene::ElementId::new(&format!("n{n}")).unwrap(), kind, keys[n].clone())
        .with_bounds(x, y, w, h);
    e.style.seed = 100 + n as u32;
    doc.elements.push(e);
    for (el, k) in doc.elements.iter_mut().zip(keys) {
        el.index = k;
    }
    n
}

/// A document that uses everything Excalidraw cannot hold. It must come back unchanged.
#[test]
fn native_documents_survive_a_trip_through_excalidraw() {
    use drafftink_scene::{Brush, FontWeight, FrameData, ImageData, LinearData, MathData, TextData, TextRun};
    let mut doc = Document::new();
    doc.background = Color::rgb(0x12, 0x34, 0x56);
    let rect = base_element(&mut doc, ElementKind::Rectangle, 0.0, 0.0, 100.0, 60.0);
    doc.elements[rect].style.corner_radius = 17.0; // not what Excalidraw's rule would give (15)
    doc.elements[rect].style.fill_color = Some(Color::rgb(1, 2, 3));
    doc.elements[rect].style.fill_pattern = FillPattern::Dots;
    doc.elements[rect].style.sloppiness = Sloppiness::Drunk;
    doc.elements[rect].style.opacity = 0.25;
    doc.elements[rect].name = Some("hero".into());
    doc.elements[rect].locked = true;
    doc.elements[rect].link = Some("https://example.com/a?b=c".into());
    doc.elements[rect].custom = Some(BTreeMap::from([("mine".to_owned(), json!({"x": [1, 2]}))]));
    let ell = base_element(&mut doc, ElementKind::Ellipse, 200.0, 0.0, 50.0, 80.0);
    doc.elements[ell].style.corner_radius = 3.0; // an ellipse has no radius in Excalidraw
    doc.elements[ell].style.fill_pattern = FillPattern::ZigZagLine;
    doc.elements[ell].transform.angle = -0.75;
    let dia = base_element(&mut doc, ElementKind::Diamond, 300.0, 0.0, 80.0, 80.0);
    doc.elements[dia].style.corner_radius = 20.0;
    // A rotated arrow bound to the rectangle and ellipse, with every kind of extra.
    let mut lin = LinearData::new(vec![Point::new(0.0, 10.0), Point::new(60.0, 0.0), Point::new(120.0, 40.0)]);
    lin.path = PathType::Round;
    lin.start_head = Head::CardinalityZeroOrMany;
    lin.end_head = Head::DiamondOutline;
    lin.head_size = Some(13.0);
    lin.start_binding = Some(drafftink_scene::Binding {
        target: doc.elements[rect].id.clone(),
        mode: BindMode::Inside,
        fixed_point: [0.3, 0.7],
    });
    lin.end_binding = Some(drafftink_scene::Binding {
        target: doc.elements[ell].id.clone(),
        mode: BindMode::Orbit,
        fixed_point: [1.0, 0.5],
    });
    let arr = base_element(&mut doc, ElementKind::Arrow(lin), 10.0, 100.0, 120.0, 40.0);
    doc.elements[arr].transform.angle = 0.6;
    // An elbow arrow with fixed segments.
    let mut elbow = LinearData::new(vec![
        Point::new(0.0, 0.0),
        Point::new(50.0, 0.0),
        Point::new(50.0, 80.0),
        Point::new(100.0, 80.0),
    ]);
    elbow.path = PathType::Elbow;
    elbow.fixed_segments =
        vec![drafftink_scene::FixedSegment { index: 2, start: Point::new(50.0, 0.0), end: Point::new(50.0, 80.0) }];
    elbow.start_is_special = true;
    base_element(&mut doc, ElementKind::Arrow(elbow), 0.0, 200.0, 100.0, 80.0);
    // A closed line and a sharp line with a corner radius.
    let mut poly = LinearData::new(vec![Point::new(0.0, 0.0), Point::new(80.0, 0.0), Point::new(40.0, 60.0)]);
    poly.closed = true;
    base_element(&mut doc, ElementKind::Line(poly), 300.0, 200.0, 80.0, 60.0);
    let mut plain = LinearData::new(vec![Point::new(0.0, 0.0), Point::new(30.0, 30.0)]);
    plain.start_head = Head::Bar;
    let ln = base_element(&mut doc, ElementKind::Line(plain), 400.0, 200.0, 30.0, 30.0);
    doc.elements[ln].style.corner_radius = 4.0;
    // Freehand with device pressure, a calligraphy brush and closed.
    let mut fh =
        drafftink_scene::FreehandData::new(vec![Point::new(0.0, 0.0), Point::new(10.0, 5.0), Point::new(20.0, 0.0)]);
    fh.pressures = vec![0.1, 0.5, 0.9];
    fh.pressure_mode = PressureMode::Device;
    fh.brush = Brush::Calligraphy;
    fh.highlighter = true;
    fh.closed = true;
    base_element(&mut doc, ElementKind::Freehand(fh), 500.0, 200.0, 20.0, 5.0);
    // Text: serif light, per-character runs, a label for the rectangle; another for the arrow.
    let mut t = TextData::new("héllo wörld");
    t.font.family = FontFamily::GelPenSerif;
    t.font.weight = FontWeight::Light;
    t.font.align = drafftink_scene::TextAlign::Center;
    t.runs = vec![
        TextRun { start: 0, end: 3, color: Some(Color::rgb(255, 0, 0)), ..Default::default() },
        TextRun { start: 5, end: 6, color: None, ..Default::default() },
        TextRun { start: 6, end: 8, color: None, bold: true, italic: true },
        TextRun { start: 8, end: 11, italic: true, ..Default::default() },
    ];
    t.container_id = Some(doc.elements[rect].id.clone());
    let label = base_element(&mut doc, ElementKind::Text(t), 10.0, 20.0, 80.0, 25.0);
    doc.elements[label].style.stroke_color = Color::rgba(10, 20, 30, 128);
    let mut t2 = TextData::new("on the arrow");
    t2.container_id = Some(doc.elements[arr].id.clone());
    t2.label_position = Some(0.25);
    t2.font.family = FontFamily::VanillaExtract;
    base_element(&mut doc, ElementKind::Text(t2), 50.0, 110.0, 90.0, 25.0);
    // Math.
    let mut m = MathData::new("\\frac{a}{b}");
    m.font_size = 33.0;
    base_element(&mut doc, ElementKind::Math(m), 600.0, 0.0, 40.0, 50.0);
    // A frame with children, no clipping, and an image.
    let (png_id, file) = drafftink_scene::FileEntry::new("image/png", {
        let mut b = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, b'I', b'H', b'D', b'R'];
        b.extend_from_slice(&8u32.to_be_bytes());
        b.extend_from_slice(&4u32.to_be_bytes());
        b
    });
    doc.files.insert(png_id.clone(), file);
    let img = base_element(
        &mut doc,
        ElementKind::Image(ImageData {
            file_id: Some(png_id),
            natural_width: 8,
            natural_height: 4,
            crop: Some(drafftink_scene::Crop { x: 1.0, y: 1.0, width: 4.0, height: 2.0 }),
        }),
        700.0,
        0.0,
        80.0,
        40.0,
    );
    doc.elements[img].transform.flip_y = true;
    doc.elements[img].transform.angle = 0.1;
    let frame = base_element(
        &mut doc,
        ElementKind::Frame(FrameData { name: Some("Frame".into()), clip: false }),
        690.0,
        -10.0,
        200.0,
        100.0,
    );
    // The frame must be above its child: swap by giving the image the frame id and moving the frame last.
    doc.elements[img].frame_id = Some(doc.elements[frame].id.clone());
    // Groups: two nested groups over three elements.
    let (g1, g2) = (drafftink_scene::GroupId::new("inner").unwrap(), drafftink_scene::GroupId::new("outer").unwrap());
    doc.elements[rect].group_ids.push(g1.clone());
    doc.elements[rect].group_ids.push(g2.clone());
    doc.elements[ell].group_ids.push(g1);
    doc.elements[ell].group_ids.push(g2.clone());
    doc.elements[dia].group_ids.push(g2);
    // A tombstone, which is not exported.
    let dead = base_element(&mut doc, ElementKind::Rectangle, 0.0, 0.0, 1.0, 1.0);
    doc.elements[dead].deleted = true;
    assert_no_errors(&doc);

    let (text, report) = export_scene_with_report(&doc).unwrap();
    assert!(report.is_clean(), "{:?}", report.warnings);
    let back = import_str(&text).unwrap();
    assert!(back.report.is_clean(), "{:?}", back.report.warnings);
    let mut expected = doc.clone();
    expected.elements.retain(|e| !e.deleted);
    // Math, fonts and fills went through customData.
    let v: Value = serde_json::from_str(&text).unwrap();
    let note = |id: &str| {
        v["elements"].as_array().unwrap().iter().find(|e| e["id"] == id).unwrap()["customData"]["drafftink"].clone()
    };
    assert_eq!(note("n10")["math"], true);
    // Excalidraw has no bold or italic per character: it shows the text plain, and the note keeps the runs.
    let label = v["elements"].as_array().unwrap().iter().find(|e| e["text"] == "héllo wörld").unwrap()["id"].clone();
    let runs = note(label.as_str().unwrap())["runs"].clone();
    assert_eq!(runs[2], json!({"start": 6, "end": 8, "bold": true, "italic": true}), "{runs}");
    assert_eq!(runs[3], json!({"start": 8, "end": 11, "italic": true}));
    assert_eq!(note("n0")["fill_pattern"], "dots");
    assert_eq!(note("n0")["sloppiness"], "drunk");
    assert_eq!(note("n0")["corner_radius"], 17.0);
    assert_eq!(v["elements"][0]["customData"]["mine"], json!({"x": [1, 2]}));
    assert_eq!(v["elements"][0]["roughness"], 2.0);
    assert_eq!(v["elements"][0]["fillStyle"], "hachure", "dots falls back to the nearest Excalidraw fill");
    assert_same_doc(&expected, &back.document);
}

#[test]
fn rotated_lines_keep_every_point_in_place() {
    // Excalidraw rotates a linear element about the centre of its points' bounding box.
    let (x, y, angle) = (10.0_f64, 20.0_f64, 0.5_f64);
    let rel = [[0.0, 0.0], [100.0, 0.0], [100.0, 50.0], [-30.0, 10.0]];
    let els = json!([
        {"id": "r", "type": "arrow", "x": x, "y": y, "angle": angle, "points": rel},
        {"id": "f", "type": "freedraw", "x": x, "y": y, "angle": angle, "points": rel, "pressures": [], "simulatePressure": true},
    ]);
    let d = import_elements(els).document;
    // Centre of the unrotated bounding box in the file's coordinates.
    let (min_x, max_x, min_y, max_y) = (-30.0, 100.0, 0.0, 50.0);
    let c = Point::new(x + (min_x + max_x) / 2.0, y + (min_y + max_y) / 2.0);
    let rot = |p: Point| {
        let (s, co) = angle.sin_cos();
        let (dx, dy) = (p.x - c.x, p.y - c.y);
        Point::new(c.x + dx * co - dy * s, c.y + dx * s + dy * co)
    };
    for id in ["r", "f"] {
        let e = get(&d, id);
        let pts = e.points().unwrap();
        for (p, r) in pts.iter().zip(rel) {
            let expect = rot(Point::new(x + r[0], y + r[1]));
            let got = e.transform.local_to_world(*p);
            assert!((got - expect).hypot() < 1e-9, "{id}: {got:?} vs {expect:?}");
        }
        assert_eq!(e.transform.angle, angle);
    }
    // And back out: the rotated arrow exports to the same x, y and relative points.
    let v: Value = serde_json::from_str(&export_scene(&d).unwrap()).unwrap();
    let a = &v["elements"][0];
    assert!((a["x"].as_f64().unwrap() - x).abs() < 1e-9 && (a["y"].as_f64().unwrap() - y).abs() < 1e-9);
    for (i, r) in rel.iter().enumerate() {
        assert!((a["points"][i][0].as_f64().unwrap() - r[0]).abs() < 1e-9);
        assert!((a["points"][i][1].as_f64().unwrap() - r[1]).abs() < 1e-9);
    }
    assert_eq!(a["angle"].as_f64().unwrap(), angle);
}

#[test]
fn export_refuses_nothing_silently() {
    // An image whose picture is missing from the table is left out, with a warning.
    let mut doc = Document::new();
    let i = base_element(
        &mut doc,
        ElementKind::Image(drafftink_scene::ImageData {
            file_id: None,
            natural_width: 1,
            natural_height: 1,
            crop: None,
        }),
        0.0,
        0.0,
        10.0,
        10.0,
    );
    base_element(&mut doc, ElementKind::Rectangle, 0.0, 0.0, 10.0, 10.0);
    let (text, report) = export_scene_with_report(&doc).unwrap();
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(report.warnings[0].contains(doc.elements[i].id.as_str()));
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap()["elements"].as_array().unwrap().len(), 1);
    // A document that breaks a model rule is an error, not a half-written file.
    let mut bad = Document::new();
    let r = base_element(&mut bad, ElementKind::Rectangle, 0.0, 0.0, 10.0, 10.0);
    bad.elements[r].transform.w = f64::NAN;
    assert!(matches!(export_scene(&bad), Err(IoError::Document(_))));
    // A user's own customData.drafftink is reported when the export note replaces it.
    let mut doc = Document::new();
    let r = base_element(&mut doc, ElementKind::Rectangle, 0.0, 0.0, 10.0, 10.0);
    doc.elements[r].style.sloppiness = Sloppiness::Drunk;
    doc.elements[r].custom = Some(BTreeMap::from([("drafftink".to_owned(), json!(1))]));
    let (_, report) = export_scene_with_report(&doc).unwrap();
    assert_eq!(report.warnings.len(), 1);
}

// ---------------------------------------------------------------------------------------------------------------
// Libraries

fn library(name: &str) -> (Library, drafftink_io::ImportReport) {
    import_library(&read(name), &mut SeqIds::new()).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn old_items(name: &str) -> Vec<(String, usize)> {
    let v = json_file(&format!("expected/{name}.items.json"));
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| (i["name"].as_str().unwrap().to_owned(), i["children"].as_u64().unwrap() as usize))
        .collect()
}

#[test]
fn libraries_have_the_items_and_names_the_old_importer_found() {
    for (file, stem) in [
        ("fixture_library.excalidrawlib", "fixture_library"),
        ("v1_library.excalidrawlib", "v1_library"),
        ("grouped_library.excalidrawlib", "grouped_library"),
    ] {
        let (lib, _) = library(file);
        for item in &lib.items {
            assert_no_errors(&item.document);
        }
        let old = old_items(stem);
        assert_eq!(lib.items.len(), old.len(), "{file}: item count");
        for (i, (item, (old_name, _))) in lib.items.iter().zip(&old).enumerate() {
            // The old code invented "Item N" for a missing or blank name; here the name is None.
            let shown = item.name.clone().unwrap_or_else(|| format!("Item {}", i + 1));
            assert_eq!(&shown, old_name, "{file} item {i}");
        }
    }
}

#[test]
fn library_items_share_group_ids_as_in_the_file() {
    let (lib, report) = library("grouped_library.excalidrawlib");
    assert_eq!(lib.items.len(), 3);
    // Item 4 only had an image (no picture in a library) and item 5 had no elements key.
    assert!(report.warnings.iter().any(|w| w.contains("item 4")), "{:?}", report.warnings);
    assert!(report.warnings.iter().any(|w| w.contains("item 5")));
    let cloud = &lib.items[0].document;
    assert_eq!(cloud.elements.len(), 4);
    assert_eq!(names(get(cloud, "c-t")), ["OUTER"]);
    // ICON_A has a single member and is gone; ICON and ICON_B stay.
    assert_eq!(names(get(cloud, "c-r")), ["ICON", "OUTER"]);
    assert_eq!(names(get(cloud, "c-e")), ["ICON_B", "ICON", "OUTER"]);
    assert_eq!(names(get(cloud, "c-l")), ["ICON_B", "ICON", "OUTER"]);
    assert_clean(cloud);
    assert_eq!(lib.items[0].name.as_deref(), Some("CloudSearch"));
    assert_eq!(lib.items[1].name, None, "a blank name is no name");
    assert_eq!(lib.items[2].name.as_deref(), Some("Arrow pair"));
}

#[test]
fn v1_and_v2_libraries_are_both_read() {
    let (v1, _) = library("fixture_library.excalidrawlib");
    assert_eq!(v1.items.len(), 1);
    assert_eq!(v1.items[0].document.elements.len(), 1);
    // v2 with items as plain arrays (some tools write that) and as objects.
    let text = json!({"type": "excalidrawlib", "version": 2, "libraryItems": [
        [{"id": "a", "type": "rectangle", "x": 0, "y": 0, "width": 5, "height": 5}],
        {"id": "i", "name": "named", "elements": [{"id": "b", "type": "ellipse", "x": 0, "y": 0, "width": 5, "height": 5}]},
        "junk", 7, {"id": "empty"},
    ]})
    .to_string();
    let (lib, report) = import_library(&text, &mut SeqIds::new()).unwrap();
    assert_eq!(lib.items.len(), 2);
    assert_eq!(lib.items[1].name.as_deref(), Some("named"));
    assert_eq!(report.warnings.len(), 3, "{:?}", report.warnings);
}

#[test]
fn not_a_library_is_rejected_for_the_right_reason() {
    let e = import_library(&read("not_a_library.excalidrawlib"), &mut SeqIds::new()).unwrap_err();
    assert!(matches!(e, IoError::WrongFormat { .. }), "{e:?}");
    assert!(matches!(import_library("nope", &mut SeqIds::new()), Err(IoError::Parse { .. })));
    assert!(matches!(import_library("[1]", &mut SeqIds::new()), Err(IoError::WrongFormat { .. })));
    // A scene is not a library, and a library is not a scene.
    assert!(matches!(
        import_scene(&read("fixture_library.excalidrawlib"), &mut SeqIds::new()),
        Err(IoError::WrongFormat { .. })
    ));
    // A library with no list is empty, not an error.
    let (lib, report) = import_library(r#"{"type":"excalidrawlib","version":2}"#, &mut SeqIds::new()).unwrap();
    assert!(lib.items.is_empty() && !report.is_clean());
}

fn old_grid_positions(name: &str) -> Vec<(String, f64, f64)> {
    // (kind, x, y) of every non-group shape in the old grid, depth first. Ellipses are given by their top-left.
    fn walk(v: &Value, out: &mut Vec<(String, f64, f64)>) {
        let Some(obj) = v.as_object() else { return };
        for (kind, body) in obj {
            match kind.as_str() {
                "Group" => body["children"].as_array().unwrap().iter().for_each(|c| walk(c, out)),
                "Ellipse" => out.push((
                    kind.clone(),
                    body["center"]["x"].as_f64().unwrap() - body["radius_x"].as_f64().unwrap(),
                    body["center"]["y"].as_f64().unwrap() - body["radius_y"].as_f64().unwrap(),
                )),
                _ => out.push((
                    kind.clone(),
                    body["position"]["x"].as_f64().unwrap(),
                    body["position"]["y"].as_f64().unwrap(),
                )),
            }
        }
    }
    let doc = json_file(&format!("expected/{name}.grid.json"));
    let mut out = Vec::new();
    for id in doc["z_order"].as_array().unwrap() {
        walk(&doc["shapes"][id.as_str().unwrap()], &mut out);
    }
    out
}

#[test]
fn grid_layout_matches_the_old_positions() {
    // These two libraries have no text whose size the old code had to estimate, so the layout is the same.
    for (file, stem) in
        [("fixture_library.excalidrawlib", "fixture_library"), ("v1_library.excalidrawlib", "v1_library")]
    {
        let (lib, _) = library(file);
        let grid = library_to_grid_document(&lib, &mut SeqIds::new());
        assert_no_errors(&grid);
        let old = old_grid_positions(stem);
        assert_eq!(grid.elements.len(), old.len(), "{stem}");
        for (e, (kind, x, y)) in grid.elements.iter().zip(&old) {
            let tag = match e.tag() {
                Kind::Rectangle => "Rectangle",
                Kind::Ellipse => "Ellipse",
                Kind::Text => "Text",
                k => panic!("{k:?}"),
            };
            assert_eq!(tag, kind, "{stem}");
            assert!(
                (e.transform.x - x).abs() < 1e-6 && (e.transform.y - y).abs() < 1e-6,
                "{stem}: {} at ({}, {}) vs ({x}, {y})",
                e.id,
                e.transform.x,
                e.transform.y
            );
        }
    }
}

#[test]
fn grid_groups_each_item_and_keeps_ids_unique() {
    let (lib, _) = library("v1_library.excalidrawlib");
    let grid = library_to_grid_document(&lib, &mut SeqIds::new());
    assert_clean(&grid);
    // Item 1 has one element, so no group; item 2 has two and one new group over both.
    let first = &grid.elements[0];
    assert!(first.group_ids.is_empty());
    let (e, t) = (&grid.elements[1], &grid.elements[2]);
    assert_eq!(e.group_ids.len(), 1);
    assert_eq!(e.group_ids, t.group_ids);

    // Grouped library: one new outermost group per item, after the file's own (innermost first, outermost last).
    let (lib, _) = library("grouped_library.excalidrawlib");
    let grid = library_to_grid_document(&lib, &mut SeqIds::new());
    assert_clean(&grid);
    let cl = get(&grid, "c-e");
    assert_eq!(cl.group_ids.len(), 4);
    assert_eq!(&names(cl)[..3], ["ICON_B", "ICON", "OUTER"]);
    let outer = cl.group_ids.last().unwrap();
    for id in ["c-t", "c-r", "c-e", "c-l"] {
        assert_eq!(get(&grid, id).group_ids.last(), Some(outer));
    }
    assert_eq!(grid.elements.len(), 4 + 1 + 2);
    // Two items with the same element ids are renamed, not merged.
    let item = |doc_id: &str| {
        let els = json!([{"id": doc_id, "type": "rectangle", "x": 3, "y": 3, "width": 10, "height": 10, "groupIds": ["G"]},
                         {"id": "second", "type": "rectangle", "x": 20, "y": 3, "width": 10, "height": 10, "groupIds": ["G"]}]);
        LibraryItem { name: None, document: import_elements(els).document }
    };
    let lib = Library { items: vec![item("same"), item("same"), item("same")] };
    let grid = library_to_grid_document(&lib, &mut SeqIds::new());
    assert_clean(&grid);
    assert_eq!(grid.elements.len(), 6);
}

#[test]
fn grid_cells_are_uniform_with_padding_and_ten_columns() {
    let items: Vec<LibraryItem> = (0..12)
        .map(|i| {
            let (w, h) = (10.0 + i as f64, 20.0);
            let els = json!([{"id": "r", "type": "rectangle", "x": 1000, "y": -50, "width": w, "height": h}]);
            LibraryItem { name: None, document: import_elements(els).document }
        })
        .collect();
    let grid = library_to_grid_document(&Library { items }, &mut SeqIds::new());
    assert_clean(&grid);
    assert_eq!(grid.elements.len(), 12);
    // 12 items: ceil(sqrt(12)) = 4 columns. Widest item is 21, so cells are 21 + 48 by 20 + 48.
    let (cw, ch) = (21.0 + 48.0, 20.0 + 48.0);
    for (i, e) in grid.elements.iter().enumerate() {
        let (col, row) = ((i % 4) as f64, (i / 4) as f64);
        let (cx, cy) = (col * cw + cw / 2.0, row * ch + ch / 2.0);
        let c = e.transform.center();
        assert!((c.x - cx).abs() < 1e-9 && (c.y - cy).abs() < 1e-9, "item {i}: {c:?} vs ({cx}, {cy})");
    }
    assert!(library_to_grid_document(&Library::default(), &mut SeqIds::new()).elements.is_empty());
}

#[test]
fn libraries_round_trip_through_export() {
    for file in ["fixture_library.excalidrawlib", "v1_library.excalidrawlib", "grouped_library.excalidrawlib"] {
        let (lib, _) = library(file);
        let (text, report) = export_library_with_report(&lib).unwrap();
        assert!(report.is_clean());
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!((v["type"].as_str(), v["version"].as_i64()), (Some("excalidrawlib"), Some(2)));
        let (back, _) = import_library(&text, &mut SeqIds::new()).unwrap();
        assert_eq!(back.items.len(), lib.items.len(), "{file}");
        for (a, b) in lib.items.iter().zip(&back.items) {
            assert_eq!(a.name, b.name);
            assert_same_doc(&a.document, &b.document);
        }
    }
}

#[test]
fn exporting_a_library_leaves_images_out_with_a_warning() {
    let mut doc = Document::new();
    base_element(&mut doc, ElementKind::Rectangle, 0.0, 0.0, 10.0, 10.0);
    let png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let (id, file) = drafftink_scene::FileEntry::new("image/png", png);
    doc.files.insert(id.clone(), file);
    base_element(
        &mut doc,
        ElementKind::Image(drafftink_scene::ImageData {
            file_id: Some(id),
            natural_width: 1,
            natural_height: 1,
            crop: None,
        }),
        0.0,
        0.0,
        5.0,
        5.0,
    );
    let lib = Library { items: vec![LibraryItem { name: Some("x".into()), document: doc }] };
    let (text, report) = export_library_with_report(&lib).unwrap();
    assert_eq!(report.warnings.len(), 1);
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap()["libraryItems"][0]["elements"].as_array().unwrap().len(),
        1
    );
}

// ---------------------------------------------------------------------------------------------------------------
// Clipboard

#[test]
fn clipboard_fixtures() {
    let ok = import_clipboard(&read("clipboard/basic.json"), &mut SeqIds::new()).unwrap();
    assert_no_errors(&ok.document);
    let kinds: Vec<Kind> = ok.document.elements.iter().map(Element::tag).collect();
    assert_eq!(kinds, [Kind::Rectangle, Kind::Ellipse, Kind::Arrow, Kind::Text]);
    // The old importer accepted exactly this one, and refused the other two.
    assert!(json_file("expected/clipboard_basic.result.json")["accepted"].as_bool().unwrap());
    for (name, why) in [("clipboard/not_clipboard.json", "WrongFormat"), ("clipboard/empty.json", "Invalid")] {
        let accepted = json_file(&format!(
            "expected/{}.result.json",
            name.replace("clipboard/", "clipboard_").trim_end_matches(".json")
        ))["accepted"]
            .as_bool()
            .unwrap();
        assert!(!accepted);
        let err = import_clipboard(&read(name), &mut SeqIds::new()).unwrap_err();
        assert!(format!("{err:?}").starts_with(why), "{name}: {err:?}");
    }
}

#[test]
fn clipboard_types_and_pictures() {
    let el = json!([{"id": "a", "type": "rectangle", "x": 0, "y": 0, "width": 5, "height": 5}]);
    for ty in ["excalidraw/clipboard", "excalidraw-api/clipboard"] {
        let t = json!({"type": ty, "elements": el}).to_string();
        assert!(import_clipboard(&t, &mut SeqIds::new()).is_ok(), "{ty}");
    }
    for ty in [json!("excalidraw"), json!("excalidraw/clipboard_with_api"), json!(null), json!(5)] {
        let t = json!({"type": ty, "elements": el}).to_string();
        assert!(matches!(import_clipboard(&t, &mut SeqIds::new()), Err(IoError::WrongFormat { .. })), "{ty}");
    }
    assert!(matches!(import_clipboard("[]", &mut SeqIds::new()), Err(IoError::WrongFormat { .. })));
    assert!(matches!(import_clipboard("not json", &mut SeqIds::new()), Err(IoError::Parse { .. })));
    // A pasted image comes with its picture.
    let t = json!({"type": "excalidraw/clipboard",
        "elements": [{"id": "i", "type": "image", "x": 0, "y": 0, "width": 4, "height": 4, "fileId": "f"}],
        "files": {"f": {"id": "f", "mimeType": "image/png", "dataURL": "data:image/png;base64,iVBORw0KGgo=", "created": 1}}})
    .to_string();
    let i = import_clipboard(&t, &mut SeqIds::new()).unwrap();
    assert_eq!(i.document.files.len(), 1);
}

// ---------------------------------------------------------------------------------------------------------------
// Hostile input

fn scene_with(elements: Value) -> String {
    json!({"type": "excalidraw", "version": 2, "elements": elements}).to_string()
}

#[test]
fn multibyte_colour_strings_do_not_panic() {
    // The old colour parser sliced bytes and panicked on these.
    let text = read("hostile/multibyte_colors.excalidraw");
    let i = import_str(&text).unwrap();
    assert_no_errors(&i.document);
    assert!(!i.document.elements.is_empty());
    assert!(i.report.warnings.iter().any(|w| w.contains("not a colour")));
    for e in &i.document.elements {
        assert_eq!(e.style.stroke_color, Color::rgb(0x1e, 0x1e, 0x1e), "{}", e.id);
        assert_eq!(e.style.fill_color, None, "{}", e.id);
    }
    // The same through the library and clipboard readers, and in the background colour.
    let lib = json!({"type": "excalidrawlib", "version": 2, "libraryItems": [{"elements": [{"id": "a", "type": "rectangle", "strokeColor": "#aé"}]}]}).to_string();
    assert!(import_library(&lib, &mut SeqIds::new()).is_ok());
    let t = json!({"type": "excalidraw", "elements": [], "appState": {"viewBackgroundColor": "#ééé"}}).to_string();
    let i = import_str(&t).unwrap();
    assert_eq!(i.document.background, Color::WHITE);
    assert!(!i.report.is_clean());
}

#[test]
fn hostile_fixture_files_load() {
    for entry in std::fs::read_dir(dir().join("hostile")).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        match path.extension().and_then(|e| e.to_str()) {
            Some("excalidraw") => {
                let d = import_str(&text).unwrap_or_else(|e| panic!("{path:?}: {e}")).document;
                assert_no_errors(&d);
            }
            Some("excalidrawlib") => {
                let (lib, _) = import_library(&text, &mut SeqIds::new()).unwrap_or_else(|e| panic!("{path:?}: {e}"));
                lib.items.iter().for_each(|i| assert_no_errors(&i.document));
                assert_no_errors(&library_to_grid_document(&lib, &mut SeqIds::new()));
            }
            _ => {}
        }
    }
}

#[test]
fn huge_numbers_are_clamped_and_wrong_types_use_defaults() {
    let els = json!([
        {"id": "big", "type": "rectangle", "x": 1e300, "y": -1e300, "width": 1e300, "height": -1e300, "angle": 1e300,
         "strokeWidth": 1e300, "opacity": 1e300, "seed": 1e300},
        {"id": "str", "type": "rectangle", "x": "12", "y": "NaN", "width": "Infinity", "height": null, "angle": "pi",
         "strokeWidth": "thick", "opacity": "half", "roughness": [], "groupIds": "g", "locked": "yes", "link": 5},
        {"id": "pts", "type": "line", "x": 0, "y": 0, "points": [[1e300, -1e300], [0, 0], [1, "x"], "p", [1], null]},
        {"id": "txt", "type": "text", "x": 0, "y": 0, "text": 5, "fontSize": 1e300, "lineHeight": -4, "fontFamily": 99, "textAlign": "diagonal"},
        {"id": "txt2", "type": "text", "x": 0, "y": 0, "text": "ok", "fontSize": 0, "lineHeight": 1e300, "width": 1e300, "height": 1e300},
        {"id": "txt3", "type": "text", "x": 0, "y": 0, "text": "ok", "fontSize": 1e-308, "lineHeight": 1.1e-308},
        7, "x", null, [], {"type": 5}, {"type": "text", "text": "no id"},
    ]);
    let i = import_str(&scene_with(els)).unwrap();
    assert_no_errors(&i.document);
    let d = &i.document;
    let big = get(d, "big");
    assert!(big.transform.x.abs() <= MAX_COORD && big.transform.w <= MAX_COORD && big.transform.h <= MAX_COORD);
    assert!(big.transform.angle.is_finite());
    assert_eq!(big.style.opacity, 1.0);
    assert!(big.style.stroke_width <= 1000.0);
    let s = get(d, "str");
    assert_eq!((s.transform.x, s.transform.y, s.transform.w, s.transform.h), (0.0, 0.0, 100.0, 100.0));
    assert!(!s.locked && s.link.is_none() && s.group_ids.is_empty());
    assert_eq!(s.style.opacity, 1.0);
    let pts = get(d, "pts");
    assert_eq!(pts.points().unwrap().len(), 2, "only the two valid number pairs stay");
    assert!(!i.report.is_clean());
    assert_eq!(get(d, "txt").text_data().unwrap().font.size, 10_000.0);
    assert_eq!(get(d, "txt2").text_data().unwrap().font.size, 20.0);
    // Subnormal sizes are below what the model accepts: they read as missing.
    let t3 = get(d, "txt3").text_data().unwrap();
    assert_eq!((t3.font.size, t3.font.line_height), (20.0, 1.25));
}

#[test]
fn a_million_empty_elements_are_bounded() {
    let text = format!(r#"{{"type":"excalidraw","elements":[{}]}}"#, vec!["{}"; 1_000_000].join(","));
    let i = import_str(&text).unwrap();
    assert!(i.document.elements.is_empty());
    assert!(i.report.warnings.len() <= drafftink_io::limits::MAX_WARNINGS);
    assert!(i.report.omitted > 900_000);
}

#[test]
fn more_elements_than_the_limit_are_cut_off() {
    let one = r#"{"type":"rectangle","width":1,"height":1}"#;
    let text = format!(r#"{{"type":"excalidraw","elements":[{}]}}"#, vec![one; MAX_ELEMENTS + 10].join(","));
    let i = import_str(&text).unwrap();
    assert_eq!(i.document.elements.len(), MAX_ELEMENTS);
    assert!(i.report.warnings.iter().any(|w| w.contains("only the first")));
}

#[test]
fn cyclic_and_self_references_are_resolved() {
    let els = json!([
        // An arrow bound to itself and to its own label; the label is bound to the arrow.
        {"id": "a", "type": "arrow", "x": 0, "y": 0, "points": [[0, 0], [10, 10]],
         "startBinding": {"elementId": "a", "mode": "orbit", "fixedPoint": [0.5, 0.5]},
         "endBinding": {"elementId": "t", "mode": "orbit", "fixedPoint": [0.5, 0.5]},
         "boundElements": [{"id": "t", "type": "text"}]},
        {"id": "t", "type": "text", "x": 0, "y": 0, "text": "label", "containerId": "a", "width": 10, "height": 10},
        // A text that is its own container, and a text inside a text.
        {"id": "s", "type": "text", "x": 0, "y": 0, "text": "self", "containerId": "s", "width": 10, "height": 10},
        {"id": "u", "type": "text", "x": 0, "y": 0, "text": "u", "containerId": "s", "width": 10, "height": 10},
        // Two texts claim one rectangle.
        {"id": "r", "type": "rectangle", "x": 0, "y": 0, "width": 50, "height": 50},
        {"id": "l1", "type": "text", "x": 0, "y": 0, "text": "1", "containerId": "r", "width": 10, "height": 10},
        {"id": "l2", "type": "text", "x": 0, "y": 0, "text": "2", "containerId": "r", "width": 10, "height": 10},
        // Frames that name each other and themselves.
        {"id": "f1", "type": "frame", "x": 0, "y": 0, "width": 9, "height": 9, "frameId": "f2"},
        {"id": "f2", "type": "frame", "x": 0, "y": 0, "width": 9, "height": 9, "frameId": "f1"},
        {"id": "x1", "type": "rectangle", "x": 0, "y": 0, "width": 9, "height": 9, "frameId": "x1"},
        // A container that is a label of the first arrow.
        {"id": "a2", "type": "arrow", "x": 0, "y": 0, "points": [[0, 0], [10, 10]], "containerId": "a"},
    ]);
    let i = import_elements(els);
    let d = &i.document;
    let a = linear(get(d, "a"));
    assert!(a.start_binding.is_none(), "an arrow cannot bind to itself");
    // Binding the arrow end to its own label is legal in the model and kept.
    assert_eq!(a.end_binding.as_ref().map(|b| b.target.as_str()), Some("t"));
    assert_eq!(get(d, "t").container_id().map(|c| c.as_str()), Some("a"));
    assert!(get(d, "s").container_id().is_none() && get(d, "u").container_id().is_none());
    let labelled = ["l1", "l2"].iter().filter(|id| get(d, id).container_id().is_some()).count();
    assert_eq!(labelled, 1);
    assert!(get(d, "f1").frame_id.is_none() && get(d, "f2").frame_id.is_none() && get(d, "x1").frame_id.is_none());
    assert_clean(d);
}

#[test]
fn group_stacks_that_contradict_or_run_too_deep_are_repaired() {
    let deep: Vec<String> = (0..40).map(|i| format!("G{i}")).collect();
    let els = json!([
        {"id": "a", "type": "rectangle", "groupIds": deep, "width": 1, "height": 1},
        {"id": "b", "type": "rectangle", "groupIds": deep, "width": 1, "height": 1},
        // Contradicting nesting: "p" is inside "q" for c and d, but "q" is inside "p" for e and f.
        {"id": "c", "type": "rectangle", "groupIds": ["p", "q"], "width": 1, "height": 1},
        {"id": "d", "type": "rectangle", "groupIds": ["p", "q"], "width": 1, "height": 1},
        {"id": "e", "type": "rectangle", "groupIds": ["q", "p"], "width": 1, "height": 1},
        {"id": "f", "type": "rectangle", "groupIds": ["q", "p"], "width": 1, "height": 1},
        // The same group twice on one element, and not strings.
        {"id": "g", "type": "rectangle", "groupIds": ["z", "z", 5, null, {"a": 1}], "width": 1, "height": 1},
        {"id": "h", "type": "rectangle", "groupIds": ["z"], "width": 1, "height": 1},
    ]);
    let i = import_elements(els);
    let d = &i.document;
    assert_eq!(get(d, "a").group_ids.len(), MAX_GROUP_DEPTH, "the outermost 16 stay");
    // groupIds run innermost first, so G39 is the outermost group and G24 the innermost one that is kept.
    assert_eq!((names(get(d, "a")).first(), names(get(d, "a")).last()), (Some(&"G24"), Some(&"G39")));
    assert_eq!(get(d, "a").group_ids, get(d, "b").group_ids);
    assert_eq!(names(get(d, "g")), ["z"]);
    assert!(!i.report.is_clean());
}

#[test]
fn duplicate_missing_and_odd_ids_are_made_valid_and_references_follow() {
    let els = json!([
        {"id": "dup", "type": "rectangle", "x": 0, "y": 0, "width": 1, "height": 1},
        {"id": "dup", "type": "rectangle", "x": 5, "y": 0, "width": 1, "height": 1},
        {"type": "rectangle", "x": 10, "y": 0, "width": 1, "height": 1},
        {"id": "", "type": "rectangle", "x": 15, "y": 0, "width": 1, "height": 1},
        {"id": 12, "type": "rectangle", "x": 20, "y": 0, "width": 1, "height": 1},
        {"id": "héllo wörld/ü", "type": "rectangle", "x": 25, "y": 0, "width": 1, "height": 1},
        {"id": "h_llo_w_rld__", "type": "rectangle", "x": 30, "y": 0, "width": 1, "height": 1},
        {"id": "e1", "type": "rectangle", "x": 35, "y": 0, "width": 1, "height": 1},
        {"id": "long-".to_owned() + &"x".repeat(200), "type": "rectangle", "x": 40, "y": 0, "width": 1, "height": 1},
        {"id": "日本語", "type": "rectangle", "x": 45, "y": 0, "width": 1, "height": 1},
        {"id": "arrow", "type": "arrow", "x": 0, "y": 0, "points": [[0, 0], [5, 5]],
         "startBinding": {"elementId": "héllo wörld/ü", "mode": "orbit", "fixedPoint": [0.5, 0.5]},
         "endBinding": {"elementId": "dup", "mode": "orbit", "fixedPoint": [0.5, 0.5]}},
    ]);
    let i = import_elements(els);
    let d = &i.document;
    let mut ids: Vec<&str> = d.elements.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids.len(), 11);
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 11, "all unique");
    assert!(ids.iter().all(|s| s.len() <= 64));
    // The first "dup" keeps its name, the second is renamed; the arrow binds to the first.
    let a = linear(get(d, "arrow"));
    assert_eq!(a.end_binding.as_ref().unwrap().target.as_str(), "dup");
    assert_eq!(get(d, "dup").transform.x, 0.0);
    // The arrow's start follows the element whose id was cleaned.
    let start = a.start_binding.as_ref().unwrap().target.clone();
    assert_eq!(get(d, start.as_str()).transform.x, 25.0);
    // "e1" from the file is not reused by the generator (which makes e1, e2, ...).
    assert_eq!(get(d, "e1").transform.x, 35.0);
}

#[test]
fn base64_junk_in_files_is_a_warning_not_a_crash() {
    let big = "A".repeat(100_000);
    let els = json!([
        {"id": "i1", "type": "image", "x": 0, "y": 0, "width": 4, "height": 4, "fileId": "j1"},
        {"id": "i2", "type": "image", "x": 0, "y": 0, "width": 4, "height": 4, "fileId": "j2"},
        {"id": "i3", "type": "image", "x": 0, "y": 0, "width": 4, "height": 4, "fileId": "j3"},
        {"id": "i4", "type": "image", "x": 0, "y": 0, "width": 4, "height": 4, "fileId": 5},
        {"id": "i5", "type": "image", "x": 0, "y": 0, "width": 4, "height": 4, "fileId": "j4"},
    ]);
    let text = json!({"type": "excalidraw", "elements": els, "files": {
        "j1": {"mimeType": "image/png", "dataURL": "data:image/png;base64,!!!!!!"},
        "j2": {"mimeType": "image/png", "dataURL": format!("data:image/png;base64,{big}")},
        "j3": {"mimeType": "image/png", "dataURL": 7},
        "j4": 5,
    }})
    .to_string();
    let i = import_str(&text).unwrap();
    assert_no_errors(&i.document);
    // j2 is valid base64 of junk: it is kept as an image with the box as its size, as the bytes are not a picture.
    assert!(i.document.files.len() <= 1);
    assert!(i.report.warnings.len() >= 3, "{:?}", i.report.warnings);
}

#[test]
fn a_100k_point_freedraw_is_fine_and_more_than_the_limit_is_skipped() {
    let pts: Vec<Value> = (0..100_000).map(|i| json!([i as f64 * 0.5, (i % 7) as f64])).collect();
    let els = json!([{"id": "big", "type": "freedraw", "x": 3, "y": 4, "points": pts, "pressures": [], "simulatePressure": true}]);
    let i = import_str(&scene_with(els)).unwrap();
    assert_no_errors(&i.document);
    assert_eq!(i.document.elements[0].points().unwrap().len(), 100_000);
    // Export and read back.
    let again = import_str(&export_scene(&i.document).unwrap()).unwrap();
    assert_same_doc(&i.document, &again.document);

    let pts: Vec<Value> = (0..200_001).map(|i| json!([i, 0])).collect();
    let els = json!([{"id": "huge", "type": "line", "points": pts}, {"id": "ok", "type": "rectangle", "width": 1, "height": 1}]);
    let i = import_str(&scene_with(els)).unwrap();
    assert_eq!(i.document.elements.len(), 1);
    assert!(i.report.warnings.iter().any(|w| w.contains("200000")));
}

#[test]
fn deeply_nested_json_is_a_parse_error_not_a_stack_overflow() {
    for depth in [200usize, 5_000, 100_000] {
        let nested = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        let text = format!(r#"{{"type":"excalidraw","elements":[{nested}]}}"#);
        assert!(matches!(import_str(&text), Err(IoError::Parse { .. })), "depth {depth}");
        let text = format!(
            r#"{{"type":"excalidraw","elements":[{{"type":"rectangle","customData":{}1{}}}]}}"#,
            "[".repeat(depth),
            "]".repeat(depth)
        );
        assert!(import_str(&text).is_err());
        let text = format!(r#"{{"type":"excalidrawlib","libraryItems":[{nested}]}}"#);
        assert!(import_library(&text, &mut SeqIds::new()).is_err());
        assert!(import_clipboard(&nested, &mut SeqIds::new()).is_err());
    }
    // Nesting within the limit is data like any other.
    let ok = format!(
        r#"{{"type":"excalidraw","elements":[{{"type":"rectangle","customData":{{"d":{}1{}}}}}]}}"#,
        "[".repeat(50),
        "]".repeat(50)
    );
    assert!(import_str(&ok).is_ok());
}

#[test]
fn drafftink_notes_in_hostile_shapes_are_ignored_safely() {
    let els = json!([
        {"id": "a", "type": "rectangle", "width": 1, "height": 1, "customData": {"drafftink": {"fill_pattern": "sparkles"}}},
        {"id": "b", "type": "rectangle", "width": 1, "height": 1, "customData": {"drafftink": "text"}},
        {"id": "c", "type": "text", "text": "ab", "customData": {"drafftink": {"runs": [{"start": 5, "end": 9, "color": null}, {"start": 1, "end": 0}]}}},
        {"id": "d", "type": "ellipse", "width": 1, "height": 1, "customData": {"drafftink": {"corner_radius": 1e300, "name": "x\u{0}y"}}},
        {"id": "e", "type": "ellipse", "width": 1, "height": 1, "customData": [1, 2]},
    ]);
    let i = import_elements(els);
    assert!(i.document.elements.len() == 5);
    // A note that is not an object is the user's own data.
    assert!(get(&i.document, "b").custom.as_ref().is_some_and(|c| c.contains_key("drafftink")));
    assert!(get(&i.document, "c").text_data().unwrap().runs.is_empty());
    assert!(get(&i.document, "d").style.corner_radius <= 1.0e6);
}

mod random {
    use super::*;
    use proptest::prelude::*;

    fn json_value() -> impl Strategy<Value = Value> {
        let leaf = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(|n| json!(n)),
            any::<f64>().prop_map(|f| json!(f)),
            prop::sample::select(vec![1e300, -1e300, 0.0, -0.0, 1e-300, 4.5e15, 100.0]).prop_map(|f| json!(f)),
            "[ -~é#日]{0,12}".prop_map(Value::String),
            prop::sample::select(vec!["#aé", "#", "#12", "NaN", "Infinity", "transparent", "rgb(1,2,3)"])
                .prop_map(|s| json!(s)),
        ];
        leaf.prop_recursive(4, 64, 6, |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
                prop::collection::vec(("[a-zA-Z]{1,10}", inner), 0..6)
                    .prop_map(|kv| Value::Object(kv.into_iter().collect())),
            ]
        })
    }

    fn max_abs(v: &Value) -> f64 {
        match v {
            Value::Number(n) => n.as_f64().map_or(0.0, f64::abs),
            Value::Array(a) => a.iter().map(max_abs).fold(0.0, f64::max),
            Value::Object(o) => o.values().map(max_abs).fold(0.0, f64::max),
            _ => 0.0,
        }
    }

    fn small_id() -> impl Strategy<Value = Value> {
        prop::sample::select(vec!["a", "b", "c", "d", "e"]).prop_map(|s| json!(s))
    }

    fn number() -> impl Strategy<Value = Value> {
        prop_oneof![(-500i32..500).prop_map(|n| json!(n)), (-1e3f64..1e3).prop_map(|f| json!(f)), Just(json!(1e12))]
    }

    fn binding() -> impl Strategy<Value = Value> {
        (small_id(), prop::sample::select(vec!["inside", "orbit", "skip", "x"]), prop::option::of((number(), number())))
            .prop_map(|(id, mode, fp)| {
                let mut v = json!({"elementId": id, "mode": mode});
                if let Some((a, b)) = fp {
                    v["fixedPoint"] = json!([a, b]);
                }
                v
            })
    }

    /// Values of the right shape for a key, so that the interesting branches (references, points, groups) are hit.
    fn typed(key: &'static str) -> BoxedStrategy<Value> {
        match key {
            "id" | "frameId" | "containerId" => prop_oneof![3 => small_id(), 1 => json_value()].boxed(),
            "groupIds" => {
                prop::collection::vec(prop::sample::select(vec!["g1", "g2", "g3"]).prop_map(|s| json!(s)), 0..4)
                    .prop_map(Value::Array)
                    .boxed()
            }
            "startBinding" | "endBinding" => prop_oneof![3 => binding(), 1 => json_value()].boxed(),
            "points" | "fixedSegments" => {
                prop::collection::vec((number(), number()).prop_map(|(a, b)| json!([a, b])), 0..8)
                    .prop_map(Value::Array)
                    .boxed()
            }
            "x" | "y" | "width" | "height" | "angle" => prop_oneof![3 => number(), 1 => json_value()].boxed(),
            "roundness" => prop_oneof![Just(Value::Null), (1i32..5).prop_map(|t| json!({"type": t}))].boxed(),
            "isDeleted" | "elbowed" | "polygon" => {
                prop_oneof![any::<bool>().prop_map(|b| json!(b)), json_value()].boxed()
            }
            _ => json_value().boxed(),
        }
    }

    /// An element with known keys and random values.
    fn element() -> impl Strategy<Value = Value> {
        let keys: Vec<&'static str> = vec![
            "id",
            "x",
            "y",
            "width",
            "height",
            "angle",
            "points",
            "groupIds",
            "frameId",
            "containerId",
            "startBinding",
            "endBinding",
            "roundness",
            "opacity",
            "roughness",
            "strokeColor",
            "backgroundColor",
            "fontFamily",
            "fontSize",
            "text",
            "originalText",
            "fixedSegments",
            "elbowed",
            "polygon",
            "fileId",
            "scale",
            "crop",
            "index",
            "customData",
            "link",
            "isDeleted",
            "pressures",
            "simulatePressure",
            "lineHeight",
            "labelPosition",
            "startArrowhead",
            "endArrowhead",
            "name",
        ];
        let types = [
            "rectangle",
            "diamond",
            "ellipse",
            "line",
            "arrow",
            "freedraw",
            "text",
            "image",
            "frame",
            "magicframe",
            "weird",
        ];
        let entry = prop::sample::select(keys).prop_flat_map(|k| typed(k).prop_map(move |v| (k, v)));
        (prop::sample::select(types.to_vec()), prop::collection::vec(entry, 0..12)).prop_map(|(ty, kv)| {
            let mut m: serde_json::Map<String, Value> = kv.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
            m.insert("type".into(), json!(ty));
            Value::Object(m)
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn garbage_json_never_panics(v in json_value()) {
            let text = v.to_string();
            let mut ids = SeqIds::new();
            let _ = import_scene(&text, &mut ids);
            let _ = import_clipboard(&text, &mut ids);
            let _ = import_library(&text, &mut ids);
        }

        #[test]
        fn random_elements_import_valid_documents_that_export_and_reimport(els in prop::collection::vec(element(), 0..14)) {
            let text = scene_with(Value::Array(els.clone()));
            let imported = import_scene(&text, &mut SeqIds::new()).unwrap();
            let errors: Vec<String> = imported.document.validate().errors().map(|i| i.to_string()).collect();
            prop_assert!(errors.is_empty(), "{errors:?}");
            let (out, _) = export_scene_with_report(&imported.document).unwrap();
            let again = import_scene(&out, &mut SeqIds::new()).unwrap();
            let errors: Vec<String> = again.document.validate().errors().map(|i| i.to_string()).collect();
            prop_assert!(errors.is_empty(), "{errors:?}");
            prop_assert_eq!(again.document.elements.len(), imported.document.elements.len());
            // Whatever came out of the importer is something the exporter writes losslessly.
            let (x, y) = (serde_json::to_value(&imported.document).unwrap(), serde_json::to_value(&again.document).unwrap());
            // (Around 1e9 an f64 has no digits left below 1e-7, so squeezed hostile boxes are compared for validity only.)
            if max_abs(&x) <= 1.0e6 {
                prop_assert!(approx_eq(&x, &y, "doc").is_ok(), "{:?}", approx_eq(&x, &y, "doc"));
            }
            // The library reader sees the same elements.
            let lib = json!({"type": "excalidrawlib", "version": 2, "libraryItems": [els]}).to_string();
            let (lib, _) = import_library(&lib, &mut SeqIds::new()).unwrap();
            let grid = library_to_grid_document(&lib, &mut SeqIds::new());
            let errors: Vec<String> = grid.validate().errors().map(|i| i.to_string()).collect();
            prop_assert!(errors.is_empty(), "{errors:?}");
            export_library(&lib).unwrap();
        }

        #[test]
        fn severity_of_findings_on_random_imports_is_never_error(els in prop::collection::vec(element(), 0..8)) {
            let imported = import_scene(&scene_with(Value::Array(els)), &mut SeqIds::new()).unwrap();
            prop_assert!(imported.document.validate().issues.iter().all(|i| i.severity != Severity::Error));
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Links (element.set_link): a link is plain text on the element and travels through the file untouched.

#[test]
fn web_and_element_links_survive_an_export_and_an_import() {
    let mut doc = Document::new();
    let a = base_element(&mut doc, ElementKind::Rectangle, 0.0, 0.0, 100.0, 60.0);
    let b = base_element(&mut doc, ElementKind::Ellipse, 200.0, 0.0, 50.0, 80.0);
    let c = base_element(&mut doc, ElementKind::Diamond, 400.0, 0.0, 80.0, 80.0);
    doc.elements[a].link = Some("https://example.com/a?b=c#d".into());
    doc.elements[b].link = Some(format!("#{}", doc.elements[a].id.as_str()));
    doc.elements[c].link = Some("mailto:a@b.example".into());
    let text = export_scene(&doc).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    let links: Vec<&Value> = v["elements"].as_array().unwrap().iter().map(|e| &e["link"]).collect();
    assert_eq!(links, [&json!("https://example.com/a?b=c#d"), &json!("#n0"), &json!("mailto:a@b.example")]);
    let back = import_str(&text).unwrap();
    assert_no_errors(&back.document);
    let got: Vec<Option<&str>> = back.document.elements.iter().map(|e| e.link.as_deref()).collect();
    assert_eq!(got, [Some("https://example.com/a?b=c#d"), Some("#n0"), Some("mailto:a@b.example")]);
}

#[test]
fn a_link_with_control_characters_is_dropped_with_a_warning_and_an_unsafe_scheme_is_kept_as_text() {
    let els = json!([
        {"id": "ctl", "type": "rectangle", "x": 0, "y": 0, "width": 10, "height": 10, "link": "https://a.example/\u{7}x"},
        {"id": "js", "type": "rectangle", "x": 0, "y": 0, "width": 10, "height": 10, "link": "javascript:alert(1)"},
    ]);
    let i = import_elements(els);
    assert_eq!(get(&i.document, "ctl").link, None);
    assert!(!i.report.is_clean(), "the dropped link is reported");
    // The text is not rewritten on import; `element.open_link` refuses it when someone tries to open it.
    assert_eq!(get(&i.document, "js").link.as_deref(), Some("javascript:alert(1)"));
}
