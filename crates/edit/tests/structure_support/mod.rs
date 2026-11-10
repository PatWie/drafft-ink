#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]
//! Builders shared by the `structure_*` integration tests.

use drafftink_scene::*;
use kurbo::Point;

pub fn id(s: &str) -> ElementId {
    ElementId::new(s).unwrap()
}

pub fn gid(s: &str) -> GroupId {
    GroupId::new(s).unwrap()
}

pub fn ids(names: &[&str]) -> Vec<ElementId> {
    names.iter().map(|n| id(n)).collect()
}

fn placeholder() -> ZIndex {
    ZIndex::first()
}

pub fn rect(name: &str, x: f64, y: f64, w: f64, h: f64) -> Element {
    Element::new(id(name), ElementKind::Rectangle, placeholder()).with_bounds(x, y, w, h)
}

pub fn ellipse(name: &str, x: f64, y: f64, w: f64, h: f64) -> Element {
    Element::new(id(name), ElementKind::Ellipse, placeholder()).with_bounds(x, y, w, h)
}

pub fn frame(name: &str, x: f64, y: f64, w: f64, h: f64) -> Element {
    Element::new(id(name), ElementKind::Frame(FrameData::default()), placeholder()).with_bounds(x, y, w, h)
}

pub fn text(name: &str, x: f64, y: f64, w: f64, h: f64, content: &str) -> Element {
    Element::new(id(name), ElementKind::Text(TextData::new(content)), placeholder()).with_bounds(x, y, w, h)
}

pub fn label(name: &str, container: &str, x: f64, y: f64, w: f64, h: f64) -> Element {
    let mut t = TextData::new("label");
    t.container_id = Some(id(container));
    Element::new(id(name), ElementKind::Text(t), placeholder()).with_bounds(x, y, w, h)
}

pub fn image(name: &str, x: f64, y: f64, w: f64, h: f64) -> Element {
    let d = ImageData { file_id: None, natural_width: 100, natural_height: 50, crop: None };
    Element::new(id(name), ElementKind::Image(d), placeholder()).with_bounds(x, y, w, h)
}

pub fn line(name: &str, pts: &[(f64, f64)]) -> Element {
    let pts: Vec<Point> = pts.iter().map(|(x, y)| Point::new(*x, *y)).collect();
    Element::linear(id(name), Kind::Line, placeholder(), &pts).unwrap()
}

pub fn arrow(name: &str, pts: &[(f64, f64)]) -> Element {
    let pts: Vec<Point> = pts.iter().map(|(x, y)| Point::new(*x, *y)).collect();
    Element::linear(id(name), Kind::Arrow, placeholder(), &pts).unwrap()
}

pub fn freehand(name: &str, pts: &[(f64, f64)]) -> Element {
    let pts: Vec<Point> = pts.iter().map(|(x, y)| Point::new(*x, *y)).collect();
    let mut e = Element::new(id(name), ElementKind::Freehand(FreehandData::new(pts)), placeholder());
    assert!(e.refit_points());
    e
}

pub fn bind(e: &mut Element, end_is_start: bool, target: &str, fixed: [f64; 2]) {
    if let ElementKind::Arrow(d) | ElementKind::Line(d) = &mut e.kind {
        let b = Binding { target: id(target), mode: BindMode::Orbit, fixed_point: fixed };
        if end_is_start {
            d.start_binding = Some(b);
        } else {
            d.end_binding = Some(b);
        }
    }
}

pub trait With {
    fn grouped(self, groups: &[&str]) -> Self;
    fn in_frame(self, f: &str) -> Self;
    fn angle(self, a: f64) -> Self;
    fn locked(self) -> Self;
}

impl With for Element {
    fn grouped(mut self, groups: &[&str]) -> Self {
        self.group_ids = groups.iter().map(|g| gid(g)).collect();
        self
    }
    fn in_frame(mut self, f: &str) -> Self {
        self.frame_id = Some(id(f));
        self
    }
    fn angle(mut self, a: f64) -> Self {
        self.transform.angle = a;
        self
    }
    fn locked(mut self) -> Self {
        self.locked = true;
        self
    }
}

/// A store holding the elements in the given z-order (back to front).
pub fn scene(elements: Vec<Element>) -> MemoryStore {
    let keys = ZIndex::rebalance(elements.len()).unwrap();
    let mut store = MemoryStore::new();
    let mut tx = store.begin();
    for (mut e, k) in elements.into_iter().zip(keys) {
        e.index = k;
        tx.insert(e).unwrap();
    }
    tx.commit().unwrap();
    store
}

/// Runs `f` in a transaction and commits it, whatever `f` returns.
pub fn run<R>(store: &mut MemoryStore, f: impl for<'a> FnOnce(&mut <MemoryStore as SceneStore>::Tx<'a>) -> R) -> R {
    let mut tx = store.begin();
    let r = f(&mut tx);
    tx.commit().unwrap();
    r
}

/// Live element ids, back to front.
pub fn order(store: &MemoryStore) -> Vec<String> {
    store.iter().map(|e| e.id.to_string()).collect()
}

/// All element ids including tombstones, back to front.
pub fn order_all(store: &MemoryStore) -> Vec<String> {
    store.iter_all().map(|e| e.id.to_string()).collect()
}

pub fn get<'a>(store: &'a MemoryStore, name: &str) -> &'a Element {
    store.get(&id(name)).unwrap()
}

pub fn stack(store: &MemoryStore, name: &str) -> Vec<String> {
    get(store, name).group_ids.iter().map(|g| g.to_string()).collect()
}

pub fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

pub fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_owned()).collect()
}

/// The scene is valid: no error-level issue.
pub fn assert_valid(store: &MemoryStore) {
    let report = validate_scene(store);
    assert!(!report.has_errors(), "scene has errors: {:?}", report.errors().collect::<Vec<_>>());
}
