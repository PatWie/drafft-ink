#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Work-counter tests of frame composition at 1k, 10k and (in release builds) 100k elements: a frame draws what
//! is visible, a repeated frame encodes nothing, and one edit re-encodes one fragment. `cargo xtask perf` runs
//! this file in release mode, which adds the 100k scene.

mod common;

use common::*;
use drafftink_profile::{Counter, WorkScope};
use drafftink_render::*;
use drafftink_scene::*;
use drafftink_typeset::Typesetter;
use kurbo::{Size, Vec2};
use vello::Scene;

/// Scene sizes. Debug builds stop at 10k.
fn sizes() -> &'static [usize] {
    if cfg!(debug_assertions) { &[1_000, 10_000] } else { &[1_000, 10_000, 100_000] }
}

struct World {
    store: MemoryStore,
    index: PaintIndex,
    ts: Typesetter,
    composer: SceneComposer,
    scene: Scene,
    side: f64,
}

impl World {
    fn new(n: usize) -> Self {
        let mut ts = Typesetter::new();
        let store = store_of(mixed_elements(n, 42, &mut ts));
        let index = build_paint_index(&store);
        let mut composer = SceneComposer::new();
        composer.set_options(RenderOptions { lod: LodConfig::OFF, ..RenderOptions::default() });
        Self { store, index, ts, composer, scene: Scene::new(), side: (n as f64).sqrt().ceil() * 140.0 }
    }

    /// 800 x 600 logical pixels at zoom 1 in the middle of the scene, shifted by `step`.
    fn view(&self, step: f64) -> View {
        let c = self.side / 2.0 + step;
        View::new(1.0, Vec2::new(-c, -c), Size::new(800.0, 600.0))
    }

    fn compose(&mut self, view: &View) -> FrameReport {
        self.composer.compose(&self.store, &self.index, view, &mut self.ts, &mut self.scene)
    }

    /// An element that `view(0.0)` shows: the view starts at the middle of the scene and is 800 by 600 units.
    fn visible_element(&self) -> String {
        let side = (self.store.len() as f64).sqrt().ceil() as usize;
        let c = (self.side / 2.0 / 140.0) as usize + 2;
        format!("e{}", c * side + c)
    }

    fn edit(&mut self, name: &str, f: impl FnOnce(&mut Element)) {
        let cs = update(&mut self.store, name, f);
        update_paint_index(&mut self.index, &self.store, &cs);
        self.composer.apply_changes(&cs);
    }
}

fn expect_tiny(what: &str, n: usize, work: &WorkScope, rep: &FrameReport) {
    assert!(rep.in_view < 200, "{what} at {n}: {} elements in view of {}", rep.in_view, rep.total);
    assert_eq!(work.count(Counter::ElementsEncoded), rep.drawn as u64, "{what} at {n}: {}", work.summary());
}

#[test]
fn a_frame_draws_the_visible_elements_and_not_the_scene() {
    for &n in sizes() {
        let mut w = World::new(n);
        let v = w.view(0.0);
        let work = WorkScope::begin();
        let rep = w.compose(&v);
        expect_tiny("cold frame", n, &work, &rep);
        assert_eq!(rep.cache_misses, rep.drawn, "every visible element is encoded once at {n}");
        assert_eq!(work.count(Counter::SpatialQueries), 1, "one index query per frame at {n}");
    }
}

#[test]
fn a_repeated_frame_encodes_nothing() {
    for &n in sizes() {
        let mut w = World::new(n);
        let v = w.view(0.0);
        w.compose(&v);
        let work = WorkScope::begin();
        let rep = w.compose(&v);
        assert_eq!(rep.cache_misses, 0, "at {n}: {rep:?}");
        assert_eq!(rep.cache_hits, rep.drawn, "at {n}");
        for c in [Counter::SketchRegens, Counter::Relayouts, Counter::CacheMiss] {
            assert_eq!(work.count(c), 0, "{c:?} at {n}: {}", work.summary());
        }
    }
}

#[test]
fn panning_encodes_only_the_elements_that_come_into_view() {
    for &n in sizes() {
        let mut w = World::new(n);
        w.compose(&w.view(0.0));
        let work = WorkScope::begin();
        let rep = w.compose(&w.view(20.0));
        assert!(rep.cache_misses <= 12, "20 units of pan reveal a few elements at {n}: {rep:?}");
        assert!(rep.cache_hits > rep.cache_misses, "most of the frame comes from the cache at {n}: {rep:?}");
        assert!(work.count(Counter::Relayouts) <= 12, "at {n}: {}", work.summary());
    }
}

#[test]
fn editing_one_visible_element_encodes_one_fragment() {
    for &n in sizes() {
        let mut w = World::new(n);
        let v = w.view(0.0);
        w.compose(&v);
        let name = w.visible_element();
        // A change that draws differently: another colour.
        w.edit(&name, |e| e.style.stroke_color = Color::rgb(200, 30, 30));
        let work = WorkScope::begin();
        let rep = w.compose(&v);
        assert_eq!(rep.cache_misses, 1, "one edit, one fragment at {n}: {rep:?}");
        assert_eq!(rep.cache_hits, rep.drawn - 1, "at {n}");
        assert_eq!(work.count(Counter::CacheMiss), 1, "at {n}: {}", work.summary());
    }
}

#[test]
fn moving_one_visible_element_replays_its_fragment() {
    for &n in sizes() {
        let mut w = World::new(n);
        let v = w.view(0.0);
        w.compose(&v);
        let name = w.visible_element();
        w.edit(&name, |e| e.transform.x += 3.0);
        let work = WorkScope::begin();
        let rep = w.compose(&v);
        assert_eq!(rep.cache_misses, 0, "a move does not draw again at {n}: {rep:?}");
        assert_eq!(rep.cache_reuses, 1, "at {n}: {rep:?}");
        assert_eq!(work.count(Counter::SketchRegens), 0, "at {n}: {}", work.summary());
    }
}

#[test]
fn editing_an_element_out_of_view_costs_the_next_frame_nothing() {
    for &n in sizes() {
        let mut w = World::new(n);
        let v = w.view(0.0);
        w.compose(&v);
        w.edit("e0", |e| e.style.stroke_width += 2.0);
        let rep = w.compose(&v);
        assert_eq!(rep.cache_misses, 0, "at {n}: {rep:?}");
    }
}
