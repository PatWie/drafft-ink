//! A deterministic scene generator: the same `(count, seed)` always gives the same document, byte for byte.
//!
//! Scenarios, integration tests and the perf benches share it, so "the 10k scene" means one thing. The scene is
//! a square grid of cells, 140 world units apart, one element per cell, in a fixed cycle of ten kinds:
//!
//! | `i % 10` | element |
//! |---|---|
//! | 0 | rectangle (every other one filled) |
//! | 1 | ellipse |
//! | 2 | diamond |
//! | 3 | line |
//! | 4 | arrow **bound** at both ends to the rectangle `i-4` and the ellipse `i-3` |
//! | 5 | freehand stroke |
//! | 6 | text |
//! | 7 | math |
//! | 8, 9 | a rectangle and an ellipse that form one **group** (`g<i/10>`) |
//!
//! Ids are `e<i>`, so a script can address element 40 as `e40`. Text and formulas get fixed boxes instead of
//! measured ones: generation needs no fonts and costs a few hundred nanoseconds per element, so 100k elements
//! take well under a second.

use drafftink_geom::kurbo::Point;
use drafftink_scene::{
    BindMode, Binding, Color, Document, Element, ElementId, ElementKind, FillPattern, FreehandData, GroupId, Head,
    LinearKind, MathData, Rng64, SplitMix64, TextData, Transform, ZIndex,
};

/// Distance between cell origins, in world units.
pub const CELL: f64 = 140.0;

/// The sizes the perf budgets and the scale scenarios use.
pub const SIZES: [usize; 3] = [1_000, 10_000, 100_000];

/// Uniform numbers from the scene crate's generator.
struct Rng(SplitMix64);

impl Rng {
    fn unit(&mut self) -> f64 {
        // 53 random bits give an exact fraction in [0, 1).
        (self.0.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }
}

/// The generator could not build a scene (the scene crate rejected an id or a z-index it builds).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("scene generation failed: {0}")]
pub struct GenError(pub String);

fn eid(i: usize) -> Result<ElementId, GenError> {
    ElementId::new(&format!("e{i}")).map_err(|e| GenError(e.to_string()))
}

/// Number of grid columns for `count` elements.
pub fn columns(count: usize) -> usize {
    (count as f64).sqrt().ceil().max(1.0) as usize
}

/// The world position of the origin of cell `i`.
pub fn cell_origin(i: usize, count: usize) -> (f64, f64) {
    let side = columns(count);
    ((i % side) as f64 * CELL, (i / side) as f64 * CELL)
}

/// A document with `count` elements. See the module docs for what they are.
pub fn generate(count: usize, seed: u64) -> Result<Document, GenError> {
    let mut rng = Rng(SplitMix64::new(seed));
    let zs = ZIndex::rebalance(count.max(1)).map_err(|e| GenError(e.to_string()))?;
    let words = ["alpha", "beta", "gamma", "delta", "Hello world", "todo", "two\nlines"];
    let formulas = ["x^2+y^2=r^2", "\\frac{a}{b}", "\\sqrt{x}", "\\sum_{i=0}^n i", "e^{i\\pi}+1=0"];
    let mut out: Vec<Element> = Vec::with_capacity(count);
    for i in 0..count {
        let (cx, cy) = cell_origin(i, count);
        let (x, y) = (cx + rng.range(0.0, 20.0), cy + rng.range(0.0, 20.0));
        let z = zs.get(i).cloned().ok_or_else(|| GenError(format!("no z-index for element {i}")))?;
        let id = eid(i)?;
        let mut e = match i % 10 {
            0 => {
                let mut e = Element::new(id.clone(), ElementKind::Rectangle, z).with_bounds(
                    x,
                    y,
                    rng.range(40.0, 100.0),
                    rng.range(30.0, 80.0),
                );
                if i % 20 == 0 {
                    e.style.fill_color = Some(Color::rgb(0x99, 0xe9, 0xf2));
                    e.style.fill_pattern = FillPattern::Solid;
                }
                e
            }
            1 => Element::new(id.clone(), ElementKind::Ellipse, z).with_bounds(
                x,
                y,
                rng.range(40.0, 100.0),
                rng.range(30.0, 80.0),
            ),
            2 => {
                let mut e = Element::new(id.clone(), ElementKind::Diamond, z).with_bounds(x, y, 80.0, 60.0);
                e.style.fill_color = Some(Color::rgb(0xff, 0xec, 0x99));
                e
            }
            3 => line(&id, LinearKind::Line, z, &[(x, y), (x + 40.0, y + 30.0), (x + 90.0, y + 10.0)]),
            4 => bound_arrow(&id, i, z, &out),
            5 => freehand(&id, z, x, y, &mut rng),
            6 => {
                let text = words.get(i / 10 % words.len()).copied().unwrap_or("text");
                let width = 9.0 * text.lines().map(str::len).max().unwrap_or(1) as f64;
                let height = 24.0 * text.lines().count().max(1) as f64;
                let mut e = Element::new(id.clone(), ElementKind::Text(TextData::new(text)), z);
                e.transform = Transform::new(x, y, width, height);
                e
            }
            7 => {
                let latex = formulas.get(i / 10 % formulas.len()).copied().unwrap_or("x");
                let mut e = Element::new(id.clone(), ElementKind::Math(MathData::new(latex)), z);
                e.transform = Transform::new(x, y, 70.0, 30.0);
                e
            }
            8 => Element::new(id.clone(), ElementKind::Rectangle, z).with_bounds(x, y, 50.0, 40.0),
            _ => Element::new(id.clone(), ElementKind::Ellipse, z).with_bounds(x, y, 50.0, 40.0),
        };
        if matches!(i % 10, 8 | 9)
            && let Ok(g) = GroupId::new(&format!("g{}", i / 10))
        {
            e.group_ids.push(g);
        }
        // A pure function of the index, so a seed change moves shapes but never reorders hand-drawn noise.
        e.style.seed = (i as u32).wrapping_mul(2_654_435_761);
        out.push(e);
    }
    // A group needs two members: a count that ends between the pair leaves one, which is no group.
    if count % 10 == 9
        && let Some(last) = out.last_mut()
    {
        last.group_ids.clear();
    }
    let mut doc = Document::new();
    doc.name = format!("generated-{count}-{seed}");
    doc.elements = out;
    Ok(doc)
}

fn line(id: &ElementId, kind: LinearKind, z: ZIndex, pts: &[(f64, f64)]) -> Element {
    let pts: Vec<Point> = pts.iter().map(|(x, y)| Point::new(*x, *y)).collect();
    Element::linear(id.clone(), kind, z.clone(), &pts)
        .unwrap_or_else(|| Element::new(id.clone(), ElementKind::Rectangle, z).with_bounds(0.0, 0.0, 1.0, 1.0))
}

/// An arrow from the right edge of element `i-4` (a rectangle) to the left edge of element `i-3` (an ellipse).
fn bound_arrow(id: &ElementId, i: usize, z: ZIndex, made: &[Element]) -> Element {
    let (a, b) = (made.get(i.wrapping_sub(4)), made.get(i.wrapping_sub(3)));
    let (Some(a), Some(b)) = (a, b) else {
        return line(id, LinearKind::Arrow, z, &[(0.0, 0.0), (60.0, 40.0)]);
    };
    let start = Point::new(a.transform.x + a.transform.w, a.transform.y + a.transform.h / 2.0);
    let end = Point::new(b.transform.x, b.transform.y + b.transform.h / 2.0);
    let mut e = line(id, LinearKind::Arrow, z, &[(start.x, start.y), (end.x, end.y)]);
    if let ElementKind::Arrow(d) = &mut e.kind {
        d.start_binding = Some(Binding { target: a.id.clone(), mode: BindMode::Orbit, fixed_point: [1.0, 0.5] });
        d.end_binding = Some(Binding { target: b.id.clone(), mode: BindMode::Orbit, fixed_point: [0.0, 0.5] });
        d.end_head = Head::Arrow;
    }
    e
}

fn freehand(id: &ElementId, z: ZIndex, x: f64, y: f64, rng: &mut Rng) -> Element {
    let pts: Vec<Point> = (0..24)
        .map(|k| Point::new(f64::from(k) * 3.0, 12.0 * (f64::from(k) * 0.4).sin() + rng.range(-1.0, 1.0) + 15.0))
        .collect();
    let mut e = Element::new(id.clone(), ElementKind::Freehand(FreehandData::new(pts)), z);
    e.transform = Transform::new(x, y, 0.0, 0.0);
    // The generated points are finite, so fitting cannot fail.
    let _ = e.refit_points();
    e
}

/// How many elements of each kind a generated scene of `count` has, by the cycle in the module docs. Tests
/// compare it with what the engine reports after loading.
pub fn expected_kind_counts(count: usize) -> std::collections::BTreeMap<&'static str, usize> {
    let mut m = std::collections::BTreeMap::new();
    for i in 0..count {
        let kind = match i % 10 {
            0 | 8 => "rectangle",
            1 | 9 => "ellipse",
            2 => "diamond",
            3 => "line",
            4 => "arrow",
            5 => "freehand",
            6 => "text",
            _ => "math",
        };
        *m.entry(kind).or_insert(0) += 1;
    }
    m
}

#[cfg(test)]
mod tests {
    use drafftink_scene::SceneRead;

    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_document_and_another_seed_a_different_one() {
        let a = generate(500, 7).unwrap();
        let b = generate(500, 7).unwrap();
        assert_eq!(a.to_json().unwrap(), b.to_json().unwrap());
        let c = generate(500, 8).unwrap();
        assert_ne!(a.to_json().unwrap(), c.to_json().unwrap());
    }

    #[test]
    fn small_and_medium_scenes_are_valid_documents() {
        for n in [0, 1, 4, 5, 9, 10, 11, 19, 100, 1_000, 10_000] {
            let doc = generate(n, 1).unwrap();
            assert_eq!(doc.elements.len(), n);
            let report = doc.validate();
            assert!(!report.has_errors(), "{n} elements: {report:?}");
        }
    }

    #[test]
    fn kinds_bindings_and_groups_are_present_in_the_documented_mix() {
        let doc = generate(1_000, 1).unwrap();
        let mut counts = std::collections::BTreeMap::new();
        let (mut bound, mut grouped) = (0, 0);
        for e in &doc.elements {
            *counts.entry(format!("{:?}", e.tag()).to_lowercase()).or_insert(0usize) += 1;
            if let ElementKind::Arrow(d) = &e.kind
                && d.start_binding.is_some()
                && d.end_binding.is_some()
            {
                bound += 1;
            }
            if !e.group_ids.is_empty() {
                grouped += 1;
            }
        }
        let expected = expected_kind_counts(1_000);
        for (kind, n) in &expected {
            assert_eq!(counts.get(*kind), Some(n), "{kind}");
        }
        assert_eq!(bound, 100, "every arrow has both ends bound");
        assert_eq!(grouped, 200);
    }

    #[test]
    fn a_hundred_thousand_elements_build_with_unique_ids_and_increasing_z() {
        let doc = generate(100_000, 3).unwrap();
        assert_eq!(doc.elements.len(), 100_000);
        let ids: std::collections::HashSet<&str> = doc.elements.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids.len(), 100_000);
        assert!(doc.elements.windows(2).all(|w| w[0].index < w[1].index));
    }

    #[test]
    fn a_generated_scene_loads_into_a_memory_store() {
        let doc = generate(2_000, 5).unwrap();
        let mut store = drafftink_scene::MemoryStore::new();
        drafftink_scene::load_document(&mut store, &doc).unwrap();
        assert_eq!(store.len(), 2_000);
    }
}
