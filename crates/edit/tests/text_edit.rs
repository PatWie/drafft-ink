#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Text rules beyond labels: font steps, formulas, colour runs, wrapping a text in a shape.

use drafftink_edit::text_bind::{
    FontStep, MathMeasure, MonospaceMeasure, TextBindError, anchor_top_left, create_label, label_of, measure_text,
    next_font_size, set_latex, step_font_size, style_range, wrap_in_container,
};
use drafftink_scene::{
    Color, Element, ElementId, ElementKind, Kind, MathData, MemoryStore, SceneRead, SceneStore, SceneTx, SeqIds,
    TextData, TextRun, Transform,
};
use drafftink_typeset::{MathError, MathErrorKind};
use kurbo::{Point, Size};

fn id(s: &str) -> ElementId {
    ElementId::new(s).unwrap()
}

struct FakeMath;
impl MathMeasure for FakeMath {
    fn math_box(&self, latex: &str, size: f64) -> Result<Size, MathError> {
        if latex.contains("\\bad") {
            return Err(MathError {
                kind: MathErrorKind::UnknownCommand("bad".to_owned()),
                span: 0..4,
                message: "unknown command `\\bad`".to_owned(),
            });
        }
        Ok(Size::new(latex.len() as f64 * size * 0.5, size * 1.5))
    }
}

fn text(store: &mut MemoryStore, s: &str, content: &str, size: f64) -> ElementId {
    let mut tx = store.begin();
    let z = tx.next_top_index().unwrap();
    let mut d = TextData::new(content);
    d.font.size = size;
    let b = measure_text(&MonospaceMeasure::default(), content, &d.font, None);
    tx.insert(Element::new(id(s), ElementKind::Text(d), z).with_bounds(10.0, 20.0, b.width, b.height)).unwrap();
    tx.commit().unwrap();
    id(s)
}

#[test]
fn steps_follow_the_presets_then_eight() {
    let up = |s| next_font_size(s, FontStep::Increase);
    let down = |s| next_font_size(s, FontStep::Decrease);
    assert_eq!([up(12.0), up(16.0), up(20.0), up(28.0), up(36.0), up(44.0)], [16.0, 20.0, 28.0, 36.0, 44.0, 52.0]);
    assert_eq!(
        [down(52.0), down(44.0), down(40.0), down(36.0), down(28.0), down(20.0)],
        [44.0, 36.0, 36.0, 28.0, 20.0, 16.0]
    );
    assert_eq!([down(16.0), down(12.0), down(8.0)], [8.0, 8.0, 8.0]);
    assert_eq!([up(199.0), up(200.0)], [200.0, 200.0]);
    assert_eq!(up(f64::NAN), 28.0);
    assert_eq!(down(f64::INFINITY), 16.0);
}

#[test]
fn stepping_a_text_remeasures_it_and_keeps_the_top_left() {
    let mut store = MemoryStore::new();
    let t = text(&mut store, "t", "abcd", 20.0);
    let mut tx = store.begin();
    let r =
        step_font_size(&mut tx, &MonospaceMeasure::default(), &FakeMath, std::slice::from_ref(&t), FontStep::Increase)
            .unwrap();
    assert_eq!(r.changed, vec![t.clone()]);
    let e = tx.get(&t).unwrap();
    assert_eq!(e.text_data().unwrap().font.size, 28.0);
    assert_eq!((e.transform.x, e.transform.y, e.transform.w), (10.0, 20.0, 4.0 * 14.0));
}

#[test]
fn stepping_at_the_limit_changes_nothing_and_a_rectangle_without_label_is_skipped() {
    let mut store = MemoryStore::new();
    let t = text(&mut store, "t", "ab", 200.0);
    let mut tx = store.begin();
    let z = tx.next_top_index().unwrap();
    tx.insert(Element::new(id("r"), ElementKind::Rectangle, z).with_bounds(0.0, 0.0, 50.0, 50.0)).unwrap();
    let r = step_font_size(&mut tx, &MonospaceMeasure::default(), &FakeMath, &[t.clone(), id("r")], FontStep::Increase)
        .unwrap();
    assert_eq!((r.changed.len(), r.unchanged, r.skipped), (0, vec![t], vec![id("r")]));
}

#[test]
fn a_container_steps_its_label() {
    let mut store = MemoryStore::new();
    let mut tx = store.begin();
    let z = tx.next_top_index().unwrap();
    tx.insert(Element::new(id("r"), ElementKind::Rectangle, z).with_bounds(0.0, 0.0, 200.0, 100.0)).unwrap();
    let mut ids = SeqIds::default();
    let l = create_label(&mut tx, &MonospaceMeasure::default(), &mut ids, &id("r"), "hi", None).unwrap().label;
    let r = step_font_size(&mut tx, &MonospaceMeasure::default(), &FakeMath, &[id("r")], FontStep::Increase).unwrap();
    assert_eq!(r.changed, vec![l.clone()]);
    assert_eq!(label_of(&tx, &id("r")), Some(l.clone()));
    assert_eq!(tx.get(&l).unwrap().text_data().unwrap().font.size, 28.0);
}

#[test]
fn set_latex_resizes_and_a_parse_error_leaves_the_element_alone() {
    let mut store = MemoryStore::new();
    let mut tx = store.begin();
    let z = tx.next_top_index().unwrap();
    tx.insert(Element::new(id("m"), ElementKind::Math(MathData::new("x")), z).with_bounds(5.0, 6.0, 10.0, 30.0))
        .unwrap();
    assert!(set_latex(&mut tx, &FakeMath, &id("m"), "x^2").unwrap());
    let e = tx.get(&id("m")).unwrap();
    assert_eq!((e.transform.x, e.transform.y, e.transform.w, e.transform.h), (5.0, 6.0, 30.0, 30.0));
    let err = set_latex(&mut tx, &FakeMath, &id("m"), "\\bad").unwrap_err();
    assert!(matches!(err, TextBindError::InvalidLatex { line: 1, column: 1, .. }), "{err:?}");
    assert!(err.to_string().contains("unknown command"));
    assert_eq!(tx.get(&id("m")).unwrap().transform.w, 30.0);
    assert!(!set_latex(&mut tx, &FakeMath, &id("m"), "x^2").unwrap());
    assert_eq!(set_latex(&mut tx, &FakeMath, &id("m"), "  ").unwrap_err(), TextBindError::EmptyFormula);
}

#[test]
fn style_range_sets_clears_and_checks_the_range() {
    let mut store = MemoryStore::new();
    let t = text(&mut store, "t", "hello", 20.0);
    let mut tx = store.begin();
    let red = Color::parse("#ff0000").unwrap();
    assert!(style_range(&mut tx, &t, 1, 3, Some(red)).unwrap());
    assert_eq!(tx.get(&t).unwrap().text_data().unwrap().runs, vec![TextRun { start: 1, end: 3, color: Some(red) }]);
    assert!(!style_range(&mut tx, &t, 1, 3, Some(red)).unwrap());
    assert!(style_range(&mut tx, &t, 1, 3, None).unwrap());
    assert!(tx.get(&t).unwrap().text_data().unwrap().runs.is_empty());
    let e = style_range(&mut tx, &t, 2, 9, Some(red)).unwrap_err();
    assert_eq!(e, TextBindError::BadRange { start: 2, end: 9, len: 5 });
    assert!(e.to_string().contains("5 characters"));
    assert_eq!(
        style_range(&mut tx, &t, 3, 3, Some(red)).unwrap_err(),
        TextBindError::EmptyRange { start: 3, end: 3, len: 5 }
    );
}

#[test]
fn wrapping_makes_a_shape_that_holds_the_text() {
    let mut store = MemoryStore::new();
    let t = text(&mut store, "t", "hello", 20.0);
    let mut tx = store.begin();
    let mut ids = SeqIds::default();
    let (w, skipped) = wrap_in_container(
        &mut tx,
        &MonospaceMeasure::default(),
        &mut ids,
        std::slice::from_ref(&t),
        Kind::Rectangle,
        16.0,
    )
    .unwrap();
    assert!(skipped.is_empty());
    let c = tx.get(&w[0].container).unwrap();
    assert_eq!(c.tag(), Kind::Rectangle);
    assert_eq!(c.transform.w, 50.0 + 32.0);
    assert_eq!(tx.get(&t).unwrap().text_data().unwrap().container_id, Some(w[0].container.clone()));
    assert!(c.index < tx.get(&t).unwrap().index);
    // A second wrap skips the label.
    let (w2, skipped2) = wrap_in_container(
        &mut tx,
        &MonospaceMeasure::default(),
        &mut ids,
        std::slice::from_ref(&t),
        Kind::Rectangle,
        16.0,
    )
    .unwrap();
    assert!((w2.len(), skipped2.len()) == (0, 1));
}

#[test]
fn rotated_text_keeps_its_top_left_corner() {
    let mut t = Transform::new(0.0, 0.0, 100.0, 20.0);
    t.angle = std::f64::consts::FRAC_PI_2;
    let before = t.local_to_world(Point::ZERO);
    let (x, y) = anchor_top_left(&t, 60.0, 20.0);
    let mut n = Transform::new(x, y, 60.0, 20.0);
    n.angle = t.angle;
    let after = n.local_to_world(Point::ZERO);
    assert!((before - after).hypot() < 1e-9);
}
