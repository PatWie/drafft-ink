#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Hostile input never panics: arbitrary text, mutated documents, extreme numbers, deep nesting, huge lists.

mod common;

use common::*;
use drafftink_scene::*;
use proptest::prelude::*;

fn valid_json() -> String {
    let z = zs(4);
    let mut doc = Document::new();
    let mut ar = arrow("ar", &z[1], &[(0.0, 0.0), (30.0, 10.0)]);
    set_end_binding(&mut ar, "a");
    let mut t = text("t", &z[2], "héllo 😀");
    if let ElementKind::Text(d) = &mut t.kind {
        d.runs = vec![TextRun { start: 1, end: 3, color: Some(Color::BLACK) }];
    }
    doc.elements = vec![rect("a", &z[0]), ar, t, frame("f", &z[3])];
    doc.to_json().unwrap()
}

fn assert_consistent(result: Result<Document, DocumentError>) {
    // Whatever came out of hostile bytes is either an error or a document that obeys every rule.
    if let Ok(doc) = result {
        assert!(!doc.validate().has_errors());
        assert!(doc.to_json().is_ok());
        assert!(MemoryStore::from_document(&doc).is_ok());
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn arbitrary_text_is_an_error_not_a_panic(s in "\\PC{0,200}") {
        assert_consistent(Document::from_json(&s));
    }

    #[test]
    fn arbitrary_json_values(v in arb_json()) {
        assert_consistent(Document::from_json(&v.to_string()));
    }

    #[test]
    fn mutated_valid_documents(pos in 0usize..2000, byte in any::<u8>(), cut in 0usize..2000, dup in any::<bool>()) {
        let mut bytes = valid_json().into_bytes();
        let p = pos % bytes.len();
        bytes[p] = byte;
        if dup { let c = cut % bytes.len(); bytes.truncate(c); }
        let s = String::from_utf8_lossy(&bytes).into_owned();
        assert_consistent(Document::from_json(&s));
    }

    #[test]
    fn elements_with_arbitrary_numbers_never_panic(
        x in any::<f64>(), y in any::<f64>(), w in any::<f64>(), h in any::<f64>(), angle in any::<f64>(),
        sw in any::<f64>(), op in any::<f64>(), cr in any::<f64>(), px in any::<f64>(), py in any::<f64>(),
    ) {
        let z = ZIndex::first();
        let mut e = rect("r", &z);
        e.transform = Transform { x, y, w, h, angle, flip_x: false, flip_y: false };
        e.style.stroke_width = sw; e.style.opacity = op; e.style.corner_radius = cr;
        let ok = validate_element(&e).is_ok();
        if ok { prop_assert!([x, y, w, h, angle, sw, op, cr].iter().all(|v| v.is_finite())); }

        let mut a = arrow("a", &z, &[(0.0, 0.0), (1.0, 1.0)]);
        if let ElementKind::Arrow(d) = &mut a.kind {
            d.points[1] = kurbo::Point::new(px, py);
            d.head_size = Some(sw);
        }
        let _ = a.refit_points();
        let _ = validate_element(&a);
        let _ = element_errors(&a);

        // And a store must refuse or accept without panicking.
        let mut s = MemoryStore::new();
        let mut tx = s.begin();
        let _ = tx.insert(e);
        let _ = tx.insert(a);
        let _ = tx.commit();
    }

    #[test]
    fn colors_ids_and_keys_from_text(s in "\\PC{0,30}") {
        let _ = Color::parse(&s);
        let _ = ElementId::new(&s);
        let _ = GroupId::new(&s);
        let _ = FileId::new(&s);
        let _ = ZIndex::parse(&s);
    }
}

fn arb_json() -> impl Strategy<Value = serde_json::Value> {
    let leaf = prop_oneof![
        Just(serde_json::Value::Null),
        any::<bool>().prop_map(serde_json::Value::from),
        any::<i64>().prop_map(serde_json::Value::from),
        any::<f64>()
            .prop_map(|f| serde_json::Number::from_f64(f).map_or(serde_json::Value::Null, serde_json::Value::Number)),
        "\\PC{0,12}".prop_map(serde_json::Value::from),
        prop::sample::select(vec!["drafftink", "rectangle", "arrow", "a0", "#fff"]).prop_map(serde_json::Value::from),
    ];
    leaf.prop_recursive(5, 64, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(serde_json::Value::Array),
            prop::collection::btree_map(
                prop::sample::select(vec![
                    "format",
                    "version",
                    "elements",
                    "id",
                    "kind",
                    "type",
                    "transform",
                    "x",
                    "y",
                    "w",
                    "h",
                    "index",
                    "points",
                    "files",
                    "style",
                    "name"
                ]),
                inner,
                0..6
            )
            .prop_map(|m| serde_json::Value::Object(m.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())),
        ]
    })
}

#[test]
fn deep_nesting_is_an_error_not_a_stack_overflow() {
    let deep = format!("{{\"format\":\"drafftink\",\"elements\":{}{}}}", "[".repeat(50_000), "]".repeat(50_000));
    assert!(Document::from_json(&deep).is_err());
    let z = ZIndex::first();
    let nested = format!(
        r#"{{"id":"r","kind":{{"type":"rectangle"}},"transform":{{"x":0,"y":0,"w":1,"h":1}},"index":"{z}","custom":{{"k":{}0{}}}}}"#,
        "[".repeat(10_000),
        "]".repeat(10_000)
    );
    assert!(serde_json::from_str::<Element>(&nested).is_err());
}

#[test]
fn extreme_numbers_are_rejected_by_validation() {
    let z = ZIndex::first();
    for n in ["1e308", "-1e308", "1e-320", "5e-324", "2e9", "-0.0"] {
        let json = format!(
            r#"{{"format":"drafftink","elements":[{{"id":"r","kind":{{"type":"rectangle"}},"transform":{{"x":{n},"y":0,"w":1,"h":1}},"index":"{z}"}}]}}"#
        );
        let r = Document::from_json(&json);
        match n {
            "1e-320" | "5e-324" | "-0.0" => assert!(r.is_ok(), "{n}"),
            _ => assert!(r.is_err(), "{n}"),
        }
    }
    // Numbers JSON cannot even express as f64.
    let json = format!(
        r#"{{"format":"drafftink","elements":[{{"id":"r","kind":{{"type":"rectangle"}},"transform":{{"x":1e999,"y":0,"w":1,"h":1}},"index":"{z}"}}]}}"#
    );
    assert!(Document::from_json(&json).is_err());
}

#[test]
fn oversized_lists_and_texts_are_refused() {
    use limits::*;
    let z = ZIndex::first();
    let pts = vec![kurbo::Point::new(0.0, 0.0); MAX_POINTS + 1];
    let mut e = Element::new(id("l"), ElementKind::Line(LinearData::new(pts)), z.clone());
    assert!(validate_element(&e).is_err());
    e.kind = ElementKind::Text(TextData::new("x".repeat(MAX_TEXT_CHARS + 1)));
    assert!(matches!(validate_element(&e), Err(ElementError::TooLong { field: "content", .. })));
    e.kind = ElementKind::Math(MathData::new("x".repeat(MAX_LATEX_CHARS + 1)));
    assert!(matches!(validate_element(&e), Err(ElementError::TooLong { field: "latex", .. })));
    let mut e = rect("r", &z);
    e.custom = Some((0..=MAX_CUSTOM_KEYS).map(|i| (format!("k{i}"), serde_json::Value::Null)).collect());
    assert!(validate_element(&e).is_err());
    // A key that grew too long.
    let long = format!("a{}", "1".repeat(200));
    assert!(ZIndex::parse(&long).is_err());
}

#[test]
fn control_characters_and_odd_unicode_in_names() {
    let z = ZIndex::first();
    for (name, ok) in [
        ("plain", true),
        ("héllo 😀", true),
        ("日本語", true),
        ("a\u{0}b", false),
        ("a\tb", false),
        ("\u{202e}rtl", true),
        ("", false),
    ] {
        let mut e = rect("r", &z);
        e.name = Some(name.to_owned());
        assert_eq!(validate_element(&e).is_ok(), ok, "{name:?}");
    }
}

#[test]
fn a_wide_but_valid_document_loads() {
    let z = zs(5000);
    let elements: Vec<Element> = z.iter().enumerate().map(|(i, k)| rect(&format!("e{i}"), k)).collect();
    let json = doc_of(elements).to_json().unwrap();
    let doc = Document::from_json(&json).unwrap();
    assert_eq!(doc.elements.len(), 5000);
}

#[test]
fn self_and_cyclic_references_do_not_loop() {
    // Frame cycle, label cycle and binding cycle: the validator reads each element once.
    let z = zs(3);
    let mut a = rect("a", &z[0]);
    let mut b = rect("b", &z[1]);
    a.frame_id = Some(id("b"));
    b.frame_id = Some(id("a"));
    let r = validate_document(&doc_of(vec![a, b]));
    assert!(r.has_errors());
    let mut ar = arrow("ar", &z[2], &[(0.0, 0.0), (1.0, 1.0)]);
    set_end_binding(&mut ar, "ar");
    assert!(validate_document(&doc_of(vec![ar])).has_errors());
    let mut t1 = text("t1", &z[0], "a");
    let mut t2 = text("t2", &z[1], "b");
    if let (ElementKind::Text(d1), ElementKind::Text(d2)) = (&mut t1.kind, &mut t2.kind) {
        d1.container_id = Some(id("t2"));
        d2.container_id = Some(id("t1"));
    }
    assert!(validate_document(&doc_of(vec![t1, t2])).has_errors());
}
