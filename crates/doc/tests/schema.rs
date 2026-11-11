#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The stored layout (docs/crdt.md): what is written, what is left out, and that every kind round-trips.

mod common;

use common::*;
use drafftink_doc::LoroStore;
use drafftink_scene::*;
use kurbo::Point;
use smallvec::smallvec;

fn styled(mut e: Element) -> Element {
    e.style.stroke_color = Color::parse("#ff0000").unwrap();
    e.style.stroke_width = 5.5;
    e.style.fill_color = Some(Color::parse("#00ff0080").unwrap());
    e.style.fill_pattern = FillPattern::Hachure;
    e.style.sloppiness = Sloppiness::Drunk;
    e.style.stroke_style = StrokeStyle::Dashed;
    e.style.opacity = 0.5;
    e.style.corner_radius = 8.0;
    e.style.seed = 424_242;
    e.transform.angle = 0.75;
    e
}

fn every_kind() -> Document {
    let z = zs(14);
    let mut els = Vec::new();
    els.push(styled(rect("a_rect", &z[0])));
    let mut dia = Element::new(eid("b_dia"), ElementKind::Diamond, z[1].clone()).with_bounds(200.0, 0.0, 80.0, 80.0);
    dia.group_ids = smallvec![gid("inner"), gid("outer")];
    els.push(dia);
    let mut ell = Element::new(eid("c_ell"), ElementKind::Ellipse, z[2].clone()).with_bounds(300.0, 0.0, 80.0, 40.0);
    ell.group_ids = smallvec![gid("inner"), gid("outer")];
    ell.locked = true;
    ell.name = Some("the ellipse".into());
    ell.link = Some("https://example.com/?q=1".into());
    ell.custom = Some([("k".to_owned(), serde_json::json!({"n": [1, 2.5, null], "s": "x"}))].into_iter().collect());
    els.push(ell);

    let mut arr = Element::linear(
        eid("d_arrow"),
        LinearKind::Arrow,
        z[3].clone(),
        &[Point::new(0.0, 100.0), Point::new(50.0, 100.0), Point::new(50.0, 150.0), Point::new(120.0, 150.0)],
    )
    .unwrap();
    if let ElementKind::Arrow(d) = &mut arr.kind {
        d.path = PathType::Elbow;
        d.start_head = Head::Bar;
        d.end_head = Head::TriangleOutline;
        d.head_size = Some(12.0);
        d.start_binding = Some(Binding { target: eid("a_rect"), mode: BindMode::Inside, fixed_point: [0.25, 0.75] });
        d.end_binding = Some(Binding { target: eid("c_ell"), mode: BindMode::Orbit, fixed_point: [0.5, 0.5] });
        d.fixed_segments = vec![FixedSegment { index: 2, start: Point::new(50.0, 0.0), end: Point::new(50.0, 50.0) }];
        d.start_is_special = true;
    }
    els.push(arr);

    let mut line = Element::linear(
        eid("e_line"),
        LinearKind::Line,
        z[4].clone(),
        &[Point::new(0.0, 200.0), Point::new(40.0, 260.0), Point::new(80.0, 200.0)],
    )
    .unwrap();
    if let ElementKind::Line(d) = &mut line.kind {
        d.closed = true;
        d.path = PathType::Round;
    }
    els.push(line);

    let mut free = freehand("f_free", &z[5], &[Point::new(5.0, 5.0), Point::new(9.5, 7.25), Point::new(15.0, 6.0)]);
    if let ElementKind::Freehand(d) = &mut free.kind {
        d.pressures = vec![0.1, 0.5, 0.9];
        d.pressure_mode = PressureMode::Device;
        d.brush = Brush::Calligraphy;
        d.highlighter = true;
    }
    els.push(free);

    let mut t = TextData::new("héllo 😀 wörld\nline two");
    t.runs = vec![
        TextRun { start: 0, end: 5, color: Some(Color::parse("#ff0000").unwrap()), ..Default::default() },
        TextRun { start: 8, end: 12, color: Some(Color::parse("#0000ff80").unwrap()), ..Default::default() },
        TextRun { start: 12, end: 15, color: None, bold: true, italic: false },
        TextRun { start: 15, end: 18, color: Some(Color::parse("#00ff00").unwrap()), bold: true, italic: true },
        TextRun { start: 19, end: 22, color: None, bold: false, italic: true },
    ];
    t.font.family = FontFamily::VanillaExtract;
    t.font.size = 28.0;
    t.font.weight = FontWeight::Heavy;
    t.font.align = TextAlign::Center;
    t.auto_resize = false;
    els.push(Element::new(eid("g_text"), ElementKind::Text(t), z[6].clone()).with_bounds(0.0, 300.0, 150.0, 60.0));

    let mut label = TextData::new("label");
    label.container_id = Some(eid("a_rect"));
    els.push(Element::new(eid("h_label"), ElementKind::Text(label), z[7].clone()).with_bounds(20.0, 30.0, 40.0, 20.0));

    let mut m = MathData::new("x^2+y^2=r^2");
    m.font_size = 33.0;
    els.push(Element::new(eid("i_math"), ElementKind::Math(m), z[8].clone()).with_bounds(0.0, 400.0, 100.0, 30.0));

    let (fid, entry) = FileEntry::new("image/png", vec![1u8, 2, 3, 4, 5]);
    let mut img = Element::new(
        eid("j_img"),
        ElementKind::Image(ImageData {
            file_id: Some(fid.clone()),
            natural_width: 640,
            natural_height: 480,
            crop: Some(Crop { x: 1.0, y: 2.0, width: 3.0, height: 4.0 }),
        }),
        z[9].clone(),
    )
    .with_bounds(500.0, 0.0, 64.0, 48.0);
    img.transform.flip_x = true;
    img.transform.flip_y = true;
    els.push(img);

    els.push(
        Element::new(
            eid("k_frame"),
            ElementKind::Frame(FrameData { name: Some("Frame 1".into()), clip: false }),
            z[10].clone(),
        )
        .with_bounds(600.0, 0.0, 300.0, 300.0),
    );
    let mut child = rect("l_child", &z[11]);
    child.frame_id = Some(eid("k_frame"));
    els.push(child);
    let mut dead = rect("m_dead", &z[12]);
    dead.deleted = true;
    els.push(dead);
    els.push(Element::new(
        eid("n_free1"),
        ElementKind::Freehand(FreehandData::new(vec![Point::new(0.0, 0.0)])),
        z[13].clone(),
    ));

    let mut doc = Document {
        name: "Every kind".into(),
        background: Color::parse("#112233").unwrap(),
        elements: els,
        ..Document::new()
    };
    doc.files.insert(fid, entry);
    doc
}

#[test]
fn every_kind_and_field_survives_the_crdt() {
    let doc = every_kind();
    assert!(!doc.validate().has_errors(), "{:?}", doc.validate());
    let mut s = peer(1);
    load_document(&mut s, &doc).unwrap();
    assert_view_matches_loro(&s);
    let back = content(&s);
    assert_eq!(back.elements.len(), doc.elements.len());
    for (a, b) in back.elements.iter().zip(&doc.elements) {
        let mut b = b.clone();
        b.version = 0;
        assert_eq!(a, &b);
    }
    assert_eq!(back.name, doc.name);
    assert_eq!(back.background, doc.background);
    assert_eq!(back.files, doc.files);
    // And through a second replica that only ever sees updates.
    let mut other = peer(2);
    push(&s, &mut other);
    assert_same(&s, &other);
}

#[test]
fn the_layout_is_flat_fields_and_omits_defaults() {
    let mut s = peer(1);
    let z = zs(1);
    add(&mut s, vec![rect("r1", &z[0])]);
    let j = s.debug_json();
    assert_eq!(j["meta"]["schema"], 1);
    let r = &j["elements"]["r1"];
    assert_eq!(r["kind"], "rectangle");
    assert_eq!(r["x"], 10.0);
    assert_eq!(r["w"], 100.0);
    assert_eq!(r["index"], z[0].as_str());
    // Defaults are not stored.
    for absent in
        ["angle", "locked", "deleted", "group_ids", "name", "style.stroke_color", "style.opacity", "style.seed"]
    {
        assert!(r.get(absent).is_none(), "{absent} should be absent: {r}");
    }
    // No shared z-order list and no name/background keys until they differ from the defaults.
    assert!(j.get("z_order").is_none());
    assert!(j["meta"].get("name").is_none());
}

#[test]
fn a_local_edit_writes_only_the_changed_fields() {
    let mut s = peer(1);
    let z = zs(200);
    add(&mut s, (0..200).map(|i| rect(&format!("r{i}"), &z[i])).collect());
    s.reset_stats();

    edit(&mut s, "r77", |e| {
        e.transform.x += 5.0;
        e.transform.y -= 3.0;
    });
    let st = s.stats();
    assert_eq!((st.elements_written, st.fields_written, st.commits), (1, 2, 1), "move writes x and y only");

    s.reset_stats();
    edit(&mut s, "r77", |e| e.style.stroke_color = Color::parse("#00ff00").unwrap());
    let st = s.stats();
    assert_eq!((st.elements_written, st.fields_written), (1, 1), "recolor writes one field");

    s.reset_stats();
    edit(&mut s, "r77", |e| e.style.stroke_color = Style::default().stroke_color);
    let st = s.stats();
    assert_eq!(st.fields_written, 1, "back to the default deletes the key");
    assert!(s.debug_json()["elements"]["r77"].get("style.stroke_color").is_none());

    s.reset_stats();
    let _ = edit(&mut s, "r77", |_| {});
    assert_eq!(s.stats().commits, 0, "a no-op commit reaches Loro not at all");
    assert_view_matches_loro(&s);
}

#[test]
fn moving_a_big_freehand_does_not_rewrite_its_points() {
    let mut s = peer(1);
    let pts: Vec<Point> = (0..20_000).map(|i| Point::new(f64::from(i), f64::from(i % 97))).collect();
    let e = freehand("big", &zs(1)[0], &pts);
    add(&mut s, vec![e]);
    s.reset_stats();
    let before = s.export_snapshot().unwrap().len();
    edit(&mut s, "big", |e| e.transform.x += 1.0);
    assert_eq!(s.stats().fields_written, 1);
    let after = s.export_snapshot().unwrap().len();
    assert!(after - before < 200, "a move added {} bytes", after - before);
}

#[test]
fn meta_and_files_are_stored_apart_from_elements() {
    let mut s = peer(1);
    let (fid, entry) = FileEntry::new("image/png", vec![9u8; 100]);
    let mut tx = s.begin();
    tx.set_name("My board").unwrap();
    tx.set_background(Color::parse("#101010").unwrap());
    tx.put_file(entry).unwrap();
    tx.commit().unwrap();
    let j = s.debug_json();
    assert_eq!(j["meta"]["name"], "My board");
    assert_eq!(j["meta"]["background"], "#101010");
    assert!(j["files"].get(fid.as_str()).is_some());
    let mut tx = s.begin();
    tx.set_name("Untitled").unwrap();
    tx.remove_file(&fid);
    tx.commit().unwrap();
    let j = s.debug_json();
    assert!(j["meta"].get("name").is_none());
    assert!(j["files"].get(fid.as_str()).is_none());
    assert_view_matches_loro(&s);
}

#[test]
fn reloading_a_snapshot_gives_the_same_scene() {
    let doc = every_kind();
    let mut s = LoroStore::from_document(&doc).unwrap();
    assert_eq!(s.undo_depth(), 0, "loading is not an undo step");
    let bytes = s.export_snapshot().unwrap();
    let t = LoroStore::from_snapshot(&bytes).unwrap();
    assert_same(&s, &t);
    let bytes = s.export_state_only().unwrap();
    assert_same(&s, &LoroStore::from_snapshot(&bytes).unwrap());
    let bytes = s.export_shallow_snapshot(0).unwrap();
    assert_same(&s, &LoroStore::from_snapshot(&bytes).unwrap());
    edit(&mut s, "a_rect", |e| e.transform.x = 1.0);
}
