#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The native JSON format: round trips, the version gate, refusals, and a checked-in golden file.

mod common;

use common::*;
use drafftink_scene::*;
use kurbo::Point;

fn sample() -> Document {
    let z = zs(8);
    let (fid, entry) = FileEntry::new("image/png", vec![137, 80, 78, 71, 13, 10, 26, 10]);
    let mut doc = Document::new();
    doc.name = "Sample".into();
    doc.background = Color::parse("#fafafa").unwrap();
    doc.files.insert(fid.clone(), entry);

    let mut frame = frame("frame", &z[7]);
    frame.transform = Transform::new(-10.0, -10.0, 400.0, 300.0);
    if let ElementKind::Frame(d) = &mut frame.kind {
        d.name = Some("Section".into());
    }

    let mut a = rect("box_a", &z[0]);
    a.style.fill_color = Some(Color::rgb(0xe0, 0xe7, 0xff));
    a.style.fill_pattern = FillPattern::Hachure;
    a.style.corner_radius = 8.0;
    a.style.seed = 1234;
    a.group_ids = smallvec::smallvec![gid("pair")];
    a.frame_id = Some(id("frame"));
    a.name = Some("start".into());
    let mut b = rect("box_b", &z[1]).with_bounds(200.0, 20.0, 100.0, 50.0);
    b.group_ids = smallvec::smallvec![gid("pair")];
    b.frame_id = Some(id("frame"));
    b.transform.angle = 0.25;

    let mut ar = arrow("link", &z[2], &[(110.0, 45.0), (200.0, 45.0)]);
    if let ElementKind::Arrow(d) = &mut ar.kind {
        d.end_head = Head::Arrow;
        d.head_size = Some(15.0);
        d.start_binding = Some(Binding { target: id("box_a"), mode: BindMode::Orbit, fixed_point: [1.0, 0.5] });
        d.end_binding = Some(Binding { target: id("box_b"), mode: BindMode::Orbit, fixed_point: [0.0, 0.5] });
    }
    ar.style.stroke_style = StrokeStyle::Dashed;

    let mut label = text("link_label", &z[3], "yes");
    if let ElementKind::Text(d) = &mut label.kind {
        d.container_id = Some(id("link"));
        d.label_position = Some(0.5);
        d.font.align = TextAlign::Center;
        d.runs = vec![TextRun { start: 0, end: 1, color: Some(Color::rgb(220, 38, 38)) }];
    }

    let mut stroke = Element::new(
        id("scribble"),
        ElementKind::Freehand(FreehandData::new(vec![
            Point::new(0.0, 0.0),
            Point::new(5.0, 9.0),
            Point::new(12.0, 3.0),
        ])),
        z[4].clone(),
    );
    if let ElementKind::Freehand(d) = &mut stroke.kind {
        d.pressures = vec![0.3, 0.8, 0.5];
        d.pressure_mode = PressureMode::Device;
    }
    stroke.refit_points().unwrap();
    stroke.transform.x += 10.0;
    stroke.transform.y += 100.0;

    let formula = Element::new(id("formula"), ElementKind::Math(MathData::new("\\frac{a}{b}")), z[5].clone())
        .with_bounds(0.0, 150.0, 40.0, 30.0);
    let img = Element::new(
        id("picture"),
        ElementKind::Image(ImageData { file_id: Some(fid), natural_width: 32, natural_height: 16, crop: None }),
        z[6].clone(),
    )
    .with_bounds(220.0, 120.0, 64.0, 32.0);

    doc.elements = vec![a, b, ar, label, stroke, formula, img, frame];
    doc
}

#[test]
fn sample_is_valid() {
    let r = sample().validate();
    // The frame is above its children, as it should be.
    assert!(r.is_clean(), "{:?}", r.issues);
}

#[test]
fn compact_and_pretty_json_round_trip() {
    let doc = sample();
    let compact = doc.to_json().unwrap();
    let pretty = doc.to_json_pretty().unwrap();
    assert!(pretty.len() > compact.len());
    assert_eq!(Document::from_json(&compact).unwrap(), doc);
    assert_eq!(Document::from_json(&pretty).unwrap(), doc);
    // Writing is deterministic.
    assert_eq!(doc.to_json().unwrap(), compact);
    // Re-reading and re-writing is byte-stable.
    assert_eq!(Document::from_json(&compact).unwrap().to_json().unwrap(), compact);
}

#[test]
fn header_fields() {
    let v: serde_json::Value = serde_json::from_str(&sample().to_json().unwrap()).unwrap();
    assert_eq!(v["format"], "drafftink");
    assert_eq!(v["version"], SCHEMA_VERSION);
    assert_eq!(v["name"], "Sample");
    assert_eq!(v["background"], "#fafafa");
    assert!(v["files"].as_object().unwrap().values().all(|f| f["mime"] == "image/png" && f["bytes"].is_string()));
}

#[test]
fn golden_file_is_stable() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/native_v1.json");
    let current = sample().to_json_pretty().unwrap();
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &current).unwrap();
    }
    let golden = std::fs::read_to_string(&path)
        .expect("tests/golden/native_v1.json exists; run with UPDATE_GOLDEN=1 to create it");
    assert_eq!(
        current, golden,
        "the native format changed; bump SCHEMA_VERSION or fix the regression (UPDATE_GOLDEN=1 rewrites the file)"
    );
    // The file written by this build can be read by this build.
    assert_eq!(Document::from_json(&golden).unwrap(), sample());
}

#[test]
fn minimal_documents_load_with_defaults() {
    let d = Document::from_json(r#"{"format":"drafftink","version":1}"#).unwrap();
    assert_eq!((d.name.as_str(), d.background, d.elements.len()), ("Untitled", Color::WHITE, 0));
    let d = Document::from_json(r#"{"format":"drafftink"}"#).unwrap();
    assert_eq!(d.schema_version, SCHEMA_VERSION);
    let d = Document::from_json(r#"{}"#).unwrap();
    assert!(d.elements.is_empty(), "format defaults, so an empty object is an empty document");
}

#[test]
fn newer_versions_are_refused_with_a_clear_message() {
    let err = Document::from_json(r#"{"format":"drafftink","version":2,"elements":[{"future":true}]}"#).unwrap_err();
    assert_eq!(err, DocumentError::NewerVersion { found: 2, max: 1 });
    assert!(err.to_string().contains("Update the app"));
    assert!(matches!(Document::from_json(r#"{"format":"drafftink","version":0}"#), Err(DocumentError::Invalid(_))));
}

#[test]
fn foreign_and_broken_input_is_refused() {
    for bad in [
        "",
        "not json",
        "[]",
        "null",
        r#"{"format":"excalidraw","version":1}"#,
        r#"{"format":"drafftink","version":"1"}"#,
        r#"{"format":"drafftink","version":1,"elements":{}}"#,
        r#"{"format":"drafftink","version":1,"extra":1}"#,
        // The old app's format: shapes map and z_order.
        r#"{"id":"intro","name":"intro","shapes":{},"z_order":[]}"#,
    ] {
        assert!(Document::from_json(bad).is_err(), "{bad}");
    }
    let err = Document::from_json("{").unwrap_err();
    assert!(matches!(err, DocumentError::Parse(_)), "{err}");
}

#[test]
fn rule_violations_are_refused_on_read_and_write() {
    let mut doc = sample();
    doc.elements[0].transform.x = f64::NAN;
    assert!(matches!(doc.to_json(), Err(DocumentError::Invalid(_))));
    assert!(matches!(doc.to_json_pretty(), Err(DocumentError::Invalid(_))));

    let mut doc = sample();
    doc.elements[2].index = doc.elements[1].index.clone();
    let json = serde_json::to_string(&doc).unwrap();
    match Document::from_json(&json) {
        Err(DocumentError::Invalid(r)) => assert!(r.has_errors()),
        other => panic!("expected Invalid, got {other:?}"),
    }
    let text = Document::from_json(&json).unwrap_err().to_string();
    assert!(text.contains("z-order"), "{text}");
}

#[test]
fn warnings_do_not_block_reading() {
    let mut doc = sample();
    doc.files.clear();
    let back = Document::from_json(&doc.to_json().unwrap()).unwrap();
    assert!(back.validate().warnings().next().is_some());
}

#[test]
fn blob_bytes_survive_as_base64() {
    let doc = sample();
    let back = Document::from_json(&doc.to_json().unwrap()).unwrap();
    let (id, entry) = back.files.iter().next().unwrap();
    assert_eq!(&entry.bytes[..], &[137, 80, 78, 71, 13, 10, 26, 10]);
    assert_eq!(*id, FileId::of(&entry.bytes));
    let bad = doc.to_json().unwrap().replace("iVBORw0KGgo=", "!!notbase64!!");
    assert!(Document::from_json(&bad).is_err());
}

#[test]
fn store_snapshot_and_json_round_trip() {
    let doc = sample();
    let mut store = MemoryStore::from_document(&doc).unwrap();
    assert_eq!(store.len(), doc.elements.len());
    let json = snapshot(&store).to_json().unwrap();
    let mut again = MemoryStore::new();
    load_document(&mut again, &Document::from_json(&json).unwrap()).unwrap();
    assert_eq!(snapshot(&again), snapshot(&store));

    // Group and frame indexes match the document.
    let d = store.derived();
    assert_eq!(d.group_members(&gid("pair")).count(), 2);
    assert_eq!(d.frame_children(&id("frame")).count(), 2);
    assert_eq!(d.label_of(&id("link")), Some(&id("link_label")));
    assert_eq!(d.bound_arrows(&id("box_a")).count(), 1);

    // An edit through the store changes the exported JSON.
    let mut tx = store.begin();
    tx.set_name("Edited").unwrap();
    tx.commit().unwrap();
    assert!(snapshot(&store).to_json().unwrap().contains("Edited"));
}

// ---- writing a shared document a slice at a time -------------------------------------------------------------------

fn written(doc: &Document, budget: usize) -> Vec<u8> {
    let mut store = MemoryStore::from_document(doc).unwrap();
    // A tombstone is written, too.
    let first = doc.elements.first().map(|e| e.id.clone());
    if let Some(first) = first {
        let mut tx = store.begin();
        tx.update(&first, |e| e.deleted = true).unwrap();
        tx.commit().unwrap();
    }
    let mut job = JsonJob::new(store.share_document()).unwrap();
    while !job.is_done() {
        assert!(job.step(budget).unwrap() > 0, "every slice writes an element");
    }
    job.finish().unwrap()
}

#[test]
fn a_job_writes_the_text_of_to_json_whatever_the_slice_size() {
    let mut doc = sample();
    doc.name = "A \"name\" with ünïcode and a \\".into();
    for budget in [1, 100, 1_000, usize::MAX] {
        let bytes = written(&doc, budget);
        let mut store = MemoryStore::from_document(&doc).unwrap();
        let first = doc.elements.first().unwrap().id.clone();
        let mut tx = store.begin();
        tx.update(&first, |e| e.deleted = true).unwrap();
        tx.commit().unwrap();
        let expect = snapshot(&store).to_json().unwrap();
        assert_eq!(String::from_utf8(bytes).unwrap(), expect, "slice budget {budget}");
    }
    // Without files and without elements.
    let empty = Document::new();
    let mut job = JsonJob::new(MemoryStore::new().share_document()).unwrap();
    assert!(job.is_done());
    assert_eq!(job.step(10).unwrap(), 0);
    assert_eq!(String::from_utf8(job.finish().unwrap()).unwrap(), empty.to_json().unwrap());
    // The text reads back as the document.
    let back = Document::from_json(&String::from_utf8(written(&sample(), 500)).unwrap()).unwrap();
    assert_eq!(back.elements.len(), sample().elements.len());
}

#[test]
fn a_job_sees_the_moment_it_started() {
    let doc = sample();
    let mut store = MemoryStore::from_document(&doc).unwrap();
    let expect = snapshot(&store).to_json().unwrap();
    let mut job = JsonJob::new(store.share_document()).unwrap();
    assert!(job.step(1).unwrap() > 0, "a first slice");
    // The store changes while the job is half way: an edit, a removal, a new element, a new name.
    let ids: Vec<ElementId> = store.iter_all().map(|e| e.id.clone()).collect();
    let mut tx = store.begin();
    tx.update(&ids[1], |e| e.transform.x += 77.0).unwrap();
    tx.remove(&ids[2]).unwrap();
    tx.set_name("Renamed").unwrap();
    tx.insert(rect("late", &ZIndex::between(store_top(&tx).as_ref(), None).unwrap())).unwrap();
    tx.commit().unwrap();
    while !job.is_done() {
        job.step(1).unwrap();
    }
    assert!(String::from_utf8(job.finish().unwrap()).unwrap() == expect, "the text is the document as it was");
}

fn store_top(tx: &impl SceneRead) -> Option<ZIndex> {
    tx.top_z()
}

#[test]
fn a_job_refuses_what_to_json_refuses() {
    // Two elements with one z key: valid in a live store, not in a file.
    let z = zs(2);
    let mut store = MemoryStore::new();
    let mut tx = store.begin();
    tx.insert(rect("a", &z[0])).unwrap();
    tx.insert(rect("b", &z[0])).unwrap();
    tx.commit().unwrap();
    let shared = store.share_document();
    assert!(shared.check().is_err());
    assert!(snapshot(&store).to_json().is_err());
    // And a job that is not finished has no text.
    let job = JsonJob::new(MemoryStore::from_document(&sample()).unwrap().share_document()).unwrap();
    assert!(job.finish().is_err());
}

#[test]
fn map_colors_reaches_the_background_strokes_fills_and_text_runs() {
    use drafftink_scene::{Color, ElementKind, TextData, TextRun};
    let mut doc = Document::new();
    doc.background = Color::rgb(5, 6, 7);
    let mut text = TextData::new("ab");
    text.runs = vec![TextRun { start: 0, end: 1, color: Some(Color::rgb(1, 2, 3)) }];
    let mut rect = Element::new(ElementId::new("r").unwrap(), ElementKind::Rectangle, ZIndex::first());
    rect.style.stroke_color = Color::rgb(10, 0, 0);
    rect.style.fill_color = Some(Color::rgb(0, 20, 0));
    let t = Element::new(ElementId::new("t").unwrap(), ElementKind::Text(text), ZIndex::first());
    doc.elements = vec![rect, t];
    doc.map_colors(|c| Color::rgba(c.r.saturating_add(1), c.g, c.b, c.a));
    assert_eq!(doc.background, Color::rgb(6, 6, 7));
    assert_eq!(doc.elements[0].style.stroke_color, Color::rgb(11, 0, 0));
    assert_eq!(doc.elements[0].style.fill_color, Some(Color::rgb(1, 20, 0)));
    let ElementKind::Text(t) = &doc.elements[1].kind else { panic!("text") };
    assert_eq!(t.runs[0].color, Some(Color::rgb(2, 2, 3)));
}
