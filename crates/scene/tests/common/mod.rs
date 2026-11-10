#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Shared builders for the integration tests.
#![allow(dead_code)]

use drafftink_scene::*;
use kurbo::Point;

pub fn id(s: &str) -> ElementId {
    ElementId::new(s).unwrap()
}

pub fn gid(s: &str) -> GroupId {
    GroupId::new(s).unwrap()
}

pub fn zs(n: usize) -> Vec<ZIndex> {
    ZIndex::rebalance(n).unwrap()
}

pub fn rect(name: &str, z: &ZIndex) -> Element {
    Element::new(id(name), ElementKind::Rectangle, z.clone()).with_bounds(10.0, 20.0, 100.0, 50.0)
}

pub fn arrow(name: &str, z: &ZIndex, pts: &[(f64, f64)]) -> Element {
    let pts: Vec<Point> = pts.iter().map(|(x, y)| Point::new(*x, *y)).collect();
    Element::linear(id(name), Kind::Arrow, z.clone(), &pts).unwrap()
}

pub fn text(name: &str, z: &ZIndex, content: &str) -> Element {
    Element::new(id(name), ElementKind::Text(TextData::new(content)), z.clone()).with_bounds(0.0, 0.0, 80.0, 25.0)
}

pub fn frame(name: &str, z: &ZIndex) -> Element {
    Element::new(id(name), ElementKind::Frame(FrameData::default()), z.clone()).with_bounds(0.0, 0.0, 300.0, 300.0)
}

pub fn set_end_binding(e: &mut Element, target: &str) {
    if let ElementKind::Arrow(d) | ElementKind::Line(d) = &mut e.kind {
        d.end_binding = Some(Binding { target: id(target), mode: BindMode::Orbit, fixed_point: [0.5, 0.5] });
    }
}

pub fn doc_of(elements: Vec<Element>) -> Document {
    Document { elements, ..Document::new() }
}

pub fn has_error(report: &Report, pred: impl Fn(&DocError) -> bool) -> bool {
    report.errors().any(|i| pred(&i.error))
}

pub fn has_warning(report: &Report, pred: impl Fn(&DocError) -> bool) -> bool {
    report.warnings().any(|i| pred(&i.error))
}
