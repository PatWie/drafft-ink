#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Work-counter tests of frame composition at 1k, 10k and (in release builds) 100k elements: a frame draws what
//! is visible, a repeated frame encodes nothing, and one edit re-encodes one fragment. `cargo xtask perf` runs
//! this file in release mode, which adds the 100k scene.

mod common;

use common::*;
use drafftink_profile::{Counter, ScenarioReport, WorkScope};
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

/// The scenario `pan` of `docs/performance.md` section 4.1 without the GPU: a sinusoidal pan over a warm cache.
/// A first pass over the path fills the cache; the second pass must not encode, generate sketches or lay text out.
#[test]
fn a_pan_encodes_what_is_visible_and_regenerates_nothing() {
    for &n in sizes() {
        let mut w = World::new(n);
        let path: Vec<f64> = (0..60).map(|k| 30.0 * (f64::from(k) * 0.2).sin()).collect();
        for &step in &path {
            w.compose(&w.view(step));
        }
        let work = WorkScope::begin();
        for (k, &step) in path.iter().enumerate() {
            let rep = w.compose(&w.view(step));
            assert!(rep.in_view < 200, "frame {k} at {n}: {} in view", rep.in_view);
            assert_eq!(rep.cache_misses, 0, "frame {k} at {n}: {rep:?}");
        }
        // Each frame draws what it shows, so the encoded count follows the visible elements and not the scene.
        assert!(work.count(Counter::ElementsEncoded) < 60 * 200, "at {n}: {}", work.summary());
        for c in [Counter::ElementsScanned, Counter::SketchRegens, Counter::Relayouts, Counter::CacheMiss] {
            assert_eq!(work.count(c), 0, "{c:?} at {n}: {}", work.summary());
        }
        ScenarioReport::from_work("pan", Some(n as u64), &work).save_if_requested().expect("the report is written");
    }
}

/// The scenario `zoom`: a sweep of the zoom with the cache warm, and the cache stays bounded.
#[test]
fn a_zoom_sweep_keeps_the_cache_bounded() {
    for &n in sizes() {
        let mut w = World::new(n);
        let c = w.side / 2.0;
        let at = |zoom: f64| View::new(zoom, Vec2::new(-c * zoom, -c * zoom), Size::new(800.0, 600.0));
        w.compose(&at(1.0));
        let work = WorkScope::begin();
        for k in 0..60 {
            let zoom = 1.0 + 0.5 * (f64::from(k) * 0.2).sin();
            let rep = w.compose(&at(zoom));
            assert!(rep.in_view < 400, "frame {k} at {n}: {} in view", rep.in_view);
        }
        assert_eq!(work.count(Counter::ElementsScanned), 0, "at {n}: {}", work.summary());
        assert!(
            work.gauge(drafftink_profile::Gauge::CacheEntries) < 1_000,
            "at {n}: the cache holds the visible elements"
        );
        ScenarioReport::from_work("zoom", Some(n as u64), &work).save_if_requested().expect("the report is written");
    }
}
