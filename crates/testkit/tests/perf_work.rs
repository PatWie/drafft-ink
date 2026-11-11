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
use drafftink_profile::{Counter, WorkScope};
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

#[test]
fn hovering_over_empty_canvas_redraws_nothing() {
    for &n in sizes() {
        let mut p = Pointer::new(n);
        let work = WorkScope::begin();
        let mut redraws = 0;
        // Far outside the generated grid: nothing is under the pointer, nothing changes between moves.
        for k in 0..1000 {
            let out = p.hover(-4000.0 - f64::from(k), -4000.0);
            redraws += usize::from(out.redraw.is_some());
        }
        assert_eq!(redraws, 0, "hover redraws at {n}");
        assert_eq!(p.h.session.take_dirty(), None, "the session is dirty after hovering at {n}");
        assert_eq!(work.count(Counter::ElementsScanned), 0, "hover at {n}: {}", work.summary());
        assert_eq!(work.redraws(), 0, "at {n}");
    }
}

#[test]
fn hovering_redraws_exactly_when_the_shape_under_the_pointer_changes() {
    for &n in sizes() {
        let mut p = Pointer::new(n);
        let work = WorkScope::begin();
        // Along the middle of the first row of cells: the pointer enters and leaves shapes at fixed places.
        let mut changes = Vec::new();
        for k in 0..400 {
            let x = f64::from(k) * 3.0;
            let out = p.hover(x, 12.0);
            changes.push(out.redraw.is_some());
        }
        let total = changes.iter().filter(|c| **c).count();
        assert!(total > 0, "the sweep crosses shapes at {n}");
        assert!(total < 40, "hover redraws only on a change, not on every move: {total} of 400 at {n}");
        assert_eq!(work.count(Counter::ElementsScanned), 0, "hover sweep at {n}: {}", work.summary());
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
