//! `highlight::apply`: elements and character ranges in one transaction, and the per-span rule.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use drafftink_edit::highlight::{CharRange, HighlightError, apply};
use drafftink_edit::style::{StyleError, StyleProps};
use drafftink_edit::text_bind::{TextBindError, highlight_range};
use drafftink_scene::{
    CharStyle, Element, ElementId, ElementKind, FrameData, MarkerColor, MemoryStore, SceneRead, SceneStore, SceneTx,
    SpanHighlight, TextData, TextRun, ZIndex,
};

fn marker(name: &str) -> MarkerColor {
    MarkerColor::from_name(name).unwrap()
}

fn id(s: &str) -> ElementId {
    ElementId::new(s).unwrap()
}

fn store() -> MemoryStore {
    let z = ZIndex::rebalance(5).unwrap();
    let mut s = MemoryStore::new();
    let mut tx = s.begin();
    tx.insert(Element::new(id("rect"), ElementKind::Rectangle, z[0].clone()).with_bounds(0.0, 0.0, 50.0, 30.0))
        .unwrap();
    tx.insert(Element::new(id("ell"), ElementKind::Ellipse, z[1].clone()).with_bounds(60.0, 0.0, 50.0, 30.0)).unwrap();
    tx.insert(
        Element::new(id("text"), ElementKind::Text(TextData::new("héllo wörld")), z[2].clone())
            .with_bounds(0.0, 50.0, 100.0, 25.0),
    )
    .unwrap();
    tx.insert(
        Element::new(id("frame"), ElementKind::Frame(FrameData::default()), z[3].clone())
            .with_bounds(0.0, 100.0, 80.0, 80.0),
    )
    .unwrap();
    let mut locked = Element::new(id("locked"), ElementKind::Rectangle, z[4].clone()).with_bounds(0.0, 200.0, 5.0, 5.0);
    locked.locked = true;
    tx.insert(locked).unwrap();
    tx.commit().unwrap();
    s
}

fn runs(s: &MemoryStore) -> Vec<TextRun> {
    match &s.get(&id("text")).unwrap().kind {
        ElementKind::Text(t) => t.runs.clone(),
        _ => panic!("text"),
    }
}

fn highlight_of(s: &MemoryStore, name: &str) -> Option<MarkerColor> {
    s.get(&id(name)).unwrap().style.highlight
}

#[test]
fn elements_and_ranges_go_through_one_transaction_and_one_change_set() {
    let mut s = store();
    let mut tx = s.begin();
    let report = apply(
        &mut tx,
        Some(marker("yellow-400")),
        &[id("rect"), id("ell"), id("frame")],
        &[CharRange { id: id("text"), start: 0, end: 5 }],
    )
    .unwrap();
    let cs = tx.commit().unwrap();
    assert_eq!(report.changed, [id("rect"), id("ell"), id("text")]);
    assert_eq!(report.skipped, [id("frame")], "a frame cannot be highlighted");
    assert_eq!(cs.elements.len(), 3, "one change set for all of it");
    assert_eq!(highlight_of(&s, "rect"), Some(marker("yellow-400")));
    assert_eq!(highlight_of(&s, "frame"), None);
    assert_eq!(highlight_of(&s, "text"), None, "the text itself is not highlighted, five of its characters are");
    assert_eq!(
        runs(&s),
        [TextRun::new(0, 5, CharStyle { highlight: SpanHighlight::On(marker("yellow-400")), ..Default::default() })]
    );
}

#[test]
fn clearing_takes_the_highlight_off_and_repeating_changes_nothing() {
    let mut s = store();
    let mut tx = s.begin();
    apply(&mut tx, Some(marker("green-400")), &[id("rect")], &[]).unwrap();
    tx.commit().unwrap();
    let mut tx = s.begin();
    let again = apply(&mut tx, Some(marker("green-400")), &[id("rect")], &[]).unwrap();
    assert_eq!((again.changed.len(), again.unchanged.len()), (0, 1));
    tx.rollback();
    let mut tx = s.begin();
    apply(&mut tx, None, &[id("rect")], &[]).unwrap();
    tx.commit().unwrap();
    assert_eq!(highlight_of(&s, "rect"), None);
}

#[test]
fn ranges_count_characters_not_bytes() {
    let mut s = store();
    let mut tx = s.begin();
    // "héllo wörld": the é and the ö are two bytes each; characters 6..11 are "wörld".
    assert!(highlight_range(&mut tx, &id("text"), 6, 11, Some(marker("yellow-400"))).unwrap());
    tx.commit().unwrap();
    assert_eq!(runs(&s)[0].start..runs(&s)[0].end, 6..11);
}

#[test]
fn a_range_of_a_highlighted_text_can_be_taken_out_and_the_rule_holds_for_every_character() {
    let mut s = store();
    let mut tx = s.begin();
    apply(&mut tx, Some(marker("yellow-400")), &[id("text")], &[]).unwrap();
    // "none" for the middle word: only a run that says so can do it.
    assert!(highlight_range(&mut tx, &id("text"), 2, 4, None).unwrap());
    // The colour that the whole text has needs no run.
    assert!(!highlight_range(&mut tx, &id("text"), 0, 2, Some(marker("yellow-400"))).unwrap());
    tx.commit().unwrap();
    let element = highlight_of(&s, "text");
    assert_eq!(element, Some(marker("yellow-400")));
    let ElementKind::Text(t) = &s.get(&id("text")).unwrap().kind else { panic!("text") };
    let shown: Vec<_> = t.char_styles().iter().map(|c| c.highlight.resolve(element)).collect();
    assert_eq!(shown[0], Some(marker("yellow-400")));
    assert_eq!(shown[2], None);
    assert_eq!(shown[3], None);
    assert_eq!(shown[4], Some(marker("yellow-400")));
    // Setting the whole text again drops the runs' own say.
    let mut tx = s.begin();
    apply(&mut tx, Some(marker("green-400")), &[id("text")], &[]).unwrap();
    tx.commit().unwrap();
    assert!(runs(&s).is_empty(), "{:?}", runs(&s));
}

#[test]
fn a_highlight_of_a_run_keeps_its_colour_and_bold() {
    let mut s = store();
    let mut tx = s.begin();
    tx.update(&id("text"), |e| {
        if let ElementKind::Text(t) = &mut e.kind {
            t.runs = vec![TextRun { start: 0, end: 5, bold: true, ..Default::default() }];
        }
    })
    .unwrap();
    highlight_range(&mut tx, &id("text"), 3, 8, Some(marker("yellow-400"))).unwrap();
    tx.commit().unwrap();
    let r = runs(&s);
    assert_eq!(r.len(), 3, "{r:?}");
    assert!(r[0].bold && r[0].highlight == SpanHighlight::Inherit);
    assert!(r[1].bold && r[1].highlight == SpanHighlight::On(marker("yellow-400")) && (r[1].start, r[1].end) == (3, 5));
    assert!(
        !r[2].bold && r[2].highlight == SpanHighlight::On(marker("yellow-400")) && (r[2].start, r[2].end) == (5, 8)
    );
}

#[test]
fn a_bad_call_changes_nothing_at_all() {
    let mut s = store();
    for (name, elements, ranges) in [
        ("empty", vec![], vec![]),
        ("locked", vec![id("rect"), id("locked")], vec![]),
        ("missing", vec![id("rect"), id("nope")], vec![]),
        ("only a frame", vec![id("frame")], vec![]),
        ("range past the end", vec![id("rect")], vec![CharRange { id: id("text"), start: 3, end: 12 }]),
        ("empty range", vec![id("rect")], vec![CharRange { id: id("text"), start: 3, end: 3 }]),
        ("range of a rectangle", vec![id("rect")], vec![CharRange { id: id("rect"), start: 0, end: 1 }]),
    ] {
        let before = s.get(&id("rect")).unwrap().clone();
        let mut tx = s.begin();
        let err = apply(&mut tx, Some(marker("yellow-400")), &elements, &ranges).expect_err(name);
        tx.rollback();
        assert_eq!(s.get(&id("rect")).unwrap(), &before, "{name}");
        match (name, &err) {
            ("empty", HighlightError::Nothing) => {}
            ("only a frame", HighlightError::Style(StyleError::NotApplicable { .. })) => {}
            ("range past the end", HighlightError::Text(TextBindError::BadRange { .. })) => {}
            ("empty range", HighlightError::Text(TextBindError::EmptyRange { .. })) => {}
            ("range of a rectangle", HighlightError::Text(TextBindError::NotText { .. })) => {}
            ("locked" | "missing", HighlightError::Style(StyleError::Target(_))) => {}
            other => panic!("{name}: unexpected {other:?}"),
        }
    }
}

#[test]
fn style_props_carry_the_highlight_and_null_removes_it() {
    let p: StyleProps = serde_json::from_value(serde_json::json!({"highlight": "pink"})).unwrap();
    assert_eq!(p.highlight, Some(Some(MarkerColor::from_name("pink-400").unwrap())));
    let p: StyleProps = serde_json::from_value(serde_json::json!({"highlight": null})).unwrap();
    assert_eq!(p.highlight, Some(None));
    assert_eq!(serde_json::from_value::<StyleProps>(serde_json::json!({})).unwrap().highlight, None);
    assert!(serde_json::from_value::<StyleProps>(serde_json::json!({"highlight": "mauve"})).is_err());
}
