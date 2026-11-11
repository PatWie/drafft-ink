#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Invariants: one test per rule, each with a case that passes and a case that breaks it.

mod common;

use common::*;
use drafftink_scene::*;
use kurbo::Point;
use smallvec::smallvec;

fn first_error(e: &Element) -> ElementError {
    validate_element(e).unwrap_err()
}

#[test]
fn a_plain_element_is_valid() {
    assert_eq!(validate_element(&rect("r", &ZIndex::first())), Ok(()));
}

#[test]
fn numbers_must_be_finite_and_in_range() {
    let z = ZIndex::first();
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut e = rect("r", &z);
        e.transform.x = bad;
        assert!(matches!(first_error(&e), ElementError::NotFinite { field: "transform.x" }));
        let mut e = rect("r", &z);
        e.transform.angle = bad;
        assert!(matches!(first_error(&e), ElementError::NotFinite { field: "transform.angle" }));
        let mut e = rect("r", &z);
        e.style.stroke_width = bad;
        assert!(validate_element(&e).is_err());
    }
    let mut e = rect("r", &z);
    e.transform.w = -1.0;
    assert!(matches!(first_error(&e), ElementError::OutOfRange { field: "transform.w", .. }));
    e.transform.w = 2.0e9;
    assert!(matches!(first_error(&e), ElementError::OutOfRange { .. }));
    let mut e = rect("r", &z);
    e.style.opacity = 1.5;
    assert!(matches!(first_error(&e), ElementError::OutOfRange { field: "style.opacity", .. }));
    e.style.opacity = 0.0;
    assert_eq!(validate_element(&e), Ok(()), "invisible is allowed");
    e.style.stroke_width = 0.0;
    e.style.corner_radius = 0.0;
    assert_eq!(validate_element(&e), Ok(()));
    e.style.corner_radius = -1.0;
    assert!(validate_element(&e).is_err());
}

#[test]
fn linear_elements_need_points_that_match_the_box() {
    let z = ZIndex::first();
    let mut a = arrow("a", &z, &[(0.0, 0.0), (10.0, 10.0)]);
    assert_eq!(validate_element(&a), Ok(()));

    a.transform.w = 11.0;
    assert!(matches!(first_error(&a), ElementError::BoxMismatch { .. }));
    a.transform.w = 10.0;

    if let ElementKind::Arrow(d) = &mut a.kind {
        d.points[0] = Point::new(-5.0, 0.0);
    }
    assert!(matches!(first_error(&a), ElementError::PointsNotAtOrigin { .. }));
    a.refit_points().unwrap();
    assert_eq!(validate_element(&a), Ok(()));

    if let ElementKind::Arrow(d) = &mut a.kind {
        d.points.truncate(1);
    }
    assert!(matches!(first_error(&a), ElementError::TooFewPoints { min: 2, have: 1, .. }));

    let mut a = arrow("a", &z, &[(0.0, 0.0), (10.0, 10.0)]);
    if let ElementKind::Arrow(d) = &mut a.kind {
        d.points[1] = Point::new(f64::NAN, 1.0);
    }
    assert!(matches!(first_error(&a), ElementError::NotFinite { field: "points" }));
}

#[test]
fn closed_needs_three_points_and_only_lines_close() {
    let z = ZIndex::first();
    let pts = [Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(10.0, 10.0)];
    let mut l = Element::linear(id("l"), LinearKind::Line, z.clone(), &pts).unwrap();
    if let ElementKind::Line(d) = &mut l.kind {
        d.closed = true;
    }
    assert_eq!(validate_element(&l), Ok(()));
    if let ElementKind::Line(d) = &mut l.kind {
        d.points.truncate(2);
    }
    l.refit_points().unwrap();
    assert!(matches!(first_error(&l), ElementError::TooFewPoints { min: 3, .. }));

    let mut a = Element::linear(id("a"), LinearKind::Arrow, z, &pts).unwrap();
    if let ElementKind::Arrow(d) = &mut a.kind {
        d.closed = true;
    }
    assert!(matches!(first_error(&a), ElementError::NotApplicable(_)));
}

#[test]
fn bindings_and_fixed_segments_are_checked() {
    let z = ZIndex::first();
    let mut a = arrow("a", &z, &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]);
    set_end_binding(&mut a, "a");
    assert!(matches!(first_error(&a), ElementError::SelfReference { field: "end_binding" }));
    set_end_binding(&mut a, "t");
    assert_eq!(validate_element(&a), Ok(()));
    if let ElementKind::Arrow(d) = &mut a.kind {
        d.end_binding.as_mut().unwrap().fixed_point = [f64::NAN, 0.0];
    }
    assert!(validate_element(&a).is_err());

    let mut a = arrow("a", &z, &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]);
    let seg = |index| FixedSegment { index, start: Point::ZERO, end: Point::new(1.0, 0.0) };
    if let ElementKind::Arrow(d) = &mut a.kind {
        d.fixed_segments = vec![seg(1)];
    }
    assert!(matches!(first_error(&a), ElementError::NotApplicable(_)), "only on elbow paths");
    if let ElementKind::Arrow(d) = &mut a.kind {
        d.path = PathType::Elbow;
    }
    assert_eq!(validate_element(&a), Ok(()));
    for bad in [vec![seg(0)], vec![seg(3)], vec![seg(2), seg(1)], vec![seg(1), seg(1)]] {
        if let ElementKind::Arrow(d) = &mut a.kind {
            d.fixed_segments = bad;
        }
        assert!(matches!(first_error(&a), ElementError::FixedSegment { .. }));
    }
}

#[test]
fn freehand_pressures_match_points() {
    let mut f = Element::new(
        id("f"),
        ElementKind::Freehand(FreehandData::new(vec![Point::new(0.0, 0.0), Point::new(4.0, 4.0)])),
        ZIndex::first(),
    );
    f.refit_points().unwrap();
    assert_eq!(validate_element(&f), Ok(()));
    if let ElementKind::Freehand(d) = &mut f.kind {
        d.pressures = vec![0.5];
    }
    assert!(matches!(first_error(&f), ElementError::PressureCount { pressures: 1, points: 2 }));
    if let ElementKind::Freehand(d) = &mut f.kind {
        d.pressures = vec![0.5, 1.5];
    }
    assert!(validate_element(&f).is_err());
    if let ElementKind::Freehand(d) = &mut f.kind {
        d.points.clear();
        d.pressures.clear();
    }
    assert!(matches!(first_error(&f), ElementError::TooFewPoints { .. }));
}

#[test]
fn text_runs_must_be_sorted_inside_the_text() {
    let mut t = text("t", &ZIndex::first(), "héllo");
    let run = |start, end| TextRun { start, end, color: Some(Color::BLACK) };
    for good in [vec![], vec![run(0, 5)], vec![run(0, 2), run(2, 4), run(4, 5)]] {
        if let ElementKind::Text(d) = &mut t.kind {
            d.runs = good;
        }
        assert_eq!(validate_element(&t), Ok(()));
    }
    for bad in
        [vec![run(0, 6)], vec![run(2, 2)], vec![run(3, 1)], vec![run(0, 3), run(2, 4)], vec![run(2, 4), run(0, 1)]]
    {
        if let ElementKind::Text(d) = &mut t.kind {
            d.runs = bad;
        }
        assert!(matches!(first_error(&t), ElementError::TextRun { .. }));
    }
    if let ElementKind::Text(d) = &mut t.kind {
        d.runs.clear();
        d.font.size = 0.0;
    }
    assert!(validate_element(&t).is_err(), "zero font size");
}

#[test]
fn names_links_groups_and_frames_on_one_element() {
    let z = ZIndex::first();
    let mut e = rect("r", &z);
    e.name = Some(String::new());
    assert!(matches!(first_error(&e), ElementError::BadText { field: "name", .. }));
    e.name = Some("a\nb".into());
    assert!(validate_element(&e).is_err());
    e.name = Some("ok".into());
    assert_eq!(validate_element(&e), Ok(()));
    e.link = Some("x".repeat(3000));
    assert!(validate_element(&e).is_err());
    e.link = None;
    e.group_ids = smallvec![gid("g"), gid("g")];
    assert!(matches!(first_error(&e), ElementError::DuplicateGroup(_)));
    e.group_ids = (0..20).map(|i| gid(&format!("g{i}"))).collect();
    assert!(matches!(first_error(&e), ElementError::GroupDepth { .. }));
    e.group_ids.clear();
    e.frame_id = Some(id("r"));
    assert!(matches!(first_error(&e), ElementError::SelfReference { field: "frame_id" }));

    let mut f = frame("f", &z);
    f.transform.angle = 0.1;
    assert!(matches!(first_error(&f), ElementError::NotApplicable(_)));
    f.transform.angle = 0.0;
    f.frame_id = Some(id("other"));
    assert!(matches!(first_error(&f), ElementError::NotApplicable(_)), "frames do not nest");
}

#[test]
fn images_and_crops() {
    let mut im = Element::new(
        id("i"),
        ElementKind::Image(ImageData {
            file_id: None,
            natural_width: 10,
            natural_height: 10,
            crop: Some(Crop { x: 0.0, y: 0.0, width: 0.0, height: 5.0 }),
        }),
        ZIndex::first(),
    );
    assert!(validate_element(&im).is_err(), "empty crop");
    if let ElementKind::Image(d) = &mut im.kind {
        d.crop = Some(Crop { x: 1.0, y: 1.0, width: 5.0, height: 5.0 });
    }
    assert_eq!(validate_element(&im), Ok(()));
}

#[test]
fn z_keys_must_be_strictly_increasing_in_a_document() {
    let z = zs(3);
    let ok = doc_of(vec![rect("a", &z[0]), rect("b", &z[1]), rect("c", &z[2])]);
    assert!(validate_document(&ok).is_clean());

    let tie = doc_of(vec![rect("a", &z[0]), rect("b", &z[0])]);
    assert!(has_error(&validate_document(&tie), |e| matches!(e, DocError::IndexOrder { .. })));
    let inverted = doc_of(vec![rect("a", &z[1]), rect("b", &z[0])]);
    assert!(has_error(&validate_document(&inverted), |e| matches!(e, DocError::IndexOrder { .. })));

    // The repair makes it valid.
    let mut doc = tie;
    let mut keys: Vec<ZIndex> = doc.elements.iter().map(|e| e.index.clone()).collect();
    ZIndex::repair(&mut keys).unwrap();
    for (e, k) in doc.elements.iter_mut().zip(keys) {
        e.index = k;
    }
    assert!(validate_document(&doc).is_clean());
}

#[test]
fn duplicate_ids_are_errors() {
    let z = zs(2);
    let doc = doc_of(vec![rect("a", &z[0]), rect("a", &z[1])]);
    assert!(has_error(&validate_document(&doc), |e| matches!(e, DocError::DuplicateId)));
}

#[test]
fn binding_targets_must_exist_and_be_bindable() {
    let z = zs(5);
    let mut ar = arrow("ar", &z[4], &[(0.0, 0.0), (10.0, 10.0)]);
    set_end_binding(&mut ar, "ghost");
    let r = validate_document(&doc_of(vec![rect("a", &z[0]), ar.clone()]));
    assert!(has_error(&r, |e| matches!(e, DocError::BindingTargetMissing { end: "end", .. })));

    set_end_binding(&mut ar, "a");
    assert!(validate_document(&doc_of(vec![rect("a", &z[0]), ar.clone()])).is_clean());

    // An arrow, a freehand and a math element are not bindable; text, image, frame are.
    let other = arrow("other", &z[1], &[(0.0, 0.0), (1.0, 1.0)]);
    set_end_binding(&mut ar, "other");
    let r = validate_document(&doc_of(vec![other, ar.clone()]));
    assert!(has_error(&r, |e| matches!(e, DocError::BindingTargetNotBindable { kind: "arrow", .. })));
    for ok in [text("tt", &z[0], "x"), frame("tt", &z[0])] {
        set_end_binding(&mut ar, "tt");
        assert!(!validate_document(&doc_of(vec![ok, ar.clone()])).has_errors());
    }

    // Deleted target: a warning, not an error.
    let mut gone = rect("a", &z[0]);
    gone.deleted = true;
    set_end_binding(&mut ar, "a");
    let r = validate_document(&doc_of(vec![gone, ar.clone()]));
    assert!(!r.has_errors());
    assert!(has_warning(&r, |e| matches!(e, DocError::BindingTargetDeleted { .. })));

    // A deleted arrow's stale binding is ignored.
    ar.deleted = true;
    set_end_binding(&mut ar, "ghost");
    assert!(validate_document(&doc_of(vec![ar])).is_clean());
}

#[test]
fn bound_text_needs_a_container() {
    let z = zs(4);
    let label = |name: &str, z: &ZIndex, c: &str| {
        let mut t = text(name, z, "x");
        if let ElementKind::Text(d) = &mut t.kind {
            d.container_id = Some(id(c));
        }
        t
    };
    assert!(validate_document(&doc_of(vec![rect("box", &z[0]), label("l", &z[1], "box")])).is_clean());

    let r = validate_document(&doc_of(vec![label("l", &z[1], "ghost")]));
    assert!(has_error(&r, |e| matches!(e, DocError::ContainerMissing(_))));

    let r = validate_document(&doc_of(vec![text("t", &z[0], "a"), label("l", &z[1], "t")]));
    assert!(has_error(&r, |e| matches!(e, DocError::NotAContainer { kind: "text", .. })));

    let r = validate_document(&doc_of(vec![rect("box", &z[0]), label("l1", &z[1], "box"), label("l2", &z[2], "box")]));
    assert!(has_error(&r, |e| matches!(e, DocError::TwoLabels { .. })));

    // The arrow label.
    assert!(
        validate_document(&doc_of(vec![arrow("ar", &z[0], &[(0.0, 0.0), (9.0, 9.0)]), label("l", &z[1], "ar")]))
            .is_clean()
    );

    // Label below its container, and container deleted.
    let r = validate_document(&doc_of(vec![label("l", &z[0], "box"), rect("box", &z[1])]));
    assert!(has_warning(&r, |e| matches!(e, DocError::LabelBelowContainer(_))));
    let mut b = rect("box", &z[0]);
    b.deleted = true;
    let r = validate_document(&doc_of(vec![b, label("l", &z[1], "box")]));
    assert!(!r.has_errors() && has_warning(&r, |e| matches!(e, DocError::ContainerDeleted(_))));
}

#[test]
fn frame_membership() {
    let z = zs(4);
    let mut child = rect("c", &z[0]);
    child.frame_id = Some(id("f"));
    assert!(validate_document(&doc_of(vec![child.clone(), frame("f", &z[1])])).is_clean());

    let r = validate_document(&doc_of(vec![child.clone()]));
    assert!(has_error(&r, |e| matches!(e, DocError::FrameMissing(_))));

    let r = validate_document(&doc_of(vec![child.clone(), rect("f", &z[1])]));
    assert!(has_error(&r, |e| matches!(e, DocError::NotAFrame { kind: "rectangle", .. })));

    let mut above = child.clone();
    above.index = z[2].clone();
    let r2 = validate_document(&doc_of(vec![frame("f", &z[0]), above]));
    assert!(has_warning(&r2, |e| matches!(e, DocError::ChildAboveFrame(_))));
    assert!(!r2.has_errors());

    let mut dead = frame("f", &z[1]);
    dead.deleted = true;
    let r = validate_document(&doc_of(vec![child, dead]));
    assert!(!r.has_errors() && has_warning(&r, |e| matches!(e, DocError::FrameDeleted(_))));
}

#[test]
fn group_stacks_must_nest_consistently() {
    let z = zs(4);
    let mk = |name: &str, z: &ZIndex, groups: &[&str]| {
        let mut e = rect(name, z);
        e.group_ids = groups.iter().map(|g| gid(g)).collect();
        e
    };
    // A inside B inside C, and a sibling in B and C.
    let ok =
        doc_of(vec![mk("a", &z[0], &["A", "B", "C"]), mk("b", &z[1], &["A", "B", "C"]), mk("c", &z[2], &["B", "C"])]);
    assert!(validate_document(&ok).is_clean(), "{:?}", validate_document(&ok));

    // Members of A disagree about what is around A.
    let bad = doc_of(vec![mk("a", &z[0], &["A", "B"]), mk("b", &z[1], &["A"]), mk("c", &z[2], &["B"])]);
    assert!(has_error(&validate_document(&bad), |e| matches!(e, DocError::GroupStack(g) if g.as_str() == "A")));

    // Order swapped between members.
    let swapped = doc_of(vec![mk("a", &z[0], &["A", "B"]), mk("b", &z[1], &["B", "A"])]);
    assert!(has_error(&validate_document(&swapped), |_| true));

    // Deleted members do not count; a one-member group is a warning.
    let mut dead = mk("b", &z[1], &["A"]);
    dead.deleted = true;
    let single = doc_of(vec![mk("a", &z[0], &["A", "B"]), dead, mk("c", &z[2], &["B"])]);
    let r = validate_document(&single);
    assert!(!r.has_errors());
    assert!(has_warning(&r, |e| matches!(e, DocError::GroupSingleton(g) if g.as_str() == "A")));
}

#[test]
fn names_should_be_unique() {
    let z = zs(2);
    let mut a = rect("a", &z[0]);
    let mut b = rect("b", &z[1]);
    a.name = Some("box".into());
    b.name = Some("box".into());
    let r = validate_document(&doc_of(vec![a, b]));
    assert!(!r.has_errors() && has_warning(&r, |e| matches!(e, DocError::DuplicateName { .. })));
}

#[test]
fn image_blobs_are_checked_against_their_hash() {
    let z = zs(1);
    let (fid, entry) = FileEntry::new("image/png", vec![1, 2, 3]);
    let img = Element::new(
        id("i"),
        ElementKind::Image(ImageData { file_id: Some(fid.clone()), natural_width: 1, natural_height: 1, crop: None }),
        z[0].clone(),
    );
    let mut doc = doc_of(vec![img.clone()]);
    let r = validate_document(&doc);
    assert!(
        !r.has_errors() && has_warning(&r, |e| matches!(e, DocError::FileMissing(_))),
        "a missing blob is only a warning"
    );

    doc.files.insert(fid.clone(), entry.clone());
    assert!(validate_document(&doc).is_clean());
    assert!(doc.unused_files().is_empty());

    let mut tampered = doc.clone();
    tampered.files.insert(fid.clone(), FileEntry { mime: "image/png".into(), bytes: vec![9, 9].into() });
    assert!(has_error(&validate_document(&tampered), |e| matches!(e, DocError::FileHashMismatch(_))));

    let mut bad_mime = doc.clone();
    bad_mime.files.insert(fid, FileEntry { mime: "text/html".into(), bytes: entry.bytes.clone() });
    assert!(has_error(&validate_document(&bad_mime), |e| matches!(e, DocError::BadMime { .. })));

    let mut extra = doc_of(vec![]);
    let (xid, xe) = FileEntry::new("image/webp", vec![5]);
    extra.files.insert(xid.clone(), xe);
    assert_eq!(extra.unused_files(), vec![xid]);
}

#[test]
fn document_level_rules() {
    let mut doc = Document::new();
    assert!(validate_document(&doc).is_clean());
    doc.name = String::new();
    assert!(has_error(&validate_document(&doc), |e| matches!(e, DocError::BadDocName)));
    doc.name = "ok".into();
    doc.schema_version = 0;
    assert!(has_error(&validate_document(&doc), |e| matches!(e, DocError::BadVersion(0))));
    doc.schema_version = 99;
    assert!(has_error(&validate_document(&doc), |e| matches!(e, DocError::BadVersion(99))));
}

#[test]
fn issues_name_the_element_and_explain_the_fix() {
    let z = zs(1);
    let mut ar = arrow("ar", &z[0], &[(0.0, 0.0), (1.0, 1.0)]);
    set_end_binding(&mut ar, "ghost");
    let r = validate_document(&doc_of(vec![ar]));
    let text = r.errors().next().unwrap().to_string();
    assert!(text.contains("ar") && text.contains("ghost") && text.contains("does not exist"), "{text}");
}

#[test]
fn every_error_has_a_readable_message() {
    // Display must never be empty or contain a debug dump.
    let z = ZIndex::first();
    let mut e = rect("r", &z);
    e.style.opacity = 9.0;
    let msg = first_error(&e).to_string();
    assert!(msg.contains("style.opacity") && msg.contains("between"), "{msg}");
}

#[test]
fn the_image_media_type_rule_has_one_definition() {
    assert!(is_image_media_type("image/png"));
    assert!(is_image_media_type("image/svg+xml"));
    assert!(!is_image_media_type("image/"));
    assert!(!is_image_media_type("text/html"));
    assert!(!is_image_media_type("image/png; charset=x"));
    assert!(!is_image_media_type(&format!("image/{}", "a".repeat(80))));
}

#[test]
fn a_name_is_one_to_the_limit_characters_without_control_characters() {
    use drafftink_scene::limits::MAX_NAME_CHARS;
    assert!(is_valid_name("a"));
    assert!(is_valid_name(&"é".repeat(MAX_NAME_CHARS)));
    assert!(!is_valid_name(""));
    assert!(!is_valid_name(&"é".repeat(MAX_NAME_CHARS + 1)));
    assert!(!is_valid_name("two\nlines"));
}

// ---- the same rules in steps ---------------------------------------------------------------------------------------

fn shared_of(doc: &Document) -> SharedDocument {
    SharedDocument {
        name: doc.name.clone(),
        background: doc.background,
        elements: doc.elements.iter().cloned().map(std::sync::Arc::new).collect(),
        files: doc.files.clone(),
    }
}

fn stepwise(doc: &Document, budget: usize) -> Report {
    let shared = shared_of(doc);
    let mut check = shared.checker();
    let mut steps = 0;
    while !check.step(&shared, budget) {
        steps += 1;
        assert!(steps < 100_000, "the check ends");
    }
    match check.finish() {
        Ok(()) => Report::default(),
        Err(DocumentError::Invalid(r)) => r,
        Err(other) => panic!("{other}"),
    }
}

#[test]
fn a_check_in_steps_reports_what_one_pass_reports() {
    let z = zs(8);
    let mut broken = Vec::new();
    broken.push(rect("a", &z[0]));
    broken.push(rect("a", &z[1])); // a second "a"
    let mut orphan = rect("b", &z[2]);
    orphan.frame_id = Some(id("nowhere"));
    broken.push(orphan);
    let mut dangling = arrow("c", &z[3], &[(0.0, 0.0), (10.0, 0.0)]);
    set_end_binding(&mut dangling, "gone");
    broken.push(dangling);
    broken.push(rect("d", &z[3])); // the same z key as "c"
    let mut named_1 = rect("e", &z[5]);
    named_1.name = Some("same".into());
    let mut named_2 = rect("f", &z[6]);
    named_2.name = Some("same".into());
    broken.push(named_1);
    broken.push(named_2);
    let mut label = text("g", &z[7], "x");
    if let ElementKind::Text(d) = &mut label.kind {
        d.container_id = Some(id("nobody"));
    }
    broken.push(label);

    let valid = doc_of(vec![rect("a", &z[0]), rect("b", &z[1]), text("t", &z[2], "hi")]);
    for doc in [valid, doc_of(broken)] {
        let expect = validate_document(&doc);
        for budget in [1, 2, 3, 100] {
            assert_eq!(stepwise(&doc, budget), expect, "budget {budget}");
        }
    }
    assert!(
        validate_document(&doc_of(vec![rect("a", &z[0]), rect("a", &z[1])])).has_errors(),
        "the broken sample breaks rules"
    );
}
