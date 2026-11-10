#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Helpers shared by the integration tests.

use drafftink_doc::{LoroConfig, LoroStore};
use drafftink_scene::*;
use kurbo::Point;

pub fn eid(s: &str) -> ElementId {
    ElementId::new(s).unwrap()
}

pub fn gid(s: &str) -> GroupId {
    GroupId::new(s).unwrap()
}

pub fn peer(n: u64) -> LoroStore {
    LoroStore::with_config(LoroConfig { peer_id: Some(n), ..LoroConfig::default() })
}

pub fn rect(id: &str, z: &ZIndex) -> Element {
    let mut e = Element::new(eid(id), ElementKind::Rectangle, z.clone());
    e.transform = Transform::new(10.0, 20.0, 100.0, 50.0);
    e
}

pub fn text(id: &str, z: &ZIndex, content: &str) -> Element {
    let mut e = Element::new(eid(id), ElementKind::Text(TextData::new(content)), z.clone());
    e.transform = Transform::new(0.0, 0.0, 80.0, 20.0);
    e
}

pub fn arrow(id: &str, z: &ZIndex) -> Element {
    Element::linear(eid(id), Kind::Arrow, z.clone(), &[Point::new(0.0, 0.0), Point::new(50.0, 20.0)]).unwrap()
}

pub fn zs(n: usize) -> Vec<ZIndex> {
    ZIndex::rebalance(n).unwrap()
}

/// Inserts the elements in one committed transaction.
pub fn add<S: SceneStore>(s: &mut S, elements: Vec<Element>) -> ChangeSet {
    let mut tx = s.begin();
    for e in elements {
        tx.insert(e).unwrap();
    }
    tx.commit().unwrap()
}

pub fn edit<S: SceneStore>(s: &mut S, id: &str, f: impl FnOnce(&mut Element)) -> ChangeSet {
    let mut tx = s.begin();
    tx.update(&eid(id), f).unwrap();
    tx.commit().unwrap()
}

/// Sends everything `from` has and `to` lacks.
pub fn push(from: &LoroStore, to: &mut LoroStore) -> ChangeSet {
    let bytes = from.export_updates_since(&to.version_vector()).unwrap();
    to.apply_remote(&bytes).unwrap()
}

/// Two-way sync until both have everything.
pub fn sync(a: &mut LoroStore, b: &mut LoroStore) {
    push(a, b);
    push(b, a);
}

/// The scene content without the local-only `version` counters.
pub fn content(s: &impl SceneRead) -> Document {
    let mut d = snapshot(s);
    for e in &mut d.elements {
        e.version = 0;
    }
    d
}

pub fn assert_same(a: &impl SceneRead, b: &impl SceneRead) {
    let (x, y) = (content(a), content(b));
    assert_eq!(x.elements.len(), y.elements.len(), "element count");
    for (l, r) in x.elements.iter().zip(&y.elements) {
        assert_eq!(l, r);
    }
    assert_eq!(x, y);
}

/// The view must equal what a fresh reader decodes from the CRDT, and the derived index must equal a rebuild.
pub fn assert_view_matches_loro(s: &LoroStore) {
    let reopened = LoroStore::from_snapshot(&s.export_snapshot().unwrap()).unwrap();
    assert_same(s, &reopened);
    let mut fresh = DerivedIndex::new();
    fresh.rebuild(s);
    assert_eq!(s.derived(), &fresh, "derived index differs from a rebuild");
    assert_eq!(s.invalid_elements().count(), 0, "unexpected hidden elements");
}

pub fn order(s: &impl SceneRead) -> Vec<String> {
    s.iter_all().map(|e| e.id.to_string()).collect()
}

pub fn freehand(id: &str, z: &ZIndex, world_points: &[Point]) -> Element {
    let mut e = Element::new(eid(id), ElementKind::Freehand(FreehandData::new(world_points.to_vec())), z.clone());
    assert!(e.refit_points());
    e
}
pub mod legacy;
