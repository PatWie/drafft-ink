//! Shared helpers for the integration tests.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(dead_code)]

use drafftink_canvas::geom::kurbo::{Point, Size};
use drafftink_canvas::scene::{
    Color, Element, ElementId, ElementKind, FrameData, Kind, MemoryStore, SceneRead, SceneStore, SceneTx, TextData,
    ZIndex,
};
use drafftink_canvas::{Canvas, Viewport};

pub fn id(s: &str) -> ElementId {
    ElementId::new(s).unwrap()
}

pub fn fill(mut e: Element) -> Element {
    e.style.fill_color = Some(Color::rgb(200, 100, 50));
    e
}

pub fn rect(name: &str, x: f64, y: f64, w: f64, h: f64) -> Element {
    Element::new(id(name), ElementKind::Rectangle, ZIndex::first()).with_bounds(x, y, w, h)
}

pub fn ellipse(name: &str, x: f64, y: f64, w: f64, h: f64) -> Element {
    Element::new(id(name), ElementKind::Ellipse, ZIndex::first()).with_bounds(x, y, w, h)
}

pub fn diamond(name: &str, x: f64, y: f64, w: f64, h: f64) -> Element {
    Element::new(id(name), ElementKind::Diamond, ZIndex::first()).with_bounds(x, y, w, h)
}

pub fn text(name: &str, x: f64, y: f64, w: f64, h: f64) -> Element {
    Element::new(id(name), ElementKind::Text(TextData::new("hello")), ZIndex::first()).with_bounds(x, y, w, h)
}

pub fn frame(name: &str, x: f64, y: f64, w: f64, h: f64) -> Element {
    Element::new(id(name), ElementKind::Frame(FrameData::default()), ZIndex::first()).with_bounds(x, y, w, h)
}

pub fn line(name: &str, pts: &[(f64, f64)]) -> Element {
    let pts: Vec<Point> = pts.iter().map(|(x, y)| Point::new(*x, *y)).collect();
    Element::linear(id(name), Kind::Line, ZIndex::first(), &pts).unwrap()
}

/// A viewport where screen pixels equal world units: 800x600, camera centred on (400, 300), zoom 1.
pub fn identity_viewport() -> Viewport {
    let mut v = Viewport::new(Size::new(800.0, 600.0), 1.0);
    v.set_camera(drafftink_canvas::Camera { scroll: Point::new(400.0, 300.0), zoom: 1.0 });
    v
}

/// Inserts elements in order, each above the previous one.
pub fn insert_all<S: SceneStore>(canvas: &mut Canvas<S>, elements: Vec<Element>) {
    canvas
        .transact(|tx| {
            for mut e in elements {
                e.index = tx.next_top_index().unwrap();
                tx.insert(e)?;
            }
            Ok(())
        })
        .unwrap();
}

pub fn canvas_with(elements: Vec<Element>) -> Canvas<MemoryStore> {
    let mut c = Canvas::new(MemoryStore::new(), identity_viewport());
    insert_all(&mut c, elements);
    c
}

pub fn live_ids<R: SceneRead>(s: &R) -> Vec<ElementId> {
    s.iter().map(|e| e.id.clone()).collect()
}
