//! Bound text properties over random containers and contents.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use drafftink_edit::text_bind::{
    GrowAnchor, MonospaceMeasure, TextMeasure, create_label, inner_box, layout_label, measure_lines, relayout_label,
};
use drafftink_scene::{
    Element, ElementId, ElementKind, Kind, MemoryStore, SceneRead, SceneStore, SceneTx, SeqIds, TextAlign, TextStyle,
    VerticalAlign,
};
use proptest::prelude::*;

fn kind_of(i: u8) -> (ElementKind, Kind) {
    match i % 3 {
        0 => (ElementKind::Rectangle, Kind::Rectangle),
        1 => (ElementKind::Ellipse, Kind::Ellipse),
        _ => (ElementKind::Diamond, Kind::Diamond),
    }
}

fn align(i: u8) -> TextAlign {
    [TextAlign::Left, TextAlign::Center, TextAlign::Right][usize::from(i % 3)]
}

fn valign(i: u8) -> VerticalAlign {
    [VerticalAlign::Top, VerticalAlign::Middle, VerticalAlign::Bottom][usize::from(i % 3)]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn the_label_always_fits_inside_the_container(
        k in 0u8..3, w in 5.0f64..400.0, h in 5.0f64..300.0,
        content in "[a-z]{1,8}( [a-z]{1,10}){0,12}(\n[a-z]{1,6}){0,2}",
        size in 8.0f64..40.0, ha in 0u8..3, va in 0u8..3, anchor in 0u8..3,
    ) {
        let (ek, kind) = kind_of(k);
        let mut e = Element::new(ElementId::new("c").unwrap(), ek, drafftink_scene::ZIndex::first()).with_bounds(10.0, 20.0, w, h);
        e.transform.angle = 0.0;
        let font = TextStyle { size, align: align(ha), vertical_align: valign(va), ..TextStyle::default() };
        let anchor = [GrowAnchor::Top, GrowAnchor::Bottom, GrowAnchor::Center][usize::from(anchor)];
        let m = MonospaceMeasure::default();
        let l = layout_label(&m, &e, &font, &content, None, anchor).unwrap();
        // The container never shrinks and keeps the anchored edge.
        prop_assert!(l.container.width() >= w - 1e-9 && l.container.height() >= h - 1e-9);
        match anchor {
            GrowAnchor::Top => prop_assert!((l.container.y0 - 20.0).abs() < 1e-9),
            GrowAnchor::Bottom => prop_assert!((l.container.y1 - (20.0 + h)).abs() < 1e-9),
            GrowAnchor::Center => prop_assert!((l.container.center().y - (20.0 + h / 2.0)).abs() < 1e-9),
        }
        // The text box lies inside the inner box of the (grown) container.
        let inner = inner_box(kind, l.container).unwrap();
        let t = drafftink_geom::kurbo::Rect::new(l.text.x, l.text.y, l.text.x + l.text.w, l.text.y + l.text.h);
        prop_assert!(t.x0 >= inner.x0 - 1e-6 && t.x1 <= inner.x1 + 1e-6, "x: {t:?} in {inner:?}");
        prop_assert!(t.y0 >= inner.y0 - 1e-6 && t.y1 <= inner.y1 + 1e-6, "y: {t:?} in {inner:?}");
        // The box is exactly what the wrapped lines measure.
        let size = measure_lines(&m, &l.lines, &font);
        prop_assert!((size.width - l.text.w).abs() < 1e-9 && (size.height - l.text.h).abs() < 1e-9);
        // Every character is still there.
        let joined: String = l.lines.concat();
        let wanted: String = content.chars().filter(|c| !c.is_whitespace()).collect();
        prop_assert_eq!(joined.chars().filter(|c| !c.is_whitespace()).collect::<String>(), wanted);
    }

    #[test]
    fn wrapping_terminates_and_keeps_every_character(
        text in "\\PC{0,80}",
        width in prop_oneof![Just(f64::NAN), Just(f64::INFINITY), Just(-1.0), Just(0.0), 0.0f64..200.0],
        size in 1.0f64..50.0,
    ) {
        let font = TextStyle { size, ..TextStyle::default() };
        let m = MonospaceMeasure::default();
        let lines = m.wrap(&text, &font, width);
        let kept: String = lines.concat().chars().filter(|c| !c.is_whitespace()).collect();
        let want: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        prop_assert_eq!(kept, want);
        // Without a usable limit, only hard breaks split the text.
        if !width.is_finite() || width < 0.0 {
            prop_assert_eq!(lines.len(), text.split('\n').count());
        }
        // A line is within the limit unless it is a single character.
        if width.is_finite() && width >= 0.0 {
            for l in &lines {
                prop_assert!(m.line_width(l, &font) <= width + 1e-9 || l.chars().filter(|c| !c.is_whitespace()).count() <= 1, "{l:?} in {width}");
            }
        }
    }

    #[test]
    fn relayout_is_idempotent(k in 0u8..3, w in 20.0f64..300.0, h in 20.0f64..200.0, content in "[a-z]{1,8}( [a-z]{1,10}){0,10}") {
        let (ek, _) = kind_of(k);
        let mut s = MemoryStore::new();
        let mut tx = s.begin();
        let id = ElementId::new("c").unwrap();
        let z = tx.next_top_index().unwrap();
        tx.insert(Element::new(id.clone(), ek, z).with_bounds(0.0, 0.0, w, h)).unwrap();
        let mut ids = SeqIds::new();
        let m = MonospaceMeasure::default();
        let label = create_label(&mut tx, &m, &mut ids, &id, &content, None).unwrap().label;
        let first = (tx.get(&id).unwrap().transform, tx.get(&label).unwrap().transform);
        relayout_label(&mut tx, &m, &id, GrowAnchor::Top).unwrap();
        relayout_label(&mut tx, &m, &id, GrowAnchor::Top).unwrap();
        let second = (tx.get(&id).unwrap().transform, tx.get(&label).unwrap().transform);
        prop_assert_eq!(first, second);
        tx.commit().unwrap();
    }
}

#[test]
fn a_custom_measure_is_used_for_wrapping() {
    // Every character is 7 wide whatever the font: 3 characters fit in 25.
    struct Seven;
    impl TextMeasure for Seven {
        fn line_width(&self, line: &str, _: &TextStyle) -> f64 {
            line.chars().count() as f64 * 7.0
        }
    }
    let lines = Seven.wrap("abcdefgh", &TextStyle::default(), 25.0);
    assert_eq!(lines, vec!["abc", "def", "gh"]);
}
