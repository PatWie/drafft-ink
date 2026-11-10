#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Frame composition without a GPU: culling, z order, cache behaviour, level of detail, frames.

mod common;

use common::*;
use drafftink_geom::aabb;
use drafftink_profile::{Counter, WorkScope};
use drafftink_render::*;
use drafftink_scene::*;
use drafftink_typeset::Typesetter;
use kurbo::{Rect, Size, Vec2};
use vello::Scene;

fn view(zoom: f64, x: f64, y: f64) -> View {
    View::new(zoom, Vec2::new(-x * zoom, -y * zoom), Size::new(800.0, 600.0))
}

fn no_lod() -> RenderOptions {
    RenderOptions { lod: LodConfig::OFF, ..RenderOptions::default() }
}

struct Rig {
    store: MemoryStore,
    index: PaintIndex,
    ts: Typesetter,
    composer: SceneComposer,
    scene: Scene,
}

impl Rig {
    fn new(n: usize, seed: u64) -> Self {
        let mut ts = Typesetter::new();
        let store = store_of(mixed_elements(n, seed, &mut ts));
        let index = build_paint_index(&store);
        let mut composer = SceneComposer::new();
        composer.set_options(no_lod());
        composer.set_trace(true);
        Self { store, index, ts, composer, scene: Scene::new() }
    }

    fn compose(&mut self, v: &View) -> FrameReport {
        self.composer.compose(&self.store, &self.index, v, &mut self.ts, &mut self.scene)
    }

    fn edit(&mut self, name: &str, f: impl FnOnce(&mut Element)) {
        let cs = update(&mut self.store, name, f);
        update_paint_index(&mut self.index, &self.store, &cs);
        self.composer.apply_changes(&cs);
    }
}

#[test]
fn culling_matches_brute_force() {
    let mut rig = Rig::new(1500, 7);
    let mut rng = Rng(99);
    for _ in 0..25 {
        let zoom = rng.range(0.05, 3.0);
        let v = view(zoom, rng.range(-200.0, 5000.0), rng.range(-200.0, 5000.0));
        let rep = rig.compose(&v);
        let world = v.world_rect();
        let mut expected: Vec<ElementId> =
            rig.store.iter().filter(|e| aabb::intersects(&world, &paint_bounds(e))).map(|e| e.id.clone()).collect();
        // Back to front, like the composer.
        expected.sort_by(|a, b| {
            let (ea, eb) = (rig.store.get(a).unwrap(), rig.store.get(b).unwrap());
            (&ea.index, &ea.id).cmp(&(&eb.index, &eb.id))
        });
        assert_eq!(rig.composer.drawn_order(), expected.as_slice(), "zoom {zoom}");
        assert_eq!(rep.drawn, expected.len());
        assert_eq!(rep.in_view, expected.len());
        assert_eq!(rep.culled, 1500 - expected.len());
    }
}

#[test]
fn both_collection_strategies_agree() {
    // Few hits sort the index result; many hits walk the store. Both must give the same order.
    let mut rig = Rig::new(600, 3);
    let wide = view(0.2, -100.0, -100.0);
    let narrow = view(1.0, 0.0, 0.0);
    for v in [wide, narrow] {
        rig.compose(&v);
        let order = rig.composer.drawn_order().to_vec();
        let mut sorted = order.clone();
        sorted.sort_by(|a, b| {
            let (ea, eb) = (rig.store.get(a).unwrap(), rig.store.get(b).unwrap());
            (&ea.index, &ea.id).cmp(&(&eb.index, &eb.id))
        });
        assert_eq!(order, sorted);
    }
}

#[test]
fn encoded_elements_follow_the_view_not_the_document() {
    let mut rig = Rig::new(4000, 1);
    let v = view(1.0, 0.0, 0.0);
    let work = WorkScope::begin();
    let rep = rig.compose(&v);
    assert!(rep.drawn > 0 && rep.drawn < 200, "{rep:?}");
    assert_eq!(work.count(Counter::ElementsEncoded), rep.drawn as u64);
    assert_eq!(work.count(Counter::ElementsCulled), (4000 - rep.drawn) as u64);
    assert_eq!(work.count(Counter::SceneBuilds), 1);
    assert_eq!(rep.cache_misses, rep.drawn);
}

#[test]
fn a_warm_pan_encodes_nothing_and_regenerates_nothing() {
    let mut rig = Rig::new(2000, 5);
    rig.compose(&view(1.0, 0.0, 0.0));
    let work = WorkScope::begin();
    let rep = rig.compose(&view(1.0, 3.0, 2.0));
    assert_eq!(rep.cache_misses, 0, "{rep:?}");
    assert_eq!(work.count(Counter::SketchRegens), 0, "{}", work.summary());
    assert_eq!(work.count(Counter::Relayouts), 0, "{}", work.summary());
    assert!(rep.cache_hits > 0);
}

#[test]
fn moving_and_rotating_reuse_the_fragment() {
    let mut rig = Rig::new(100, 5);
    let v = view(1.0, 0.0, 0.0);
    rig.compose(&v);
    // Element e5 is text, e0 a rectangle, e3 an arrow: one of each family of work.
    for name in ["e0", "e3", "e5", "e6", "e4"] {
        if rig.composer.drawn_order().iter().all(|i| i.as_str() != name) {
            continue;
        }
        let work = WorkScope::begin();
        rig.edit(name, |e| {
            e.transform.x += 4.0;
            e.transform.y += 1.0;
        });
        rig.edit(name, |e| e.transform.angle = 0.4);
        let rep = rig.compose(&v);
        assert_eq!(rep.cache_misses, 0, "{name}: {rep:?}");
        assert!(rep.cache_reuses >= 1, "{name}: {rep:?}");
        assert_eq!(work.count(Counter::SketchRegens), 0, "{name} {}", work.summary());
        assert_eq!(work.count(Counter::Relayouts), 0, "{name} {}", work.summary());
    }
}

#[test]
fn resize_encodes_again_and_regenerates_the_sketch() {
    let mut rig = Rig::new(100, 5);
    let v = view(1.0, 0.0, 0.0);
    rig.compose(&v);
    let work = WorkScope::begin();
    rig.edit("e0", |e| e.transform.w += 10.0);
    let rep = rig.compose(&v);
    assert_eq!(rep.cache_misses, 1, "{rep:?}");
    assert_eq!(work.count(Counter::SketchRegens), 1, "{}", work.summary());
}

#[test]
fn recolouring_encodes_again_but_keeps_the_sketch() {
    let mut rig = Rig::new(100, 5);
    let v = view(1.0, 0.0, 0.0);
    rig.compose(&v);
    let work = WorkScope::begin();
    rig.edit("e0", |e| e.style.stroke_color = Color::rgb(200, 0, 0));
    let rep = rig.compose(&v);
    assert_eq!(rep.cache_misses, 1, "{rep:?}");
    assert_eq!(work.count(Counter::SketchRegens), 0, "colour is not part of the sketch key: {}", work.summary());
    // A wider stroke is part of it.
    let work = WorkScope::begin();
    rig.edit("e0", |e| e.style.stroke_width = 5.0);
    rig.compose(&v);
    assert_eq!(work.count(Counter::SketchRegens), 1);
}

#[test]
fn text_opacity_changes_the_picture_without_a_new_layout() {
    let mut rig = Rig::new(100, 5);
    let v = view(1.0, 0.0, 0.0);
    rig.compose(&v);
    let Some(name) =
        ["e5", "e13", "e21", "e29"].into_iter().find(|n| rig.composer.drawn_order().iter().any(|i| i.as_str() == *n))
    else {
        panic!("no text in view")
    };
    let before = rig.composer.cache().stats();
    let work = WorkScope::begin();
    rig.edit(name, |e| e.style.opacity = 0.4);
    let rep = rig.compose(&v);
    assert_eq!(rep.cache_misses, 1);
    assert_eq!(work.count(Counter::Relayouts), 0, "opacity must not re-lay-out: {}", work.summary());
    assert_eq!(rig.composer.cache().stats().misses, before.misses + 1);

    // And the encoded colour really changed (the old cache ignored opacity).
    let mut painter = ElementPainter::new();
    let mut ts = Typesetter::new();
    let mut e = rig.store.get(&id(name)).unwrap().clone();
    e.style.opacity = 1.0;
    let full = painter.paint(&e, &RenderOptions::default(), &mut ts, &|_| None);
    e.style.opacity = 0.4;
    let faded = painter.paint(&e, &RenderOptions::default(), &mut ts, &|_| None);
    assert_ne!(full.scene.encoding().draw_data, faded.scene.encoding().draw_data);
}

#[test]
fn changes_that_do_not_draw_keep_the_fragment() {
    let mut rig = Rig::new(100, 5);
    let v = view(1.0, 0.0, 0.0);
    rig.compose(&v);
    rig.edit("e0", |e| e.name = Some("renamed".into()));
    rig.edit("e0", |e| e.locked = true);
    let rep = rig.compose(&v);
    assert_eq!(rep.cache_misses, 0);
    assert!(rep.cache_reuses >= 1);
}

#[test]
fn deleted_elements_vanish_and_free_their_fragment() {
    let mut rig = Rig::new(50, 5);
    let v = view(1.0, 0.0, 0.0);
    rig.compose(&v);
    assert!(rig.composer.cache().contains(&id("e0"), Theme::Light));
    rig.edit("e0", |e| e.deleted = true);
    rig.compose(&v);
    assert!(rig.composer.drawn_order().iter().all(|i| i.as_str() != "e0"));
    assert!(!rig.composer.cache().contains(&id("e0"), Theme::Light));
    // The index no longer has it either.
    assert!(!rig.index.contains(&id("e0")));
}

#[test]
fn the_cache_is_bounded_and_keeps_what_was_used_last() {
    let mut ts = Typesetter::new();
    let store = store_of(mixed_elements(800, 4, &mut ts));
    let index = build_paint_index(&store);
    let mut composer = SceneComposer::with_cache_bytes(300_000);
    composer.set_options(no_lod());
    let mut scene = Scene::new();
    let mut last = None;
    for step in 0..40 {
        let v = view(1.0, f64::from(step) * 400.0 % 3000.0, f64::from(step) * 170.0 % 3000.0);
        composer.compose(&store, &index, &v, &mut ts, &mut scene);
        let s = composer.cache().stats();
        assert!(s.bytes <= 300_000 + 60_000, "step {step}: {} bytes", s.bytes);
        last = Some(s);
    }
    let s = last.unwrap();
    assert!(s.evictions > 0, "{s:?}");
    // The last frame's elements are still cached: composing it again is all hits.
    let v = view(1.0, 39.0 * 400.0 % 3000.0, 39.0 * 170.0 % 3000.0);
    let rep = composer.compose(&store, &index, &v, &mut ts, &mut scene);
    assert_eq!(rep.cache_misses, 0, "{rep:?}");
}

#[test]
fn themes_have_separate_fragments() {
    let mut rig = Rig::new(60, 5);
    let v = view(1.0, 0.0, 0.0);
    let a = rig.compose(&v);
    rig.composer.set_options(RenderOptions { theme: Theme::Dark, ..no_lod() });
    let b = rig.compose(&v);
    assert_eq!(b.cache_misses, a.drawn, "every element is encoded again for the new theme");
    rig.composer.set_options(no_lod());
    let c = rig.compose(&v);
    assert_eq!(c.cache_misses, 0, "the light fragments are still there");
    assert_eq!(rig.composer.background(&rig.store), peniko::Color::from_rgba8(255, 255, 255, 255));
    rig.composer.set_options(RenderOptions { theme: Theme::Dark, ..no_lod() });
    assert_eq!(rig.composer.background(&rig.store), peniko::Color::from_rgba8(18, 18, 18, 255));
}

#[test]
fn zoomed_out_elements_become_boxes_then_vanish() {
    let mut ts = Typesetter::new();
    let store = store_of(mixed_elements(2000, 2, &mut ts));
    let index = build_paint_index(&store);
    let mut composer = SceneComposer::new();
    composer.set_trace(true);
    let mut scene = Scene::new();
    let near = composer.compose(&store, &index, &view(1.0, 0.0, 0.0), &mut ts, &mut scene);
    assert_eq!(near.lod_boxes + near.lod_skipped + near.lod_greeked, 0, "{near:?}");
    let far = composer.compose(&store, &index, &view(0.025, -3000.0, -2000.0), &mut ts, &mut scene);
    assert!(far.lod_boxes > far.drawn, "{far:?}");
    assert_eq!(
        far.cache_misses + far.cache_hits + far.cache_reuses,
        far.drawn,
        "boxes never touch the fragment cache: {far:?}"
    );
    let tiny = composer.compose(&store, &index, &view(0.004, -30000.0, -20000.0), &mut ts, &mut scene);
    assert!(tiny.lod_skipped > tiny.lod_boxes, "{tiny:?}");
    assert_eq!(tiny.drawn + tiny.lod_boxes + tiny.lod_skipped + tiny.lod_greeked + tiny.lod_dropped, tiny.in_view);
}

#[test]
fn small_text_is_greeked() {
    let mut ts = Typesetter::new();
    let mut e = text_el(&mut ts, "t", &ZIndex::first(), 0.0, 0.0, "a line of text\nand another one");
    e.transform.angle = 0.2;
    let store = store_of(vec![e]);
    let index = build_paint_index(&store);
    let mut composer = SceneComposer::new();
    let mut scene = Scene::new();
    // 20 px text at zoom 0.1 is 2 px tall: bars. The element itself is 25 px wide, so it is not a box.
    let v = View::new(0.15, Vec2::new(10.0, 10.0), Size::new(400.0, 300.0));
    let rep = composer.compose(&store, &index, &v, &mut ts, &mut scene);
    assert_eq!(rep.lod_greeked, 1, "{rep:?}");
    assert_eq!(rep.cache_misses, 0);
    let rep = composer.compose(
        &store,
        &index,
        &View::new(1.0, Vec2::new(10.0, 10.0), Size::new(400.0, 300.0)),
        &mut ts,
        &mut scene,
    );
    assert_eq!((rep.lod_greeked, rep.drawn), (0, 1));
}

#[test]
fn cheap_stand_ins_are_capped_per_frame() {
    let mut ts = Typesetter::new();
    let store = store_of(mixed_elements(1000, 2, &mut ts));
    let index = build_paint_index(&store);
    let mut composer = SceneComposer::new();
    let mut opts = RenderOptions::default();
    opts.lod.max_cheap_per_frame = 10;
    composer.set_options(opts);
    let mut scene = Scene::new();
    let rep = composer.compose(&store, &index, &view(0.03, -2000.0, -1500.0), &mut ts, &mut scene);
    assert_eq!(rep.lod_boxes + rep.lod_greeked, 10, "{rep:?}");
    assert!(rep.lod_dropped > 0);
}

#[test]
fn frame_children_are_clipped_or_culled() {
    let z = ZIndex::rebalance(8).unwrap();
    let mut ts = Typesetter::new();
    let mut elements = vec![frame_el("f", &z[0], 100.0, 100.0, 200.0, 200.0)];
    let mut inside = rect_el("inside", &z[1], 120.0, 120.0, 50.0, 50.0);
    inside.frame_id = Some(id("f"));
    let mut crossing = rect_el("crossing", &z[2], 250.0, 150.0, 100.0, 50.0);
    crossing.frame_id = Some(id("f"));
    let mut crossing2 = rect_el("crossing2", &z[3], 150.0, 250.0, 100.0, 100.0);
    crossing2.frame_id = Some(id("f"));
    let mut outside = rect_el("outside", &z[4], 400.0, 400.0, 50.0, 50.0);
    outside.frame_id = Some(id("f"));
    let free = rect_el("free", &z[5], 280.0, 280.0, 60.0, 60.0);
    elements.extend([inside, crossing, crossing2, outside, free]);
    let store = store_of(elements);
    let index = build_paint_index(&store);
    let mut composer = SceneComposer::new();
    composer.set_options(no_lod());
    composer.set_trace(true);
    let mut scene = Scene::new();
    let rep = composer.compose(&store, &index, &view(1.0, 0.0, 0.0), &mut ts, &mut scene);
    let drawn: Vec<&str> = composer.drawn_order().iter().map(|i| i.as_str()).collect();
    assert_eq!(drawn, ["f", "inside", "crossing", "crossing2", "free"], "the outside child is culled");
    // Two children cross the edge and are neighbours in z, so they share one clip layer.
    assert_eq!(rep.clips, 1, "{rep:?}");
    assert_eq!(rep.culled, 1);
    // A frame without clipping draws everything.
    update(&mut { store }, "f", |e| e.kind = ElementKind::Frame(FrameData { name: None, clip: false }));
}

#[test]
fn a_frame_with_clip_off_does_not_clip() {
    let z = ZIndex::rebalance(3).unwrap();
    let mut ts = Typesetter::new();
    let mut f = frame_el("f", &z[0], 0.0, 0.0, 100.0, 100.0);
    f.kind = ElementKind::Frame(FrameData { name: Some("x".into()), clip: false });
    let mut c = rect_el("c", &z[1], 80.0, 80.0, 100.0, 100.0);
    c.frame_id = Some(id("f"));
    let mut far = rect_el("far", &z[2], 500.0, 500.0, 10.0, 10.0);
    far.frame_id = Some(id("f"));
    let store = store_of(vec![f, c, far]);
    let index = build_paint_index(&store);
    let mut composer = SceneComposer::new();
    composer.set_options(no_lod());
    let mut scene = Scene::new();
    let rep = composer.compose(&store, &index, &view(1.0, 0.0, 0.0), &mut ts, &mut scene);
    assert_eq!((rep.clips, rep.drawn), (0, 3));
}

#[test]
fn hidden_elements_are_not_drawn() {
    let mut rig = Rig::new(40, 1);
    let v = view(1.0, 0.0, 0.0);
    rig.compose(&v);
    let victim = rig.composer.drawn_order()[0].clone();
    rig.composer.set_hidden(vec![victim.clone()]);
    rig.compose(&v);
    assert!(!rig.composer.drawn_order().contains(&victim));
}

#[test]
fn hostile_views_and_elements_never_panic() {
    let mut rig = Rig::new(30, 1);
    for v in [
        View::new(f64::NAN, Vec2::ZERO, Size::new(800.0, 600.0)),
        View::new(0.0, Vec2::ZERO, Size::new(800.0, 600.0)),
        View::new(1.0, Vec2::new(f64::INFINITY, 0.0), Size::new(800.0, 600.0)),
        View::new(1.0, Vec2::ZERO, Size::new(0.0, 0.0)),
        View::new(1e300, Vec2::ZERO, Size::new(800.0, 600.0)),
        View::new(1e-300, Vec2::ZERO, Size::new(800.0, 600.0)),
        View::new(1.0, Vec2::ZERO, Size::new(800.0, 600.0)).with_dpi(f64::NAN),
    ] {
        let _ = rig.compose(&v);
    }
    // Elements with nonsense numbers painted directly.
    let mut ts = Typesetter::new();
    let mut painter = ElementPainter::new();
    let z = ZIndex::first();
    let mut weird = vec![
        rect_el("a", &z, f64::NAN, f64::INFINITY, -5.0, 1e300),
        ellipse_el("b", &z, 0.0, 0.0, f64::NAN, f64::NAN),
        line_el("c", &z, Kind::Arrow, &[(0.0, 0.0), (0.0, 0.0)]),
    ];
    weird[0].style.stroke_width = f64::NAN;
    weird[0].style.opacity = f64::NAN;
    weird[1].style.corner_radius = -4.0;
    weird[1].style.stroke_width = 1e12;
    for e in &weird {
        let f = painter.paint(e, &RenderOptions::default(), &mut ts, &|_| None);
        assert!(f.bounds.x0.is_finite() || f.is_empty());
        let _ = paint_bounds(e);
        let _ = paint_hash(e);
    }
}

#[test]
fn the_index_follows_commits() {
    let mut rig = Rig::new(200, 9);
    let before = rig.index.len();
    assert_eq!(before, 200);
    rig.edit("e1", |e| e.transform.x += 5000.0);
    assert!(rig.index.get(&id("e1")).unwrap().x0 > 4000.0);
    // Equals a fresh build.
    let fresh = build_paint_index(&rig.store);
    for e in rig.store.iter() {
        assert_eq!(rig.index.get(&e.id), fresh.get(&e.id));
    }
    // Removal.
    let mut tx = rig.store.begin();
    tx.remove(&id("e2")).unwrap();
    let cs = tx.commit().unwrap();
    update_paint_index(&mut rig.index, &rig.store, &cs);
    assert_eq!(rig.index.len(), 199);
}

#[test]
fn transient_elements_are_appended_without_caching() {
    let mut rig = Rig::new(10, 1);
    let v = view(1.0, 0.0, 0.0);
    rig.compose(&v);
    let before = rig.composer.cache().len();
    let preview = rect_el("preview", &ZIndex::first(), 10.0, 10.0, 50.0, 50.0);
    let n0 = rig.scene.encoding().path_tags.len();
    rig.composer.compose_transient(&rig.store, &[preview], &v, &mut rig.ts, &mut rig.scene);
    assert!(rig.scene.encoding().path_tags.len() > n0);
    assert_eq!(rig.composer.cache().len(), before);
}

#[test]
fn paint_bounds_contain_everything_that_is_drawn() {
    let mut ts = Typesetter::new();
    let mut painter = ElementPainter::new();
    for e in mixed_elements(160, 12, &mut ts) {
        let frag = painter.paint(&e, &RenderOptions::default(), &mut ts, &|_| None);
        let m = paint_margin(&e);
        let allowed = Rect::new(-m.left, -m.top, e.transform.w + m.right, e.transform.h + m.bottom);
        assert!(
            frag.bounds.x0 >= allowed.x0 - 1e-6
                && frag.bounds.y0 >= allowed.y0 - 1e-6
                && frag.bounds.x1 <= allowed.x1 + 1e-6
                && frag.bounds.y1 <= allowed.y1 + 1e-6,
            "{:?} drew {:?} outside {:?}",
            e.kind.kind(),
            frag.bounds,
            allowed
        );
    }
}

#[test]
fn text_only_fragments_are_appended() {
    // A glyph run has draw data but no path data until the scene is rendered. A fragment that holds only text
    // must still reach the frame (an early version dropped it as empty).
    let mut ts = Typesetter::new();
    let t = text_el(&mut ts, "t", &ZIndex::first(), 10.0, 10.0, "only text");
    let store = store_of(vec![t]);
    let index = build_paint_index(&store);
    let mut composer = SceneComposer::new();
    let mut scene = Scene::new();
    composer.compose(&store, &index, &view(1.0, 0.0, 0.0), &mut ts, &mut scene);
    assert_eq!(scene.encoding().resources.glyph_runs.len(), 1);
    assert!(scene.encoding().path_tags.is_empty(), "the test is about a scene with no path data");
}
