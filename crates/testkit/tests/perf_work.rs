#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Work-counter tests of the whole engine at 1k, 10k and (in release builds) 100k elements: what a pointer
//! event, a gesture frame or a quiet moment makes the app do, counted and never timed
//! (`docs/performance.md` section 2.5).
//!
//! The rules (`docs/performance.md` section 1): idle means idle, a hover that changes nothing redraws nothing,
//! and the work of a gesture frame follows what the gesture changes, not the size of the scene. `cargo xtask perf`
//! runs this file in release mode, which adds the 100k scene.

use drafftink_canvas::{Button, Buttons, InputEvent, Modifiers, PointerEvent, PointerId, PointerKind, PointerPhase};
use drafftink_engine::interact::Interaction;
use drafftink_geom::kurbo::Point;
use drafftink_profile::{Counter, RedrawReason, ScenarioReport, WorkScope, redraw};
use drafftink_scene::{SceneRead, SceneStore};
use drafftink_testkit::{Harness, scenegen};
use serde_json::{Value, json};

/// Scene sizes. Debug builds stop at 10k: loading 100k elements unoptimized takes tens of seconds.
fn sizes() -> &'static [usize] {
    if cfg!(debug_assertions) { &[1_000, 10_000] } else { &[1_000, 10_000, 100_000] }
}

fn loaded(n: usize) -> Harness {
    let mut h = Harness::new();
    h.load_generated(n, 1).expect("the generated scene loads");
    h
}

/// Runs a gesture of `command`: `begin`, five `update` frames, `end`. Returns the work of each phase.
fn gesture(h: &mut Harness, command: &str, frame: impl Fn(f64) -> Value) -> Vec<(&'static str, WorkScope)> {
    let mut phases = Vec::new();
    let work = WorkScope::begin();
    h.session.execute("gesture.begin", json!({ "command": command, "args": frame(1.0) })).expect("begin");
    phases.push(("begin", work));
    let work = WorkScope::begin();
    let key = h.session.running_gesture().expect("a gesture runs").to_string();
    for k in 2..=6 {
        h.session.execute("gesture.update", json!({ "gesture": key, "args": frame(f64::from(k)) })).expect("update");
    }
    phases.push(("5 updates", work));
    let work = WorkScope::begin();
    h.session.execute("gesture.end", json!({})).expect("end");
    phases.push(("end", work));
    phases
}

fn assert_no_scan(what: &str, n: usize, phases: &[(&'static str, WorkScope)]) {
    for (phase, work) in phases {
        assert_eq!(work.count(Counter::ElementsScanned), 0, "{what}, {phase} at {n}: {}", work.summary());
    }
}

#[test]
fn moving_an_element_with_a_bound_arrow_scans_nothing() {
    for &n in sizes() {
        let mut h = loaded(n);
        // e40 is a rectangle, e44 an arrow bound to it and to e41.
        let phases = gesture(&mut h, "element.move", |k| json!({ "ids": ["e40"], "dx": 3.0 * k, "dy": 2.0 * k }));
        assert_no_scan("move", n, &phases);
    }
}

#[test]
fn moving_with_smart_guides_scans_nothing_once_the_index_exists() {
    for &n in sizes() {
        let mut h = loaded(n);
        h.session.execute("settings.set", json!({ "key": "snap.smart_guides", "value": true })).expect("setting");
        h.session.execute("view.set", json!({ "scroll": [400, 300], "zoom": 1 })).expect("view");
        // The first gesture builds the spatial index from the whole scene, once.
        gesture(&mut h, "element.move", |k| json!({ "ids": ["e40"], "dx": 3.0 * k, "dy": 2.0 * k }));
        let phases = gesture(&mut h, "element.move", |k| json!({ "ids": ["e40"], "dx": 3.0 * k, "dy": 2.0 * k }));
        assert_no_scan("smart-guide move", n, &phases);
    }
}

#[test]
fn dragging_a_hundred_elements_scans_nothing_and_commits_once() {
    for &n in sizes() {
        let mut h = loaded(n);
        // 100 rectangles (every tenth element of the generated cycle starts one).
        let ids: Vec<String> = (0..100).map(|k| format!("e{}", k * 10)).collect();
        let work = WorkScope::begin();
        let phases = gesture(&mut h, "element.move", |k| json!({ "ids": ids, "dx": 4.0 * k, "dy": 3.0 * k }));
        assert_no_scan("drag of 100", n, &phases);
        assert_eq!(h.session.tab().doc.undo_depth(), 1, "the whole drag is one undo step at {n}");
        report("drag", n, &work);
    }
}

#[test]
fn resizing_and_rotating_scan_nothing() {
    for &n in sizes() {
        let mut h = loaded(n);
        let phases =
            gesture(&mut h, "element.resize", |k| json!({ "ids": ["e40"], "handle": "se", "dx": 2.0 * k, "dy": k }));
        assert_no_scan("resize", n, &phases);
        let phases = gesture(&mut h, "element.rotate", |k| json!({ "ids": ["e40"], "angle": 3.0 * k }));
        assert_no_scan("rotate", n, &phases);
    }
}

#[test]
fn dragging_the_end_of_an_arrow_scans_nothing_once_the_index_exists() {
    for &n in sizes() {
        let mut h = loaded(n);
        let frame = |k: f64| json!({ "arrow": "e44", "index": "end", "to": [150.0 + k, 25.0 + k] });
        gesture(&mut h, "arrow.move_point", frame);
        let phases = gesture(&mut h, "arrow.move_point", frame);
        assert_no_scan("arrow end", n, &phases);
    }
}

#[test]
fn a_gesture_frame_touches_the_same_few_elements_at_every_size() {
    let mut touched = Vec::new();
    for &n in sizes() {
        let mut h = loaded(n);
        h.session
            .execute(
                "gesture.begin",
                json!({ "command": "element.move", "args": { "ids": ["e40"], "dx": 1, "dy": 1 } }),
            )
            .expect("begin");
        let before = h.session.tab().doc.store().revision();
        h.session
            .execute("gesture.update", json!({ "gesture": "g1", "args": { "ids": ["e40"], "dx": 9, "dy": 9 } }))
            .expect("update");
        // One frame rolls the base back and runs the command again: a fixed number of working-store commits.
        touched.push(h.session.tab().doc.store().revision() - before);
        h.session.execute("gesture.cancel", json!({})).expect("cancel");
    }
    assert!(touched.windows(2).all(|w| w[0] == w[1]), "commits per frame by scene size: {touched:?}");
}

// ---- availability ------------------------------------------------------------------------------------------------

#[test]
fn asking_whether_group_ungroup_align_and_distribute_are_available_scans_nothing() {
    // The UI asks for every command when the selection or the document changes. A pass over the scene here was
    // 47 ms per frame at 100,000 elements while zooming in the browser, and three passes at the start of a drag.
    for &n in sizes() {
        let mut h = loaded(n);
        // e8 and e9 form a group, e0 is a plain rectangle.
        for (select, grouped) in [(None, false), (Some("e0"), false), (Some("e8"), true)] {
            match select {
                Some(id) => h.session.execute("selection.select", json!({ "ids": [id] })).expect("select"),
                None => h.session.execute("selection.clear", json!({})).expect("clear"),
            };
            let work = WorkScope::begin();
            let available = h.session.availability("group.ungroup").is_ok();
            assert_eq!(available, grouped, "ungroup availability at {n} with {select:?}");
            for id in ["group.create", "arrange.align", "arrange.distribute"] {
                let _ = h.session.availability(id);
            }
            assert_eq!(work.count(Counter::ElementsScanned), 0, "availability at {n}: {}", work.summary());
        }
        // A selection of 100 elements, as in a drag.
        let ids: Vec<String> = (0..100).map(|i| format!("e{i}")).collect();
        h.session.execute("selection.select", json!({ "ids": ids })).expect("select");
        let work = WorkScope::begin();
        for id in ["group.create", "group.ungroup", "arrange.align", "arrange.distribute"] {
            let _ = h.session.availability(id);
        }
        assert_eq!(
            work.count(Counter::ElementsScanned),
            0,
            "availability of a selection of 100 at {n}: {}",
            work.summary()
        );
    }
}

// ---- pointer input -----------------------------------------------------------------------------------------------

/// A session, an interaction layer and a clock for scripted pointer events.
struct Pointer {
    h: Harness,
    i: Interaction,
    now: f64,
}

impl Pointer {
    fn new(n: usize) -> Self {
        let mut h = loaded(n);
        h.session.execute("view.set_viewport", json!({ "width": 800, "height": 600 })).expect("viewport");
        h.session.execute("view.set", json!({ "scroll": [400, 300], "zoom": 1 })).expect("view");
        let mut p = Self { h, i: Interaction::new(), now: 1000.0 };
        // The first event builds the index; leave nothing pending.
        p.hover(-5000.0, -5000.0);
        p.h.session.take_dirty();
        p
    }

    fn event(
        &mut self,
        phase: PointerPhase,
        x: f64,
        y: f64,
        button: Button,
        buttons: Buttons,
    ) -> drafftink_engine::interact::Outcome {
        self.now += 8.0;
        let ev = PointerEvent::new(PointerId(0), PointerKind::Mouse, phase, Point::new(x, y), self.now)
            .with_button(button, buttons)
            .with_modifiers(Modifiers::NONE);
        self.i.input(&mut self.h.session, &InputEvent::Pointer(ev))
    }

    fn hover(&mut self, x: f64, y: f64) -> drafftink_engine::interact::Outcome {
        self.event(PointerPhase::Move, x, y, Button::None, Buttons::NONE)
    }
}

/// What a shell does with an outcome: a hover that changes the outline asks for one frame.
fn shell_frame(out: &drafftink_engine::interact::Outcome) {
    if out.redraw.is_some() {
        redraw(RedrawReason::HoverChange);
    }
}

fn report(scenario: &str, n: usize, work: &WorkScope) {
    ScenarioReport::from_work(scenario, Some(n as u64), work).save_if_requested().expect("the report is written");
}

#[test]
fn hovering_over_empty_canvas_redraws_nothing() {
    for &n in sizes() {
        let mut p = Pointer::new(n);
        let work = WorkScope::begin();
        let mut redraws = 0;
        // Far outside the generated grid: nothing is under the pointer, nothing changes between moves.
        for k in 0..1000 {
            let out = p.hover(-4000.0 - f64::from(k), -4000.0);
            shell_frame(&out);
            redraws += usize::from(out.redraw.is_some());
        }
        assert_eq!(redraws, 0, "hover redraws at {n}");
        assert_eq!(p.h.session.take_dirty(), None, "the session is dirty after hovering at {n}");
        assert_eq!(work.count(Counter::ElementsScanned), 0, "hover at {n}: {}", work.summary());
        assert_eq!(work.redraws(), 0, "at {n}");
        assert_eq!(work.count(Counter::RedrawsSkipped), 1000, "every move reached the gate and was dropped at {n}");
    }
}

#[test]
fn hovering_redraws_exactly_when_the_shape_under_the_pointer_changes() {
    for &n in sizes() {
        let mut p = Pointer::new(n);
        let work = WorkScope::begin();
        // Along the first row of cells, 1000 moves: the pointer enters and leaves shapes at fixed places.
        let mut changes = 0;
        for k in 0..1000 {
            let out = p.hover(f64::from(k) * 0.8, 12.0);
            shell_frame(&out);
            changes += u64::from(out.redraw.is_some());
        }
        assert!(changes > 0, "the sweep crosses shapes at {n}");
        assert!(changes <= 50, "hover redraws only on a change, not on every move: {changes} of 1000 at {n}");
        assert_eq!(work.redraws(), changes, "one frame request per change at {n}: {}", work.summary());
        assert_eq!(work.redraws_for(RedrawReason::Other), 0, "every redraw has a reason at {n}");
        assert_eq!(work.count(Counter::RedrawsSkipped), 1000 - changes, "the rest is dropped at the gate at {n}");
        assert_eq!(work.count(Counter::ElementsScanned), 0, "hover sweep at {n}: {}", work.summary());
        report("hover", n, &work);
    }
}

#[test]
fn the_same_sweep_makes_the_same_number_of_redraws_at_every_size() {
    let mut counts = Vec::new();
    for &n in sizes() {
        let mut p = Pointer::new(n);
        let mut redraws = 0;
        for k in 0..400 {
            redraws += usize::from(p.hover(f64::from(k) * 3.0, 12.0).redraw.is_some());
        }
        counts.push(redraws);
    }
    assert!(counts.windows(2).all(|w| w[0] == w[1]), "hover redraws by scene size: {counts:?}");
}

#[test]
fn an_idle_interaction_has_no_deadline_and_no_dirty_reason() {
    for &n in sizes() {
        let mut p = Pointer::new(n);
        let work = WorkScope::begin();
        for _ in 0..100 {
            p.now += 1000.0;
            let out = p.i.tick(&mut p.h.session, p.now);
            assert!(out.redraw.is_none(), "tick asked for a redraw at {n}");
        }
        assert_eq!(p.i.next_deadline(), None, "an idle interaction wants no wake-up at {n}");
        assert_eq!(p.h.session.take_dirty(), None, "at {n}");
        assert_eq!(work.redraws(), 0, "at {n}: {}", work.summary());
        assert_eq!(work.count(Counter::ElementsScanned), 0, "at {n}: {}", work.summary());
        // `idle` has no scene size in the budgets file.
        ScenarioReport::from_work("idle", None, &work).save_if_requested().expect("the report is written");
    }
}

#[test]
fn dragging_a_shape_with_the_pointer_scans_nothing() {
    for &n in sizes() {
        let mut p = Pointer::new(n);
        // The first rectangle (e0) sits at the top left of the grid, 40 to 100 units wide.
        let (x, y) = {
            let e = p.h.session.store().get(&scene_id("e0")).expect("e0 exists").transform;
            (e.x + 2.0, e.y + 2.0)
        };
        // The rectangle is unfilled: press on its outline.
        p.event(PointerPhase::Down, x, y, Button::Primary, Buttons::PRIMARY);
        let work = WorkScope::begin();
        for k in 1..=20 {
            p.event(PointerPhase::Move, x + f64::from(k) * 3.0, y + f64::from(k) * 2.0, Button::None, Buttons::PRIMARY);
        }
        assert_eq!(work.count(Counter::ElementsScanned), 0, "drag frames at {n}: {}", work.summary());
        assert_eq!(
            work.count(Counter::HitTests),
            0,
            "nothing hit-tests while a button is down, at {n}: {}",
            work.summary()
        );
        let work = WorkScope::begin();
        p.event(PointerPhase::Up, x + 60.0, y + 40.0, Button::Primary, Buttons::NONE);
        assert_eq!(work.count(Counter::ElementsScanned), 0, "drop at {n}: {}", work.summary());
        let moved = p.h.session.store().get(&scene_id("e0")).expect("e0 exists").transform;
        assert!(moved.x > x, "the drag moved the rectangle at {n}");
    }
}

fn scene_id(s: &str) -> drafftink_scene::ElementId {
    drafftink_scene::ElementId::new(s).expect("a valid id")
}

#[test]
fn the_generated_scene_is_what_the_tests_assume() {
    // e40 is a rectangle and e44 an arrow bound to it: the shape of the cycle in `scenegen`.
    let doc = scenegen::generate(50, 1).expect("scene");
    let kinds = scenegen::expected_kind_counts(50);
    assert_eq!(kinds.get("arrow"), Some(&5));
    assert!(doc.elements.iter().any(|e| e.id.as_str() == "e44"));
}

// ---- layouts during a drag ---------------------------------------------------------------------------------------

/// The picture side of a drag frame, as the app does it: follow the commits' touched ids, then compose.
struct Drawing {
    composer: drafftink_render::SceneComposer,
    index: drafftink_render::PaintIndex,
    scene: drafftink_render::Scene,
    typesetter: drafftink_engine::services::SharedTypesetter,
    view: drafftink_render::View,
}

impl Drawing {
    /// The view of the `drag` scenario: zoom 0.5, the corner of the grid at (150, 150) on a 1280 x 800 canvas.
    fn new(h: &mut Harness) -> Self {
        let store = h.session.tab().doc.store();
        let view = drafftink_render::View::new(
            0.5,
            drafftink_geom::kurbo::Vec2::new(170.0, 170.0),
            drafftink_geom::kurbo::Size::new(1280.0, 800.0),
        );
        let mut me = Self {
            composer: drafftink_render::SceneComposer::new(),
            index: drafftink_render::build_paint_index(store),
            scene: drafftink_render::Scene::new(),
            typesetter: drafftink_engine::services::LazyTypesetter::shared(),
            view,
        };
        h.session.tab().doc.take_touched();
        me.compose(h);
        me
    }

    fn compose(&mut self, h: &mut Harness) -> drafftink_render::FrameReport {
        let doc = &h.session.tab().doc;
        let touched = doc.take_touched();
        let store = doc.store();
        let changes = drafftink_scene::ChangeSet {
            revision: doc.revision(),
            elements: touched
                .into_iter()
                .map(|id| drafftink_scene::Change {
                    id,
                    kind: drafftink_scene::ChangeKind::Updated,
                    fields: drafftink_scene::Fields::all(),
                    version: 0,
                })
                .collect(),
            meta: false,
            files: Vec::new(),
        };
        drafftink_render::update_paint_index(&mut self.index, store, &changes);
        let Self { composer, index, scene, typesetter, view } = self;
        typesetter.with(|ts| composer.compose(store, index, view, ts, scene))
    }
}

/// The point the `drag` driver presses, from `scripts/perf_scenarios.py` (`DRAG_GRAB`), so the two cannot drift.
fn drag_grab() -> (f64, f64) {
    let script =
        std::fs::read_to_string(drafftink_testkit::harness::fixtures_dir().join("../../scripts/perf_scenarios.py"))
            .expect("the driver script");
    let line = script.lines().find(|l| l.starts_with("DRAG_GRAB")).expect("DRAG_GRAB is defined");
    let nums: Vec<f64> = line
        .split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .filter(|t| !t.is_empty())
        .filter_map(|t| t.parse().ok())
        .collect();
    (nums[0], nums[1])
}

/// The `drag` scenario without a window (`apps/drafftink`'s `scene_for`): the first hundred elements selected, zoom
/// 0.5, the grid corner 150 points from the top left of a 1280 x 800 canvas, the driver's press on `DRAG_GRAB`.
#[test]
fn the_drag_scenario_moves_its_selection_and_lays_nothing_out_again() {
    for &n in sizes() {
        let mut h = loaded(n);
        h.session.execute("view.set_viewport", json!({ "width": 1280, "height": 800 })).expect("viewport");
        h.session.execute("view.set", json!({ "scroll": [920, 440], "zoom": 0.5 })).expect("view");
        let ids: Vec<String> = (0..100).map(|k| format!("e{k}")).collect();
        h.session.execute("selection.select", json!({ "ids": ids })).expect("select");
        let before: Vec<_> =
            ids.iter().map(|id| h.session.store().get(&scene_id(id)).expect("exists").transform).collect();
        let mut d = Drawing::new(&mut h);
        let mut p = Pointer { h, i: Interaction::new(), now: 1000.0 };
        let (gx, gy) = drag_grab();
        p.hover(gx, gy);
        p.event(PointerPhase::Down, gx, gy, Button::Primary, Buttons::PRIMARY);
        let work = WorkScope::begin();
        for k in 1..=20 {
            let (dx, dy) = (f64::from(k) * 4.0, f64::from(k) * 3.0);
            p.event(PointerPhase::Move, gx + dx, gy + dy, Button::None, Buttons::PRIMARY);
            d.compose(&mut p.h);
        }
        assert_eq!(work.count(Counter::Relayouts), 0, "20 drag frames at {n}: {}", work.summary());
        p.event(PointerPhase::Up, gx + 80.0, gy + 60.0, Button::Primary, Buttons::NONE);
        // Arrows bound to something that stays put change their size; every other kind keeps its size and moves.
        for (k, (id, was)) in ids.iter().zip(&before).enumerate().filter(|(k, _)| ![3, 4, 5].contains(&(k % 10))) {
            let now = p.h.session.store().get(&scene_id(id)).expect("exists").transform;
            assert_eq!((now.w, now.h), (was.w, was.h), "{id} was moved and not resized at {n}");
            assert!(now.x > was.x && now.y > was.y, "{id} (element {k}) moved at {n}: {was:?} to {now:?}");
        }
    }
}
