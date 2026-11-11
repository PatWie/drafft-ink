#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Builders shared by the integration tests and the benches: a deterministic mixed scene generator.
#![allow(dead_code)]

use drafftink_render::{math_spec, text_spec};
use drafftink_scene::*;
use drafftink_typeset::Typesetter;
use kurbo::Point;

pub struct Rng(pub u64);

impl Rng {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }
}

pub fn id(s: &str) -> ElementId {
    ElementId::new(s).unwrap()
}

pub fn rect_el(name: &str, z: &ZIndex, x: f64, y: f64, w: f64, h: f64) -> Element {
    Element::new(id(name), ElementKind::Rectangle, z.clone()).with_bounds(x, y, w, h)
}

pub fn ellipse_el(name: &str, z: &ZIndex, x: f64, y: f64, w: f64, h: f64) -> Element {
    Element::new(id(name), ElementKind::Ellipse, z.clone()).with_bounds(x, y, w, h)
}

pub fn line_el(name: &str, z: &ZIndex, kind: LinearKind, pts: &[(f64, f64)]) -> Element {
    let pts: Vec<Point> = pts.iter().map(|(x, y)| Point::new(*x, *y)).collect();
    Element::linear(id(name), kind, z.clone(), &pts).unwrap()
}

pub fn freehand_el(name: &str, z: &ZIndex, x: f64, y: f64, n: usize, rng: &mut Rng, pressure: bool) -> Element {
    let pts: Vec<Point> = (0..n)
        .map(|i| Point::new(i as f64 * 3.0, 12.0 * (i as f64 * 0.4).sin() + rng.range(-1.0, 1.0) + 15.0))
        .collect();
    let mut data = FreehandData::new(pts);
    if pressure {
        data.pressures = (0..n).map(|i| 0.4 + 0.6 * ((i as f64 * 0.3).sin().abs())).collect();
        data.pressure_mode = PressureMode::Device;
    }
    let mut e = Element::new(id(name), ElementKind::Freehand(data), z.clone());
    e.transform = Transform::new(x, y, 0.0, 0.0);
    e.refit_points().unwrap();
    e
}

/// A text element whose box is measured by the typesetter, as the editor does.
pub fn text_el(ts: &mut Typesetter, name: &str, z: &ZIndex, x: f64, y: f64, content: &str) -> Element {
    let mut e = Element::new(id(name), ElementKind::Text(TextData::new(content)), z.clone());
    e.transform = Transform::new(x, y, 0.0, 0.0);
    measure_text(ts, &mut e);
    e
}

pub fn measure_text(ts: &mut Typesetter, e: &mut Element) {
    if let ElementKind::Text(t) = &e.kind {
        let size = ts.measure(&text_spec(t, e.style.stroke_color, e.transform.w));
        e.transform.w = size.width;
        e.transform.h = size.height;
    }
}

/// A math element sized from its layout.
pub fn math_el(ts: &mut Typesetter, name: &str, z: &ZIndex, x: f64, y: f64, latex: &str) -> Element {
    let m = MathData::new(latex);
    let size = ts.math(&math_spec(&m)).map(|l| l.size()).unwrap_or(kurbo::Size::new(40.0, 24.0));
    let mut e = Element::new(id(name), ElementKind::Math(m), z.clone());
    e.transform = Transform::new(x, y, size.width, size.height);
    e
}

pub fn frame_el(name: &str, z: &ZIndex, x: f64, y: f64, w: f64, h: f64) -> Element {
    Element::new(id(name), ElementKind::Frame(FrameData::default()), z.clone()).with_bounds(x, y, w, h)
}

/// A mixed scene of `n` elements on a square of side `sqrt(n) * 140`, like the old app's large fixtures:
/// rectangles, ellipses, diamonds, lines, arrows, freehand strokes, text and math.
pub fn mixed_elements(n: usize, seed: u64, ts: &mut Typesetter) -> Vec<Element> {
    let zs = ZIndex::rebalance(n.max(1)).unwrap();
    let mut rng = Rng(seed);
    let side = ((n as f64).sqrt().ceil() as usize).max(1);
    let words = ["alpha", "beta", "gamma", "delta", "Hello world", "todo", "two\nlines"];
    let formulas = ["x^2+y^2=r^2", "\\frac{a}{b}", "\\sqrt{x}", "\\sum_{i=0}^n i", "e^{i\\pi}+1=0"];
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let (cx, cy) = ((i % side) as f64 * 140.0, (i / side) as f64 * 140.0);
        let (x, y) = (cx + rng.range(0.0, 20.0), cy + rng.range(0.0, 20.0));
        let name = format!("e{i}");
        let z = &zs[i];
        let mut e = match i % 8 {
            0 => {
                let mut e = rect_el(&name, z, x, y, rng.range(40.0, 110.0), rng.range(30.0, 90.0));
                if i % 16 == 0 {
                    e.style.fill_color = Some(Color::rgb(0x99, 0xe9, 0xf2));
                    e.style.fill_pattern = FillPattern::Hachure;
                }
                e
            }
            1 => ellipse_el(&name, z, x, y, rng.range(40.0, 110.0), rng.range(30.0, 90.0)),
            2 => line_el(&name, z, LinearKind::Line, &[(x, y), (x + 40.0, y + 30.0), (x + 90.0, y + 10.0)]),
            3 => {
                let mut e =
                    line_el(&name, z, LinearKind::Arrow, &[(x, y), (x + 60.0, y + 40.0), (x + 100.0, y + 40.0)]);
                if let ElementKind::Arrow(d) = &mut e.kind {
                    d.end_head = Head::Arrow;
                }
                e
            }
            4 => freehand_el(&name, z, x, y, 24, &mut rng, i % 16 == 4),
            5 => text_el(ts, &name, z, x, y, words[i % words.len()]),
            6 => math_el(ts, &name, z, x, y, formulas[(i / 8) % formulas.len()]),
            _ => {
                let mut e = Element::new(id(&name), ElementKind::Diamond, z.clone()).with_bounds(x, y, 80.0, 60.0);
                e.style.fill_color = Some(Color::rgb(0xff, 0xec, 0x99));
                e
            }
        };
        e.style.seed = (i as u32).wrapping_mul(2_654_435_761);
        out.push(e);
    }
    out
}

pub fn store_of(elements: Vec<Element>) -> MemoryStore {
    let mut store = MemoryStore::new();
    let mut tx = store.begin();
    for e in elements {
        tx.insert(e).unwrap();
    }
    tx.commit().unwrap();
    store
}

pub fn update(store: &mut MemoryStore, name: &str, f: impl FnOnce(&mut Element)) -> ChangeSet {
    let mut tx = store.begin();
    tx.update(&id(name), f).unwrap();
    tx.commit().unwrap()
}
