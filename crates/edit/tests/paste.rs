#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The rules for putting outside elements into a scene: new ids, groups, bindings, order, pictures, limits.

use std::collections::BTreeMap;

use drafftink_edit::CopyError;
use drafftink_edit::create::CreateError;
use drafftink_edit::paste::{PasteError, PasteOpts, insert_elements, insert_text};
use drafftink_edit::text_bind::MonospaceMeasure;
use drafftink_scene::{
    BindMode, Binding, Element, ElementId, ElementKind, FileEntry, GroupId, ImageData, LinearData, MemoryStore,
    SceneRead, SceneStore, SceneTx, SeqIds, Style, TextStyle, ZIndex,
};
use kurbo::Point;

fn id(s: &str) -> ElementId {
    ElementId::new(s).unwrap()
}

fn rect(name: &str, x: f64, y: f64, w: f64, h: f64, z: &str) -> Element {
    Element::new(id(name), ElementKind::Rectangle, ZIndex::parse(z).unwrap()).with_bounds(x, y, w, h)
}

fn centred(c: (f64, f64)) -> PasteOpts {
    PasteOpts { center: Some(Point::new(c.0, c.1)) }
}

#[test]
fn elements_get_new_ids_keep_their_order_and_go_on_top() {
    let mut store = MemoryStore::new();
    let mut tx = store.begin();
    tx.insert(rect("e1", 0.0, 0.0, 10.0, 10.0, "a0")).unwrap();
    tx.commit().unwrap();
    let src = [rect("b", 0.0, 0.0, 10.0, 10.0, "a1"), rect("a", 20.0, 0.0, 10.0, 10.0, "a0")];
    let mut tx = store.begin();
    let r = insert_elements(&mut tx, &src, &BTreeMap::new(), &PasteOpts::default(), &mut SeqIds::new()).unwrap();
    tx.commit().unwrap();
    assert_eq!(r.copies.iter().map(|(o, _)| o.as_str()).collect::<Vec<_>>(), ["a", "b"], "source z-order");
    assert!(r.created.iter().all(|n| n.as_str() != "a" && n.as_str() != "b"));
    assert!(r.created.iter().all(|n| store.get(n).unwrap().index > store.get(&id("e1")).unwrap().index));
    assert_eq!(store.len(), 3);
}

#[test]
fn the_block_is_centred_and_labels_do_not_count_for_the_bounds() {
    let mut store = MemoryStore::new();
    let mut label =
        Element::new(id("t"), ElementKind::Text(drafftink_scene::TextData::new("x")), ZIndex::parse("a1").unwrap())
            .with_bounds(5000.0, 5000.0, 10.0, 10.0);
    if let ElementKind::Text(d) = &mut label.kind {
        d.container_id = Some(id("a"));
    }
    let src = [rect("a", 0.0, 0.0, 100.0, 50.0, "a0"), label];
    let mut tx = store.begin();
    let r = insert_elements(&mut tx, &src, &BTreeMap::new(), &centred((1000.0, 1000.0)), &mut SeqIds::new()).unwrap();
    tx.commit().unwrap();
    assert_eq!(r.offset, (950.0, 975.0));
    assert_eq!(r.selection.len(), 1, "the label is not selected");
    let new_label = store.get(&r.created[1]).unwrap();
    assert_eq!(new_label.container_id(), Some(&r.created[0]));
}

#[test]
fn groups_are_regenerated_and_lone_members_leave_their_group() {
    let mut store = MemoryStore::new();
    let g = GroupId::new("g").unwrap();
    let mut a = rect("a", 0.0, 0.0, 10.0, 10.0, "a0");
    let mut b = rect("b", 0.0, 0.0, 10.0, 10.0, "a1");
    let mut c = rect("c", 0.0, 0.0, 10.0, 10.0, "a2");
    a.group_ids.push(g.clone());
    b.group_ids.push(g.clone());
    c.group_ids.push(GroupId::new("lone").unwrap());
    let mut tx = store.begin();
    let r = insert_elements(&mut tx, &[a, b, c], &BTreeMap::new(), &PasteOpts::default(), &mut SeqIds::new()).unwrap();
    tx.commit().unwrap();
    let groups: Vec<_> = r.created.iter().map(|i| store.get(i).unwrap().group_ids.to_vec()).collect();
    assert_eq!(groups[0].len(), 1);
    assert_eq!(groups[0], groups[1]);
    assert_ne!(groups[0][0], g);
    assert!(groups[2].is_empty());
}

#[test]
fn bindings_follow_the_copies() {
    let mut store = MemoryStore::new();
    let mut arrow = Element::new(
        id("ar"),
        ElementKind::Arrow(LinearData::new(vec![Point::new(0.0, 0.0), Point::new(100.0, 0.0)])),
        ZIndex::parse("a2").unwrap(),
    )
    .with_bounds(110.0, 30.0, 100.0, 0.0);
    if let ElementKind::Arrow(d) = &mut arrow.kind {
        let b = |t: &str| Some(Binding { target: id(t), mode: BindMode::Orbit, fixed_point: [0.5, 0.5] });
        d.start_binding = b("a");
        d.end_binding = b("outside");
    }
    let src = [rect("a", 0.0, 0.0, 100.0, 60.0, "a0"), arrow];
    let mut tx = store.begin();
    let r = insert_elements(&mut tx, &src, &BTreeMap::new(), &PasteOpts::default(), &mut SeqIds::new()).unwrap();
    tx.commit().unwrap();
    let ElementKind::Arrow(d) = &store.get(&r.created[1]).unwrap().kind else { panic!() };
    assert_eq!(d.start_binding.as_ref().map(|b| &b.target), Some(&r.created[0]));
    assert!(d.end_binding.is_none());
}

#[test]
fn pictures_are_stored_once_by_content() {
    let (fid, entry) = FileEntry::new("image/png", vec![1u8, 2, 3]);
    let files = BTreeMap::from([(fid.clone(), entry)]);
    let img = |n: &str, z: &str| {
        Element::new(
            id(n),
            ElementKind::Image(ImageData {
                file_id: Some(fid.clone()),
                natural_width: 4,
                natural_height: 4,
                crop: None,
            }),
            ZIndex::parse(z).unwrap(),
        )
        .with_bounds(0.0, 0.0, 4.0, 4.0)
    };
    let mut store = MemoryStore::new();
    let mut tx = store.begin();
    insert_elements(&mut tx, &[img("i1", "a0"), img("i2", "a1")], &files, &PasteOpts::default(), &mut SeqIds::new())
        .unwrap();
    tx.commit().unwrap();
    assert_eq!(store.files().count(), 1);
}

#[test]
fn refusals_leave_the_transaction_untouched() {
    let mut store = MemoryStore::new();
    let mut tx = store.begin();
    let ids = &mut SeqIds::new();
    let none = BTreeMap::new();
    assert!(matches!(insert_elements(&mut tx, &[], &none, &PasteOpts::default(), ids), Err(PasteError::Empty)));
    let mut gone = rect("x", 0.0, 0.0, 1.0, 1.0, "a0");
    gone.deleted = true;
    assert!(matches!(insert_elements(&mut tx, &[gone], &none, &PasteOpts::default(), ids), Err(PasteError::Empty)));
    let src = [rect("x", 0.0, 0.0, 1.0, 1.0, "a0")];
    assert!(matches!(
        insert_elements(&mut tx, &src, &none, &centred((f64::NAN, 0.0)), ids),
        Err(PasteError::NotFinite)
    ));
    assert!(matches!(insert_elements(&mut tx, &src, &none, &centred((1e300, 0.0)), ids), Err(PasteError::NotFinite)));
    let mut bad = rect("y", 0.0, 0.0, 1.0, 1.0, "a0");
    bad.transform.w = f64::NAN;
    assert!(matches!(
        insert_elements(&mut tx, &[bad], &none, &PasteOpts::default(), ids),
        Err(PasteError::Copy(CopyError::Invalid { .. }))
    ));
    assert_eq!(tx.len_all(), 0);
}

#[test]
fn text_is_measured_centred_and_limited() {
    let mut store = MemoryStore::new();
    let mut tx = store.begin();
    let m = MonospaceMeasure::default();
    let font = TextStyle::default();
    let new =
        insert_text(&mut tx, &m, &mut SeqIds::new(), "abcd", font.clone(), Style::default(), Point::new(100.0, 100.0))
            .unwrap();
    let e = tx.get(&new).unwrap();
    assert_eq!((e.transform.x + e.transform.w / 2.0, e.transform.y + e.transform.h / 2.0), (100.0, 100.0));
    assert!(e.transform.w > 0.0 && e.transform.h > 0.0);
    let ids = &mut SeqIds::new();
    assert!(matches!(
        insert_text(&mut tx, &m, ids, "  \n", font.clone(), Style::default(), Point::ORIGIN),
        Err(PasteError::Text(CreateError::EmptyText))
    ));
    let long = "x".repeat(1_000_001);
    assert!(matches!(
        insert_text(&mut tx, &m, ids, &long, font, Style::default(), Point::ORIGIN),
        Err(PasteError::Text(CreateError::TextTooLong { .. }))
    ));
}

#[test]
fn pasted_and_created_free_text_are_built_by_one_function() {
    use drafftink_edit::create::{CreateError, TextPlace, TextSpec, text_element};
    let m = MonospaceMeasure::default();
    let font = TextStyle::default();
    let mut store = MemoryStore::new();
    let mut tx = store.begin();
    let pasted =
        insert_text(&mut tx, &m, &mut SeqIds::new(), "abcd", font.clone(), Style::default(), Point::new(50.0, 30.0))
            .unwrap();
    let pasted = tx.get(&pasted).unwrap().clone();
    // The same text, built by the creation code with the same centre, is the same element.
    let spec = TextSpec { font: font.clone(), place: TextPlace::Center(Point::new(50.0, 30.0)), width: None };
    let mut created =
        text_element(&m, pasted.id.clone(), pasted.index.clone(), "abcd", spec, pasted.style.clone()).unwrap();
    created.version = pasted.version;
    assert_eq!(created, pasted);
    // The rules of the single builder apply to creation as well.
    let long = "x".repeat(1_000_001);
    let at = TextPlace::TopLeft(Point::ORIGIN);
    let spec = || TextSpec { font: font.clone(), place: at, width: None };
    let make =
        |content: &str, spec| text_element(&m, id("t1"), ZIndex::parse("a0").unwrap(), content, spec, Style::default());
    assert!(matches!(make(&long, spec()), Err(CreateError::TextTooLong { .. })));
    let far = TextSpec { font: font.clone(), place: TextPlace::TopLeft(Point::new(f64::NAN, 0.0)), width: None };
    assert!(matches!(make("abcd", far), Err(CreateError::NotFinite)));
}
