//! Property test: after any sequence of edits, the incrementally maintained index equals a full recomputation, and
//! hit tests and queries equal a brute-force scan of the scene.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use drafftink_canvas::geom::kurbo::{Point, Rect};
use drafftink_canvas::scene::{Element, ElementId, ElementKind, LinearKind, MemoryStore, SceneRead, SceneTx, ZIndex};
use drafftink_canvas::shape::{index_bounds, shape_of};
use drafftink_canvas::{Canvas, HitOptions, MarqueeMode, hit_element, z_cmp};
use proptest::prelude::*;

#[derive(Debug, Clone)]
enum Op {
    Add { kind: u8, x: f64, y: f64, w: f64, h: f64, angle: f64, filled: bool },
    Move { i: usize, dx: f64, dy: f64 },
    Resize { i: usize, w: f64, h: f64 },
    Rotate { i: usize, angle: f64 },
    Stroke { i: usize, width: f64 },
    Delete { i: usize },
    Undelete { i: usize },
    Remove { i: usize },
    ToFront { i: usize },
    Lock { i: usize, on: bool },
    Rename { i: usize },
    EditPoints { i: usize, dx: f64, dy: f64 },
    Batch { moves: Vec<(usize, f64, f64)> },
}

fn coord() -> impl Strategy<Value = f64> {
    -500.0f64..500.0
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (0u8..6, coord(), coord(), 0.0f64..300.0, 0.0f64..300.0, -3.2f64..3.2, any::<bool>())
            .prop_map(|(kind, x, y, w, h, angle, filled)| Op::Add { kind, x, y, w, h, angle, filled }),
        3 => (any::<usize>(), -100.0f64..100.0, -100.0f64..100.0).prop_map(|(i, dx, dy)| Op::Move { i, dx, dy }),
        2 => (any::<usize>(), 0.0f64..300.0, 0.0f64..300.0).prop_map(|(i, w, h)| Op::Resize { i, w, h }),
        2 => (any::<usize>(), -3.2f64..3.2).prop_map(|(i, angle)| Op::Rotate { i, angle }),
        1 => (any::<usize>(), 0.0f64..30.0).prop_map(|(i, width)| Op::Stroke { i, width }),
        2 => any::<usize>().prop_map(|i| Op::Delete { i }),
        1 => any::<usize>().prop_map(|i| Op::Undelete { i }),
        1 => any::<usize>().prop_map(|i| Op::Remove { i }),
        1 => any::<usize>().prop_map(|i| Op::ToFront { i }),
        1 => (any::<usize>(), any::<bool>()).prop_map(|(i, on)| Op::Lock { i, on }),
        1 => any::<usize>().prop_map(|i| Op::Rename { i }),
        1 => (any::<usize>(), -50.0f64..50.0, -50.0f64..50.0).prop_map(|(i, dx, dy)| Op::EditPoints { i, dx, dy }),
        1 => proptest::collection::vec((any::<usize>(), -30.0f64..30.0, -30.0f64..30.0), 1..6)
            .prop_map(|moves| Op::Batch { moves }),
    ]
}

#[allow(clippy::too_many_arguments)]
fn make(n: usize, kind: u8, x: f64, y: f64, w: f64, h: f64, angle: f64, filled: bool) -> Element {
    let id = ElementId::new(&format!("e{n}")).unwrap();
    let z = ZIndex::first();
    let mut e = match kind {
        0 => Element::new(id, ElementKind::Rectangle, z).with_bounds(x, y, w, h),
        1 => Element::new(id, ElementKind::Ellipse, z).with_bounds(x, y, w, h),
        2 => Element::new(id, ElementKind::Diamond, z).with_bounds(x, y, w, h),
        3 => {
            Element::new(id, ElementKind::Text(drafftink_canvas::scene::TextData::new("t")), z).with_bounds(x, y, w, h)
        }
        4 => Element::linear(
            id,
            LinearKind::Arrow,
            z,
            &[Point::new(x, y), Point::new(x + w, y + h), Point::new(x + w, y - h)],
        )
        .unwrap_or_else(|| {
            Element::new(ElementId::new(&format!("e{n}")).unwrap(), ElementKind::Rectangle, ZIndex::first())
        }),
        _ => Element::linear(id, LinearKind::Line, z, &[Point::new(x, y), Point::new(x + w, y + h)]).unwrap_or_else(
            || Element::new(ElementId::new(&format!("e{n}")).unwrap(), ElementKind::Rectangle, ZIndex::first()),
        ),
    };
    e.transform.angle = angle;
    if filled {
        e = fill(e);
    }
    e
}

struct Model {
    canvas: Canvas<MemoryStore>,
    ids: Vec<ElementId>,
    counter: usize,
}

impl Model {
    fn pick(&self, i: usize) -> Option<ElementId> {
        (!self.ids.is_empty()).then(|| self.ids[i % self.ids.len()].clone())
    }

    fn apply(&mut self, op: &Op) {
        match op {
            Op::Add { kind, x, y, w, h, angle, filled } => {
                self.counter += 1;
                let e = make(self.counter, *kind, *x, *y, *w, *h, *angle, *filled);
                let id = e.id.clone();
                let r = self.canvas.transact(|tx| {
                    let mut e = e;
                    e.index = tx.next_top_index().unwrap();
                    tx.insert(e)
                });
                if r.is_ok() {
                    self.ids.push(id);
                }
            }
            Op::Move { i, dx, dy } => self.update(*i, |e| {
                e.transform.x += dx;
                e.transform.y += dy;
            }),
            Op::Resize { i, w, h } => self.update(*i, |e| {
                if e.points().is_none() {
                    e.transform.w = *w;
                    e.transform.h = *h;
                }
            }),
            Op::Rotate { i, angle } => self.update(*i, |e| e.transform.angle = *angle),
            Op::Stroke { i, width } => self.update(*i, |e| e.style.stroke_width = *width),
            Op::Delete { i } => self.update(*i, |e| e.deleted = true),
            Op::Undelete { i } => self.update(*i, |e| e.deleted = false),
            Op::Remove { i } => {
                if let Some(id) = self.pick(*i)
                    && self.canvas.transact(|tx| tx.remove(&id)).is_ok()
                {
                    self.ids.retain(|x| x != &id);
                }
            }
            Op::ToFront { i } => {
                if let Some(id) = self.pick(*i) {
                    let _ = self.canvas.transact(|tx| {
                        let z = tx.next_top_index().unwrap();
                        tx.update(&id, |e| e.index = z)
                    });
                }
            }
            Op::Lock { i, on } => self.update(*i, |e| e.locked = *on),
            Op::Rename { i } => self.update(*i, |e| e.name = Some("renamed".into())),
            Op::EditPoints { i, dx, dy } => self.update(*i, |e| {
                let moved = match &mut e.kind {
                    ElementKind::Line(d) | ElementKind::Arrow(d) => {
                        if let Some(last) = d.points.last_mut() {
                            last.x += dx;
                            last.y += dy;
                        }
                        true
                    }
                    _ => false,
                };
                if moved {
                    let _ = e.refit_points();
                }
            }),
            Op::Batch { moves } => {
                let targets: Vec<(ElementId, f64, f64)> =
                    moves.iter().filter_map(|(i, dx, dy)| self.pick(*i).map(|id| (id, *dx, *dy))).collect();
                let _ = self.canvas.transact(|tx| {
                    for (id, dx, dy) in &targets {
                        tx.update(id, |e| {
                            e.transform.x += dx;
                            e.transform.y += dy;
                        })?;
                    }
                    Ok(())
                });
            }
        }
    }

    fn update(&mut self, i: usize, f: impl FnOnce(&mut Element)) {
        if let Some(id) = self.pick(i) {
            let _ = self.canvas.transact(|tx| tx.update(&id, f));
        }
    }

    /// Everything the index answers must equal a scan of the store.
    fn check(&self, probes: &[(f64, f64)], rects: &[(f64, f64, f64, f64)]) -> Result<(), TestCaseError> {
        let store = self.canvas.store();
        let index = self.canvas.index();
        index.verify(store).map_err(|e| TestCaseError::fail(e.to_string()))?;

        let zoom = self.canvas.viewport().zoom();
        let tol_px = 5.0;
        for &(x, y) in probes {
            let p = Point::new(x, y);
            // Brute force: every live, unlocked element, exact test, topmost.
            let mut expected: Vec<&Element> = store
                .iter()
                .filter(|e| !e.locked && index_bounds(e).is_some())
                .filter(|e| hit_element(store, e, p, tol_px / zoom).is_some())
                .collect();
            expected.sort_by(|a, b| z_cmp(b, a));
            let got = index.hit_all(store, p, zoom, &HitOptions::with_tolerance(tol_px));
            let got_ids: Vec<&ElementId> = got.iter().map(|h| &h.id).collect();
            let want_ids: Vec<&ElementId> = expected.iter().map(|e| &e.id).collect();
            prop_assert_eq!(&got_ids, &want_ids, "hit_all at {:?}", p);
            let top = index.hit(store, p, zoom, &HitOptions::with_tolerance(tol_px));
            prop_assert_eq!(top.as_ref().map(|h| &h.id), want_ids.first().copied(), "hit at {:?}", p);
        }
        for &(x, y, w, h) in rects {
            let r = Rect::new(x, y, x + w, y + h);
            let mut want: Vec<ElementId> = store
                .iter()
                .filter_map(|e| {
                    index_bounds(e).filter(|b| drafftink_canvas::geom::aabb::intersects(b, &r)).map(|_| e.id.clone())
                })
                .collect();
            want.sort();
            let mut got: Vec<ElementId> = index.in_rect(store, &r).into_iter().map(|e| e.id.clone()).collect();
            got.sort();
            prop_assert_eq!(got, want, "in_rect {:?}", r);

            // Contain marquee: elements whose exact bounds lie inside the rect.
            let mut want: Vec<ElementId> = store
                .iter()
                .filter(|e| !e.locked)
                .filter_map(|e| shape_of(e).map(|s| (e, s)))
                .filter(|(_, s)| {
                    use drafftink_canvas::geom::Outline;
                    drafftink_canvas::geom::aabb::contains_rect(&r, &s.bounds())
                })
                .map(|(e, _)| e.id.clone())
                .collect();
            want.sort();
            let mut got = index.marquee(store, &r, MarqueeMode::Contain, false);
            got.sort();
            prop_assert_eq!(got, want, "marquee contain {:?}", r);
        }
        // The index never holds more than the store has.
        prop_assert!(index.len() <= store.len());
        Ok(())
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 96, .. ProptestConfig::default() })]

    #[test]
    fn index_stays_equal_to_brute_force_under_random_edits(
        ops in proptest::collection::vec(op(), 1..60),
        probes in proptest::collection::vec((coord(), coord()), 6),
        rects in proptest::collection::vec((coord(), coord(), 0.0f64..400.0, 0.0f64..400.0), 3),
    ) {
        let mut m = Model { canvas: Canvas::new(MemoryStore::new(), identity_viewport()), ids: Vec::new(), counter: 0 };
        for (n, o) in ops.iter().enumerate() {
            m.apply(o);
            if n % 5 == 4 {
                m.check(&probes, &rects)?;
            }
        }
        m.check(&probes, &rects)?;
        // Edits never trigger a bulk rebuild: only the initial build did.
        prop_assert_eq!(m.canvas.index().bulk_rebuilds(), 1);
    }

    #[test]
    fn a_fresh_build_equals_the_incremental_index(ops in proptest::collection::vec(op(), 1..40)) {
        let mut m = Model { canvas: Canvas::new(MemoryStore::new(), identity_viewport()), ids: Vec::new(), counter: 0 };
        for o in &ops {
            m.apply(o);
        }
        let fresh = drafftink_canvas::SceneIndex::build(m.canvas.store());
        prop_assert_eq!(fresh.len(), m.canvas.index().len());
        for e in m.canvas.store().iter() {
            prop_assert_eq!(fresh.bounds_of(&e.id), m.canvas.index().bounds_of(&e.id));
        }
    }
}

#[test]
fn a_huge_change_set_rebuilds_in_bulk_and_stays_correct() {
    use drafftink_canvas::scene::Document;
    let mut c = Canvas::new(MemoryStore::new(), identity_viewport());
    let mut doc = Document::default();
    let keys = ZIndex::n_between(None, None, 3000).unwrap();
    for (i, z) in keys.into_iter().enumerate() {
        let mut e = rect(&format!("r{i}"), (i % 100) as f64 * 20.0, (i / 100) as f64 * 20.0, 10.0, 10.0);
        e.index = z;
        doc.elements.push(e);
    }
    let before = c.index().bulk_rebuilds();
    drafftink_canvas::scene::load_document(&mut *c.store_mut(), &doc).unwrap();
    assert_eq!(c.index().len(), 3000);
    assert_eq!(c.index().bulk_rebuilds(), before + 1, "loading a document is one bulk rebuild");
    c.index().verify(c.store()).unwrap();
    // A small edit afterwards is incremental.
    c.transact(|tx| tx.update(&id("r5"), |e| e.transform.x += 1.0)).unwrap();
    assert_eq!(c.index().bulk_rebuilds(), before + 1);
    c.index().verify(c.store()).unwrap();
}

#[test]
fn only_geometry_fields_touch_the_index() {
    let mut c = canvas_with(vec![rect("a", 0.0, 0.0, 10.0, 10.0), rect("b", 50.0, 0.0, 10.0, 10.0)]);
    let (_, cs) = c.transact(|tx| tx.update(&id("a"), |e| e.name = Some("x".into()))).unwrap();
    let stats = c.index().clone().apply(c.store(), &cs);
    assert_eq!((stats.updated, stats.skipped), (0, 1), "a rename costs nothing");
    let (_, cs) = c.transact(|tx| tx.update(&id("a"), |e| e.transform.x += 5.0)).unwrap();
    let stats = c.index().clone().apply(c.store(), &cs);
    assert_eq!(stats.updated, 1);
}

#[test]
fn verify_notices_a_stale_index() {
    let mut c = canvas_with(vec![rect("a", 0.0, 0.0, 10.0, 10.0), rect("b", 50.0, 0.0, 10.0, 10.0)]);
    let mut stale = c.index().clone();
    let (_, cs) = c.transact(|tx| tx.update(&id("a"), |e| e.transform.x += 100.0)).unwrap();
    assert!(stale.verify(c.store()).is_err(), "not yet applied");
    stale.apply(c.store(), &cs);
    assert!(stale.verify(c.store()).is_ok());
    // A lost removal is noticed too.
    let mut stale = c.index().clone();
    let (_, _cs) = c.transact(|tx| tx.update(&id("b"), |e| e.deleted = true)).unwrap();
    assert!(stale.verify(c.store()).is_err());
    stale.rebuild(c.store());
    assert!(stale.verify(c.store()).is_ok());
}

#[test]
fn the_random_edits_do_build_scenes_with_hits() {
    // Guards the property test against passing vacuously: a fixed sequence of the same operations.
    let mut m = Model { canvas: Canvas::new(MemoryStore::new(), identity_viewport()), ids: Vec::new(), counter: 0 };
    for k in 0..40u8 {
        m.apply(&Op::Add {
            kind: k % 6,
            x: (k as f64) * 10.0,
            y: 0.0,
            w: 80.0,
            h: 60.0,
            angle: 0.3,
            filled: k % 2 == 0,
        });
    }
    assert!(m.canvas.store().len() >= 38, "{}", m.canvas.store().len());
    assert_eq!(m.canvas.index().len(), m.canvas.store().len());
    let hits: usize = (0..50).map(|i| m.canvas.hit_all_at(Point::new(i as f64 * 8.0, 30.0)).len()).sum();
    assert!(hits > 50, "{hits}");
}
