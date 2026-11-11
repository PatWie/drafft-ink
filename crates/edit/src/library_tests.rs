use drafftink_scene::{
    BindMode, Binding, Document, Element, ElementId, ElementKind, GroupId, LinearData, MemoryStore, SceneRead,
    SceneStore, SceneTx, SeqIds, TextData, ZIndex,
};
use kurbo::Point;

use super::*;
use crate::target::TargetError;

fn id(s: &str) -> ElementId {
    ElementId::new(s).unwrap()
}

fn gid(s: &str) -> GroupId {
    GroupId::new(s).unwrap()
}

fn rect(name: &str, x: f64, y: f64) -> Element {
    Element::new(id(name), ElementKind::Rectangle, ZIndex::first()).with_bounds(x, y, 100.0, 50.0)
}

/// A fragment with a group of two rectangles, a label on one of them and an arrow between them.
fn fragment(offset: f64) -> Document {
    let mut doc = Document::new();
    let mut idx = ZIndex::first();
    let mut put = |mut e: Element| {
        e.index = idx.clone();
        idx = ZIndex::between(Some(&idx), None).unwrap();
        doc.elements.push(e);
    };
    let mut a = rect("a", offset, offset);
    a.group_ids.push(gid("g1"));
    a.name = Some("first".into());
    let mut b = rect("b", offset + 200.0, offset);
    b.group_ids.push(gid("g1"));
    let mut t = TextData::new("hi");
    t.container_id = Some(id("a"));
    t.label_position = Some(0.5);
    let label = Element::new(id("t"), ElementKind::Text(t), ZIndex::first()).with_bounds(
        offset + 10.0,
        offset + 10.0,
        30.0,
        12.0,
    );
    let mut d = LinearData::new(vec![Point::new(0.0, 0.0), Point::new(100.0, 0.0)]);
    d.start_binding = Some(Binding { target: id("a"), mode: BindMode::Orbit, fixed_point: [1.0, 0.5] });
    d.end_binding = Some(Binding { target: id("b"), mode: BindMode::Orbit, fixed_point: [0.0, 0.5] });
    d.end_binding.as_mut().unwrap().target = id("outside");
    let arrow = Element::new(id("ar"), ElementKind::Arrow(d), ZIndex::first()).with_bounds(
        offset + 100.0,
        offset + 25.0,
        100.0,
        0.0,
    );
    put(a);
    put(b);
    put(label);
    put(arrow);
    doc
}

fn scene() -> MemoryStore {
    MemoryStore::new()
}

#[test]
fn insert_gives_new_ids_groups_and_follows_references() {
    let mut store = scene();
    let mut ids = SeqIds::new();
    let mut tx = store.begin();
    let report = insert_fragment(&mut tx, &fragment(0.0), Placement::AsIs, &mut ids).unwrap();
    tx.commit().unwrap();
    assert_eq!(report.inserted.len(), 4);
    assert!(report.inserted.iter().all(|i| !["a", "b", "t", "ar"].contains(&i.as_str())));
    let all: Vec<&Element> = report.inserted.iter().map(|i| store.get(i).unwrap()).collect();
    assert!(all.iter().all(|e| e.name.is_none()), "names address one element and are dropped");
    // The two group members share one new group id.
    assert_eq!(all[0].group_ids, all[1].group_ids);
    assert_eq!(report.outer_groups, vec![all[0].group_ids[0].clone()]);
    // The label follows its container.
    let ElementKind::Text(t) = &all[2].kind else { panic!("text") };
    assert_eq!(t.container_id.as_ref(), Some(&all[0].id));
    // The arrow is bound to the copy; its end that pointed outside the fragment is free.
    let ElementKind::Arrow(d) = &all[3].kind else { panic!("arrow") };
    assert_eq!(d.start_binding.as_ref().map(|b| &b.target), Some(&all[0].id));
    assert!(d.end_binding.is_none());
    // Back to front order is kept, on top of the scene.
    let z: Vec<_> = all.iter().map(|e| e.index.clone()).collect();
    assert!(z.windows(2).all(|w| w[0] < w[1]));
}

#[test]
fn insert_on_top_of_existing_elements_and_twice_gives_distinct_ids() {
    let mut store = scene();
    let mut ids = SeqIds::new();
    let mut tx = store.begin();
    tx.insert(rect("under", 0.0, 0.0)).unwrap();
    let one = insert_fragment(&mut tx, &fragment(0.0), Placement::AsIs, &mut ids).unwrap();
    let two = insert_fragment(&mut tx, &fragment(0.0), Placement::AsIs, &mut ids).unwrap();
    tx.commit().unwrap();
    assert_eq!(store.len(), 9);
    assert!(one.inserted.iter().all(|i| !two.inserted.contains(i)));
    assert!(store.get(&one.inserted[0]).unwrap().index > store.get(&id("under")).unwrap().index);
    assert_ne!(store.get(&one.inserted[0]).unwrap().group_ids, store.get(&two.inserted[0]).unwrap().group_ids);
}

#[test]
fn centre_places_the_middle_of_the_bounds_on_the_point() {
    let mut store = scene();
    let mut ids = SeqIds::new();
    let frag = fragment(0.0);
    let b = bounds(&frag).unwrap();
    let mut tx = store.begin();
    let r = insert_fragment(&mut tx, &frag, Placement::Centre(Point::new(1000.0, -500.0)), &mut ids).unwrap();
    tx.commit().unwrap();
    let got = r.inserted.iter().map(|i| element_bounds(store.get(i).unwrap())).reduce(|a, b| a.union(b)).unwrap();
    assert!((got.center().x - 1000.0).abs() < 1e-9 && (got.center().y + 500.0).abs() < 1e-9, "{got:?} from {b:?}");
}

#[test]
fn empty_fragment_and_bad_position_write_nothing() {
    let mut store = scene();
    let mut ids = SeqIds::new();
    let mut tx = store.begin();
    assert!(matches!(insert_fragment(&mut tx, &Document::new(), Placement::AsIs, &mut ids), Err(LibraryError::Empty)));
    let r = insert_fragment(&mut tx, &fragment(0.0), Placement::Centre(Point::new(f64::NAN, 0.0)), &mut ids);
    assert!(matches!(r, Err(LibraryError::NotFinite)));
    tx.commit().unwrap();
    assert!(store.is_empty());
}

#[test]
fn members_adds_labels_and_frame_children_and_refuses_nothing() {
    let mut store = scene();
    let mut tx = store.begin();
    let mut frame = Element::new(id("f"), ElementKind::Frame(Default::default()), ZIndex::first());
    frame = frame.with_bounds(0.0, 0.0, 500.0, 500.0);
    tx.insert(frame).unwrap();
    for e in fragment(10.0).elements {
        let mut e = e;
        e.index = tx.next_top_index().unwrap();
        e.frame_id = Some(id("f"));
        tx.insert(e).unwrap();
    }
    tx.commit().unwrap();
    let m = members(&store, &[id("a")]).unwrap();
    assert_eq!(m, vec![id("a"), id("t")], "a container brings its label");
    let m = members(&store, &[id("f")]).unwrap();
    assert_eq!(m.len(), 5, "a frame brings its children");
    assert!(matches!(members(&store, &[]), Err(LibraryError::NoTarget)));
    assert!(matches!(members(&store, &[id("nope")]), Err(LibraryError::Target(TargetError::NotFound(_)))));
}

#[test]
fn normalize_moves_to_the_origin_and_drops_tombstones_and_names() {
    let mut doc = fragment(300.0);
    doc.elements[1].deleted = true;
    assert!(normalize(&mut doc));
    assert_eq!(doc.elements.len(), 3);
    let b = bounds(&doc).unwrap();
    assert!(b.x0.abs() < 1e-9 && b.y0.abs() < 1e-9);
    assert!(doc.elements.iter().all(|e| e.name.is_none()));
    assert!(!normalize(&mut Document::new()));
}

#[test]
fn content_hash_ignores_ids_names_seeds_and_position_but_not_content() {
    let mut a = fragment(0.0);
    let mut b = fragment(500.0);
    normalize(&mut a);
    normalize(&mut b);
    // Different ids, names and seeds.
    let rename = |d: &mut Document| {
        for e in &mut d.elements {
            let new = format!("x{}", e.id.as_str());
            for g in e.group_ids.iter_mut() {
                *g = gid("other");
            }
            e.style.seed = 99;
            let _ = new;
        }
    };
    rename(&mut b);
    assert_eq!(content_hash(&a), content_hash(&b));
    let mut c = a.clone();
    c.elements[0].transform.w += 1.0;
    assert_ne!(content_hash(&a), content_hash(&c));
    let mut d = a.clone();
    d.elements.pop();
    assert_ne!(content_hash(&a), content_hash(&d));
    assert_eq!(content_hash(&Document::new()), content_hash(&Document::new()));
}
