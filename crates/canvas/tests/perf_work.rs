#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Work-counter tests of the spatial index at 1k, 10k and (in release builds) 100k elements: an edit updates
//! the entries it touched, a hit test asks the index and runs the exact test on a few candidates, and nothing
//! rebuilds the index in bulk after the first build. `cargo xtask perf` runs this file in release mode, which
//! adds the 100k scene.

mod common;

use common::*;
use drafftink_canvas::geom::kurbo::Point;
use drafftink_canvas::scene::{
    ChangeKind, Document, Element, ElementKind, Fields, MemoryStore, SceneRead, SceneStore, SceneTx, ZIndex,
};
use drafftink_canvas::{HitOptions, MarqueeMode, SceneIndex};
use drafftink_profile::{Counter, WorkScope};

/// Scene sizes. Debug builds stop at 10k.
fn sizes() -> &'static [usize] {
    if cfg!(debug_assertions) { &[1_000, 10_000] } else { &[1_000, 10_000, 100_000] }
}

const CELL: f64 = 140.0;

/// `n` filled rectangles on a grid, ids `r0..`, each in its own 140-unit cell.
fn scene(n: usize) -> MemoryStore {
    let side = (n as f64).sqrt().ceil() as usize;
    let keys = ZIndex::rebalance(n).unwrap();
    let mut doc = Document::new();
    doc.elements = (0..n)
        .map(|i| {
            let mut e = Element::new(id(&format!("r{i}")), ElementKind::Rectangle, keys[i].clone()).with_bounds(
                (i % side) as f64 * CELL,
                (i / side) as f64 * CELL,
                80.0,
                60.0,
            );
            e.style.fill_color = Some(drafftink_canvas::scene::Color::rgb(200, 100, 50));
            e
        })
        .collect();
    MemoryStore::from_document(&doc).unwrap()
}

#[test]
fn moving_one_element_updates_one_index_entry_at_every_size() {
    for &n in sizes() {
        let mut store = scene(n);
        let mut index = SceneIndex::build(&store);
        assert_eq!(index.len(), n);
        let rebuilds = index.bulk_rebuilds();
        let mut tx = store.begin();
        tx.update(&id("r5"), |e| e.transform.x += 12.0).unwrap();
        let cs = tx.commit().unwrap();
        let work = WorkScope::begin();
        let stats = index.apply(&store, &cs);
        assert_eq!((stats.updated, stats.removed, stats.skipped, stats.rebuilt), (1, 0, 0, false), "at {n}");
        assert_eq!(index.bulk_rebuilds(), rebuilds, "no bulk rebuild at {n}");
        assert_eq!(work.count(Counter::ElementsScanned), 0, "at {n}: {}", work.summary());
    }
}

#[test]
fn a_change_that_does_not_move_anything_costs_a_field_check() {
    for &n in sizes() {
        let mut store = scene(n);
        let mut index = SceneIndex::build(&store);
        let mut tx = store.begin();
        tx.update(&id("r7"), |e| e.locked = true).unwrap();
        let cs = tx.commit().unwrap();
        assert!(!cs.get(&id("r7")).unwrap().fields.intersects(Fields::POSITION | Fields::SIZE));
        let stats = index.apply(&store, &cs);
        assert_eq!((stats.updated, stats.skipped), (0, 1), "at {n}");
    }
}

#[test]
fn a_hit_test_asks_the_index_and_tests_a_few_candidates() {
    for &n in sizes() {
        let store = scene(n);
        let index = SceneIndex::build(&store);
        let side = (n as f64).sqrt().ceil() as usize;
        let target = (side / 2) * side + side / 2;
        let p = Point::new((target % side) as f64 * CELL + 40.0, (target / side) as f64 * CELL + 30.0);
        let work = WorkScope::begin();
        let hit = index.hit(&store, p, 1.0, &HitOptions::default()).unwrap();
        assert_eq!(hit.id, id(&format!("r{target}")), "at {n}");
        assert_eq!(work.count(Counter::HitTests), 1, "at {n}: {}", work.summary());
        assert_eq!(work.count(Counter::ElementsScanned), 0, "at {n}: {}", work.summary());
        // Empty space between the cells: no hit, still no scan.
        let gap = Point::new(p.x + 70.0, p.y);
        assert!(index.hit(&store, gap, 1.0, &HitOptions::default()).is_none(), "at {n}");
        assert_eq!(work.count(Counter::ElementsScanned), 0, "at {n}: {}", work.summary());
    }
}

#[test]
fn a_marquee_returns_the_few_elements_it_covers() {
    for &n in sizes() {
        let store = scene(n);
        let index = SceneIndex::build(&store);
        let rect = drafftink_canvas::geom::kurbo::Rect::new(-10.0, -10.0, CELL * 3.0, CELL * 2.0);
        let work = WorkScope::begin();
        let ids = index.marquee(&store, &rect, MarqueeMode::Contain, false);
        assert_eq!(ids.len(), 6, "three columns and two rows at {n}");
        assert_eq!(work.count(Counter::SpatialQueries), 1, "at {n}: {}", work.summary());
        assert_eq!(work.count(Counter::ElementsScanned), 0, "at {n}: {}", work.summary());
    }
}

#[test]
fn deleting_and_restoring_one_element_touches_one_entry() {
    for &n in sizes() {
        let mut store = scene(n);
        let mut index = SceneIndex::build(&store);
        for deleted in [true, false] {
            let mut tx = store.begin();
            tx.update(&id("r3"), |e| e.deleted = deleted).unwrap();
            let cs = tx.commit().unwrap();
            assert_eq!(cs.get(&id("r3")).unwrap().kind, ChangeKind::Updated);
            let stats = index.apply(&store, &cs);
            assert_eq!(stats.updated + stats.removed, 1, "at {n}, deleted {deleted}");
            assert!(!stats.rebuilt);
        }
        assert_eq!(index.len(), store.len());
    }
}
