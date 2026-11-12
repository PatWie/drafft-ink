#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::field_reassign_with_default)]
//! Migration of old drafft-ink files: documents, gzip, Loro snapshots, clipboard, and hostile input.

use std::path::{Path, PathBuf};

use drafftink_io::legacy::{
    gunzip, gunzip_limited, is_legacy_json, migrate_clipboard, migrate_gzip, migrate_json, migrate_snapshot,
};
use drafftink_io::measure::{EstimateMeasure, MathBox, TextMeasure};
use drafftink_io::native::{self, Format};
use drafftink_scene::SceneRead as _;
use drafftink_scene::{
    Color, Document, Element, ElementKind, FontFamily, FontWeight, Head, Kind, MemoryStore, PathType, PressureMode,
    SeqIds, Sloppiness,
};
use loro::{ExportMode, LoroDoc, LoroList, LoroMap};
use serde_json::{Value, json};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

fn migrate(v: &Value) -> drafftink_io::Imported {
    migrate_json(&v.to_string(), &mut SeqIds::new(), &EstimateMeasure).unwrap()
}

fn style() -> Value {
    json!({"stroke_color":{"r":1,"g":2,"b":3,"a":255},"stroke_width":2.0,"fill_color":null,"fill_pattern":"Solid","sloppiness":"Artist","seed":7,"opacity":1.0})
}

fn rect(id: &str, x: f64, y: f64, w: f64, h: f64) -> Value {
    json!({"Rectangle":{"id":id,"position":{"x":x,"y":y},"width":w,"height":h,"corner_radius":0.0,"rotation":0.0,"style":style()}})
}

fn group(id: &str, children: Vec<Value>, rotation: f64) -> Value {
    json!({"Group":{"id":id,"children":children,"rotation":rotation,"style":style()}})
}

/// An old document from shapes keyed by their ids, in z-order.
fn doc_of(shapes: Vec<(&str, Value)>) -> Value {
    let order: Vec<&str> = shapes.iter().map(|(k, _)| *k).collect();
    let map: serde_json::Map<String, Value> = shapes.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect();
    json!({"id":"d","name":"t","shapes":map,"z_order":order})
}

fn assert_valid(doc: &Document) {
    let r = doc.validate();
    assert!(!r.has_errors(), "{:?}", r.errors().take(3).collect::<Vec<_>>());
    assert_eq!(&Document::from_json(&doc.to_json().unwrap()).unwrap(), doc, "native round trip");
    assert_eq!(MemoryStore::from_document(doc).unwrap().len(), doc.elements.iter().filter(|e| !e.deleted).count());
}

#[test]
fn every_legacy_document_loads_validates_and_round_trips() {
    let dir = fixtures().join("legacy");
    let mut n = 0;
    for entry in std::fs::read_dir(&dir).unwrap().filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.contains(".facts.") || entry.path().is_dir() {
            continue;
        }
        let bytes = std::fs::read(entry.path()).unwrap();
        let imported = if name.ends_with(".json.gz") {
            assert_eq!(native::sniff(&bytes), Format::Gzip);
            migrate_gzip(&bytes, &mut SeqIds::new(), &EstimateMeasure).unwrap()
        } else if name.ends_with(".json") {
            let text = String::from_utf8(bytes).unwrap();
            assert_eq!(native::sniff(text.as_bytes()), Format::LegacyJson, "{name}");
            assert!(is_legacy_json(&text));
            migrate_json(&text, &mut SeqIds::new(), &EstimateMeasure).unwrap()
        } else {
            continue;
        };
        assert_valid(&imported.document);
        n += 1;
    }
    assert_eq!(n, 15, "fixtures loaded");
}

#[test]
fn welcome_document_has_the_documented_content() {
    // docs/formats.md 1.7: 49 top-level shapes, 54 with the 5 children of 2 groups.
    let text = std::fs::read_to_string(fixtures().join("legacy/welcome.json")).unwrap();
    let imp = migrate_json(&text, &mut SeqIds::new(), &EstimateMeasure).unwrap();
    let d = &imp.document;
    assert_eq!(d.elements.len(), 52);
    let count = |k: Kind| d.elements.iter().filter(|e| e.tag() == k).count();
    assert_eq!((count(Kind::Text), count(Kind::Arrow), count(Kind::Line), count(Kind::Freehand)), (29, 11, 4, 3));
    assert_eq!((count(Kind::Rectangle), count(Kind::Ellipse), count(Kind::Math)), (3, 1, 1));
    assert!(imp.report.is_clean(), "{:?}", imp.report);
}

#[test]
fn large_gzipped_fixtures_migrate_with_the_documented_counts() {
    for (file, n) in [("large_1000.json.gz", 1000usize), ("large_10000.json.gz", 10_000)] {
        let bytes = std::fs::read(fixtures().join("legacy").join(file)).unwrap();
        let imp = migrate_gzip(&bytes, &mut SeqIds::new(), &EstimateMeasure).unwrap();
        let top: Value = serde_json::from_slice(&gunzip(&bytes).unwrap()).unwrap();
        assert_eq!(top["z_order"].as_array().unwrap().len(), n);
        assert!(imp.document.elements.len() >= n, "groups add members: {}", imp.document.elements.len());
        assert!(imp.document.elements.windows(2).all(|w| w[0].index < w[1].index));
    }
}

#[test]
fn gzip_inflation_is_bounded() {
    use std::io::Write;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    enc.write_all(&vec![b' '; 5_000_000]).unwrap();
    let gz = enc.finish().unwrap();
    assert!(gz.len() < 20_000);
    assert!(gunzip_limited(&gz, 1_000_000).is_err());
    assert_eq!(gunzip_limited(&gz, 5_000_000).unwrap().len(), 5_000_000);
    assert!(migrate_gzip(b"not gzip at all", &mut SeqIds::new(), &EstimateMeasure).is_err());
}

#[test]
fn nested_groups_become_group_id_stacks_innermost_first() {
    let inner = group("g-in", vec![rect("a", 0.0, 0.0, 10.0, 10.0), rect("b", 20.0, 0.0, 10.0, 10.0)], 0.0);
    let outer = group("g-out", vec![inner, rect("c", 40.0, 0.0, 10.0, 10.0)], 0.0);
    let imp = migrate(&doc_of(vec![("top", outer), ("loose", rect("d", 60.0, 0.0, 10.0, 10.0))]));
    let ids: Vec<(&str, Vec<&str>)> = imp
        .document
        .elements
        .iter()
        .map(|e| (e.id.as_str(), e.group_ids.iter().map(|g| g.as_str()).collect()))
        .collect();
    assert_eq!(
        ids,
        vec![("a", vec!["g-in", "g-out"]), ("b", vec!["g-in", "g-out"]), ("c", vec!["g-out"]), ("d", vec![])]
    );
    assert_valid(&imp.document);
}

#[test]
fn groups_that_add_nothing_get_no_id() {
    let one = group("g1", vec![rect("a", 0.0, 0.0, 5.0, 5.0)], 0.0);
    let empty = group("g0", vec![], 0.0);
    let wrapper =
        group("gw", vec![group("gi", vec![rect("b", 0.0, 0.0, 5.0, 5.0), rect("c", 9.0, 0.0, 5.0, 5.0)], 0.0)], 0.0);
    let mixed =
        group("gm", vec![group("gs", vec![rect("d", 0.0, 0.0, 5.0, 5.0)], 0.0), rect("e", 9.0, 0.0, 5.0, 5.0)], 0.0);
    let imp = migrate(&doc_of(vec![("1", one), ("2", empty), ("3", wrapper), ("4", mixed)]));
    let groups = |id: &str| -> Vec<String> {
        imp.document
            .elements
            .iter()
            .find(|e| e.id.as_str() == id)
            .unwrap()
            .group_ids
            .iter()
            .map(|g| g.to_string())
            .collect()
    };
    assert!(groups("a").is_empty(), "a one-child group is no group");
    assert_eq!(groups("b"), vec!["gi"], "a wrapper around one group adds no second id");
    assert_eq!(groups("d"), vec!["gm"], "the inner one-child group is dropped, the outer pair stays");
    assert_eq!(groups("e"), vec!["gm"]);
    assert_eq!(imp.document.elements.len(), 5, "the empty group leaves nothing behind");
    assert!(imp.document.validate().warnings().next().is_none(), "no single-member group warnings");
}

#[test]
fn group_rotation_is_baked_into_children() {
    // Two 10x10 squares side by side: union (0,0)-(30,10), pivot (15,5). A quarter turn clockwise (y down) moves
    // the point (x, y) to (15 - (y - 5), 5 + (x - 15)).
    let quarter = std::f64::consts::FRAC_PI_2;
    let g = group("g", vec![rect("a", 0.0, 0.0, 10.0, 10.0), rect("b", 20.0, 0.0, 10.0, 10.0)], quarter);
    let line = json!({"Line":{"id":"l","start":{"x":0.0,"y":0.0},"end":{"x":30.0,"y":0.0},"style":style()}});
    let g2 = group("g2", vec![line, rect("c", 0.0, 0.0, 30.0, 10.0)], quarter);
    let imp = migrate(&doc_of(vec![("1", g), ("2", g2)]));
    let by = |id: &str| -> &Element { imp.document.elements.iter().find(|e| e.id.as_str() == id).unwrap() };
    let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
    // Square a: centre (5,5) -> (15, -5); box 10x10; angle a quarter turn.
    let (a, b) = (by("a").transform, by("b").transform);
    assert!(close(a.center().x, 15.0) && close(a.center().y, -5.0), "{a:?}");
    assert!(close(b.center().x, 15.0) && close(b.center().y, 15.0), "{b:?}");
    assert!(close(a.angle, quarter) && close(b.angle, quarter));
    assert!(close(a.w, 10.0) && close(a.h, 10.0), "the box keeps its size, the angle carries the turn");
    // The line has no angle in the old model, so its points moved: (0,0)-(30,0) about (15,5) -> (20,-10)-(20,20).
    let l = by("l");
    assert_eq!(l.transform.angle, 0.0);
    let p = l.points().unwrap();
    let w: Vec<_> = p.iter().map(|q| l.transform.local_to_world(*q)).collect();
    assert!(close(w[0].x, 20.0) && close(w[0].y, -10.0), "{w:?}");
    assert!(close(w[1].x, 20.0) && close(w[1].y, 20.0), "{w:?}");
    assert_valid(&imp.document);
}

#[test]
fn nested_group_rotations_compose_inner_first() {
    let q = std::f64::consts::FRAC_PI_2;
    // Inner pair rotated a quarter about its own centre (10, 5): the pair stands upright, centre unchanged.
    let inner = group("gi", vec![rect("a", 0.0, 0.0, 10.0, 10.0), rect("b", 10.0, 0.0, 10.0, 10.0)], q);
    // Outer adds a third square far right and is not rotated: nothing else moves.
    let outer = group("go", vec![inner, rect("c", 100.0, 0.0, 10.0, 10.0)], 0.0);
    let imp = migrate(&doc_of(vec![("1", outer)]));
    let by = |id: &str| imp.document.elements.iter().find(|e| e.id.as_str() == id).unwrap().transform;
    let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
    assert!(close(by("a").center().x, 10.0) && close(by("a").center().y, 0.0), "{:?}", by("a"));
    assert!(close(by("b").center().x, 10.0) && close(by("b").center().y, 10.0));
    assert_eq!(by("c").angle, 0.0);
}

#[test]
fn a_tiny_group_rotation_is_not_baked() {
    // The old renderer ignored rotations of 0.001 rad or less.
    let g = group("g", vec![rect("a", 0.0, 0.0, 10.0, 10.0), rect("b", 20.0, 0.0, 10.0, 10.0)], 0.0005);
    let imp = migrate(&doc_of(vec![("1", g)]));
    assert!(imp.document.elements.iter().all(|e| e.transform.angle == 0.0 && e.transform.x.fract() == 0.0));
}

#[test]
fn old_ids_are_kept_and_bad_or_duplicate_ids_are_replaced() {
    let uuid = "44093825-1401-4f08-9fd9-84d6a593a5d2";
    let doc = doc_of(vec![
        ("k1", rect(uuid, 0.0, 0.0, 1.0, 1.0)),
        ("k2", rect(uuid, 5.0, 0.0, 1.0, 1.0)),
        ("k3", rect("has space!", 9.0, 0.0, 1.0, 1.0)),
    ]);
    let imp = migrate(&doc);
    let ids: Vec<&str> = imp.document.elements.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids[0], uuid);
    assert_ne!(ids[1], uuid);
    assert_ne!(ids[2], "has space!");
    assert_eq!(ids.iter().collect::<std::collections::HashSet<_>>().len(), 3);
    assert_valid(&imp.document);
}

#[test]
fn shapes_outside_z_order_are_dropped_with_a_warning() {
    let mut doc = doc_of(vec![("a", rect("a", 0.0, 0.0, 1.0, 1.0))]);
    doc["shapes"]["ghost"] = rect("ghost", 3.0, 3.0, 1.0, 1.0);
    doc["z_order"].as_array_mut().unwrap().push(json!("missing"));
    let imp = migrate(&doc);
    assert_eq!(imp.document.elements.len(), 1);
    assert_eq!(imp.report.warnings.len(), 2, "{:?}", imp.report);
}

#[test]
fn one_bad_shape_no_longer_fails_the_file() {
    let mut doc =
        doc_of(vec![("a", rect("a", 0.0, 0.0, 1.0, 1.0)), ("b", json!({"Diamond": {"id": "x"}})), ("c", json!(5))]);
    doc["shapes"]["d"] = rect("d", 0.0, 0.0, 1.0, 1.0);
    doc["z_order"].as_array_mut().unwrap().push(json!("d"));
    let imp = migrate(&doc);
    assert_eq!(imp.document.elements.len(), 2);
    assert!(imp.report.warnings.iter().any(|w| w.contains("Diamond")));
}

#[test]
fn not_an_old_document_is_a_wrong_format_error() {
    for bad in ["[]", "{}", "{\"shapes\": 3, \"z_order\": []}", "null", "\"x\"", "{\"shapes\":{}}"] {
        let e = migrate_json(bad, &mut SeqIds::new(), &EstimateMeasure);
        assert!(e.is_err(), "{bad}");
    }
    assert!(migrate_json("not json {", &mut SeqIds::new(), &EstimateMeasure).is_err());
}

#[test]
fn old_defaults_for_omitted_fields() {
    // Mirrors the older file revisions in welcome.json: no stroke_style/closed/pressures, no fill_pattern/seed/opacity.
    let bare_style = json!({"stroke_color":{"r":0,"g":0,"b":0,"a":255},"stroke_width":3.0,"sloppiness":"Cartoonist"});
    let line = json!({"Line":{"id":"l","start":{"x":0.0,"y":0.0},"end":{"x":4.0,"y":2.0},"style":bare_style}});
    let free = json!({"Freehand":{"id":"f","points":[{"x":0.0,"y":0.0},{"x":1.0,"y":2.0}],"style":bare_style}});
    let imp = migrate_json(&doc_of(vec![("l", line), ("f", free)]).to_string(), &mut SeqIds::new(), &EstimateMeasure)
        .unwrap();
    let d = &imp.document;
    let l = d.elements[0].linear_data().unwrap();
    assert_eq!((l.path, l.closed, l.points.len()), (PathType::Sharp, false, 2));
    assert_eq!(d.elements[0].style.stroke_style, drafftink_scene::StrokeStyle::Solid);
    assert_eq!(d.elements[0].style.sloppiness, Sloppiness::Cartoonist);
    assert_eq!(d.elements[0].style.opacity, 1.0);
    // Missing seeds come from the IdSource: SeqIds hands out distinct ones.
    assert_ne!(d.elements[0].style.seed, d.elements[1].style.seed);
    let ElementKind::Freehand(f) = &d.elements[1].kind else { panic!() };
    assert_eq!(f.pressure_mode, PressureMode::Uniform);
}

#[test]
fn lines_arrows_and_strokes() {
    let st = style();
    let arrow = json!({"Arrow":{"id":"a","start":{"x":0.0,"y":0.0},"end":{"x":10.0,"y":10.0},"intermediate_points":[{"x":5.0,"y":0.0}],"path_style":"Angular","stroke_style":"Dashed","head_size":22.0,"style":st}});
    let closed2 =
        json!({"Line":{"id":"c2","start":{"x":0.0,"y":0.0},"end":{"x":10.0,"y":10.0},"closed":true,"style":st}});
    let closed3 = json!({"Line":{"id":"c3","start":{"x":0.0,"y":0.0},"end":{"x":10.0,"y":10.0},"intermediate_points":[{"x":0.0,"y":10.0}],"closed":true,"path_style":"Flowing","style":st}});
    let free = json!({"Freehand":{"id":"f","points":[{"x":0.0,"y":0.0},{"x":1.0,"y":2.0}],"pressures":[0.5,0.7],"closed":true,"style":st}});
    let bad_pressure =
        json!({"Freehand":{"id":"g","points":[{"x":0.0,"y":0.0},{"x":1.0,"y":2.0}],"pressures":[0.5],"style":st}});
    let empty = json!({"Freehand":{"id":"e","points":[],"style":st}});
    let imp = migrate(&doc_of(vec![
        ("a", arrow),
        ("c2", closed2),
        ("c3", closed3),
        ("f", free),
        ("g", bad_pressure),
        ("e", empty),
    ]));
    let d = &imp.document;
    assert_eq!(d.elements.len(), 5, "the empty stroke is skipped");
    let a = d.elements[0].linear_data().unwrap();
    assert_eq!((a.path, a.end_head, a.start_head, a.head_size), (PathType::Elbow, Head::Arrow, Head::None, Some(22.0)));
    assert_eq!(a.points.len(), 3);
    assert_eq!(d.elements[0].style.stroke_style, drafftink_scene::StrokeStyle::Dashed);
    assert!(!d.elements[1].linear_data().unwrap().closed, "a closed line needs 3 points");
    let c3 = d.elements[2].linear_data().unwrap();
    assert!(c3.closed && c3.path == PathType::Round);
    let ElementKind::Freehand(f) = &d.elements[3].kind else { panic!() };
    assert!(f.closed && f.pressure_mode == PressureMode::Device && f.pressures == vec![0.5, 0.7]);
    let ElementKind::Freehand(g) = &d.elements[4].kind else { panic!() };
    assert!(g.pressures.is_empty() && g.pressure_mode == PressureMode::Uniform);
    assert_valid(d);
}

#[test]
fn negative_sizes_are_normalised_and_ellipses_become_boxes() {
    let st = style();
    let neg = json!({"Rectangle":{"id":"r","position":{"x":100.0,"y":50.0},"width":-40.0,"height":-10.0,"corner_radius":4.0,"style":st}});
    let ell = json!({"Ellipse":{"id":"e","center":{"x":50.0,"y":50.0},"radius_x":30.0,"radius_y":20.0,"rotation":0.5,"style":st}});
    let imp = migrate(&doc_of(vec![("r", neg), ("e", ell)]));
    let (r, e) = (&imp.document.elements[0], &imp.document.elements[1]);
    assert_eq!((r.transform.x, r.transform.y, r.transform.w, r.transform.h), (60.0, 40.0, 40.0, 10.0));
    assert_eq!(r.style.corner_radius, 4.0);
    assert_eq!(
        (e.transform.x, e.transform.y, e.transform.w, e.transform.h, e.transform.angle),
        (20.0, 30.0, 60.0, 40.0, 0.5)
    );
}

#[test]
fn text_runs_follow_char_colors_of_any_length() {
    let c = |r: u8| json!({"r":r,"g":0,"b":0,"a":255});
    let mk = |content: &str, colors: Value| json!({"Text":{"id":"t","position":{"x":1.0,"y":2.0},"content":content,"font_size":24.0,"font_family":"GelPenSerif","font_weight":"Heavy","rotation":0.25,"style":style(),"char_colors":colors}});
    // "héllo" has 5 scalar values; entries beyond them are ignored, missing ones are the default colour.
    let imp = migrate(&doc_of(vec![
        ("a", mk("héllo", json!([null, c(1), c(1), null, c(2), c(3), c(3)]))),
        ("b", mk("ab", json!([]))),
    ]));
    let ElementKind::Text(t) = &imp.document.elements[0].kind else { panic!() };
    assert_eq!(t.char_colors().iter().filter(|c| c.is_some()).count(), 3);
    assert_eq!(t.runs.len(), 2, "equal neighbours merge: {:?}", t.runs);
    assert_eq!((t.font.family, t.font.weight, t.font.size), (FontFamily::GelPenSerif, FontWeight::Heavy, 24.0));
    assert_eq!(imp.document.elements[0].transform.angle, 0.25);
    let ElementKind::Text(t2) = &imp.document.elements[1].kind else { panic!() };
    assert!(t2.runs.is_empty());
    assert_valid(&imp.document);
}

struct Fake;
impl TextMeasure for Fake {
    fn text_size(&self, text: &str, _: FontFamily, _: FontWeight, size: f64, lh: f64) -> (f64, f64) {
        (text.chars().count() as f64 * size * 0.5, size * lh)
    }
    fn math_size(&self, latex: &str, size: f64) -> Option<MathBox> {
        (latex != "bad").then_some(MathBox { width: 40.0, height: size * 0.8, depth: size * 0.2 })
    }
}

#[test]
fn text_and_math_boxes_come_from_the_injected_measure() {
    let t = json!({"Text":{"id":"t","position":{"x":10.0,"y":20.0},"content":"abcd","font_size":10.0,"font_family":"GelPen","font_weight":"Regular","style":style()}});
    let m = json!({"Math":{"id":"m","position":{"x":100.0,"y":200.0},"latex":"x^2","font_size":20.0,"rotation":0.1,"style":style()}});
    let bad = json!({"Math":{"id":"b","position":{"x":0.0,"y":0.0},"latex":"bad","font_size":20.0,"style":style()}});
    let imp =
        migrate_json(&doc_of(vec![("t", t), ("m", m), ("b", bad)]).to_string(), &mut SeqIds::new(), &Fake).unwrap();
    let e = &imp.document.elements;
    assert_eq!((e[0].transform.w, e[0].transform.h), (20.0, 12.5));
    // The old position is the baseline origin: the box top is `height` (16) above it, and it is 16 + 4 tall.
    assert_eq!((e[1].transform.x, e[1].transform.y, e[1].transform.w, e[1].transform.h), (100.0, 184.0, 40.0, 20.0));
    assert_eq!(e[1].transform.angle, 0.1);
    // A formula the measure cannot lay out keeps the estimate box.
    assert_eq!(e[2].transform.h, 26.0);
}

#[test]
fn images_are_deduplicated_and_typed_by_their_bytes() {
    let png = [0x89u8, b'P', b'N', b'G', 1, 2, 3];
    let b64 = |b: &[u8]| {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(b)
    };
    let img = |id: &str, data: &str, fmt: &str| json!({"Image":{"id":id,"position":{"x":0.0,"y":0.0},"width":4.0,"height":5.0,"source_width":4,"source_height":5,"format":fmt,"data_base64":data,"style":style()}});
    let jpeg = [0xFFu8, 0xD8, 0xFF, 0xE0, 9];
    let doc = doc_of(vec![
        ("1", img("i1", &b64(&png), "Jpeg")),
        ("2", img("i2", &b64(&png), "Png")),
        ("3", img("i3", &b64(&jpeg), "Png")),
        ("4", img("i4", "!!! not base64 !!!", "Png")),
        ("5", img("i5", "", "WebP")),
    ]);
    let imp = migrate(&doc);
    let d = &imp.document;
    assert_eq!(d.files.len(), 2, "identical bytes share one blob");
    let mimes: std::collections::BTreeSet<&str> = d.files.values().map(|f| f.mime.as_str()).collect();
    assert_eq!(mimes, ["image/jpeg", "image/png"].into_iter().collect(), "the format field lied, the bytes decide");
    let ElementKind::Image(i) = &d.elements[0].kind else { panic!() };
    assert_eq!((i.natural_width, i.natural_height), (4, 5));
    assert!(i.file_id.is_some());
    for k in [3, 4] {
        let ElementKind::Image(i) = &d.elements[k].kind else { panic!() };
        assert!(i.file_id.is_none(), "no data, no file");
    }
    assert!(imp.report.warnings.iter().any(|w| w.contains("base64")));
    assert_valid(d);
}

#[test]
fn a_migrated_picture_has_no_outline() {
    // The old app kept a stroke on every picture (black, 2 wide) and never drew it. The new one draws a stroke, so
    // the colour is cleared; the other shapes keep theirs.
    let png = [0x89u8, b'P', b'N', b'G', 1, 2, 3];
    let data = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(png)
    };
    let img = json!({"Image":{"id":"i","position":{"x":0.0,"y":0.0},"width":4.0,"height":5.0,"source_width":4,"source_height":5,"format":"Png","data_base64":data,"style":style()}});
    let imp = migrate(&doc_of(vec![("1", img), ("2", rect("r", 0.0, 0.0, 10.0, 10.0))]));
    let d = &imp.document;
    assert_eq!(d.elements[0].style.stroke_color, Color::TRANSPARENT);
    assert_eq!(d.elements[0].style.stroke_width, 2.0);
    assert_eq!(d.elements[1].style.stroke_color, Color::rgb(1, 2, 3));
}

#[test]
fn hostile_numbers_are_clamped() {
    let huge = json!({"Rectangle":{"id":"r","position":{"x":1e300,"y":-1e300},"width":1e300,"height":3.0,"corner_radius":1e300,"rotation":1e300,"style":{"stroke_color":{"r":999,"g":-5,"b":0.5,"a":255},"stroke_width":1e300,"sloppiness":"Artist","opacity":50.0,"seed":-3}}});
    let nan_like =
        json!({"Line":{"id":"l","start":{"x":null,"y":"a"},"end":[1,2],"intermediate_points":"no","style":style()}});
    let imp = migrate(&doc_of(vec![("r", huge), ("l", nan_like)]));
    assert_valid(&imp.document);
    let r = &imp.document.elements[0];
    assert_eq!(r.style.opacity, 1.0);
    assert!(r.style.stroke_width <= 1000.0);
}

#[test]
fn deeply_nested_groups_do_not_overflow_the_stack() {
    let mut s = rect("leaf", 0.0, 0.0, 1.0, 1.0);
    for i in 0..30 {
        s = group(&format!("g{i}"), vec![s, rect(&format!("r{i}"), 5.0, 5.0, 1.0, 1.0)], 0.0);
    }
    let imp = migrate(&doc_of(vec![("top", s)]));
    assert_valid(&imp.document);
    assert!(imp.document.elements.iter().all(|e| e.group_ids.len() <= 16));
    // And as raw text, far deeper than serde_json allows: an error, not a crash.
    let deep = format!("{}{}", "{\"Group\":{\"children\":[".repeat(5000), "]}}".repeat(5000));
    let text = format!("{{\"shapes\":{{\"a\":{deep}}},\"z_order\":[\"a\"]}}");
    assert!(migrate_json(&text, &mut SeqIds::new(), &EstimateMeasure).is_err());
}

#[test]
fn old_clipboard_array_migrates() {
    let text = json!([rect("a", 0.0, 0.0, 5.0, 5.0), rect("b", 9.0, 0.0, 5.0, 5.0)]).to_string();
    assert_eq!(native::sniff(text.as_bytes()), Format::LegacyClipboard);
    let imp = migrate_clipboard(&text, &mut SeqIds::new(), &EstimateMeasure).unwrap();
    assert_eq!(imp.document.elements.len(), 2);
    assert!(migrate_clipboard("{}", &mut SeqIds::new(), &EstimateMeasure).is_err());
}

// ---------------------------------------------------------------------------------------------------------------
// Loro snapshots
// ---------------------------------------------------------------------------------------------------------------

fn snapshot(build: impl FnOnce(&LoroMap, &LoroList)) -> Vec<u8> {
    let doc = LoroDoc::new();
    let shapes = doc.get_map("shapes");
    let order = doc.get_list("z_order");
    doc.get_text("name").insert(0, "snap").unwrap();
    build(&shapes, &order);
    doc.commit();
    doc.export(ExportMode::Snapshot).unwrap()
}

fn base_shape(shapes: &LoroMap, order: &LoroList, id: &str, kind: &str) -> LoroMap {
    let m = shapes.insert_container(id, LoroMap::new()).unwrap();
    m.insert("type", kind).unwrap();
    m.insert("id", id).unwrap();
    for (k, v) in [("stroke_r", 10i64), ("stroke_g", 20), ("stroke_b", 300), ("stroke_a", 255)] {
        m.insert(k, v).unwrap();
    }
    m.insert("stroke_width", 2.0).unwrap();
    order.push(id).unwrap();
    m
}

#[test]
fn snapshot_enum_codes_out_of_range_map_to_the_last_variant() {
    let bytes = snapshot(|shapes, order| {
        let m = base_shape(shapes, order, "r1", "rectangle");
        for (k, v) in [("x", 1.0), ("y", 2.0), ("width", 3.0), ("height", 4.0)] {
            m.insert(k, v).unwrap();
        }
        m.insert("sloppiness", 99i64).unwrap();
        m.insert("fill_pattern", -1i64).unwrap();
        m.insert("has_fill", true).unwrap();
        m.insert("fill_r", 5i64).unwrap();
        let e = base_shape(shapes, order, "e1", "ellipse");
        for (k, v) in [("x", 50.0), ("y", 60.0), ("width", 10.0), ("height", 5.0)] {
            e.insert(k, v).unwrap();
        }
        let t = base_shape(shapes, order, "t1", "text");
        t.insert("x", 0.0).unwrap();
        t.insert("y", 0.0).unwrap();
        t.insert("content", "hi").unwrap();
        t.insert("font_family", 9i64).unwrap();
        let math = base_shape(shapes, order, "m1", "math");
        math.insert("x", 0.0).unwrap();
        math.insert("y", 0.0).unwrap();
        math.insert("content", "x^2").unwrap();
    });
    assert_eq!(native::sniff(&bytes), Format::LegacySnapshot);
    let imp = migrate_snapshot(&bytes, &mut SeqIds::new(), &EstimateMeasure).unwrap();
    let d = &imp.document;
    assert_eq!(d.name, "snap");
    assert_eq!(d.elements.len(), 4);
    let r = &d.elements[0];
    assert_eq!(r.style.sloppiness, Sloppiness::Drunk);
    assert_eq!(r.style.fill_pattern, drafftink_scene::FillPattern::ZigZagLine);
    // Colours wrap like the old `as u8` cast: 300 -> 44.
    assert_eq!((r.style.stroke_color.b, r.style.fill_color.unwrap().a), (44, 255));
    // The ellipse keys carry radii: x,y is the centre.
    let e = &d.elements[1].transform;
    assert_eq!((e.x, e.y, e.w, e.h), (40.0, 55.0, 20.0, 10.0));
    let ElementKind::Text(t) = &d.elements[2].kind else { panic!() };
    assert_eq!(t.font.family, FontFamily::VanillaExtract);
    let ElementKind::Math(m) = &d.elements[3].kind else { panic!() };
    assert_eq!(m.latex, "x^2");
    assert_valid(d);
}

#[test]
fn snapshot_shapes_missing_required_keys_are_dropped() {
    let bytes = snapshot(|shapes, order| {
        let m = base_shape(shapes, order, "r1", "rectangle");
        m.insert("x", 1.0).unwrap(); // no y, width, height
        let ok = base_shape(shapes, order, "r2", "rectangle");
        for (k, v) in [("x", 1.0), ("y", 2.0), ("width", 3.0), ("height", 4.0)] {
            ok.insert(k, v).unwrap();
        }
        let unknown = base_shape(shapes, order, "u", "hexagon");
        unknown.insert("x", 1.0).unwrap();
        // Present but not in z_order: never drawn.
        let ghost = shapes.insert_container("ghost", LoroMap::new()).unwrap();
        ghost.insert("type", "rectangle").unwrap();
    });
    let imp = migrate_snapshot(&bytes, &mut SeqIds::new(), &EstimateMeasure).unwrap();
    assert_eq!(imp.document.elements.len(), 1);
    assert_eq!(imp.document.elements[0].id.as_str(), "r2");
    assert!(!imp.report.is_clean());
}

#[test]
fn snapshots_that_are_not_old_documents_are_errors() {
    let empty = LoroDoc::new().export(ExportMode::Snapshot).unwrap();
    assert!(migrate_snapshot(&empty, &mut SeqIds::new(), &EstimateMeasure).is_err());
    assert!(migrate_snapshot(b"", &mut SeqIds::new(), &EstimateMeasure).is_err());
    assert!(migrate_snapshot(b"loro\x00\x01\x02", &mut SeqIds::new(), &EstimateMeasure).is_err());
}

#[test]
fn truncated_and_flipped_snapshots_never_panic() {
    let bytes = std::fs::read(fixtures().join("legacy/crdt/all_shapes.loro")).unwrap();
    for cut in (0..bytes.len()).step_by(7) {
        let _ = migrate_snapshot(&bytes[..cut], &mut SeqIds::new(), &EstimateMeasure);
    }
    for i in (0..bytes.len()).step_by(11) {
        let mut b = bytes.clone();
        b[i] ^= 0xA5;
        let _ = migrate_snapshot(&b, &mut SeqIds::new(), &EstimateMeasure);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Facts check: it must fail when the migration is wrong
// ---------------------------------------------------------------------------------------------------------------

#[test]
fn the_facts_check_notices_tampering() {
    use drafftink_io::facts::check_legacy_facts;
    let dir = fixtures().join("legacy");
    let doc = std::fs::read(dir.join("groups_nested.json")).unwrap();
    let facts = std::fs::read_to_string(dir.join("groups_nested.facts.json")).unwrap();
    assert!(check_legacy_facts("g", &doc, facts.as_bytes(), &EstimateMeasure).failures.is_empty());

    let mutate = |f: &dyn Fn(&mut Value)| -> usize {
        let mut v: Value = serde_json::from_str(&facts).unwrap();
        f(&mut v);
        check_legacy_facts("g", &doc, v.to_string().as_bytes(), &EstimateMeasure).failures.len()
    };
    // A changed bounds value.
    assert!(mutate(&|v| v["doc"]["shapes"][1]["bounds"][2] = json!(81.0)) > 0);
    // A child moved to another group.
    assert!(mutate(&|v| v["doc"]["shapes"][3]["parent"] = json!(4)) > 0);
    // A different shape type.
    assert!(mutate(&|v| v["doc"]["shapes"][1]["type"] = json!("Ellipse")) > 0);
    // A different rotation of the rotated group.
    assert!(
        mutate(&|v| v["doc"]["shapes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|s| s["rotation"] == json!(0.6))
            .unwrap()["rotation"] = json!(0.7))
            > 0
    );
    // A wrong id (z-order).
    assert!(mutate(&|v| v["doc"]["shapes"][1]["id"] = json!("nope")) > 0);
    // A tampered document hash.
    let mut doc2 = doc.clone();
    doc2.push(b' ');
    assert!(!check_legacy_facts("g", &doc2, facts.as_bytes(), &EstimateMeasure).failures.is_empty());
}

#[test]
fn migrated_documents_survive_a_store() {
    // Everything the old fixtures hold fits the live store, not only the file form.
    let text = std::fs::read_to_string(fixtures().join("legacy/all_shapes.json")).unwrap();
    let imp = migrate_json(&text, &mut SeqIds::new(), &EstimateMeasure).unwrap();
    let store = MemoryStore::from_document(&imp.document).unwrap();
    assert_eq!(drafftink_scene::snapshot(&store).elements.len(), imp.document.elements.len());
}

// ---------------------------------------------------------------------------------------------------------------
// Property tests: any input is an error or a valid document
// ---------------------------------------------------------------------------------------------------------------

mod props {
    use super::*;
    use proptest::prelude::*;

    fn leaf() -> impl Strategy<Value = Value> {
        prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(|n| json!(n)),
            (-1e300f64..1e300).prop_map(|n| json!(n)),
            prop::sample::select(vec![0.0, -0.0, 1e308, -1e308, 5e-324, 1.5, 255.0]).prop_map(|n: f64| json!(n)),
            "\\PC{0,12}".prop_map(Value::String),
            prop::sample::select(vec![
                "Solid", "Hachure", "Wild", "GelPen", "Light", "Png", "Direct", "Angular", "Artist", "Drunk"
            ])
            .prop_map(|s| json!(s)),
        ]
    }

    fn value() -> impl Strategy<Value = Value> {
        leaf().prop_recursive(4, 48, 6, |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
                prop::collection::vec(("[a-z_]{1,14}", inner), 0..6)
                    .prop_map(|kv| Value::Object(kv.into_iter().collect())),
            ]
        })
    }

    /// A shape with a known tag whose fields are arbitrary values under the real field names.
    fn shape() -> impl Strategy<Value = Value> {
        let tag = prop::sample::select(vec![
            "Rectangle",
            "Ellipse",
            "Line",
            "Arrow",
            "Freehand",
            "Text",
            "Math",
            "Image",
            "Group",
            "Other",
        ]);
        let fields = prop::collection::vec(
            (
                prop::sample::select(vec![
                    "id",
                    "position",
                    "width",
                    "height",
                    "corner_radius",
                    "rotation",
                    "style",
                    "center",
                    "radius_x",
                    "radius_y",
                    "start",
                    "end",
                    "intermediate_points",
                    "path_style",
                    "stroke_style",
                    "closed",
                    "head_size",
                    "points",
                    "pressures",
                    "content",
                    "font_size",
                    "font_family",
                    "font_weight",
                    "char_colors",
                    "latex",
                    "source_width",
                    "source_height",
                    "format",
                    "data_base64",
                    "children",
                    "stroke_color",
                    "fill_color",
                    "sloppiness",
                    "seed",
                    "opacity",
                ]),
                value(),
            ),
            0..8,
        );
        (tag, fields).prop_map(|(t, f)| {
            let mut o: serde_json::Map<String, Value> = f.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
            if let Some(st) = o.get("style").cloned() {
                o.insert("style".into(), st);
            }
            json!({ t: Value::Object(o) })
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(300))]

        #[test]
        fn arbitrary_shapes_never_panic_and_always_validate(shapes in prop::collection::vec(shape(), 0..8)) {
            let order: Vec<String> = (0..shapes.len()).map(|i| format!("s{i}")).collect();
            let map: serde_json::Map<String, Value> = order.iter().cloned().zip(shapes).collect();
            let doc = json!({"id": "x", "name": "n", "shapes": map, "z_order": order});
            if let Ok(imp) = migrate_json(&doc.to_string(), &mut SeqIds::new(), &EstimateMeasure) {
                let report = imp.document.validate();
                prop_assert!(!report.has_errors(), "{:?}", report.errors().take(2).collect::<Vec<_>>());
                prop_assert!(imp.document.to_json().is_ok());
            }
        }

        #[test]
        fn arbitrary_json_never_panics(v in value()) {
            let _ = migrate_json(&v.to_string(), &mut SeqIds::new(), &EstimateMeasure);
            let _ = migrate_clipboard(&v.to_string(), &mut SeqIds::new(), &EstimateMeasure);
            let _ = native::sniff(v.to_string().as_bytes());
        }

        #[test]
        fn arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..400)) {
            let _ = migrate_snapshot(&bytes, &mut SeqIds::new(), &EstimateMeasure);
            let _ = migrate_gzip(&bytes, &mut SeqIds::new(), &EstimateMeasure);
            let _ = migrate_json(&String::from_utf8_lossy(&bytes), &mut SeqIds::new(), &EstimateMeasure);
        }
    }
}
