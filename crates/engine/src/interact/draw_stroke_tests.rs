//! Tests of freehand, highlighter, eraser, laser, text and math.

use drafftink_canvas::PointerPhase;
use drafftink_scene::{Element, ElementId, ElementKind, Kind, SceneStore};
use serde_json::json;

use crate::interact::testutil::Rig;
use crate::interact::{Key, UiRequest};
use crate::state::Tool;

fn rig_with(tool: Tool) -> (Rig, usize, usize) {
    let mut rig = Rig::new();
    rig.tool(tool);
    let (ring, undo) = (rig.ring_ids().len(), rig.undo_depth());
    (rig, ring, undo)
}

fn stroke_of(e: &Element) -> &drafftink_scene::FreehandData {
    match &e.kind {
        ElementKind::Freehand(d) => d,
        other => panic!("not a freehand stroke: {other:?}"),
    }
}

/// A wavy stroke of `n` samples, 3 units apart along x.
fn wave(n: usize) -> Vec<(f64, f64)> {
    (0..n).map(|i| (50.0 + 3.0 * i as f64 / 2.0, 300.0 + 40.0 * (i as f64 * 0.07).sin())).collect()
}

fn world(e: &Element) -> Vec<(f64, f64)> {
    let pts = e.points().expect("a stroke has points");
    pts.iter().map(|p| e.transform.local_to_world(*p)).map(|p| (p.x, p.y)).collect()
}

// ---- freehand ---------------------------------------------------------------------------------------------------

#[test]
fn a_mouse_stroke_is_one_create_with_no_pressures() {
    let (mut rig, ring, undo) = rig_with(Tool::Freehand);
    let samples = [(100.0, 100.0), (130.0, 120.0), (160.0, 130.0), (190.0, 128.0), (220.0, 110.0), (250.0, 90.0)];
    rig.trace(&samples);

    assert_eq!(rig.commands_since(ring), ["element.create"]);
    assert_eq!(rig.undo_depth(), undo + 1);
    let e = rig.only();
    assert_eq!(e.tag(), Kind::Freehand);
    let d = stroke_of(&e);
    assert!(d.pressures.is_empty(), "a mouse has no pressure");
    assert!(!d.highlighter);
    assert!(d.points.len() >= 2);
    assert_eq!(rig.s.tool.active, Tool::Select);
    assert!(rig.preview().is_none());
    let ring_entries = rig.ring();
    let args = &ring_entries.last().expect("a ring entry").args;
    assert!(!args.contains("pressures"), "{args}");
}

#[test]
fn the_stroke_preview_grows_with_the_samples_and_the_document_stays_untouched() {
    let (mut rig, ring, undo) = rig_with(Tool::Freehand);
    rig.down(100.0, 100.0);
    rig.drag_to(120.0, 110.0);
    rig.drag_to(150.0, 140.0);
    let p = rig.preview().expect("a preview");
    assert_eq!(p.tag(), Kind::Freehand);
    assert_eq!(world(&p), [(100.0, 100.0), (120.0, 110.0), (150.0, 140.0)]);
    rig.drag_to(180.0, 150.0);
    assert_eq!(stroke_of(&rig.preview().expect("a preview")).points.len(), 4);
    assert!(rig.elements().is_empty());
    assert_eq!(rig.commands_since(ring), Vec::<String>::new());
    assert_eq!(rig.undo_depth(), undo);
    rig.up(180.0, 150.0);
    assert!(rig.preview().is_none());
}

#[test]
fn samples_closer_than_half_a_pixel_are_skipped() {
    let (mut rig, _, _) = rig_with(Tool::Freehand);
    rig.down(100.0, 100.0);
    rig.drag_to(100.2, 100.0);
    rig.drag_to(100.3, 100.1);
    rig.drag_to(110.0, 100.0);
    assert_eq!(world(&rig.preview().expect("a preview")), [(100.0, 100.0), (110.0, 100.0)]);
    let out = rig.drag_to(110.1, 100.0);
    assert_eq!(out.redraw, None, "a skipped sample changes nothing");
}

#[test]
fn a_pen_stroke_sends_the_pressure_of_every_sample() {
    let (mut rig, ring, _) = rig_with(Tool::Freehand);
    rig.pen(PointerPhase::Down, 100.0, 100.0, 0.2);
    for (i, p) in [0.4_f32, 0.6, 0.9, 0.7, 0.3].into_iter().enumerate() {
        rig.pen(PointerPhase::Move, 100.0 + 30.0 * (i as f64 + 1.0), 100.0 + 20.0 * (i as f64 % 2.0), p);
    }
    rig.pen(PointerPhase::Up, 250.0, 100.0, 0.3);

    assert_eq!(rig.commands_since(ring), ["element.create"]);
    let e = rig.only();
    let d = stroke_of(&e);
    assert_eq!(d.pressures.len(), d.points.len(), "one pressure per point");
    assert!(d.pressures.iter().all(|p| (0.0..=1.0).contains(p)));
    assert!(
        d.pressures.iter().any(|p| (*p - 0.9).abs() < 1e-6),
        "the pressure of the sample is the pen's: {:?}",
        d.pressures
    );
}

#[test]
fn a_touch_stroke_has_no_pressures() {
    use drafftink_canvas::{Buttons, InputEvent, PointerEvent};
    let (mut rig, _, _) = rig_with(Tool::Freehand);
    for (phase, x) in [
        (PointerPhase::Down, 100.0),
        (PointerPhase::Move, 140.0),
        (PointerPhase::Move, 180.0),
        (PointerPhase::Up, 180.0),
    ] {
        rig.now += 8.0;
        let mut ev = PointerEvent::touch(0, phase, x, 100.0, rig.now);
        ev.pressure = 0.9;
        if phase == PointerPhase::Move {
            ev.buttons = Buttons::PRIMARY;
        }
        rig.input(InputEvent::Pointer(ev));
    }
    assert!(stroke_of(&rig.only()).pressures.is_empty());
}

// ---- brush and pressure -------------------------------------------------------------------------------------------

/// Sets how new strokes are drawn, the way the panel does.
fn draw_with(rig: &mut Rig, brush: &str, pressure: &str) {
    rig.run(
        "style.set",
        json!({ "scope": "defaults", "props": { "freehand_style": brush, "pressure_mode": pressure } }),
    );
}

/// A mouse stroke along y = 100: `slow` steps of 1 unit, then `fast` steps of 40. One event per 16 ms.
fn slow_then_fast(rig: &mut Rig, slow: usize, fast: usize) {
    rig.down(100.0, 100.0);
    let mut x = 100.0;
    for _ in 0..slow {
        x += 1.0;
        rig.drag_to(x, 100.0);
    }
    for _ in 0..fast {
        x += 40.0;
        rig.drag_to(x, 100.0);
    }
    rig.up(x, 100.0);
}

#[test]
fn a_simulated_pressure_stroke_is_thick_where_the_mouse_was_slow_and_thin_where_it_was_fast() {
    let (mut rig, ring, _) = rig_with(Tool::Freehand);
    draw_with(&mut rig, "normal", "simulated");
    slow_then_fast(&mut rig, 60, 12);

    assert_eq!(rig.commands_since(ring + 1), ["element.create"]);
    let e = rig.only();
    let d = stroke_of(&e);
    assert_eq!(d.pressure_mode, drafftink_scene::PressureMode::Simulated);
    assert_eq!(d.pressures.len(), d.points.len(), "one pressure per point");
    assert!(d.pressures.iter().all(|p| (0.2..=1.0).contains(p)), "{:?}", d.pressures);
    let first = d.pressures.first().copied().unwrap_or_default();
    let last = d.pressures.last().copied().unwrap_or_default();
    assert!(first > 0.95, "the slow start is thick: {first}");
    assert!(last < 0.4, "the fast end is thin: {last}");
}

#[test]
fn the_simulated_pressure_shows_in_the_preview_while_drawing() {
    let (mut rig, _, _) = rig_with(Tool::Freehand);
    draw_with(&mut rig, "normal", "simulated");
    rig.down(100.0, 100.0);
    for i in 1..=8 {
        rig.drag_to(100.0 + 40.0 * f64::from(i), 100.0);
    }
    let preview = rig.preview().expect("a preview shows");
    let d = stroke_of(&preview);
    assert_eq!(d.pressures.len(), d.points.len());
    assert!(d.pressures.last().is_some_and(|p| *p < 0.8), "{:?}", d.pressures);
}

#[test]
fn a_touch_stroke_gets_the_simulated_pressure_because_the_finger_has_none() {
    use drafftink_canvas::{Buttons, InputEvent, PointerEvent};
    let (mut rig, _, _) = rig_with(Tool::Freehand);
    draw_with(&mut rig, "normal", "simulated");
    for (phase, x) in [
        (PointerPhase::Down, 100.0),
        (PointerPhase::Move, 150.0),
        (PointerPhase::Move, 200.0),
        (PointerPhase::Move, 250.0),
        (PointerPhase::Up, 250.0),
    ] {
        rig.now += 16.0;
        let mut ev = PointerEvent::touch(0, phase, x, 100.0, rig.now);
        if phase == PointerPhase::Move {
            ev.buttons = Buttons::PRIMARY;
        }
        rig.input(InputEvent::Pointer(ev));
    }
    let e = rig.only();
    let d = stroke_of(&e);
    assert_eq!(d.pressures.len(), d.points.len());
    assert!(d.pressures.iter().any(|p| *p < 1.0), "{:?}", d.pressures);
}

#[test]
fn a_pen_keeps_its_own_pressure_when_the_mode_is_simulated() {
    let (mut rig, _, _) = rig_with(Tool::Freehand);
    draw_with(&mut rig, "normal", "simulated");
    rig.pen(PointerPhase::Down, 100.0, 100.0, 0.3);
    for (i, p) in [0.35_f32, 0.8, 0.85, 0.5].into_iter().enumerate() {
        rig.pen(PointerPhase::Move, 100.0 + 60.0 * (i as f64 + 1.0), 100.0 + 25.0 * (i as f64 % 2.0), p);
    }
    rig.pen(PointerPhase::Up, 340.0, 100.0, 0.5);
    let e = rig.only();
    let d = stroke_of(&e);
    assert_eq!(d.pressures.len(), d.points.len());
    assert!(d.pressures.iter().any(|p| (*p - 0.8).abs() < 1e-6), "the pen's value is used: {:?}", d.pressures);
    assert!(d.pressures.iter().any(|p| (*p - 0.85).abs() < 1e-6), "{:?}", d.pressures);
}

#[test]
fn the_uniform_mouse_stroke_still_has_no_pressures_and_the_old_points() {
    let (mut rig, _, _) = rig_with(Tool::Freehand);
    draw_with(&mut rig, "normal", "uniform");
    slow_then_fast(&mut rig, 20, 5);
    let e = rig.only();
    assert!(stroke_of(&e).pressures.is_empty());
}

#[test]
fn the_calligraphy_brush_trails_behind_the_pointer_and_drops_the_small_moves() {
    let (mut rig, _, _) = rig_with(Tool::Freehand);
    draw_with(&mut rig, "calligraphy", "uniform");
    // 60 steps of 12 units, 16 ms apart.
    let line: Vec<(f64, f64)> = (0..60).map(|i| (100.0 + 12.0 * f64::from(i), 100.0)).collect();
    rig.trace(&line);
    let e = rig.only();
    let d = stroke_of(&e);
    assert_eq!(d.brush, drafftink_scene::Brush::Calligraphy);
    assert!(d.pressures.is_empty(), "the brush alone adds no pressure");
    let pts = world(&e);
    let end = pts.last().copied().unwrap_or_default();
    assert!(end.0 < 100.0 + 12.0 * 59.0 - 30.0, "the pen trails the pointer: {end:?}");
    assert!(end.0 > 100.0 + 12.0 * 59.0 - 120.0, "but follows it: {end:?}");
    assert!(pts.windows(2).all(|w| w[1].0 >= w[0].0 - 1e-6), "critical damping never doubles back: {pts:?}");
}

#[test]
fn the_calligraphy_brush_rounds_a_corner() {
    let (mut rig, _, _) = rig_with(Tool::Freehand);
    draw_with(&mut rig, "calligraphy", "uniform");
    let mut path: Vec<(f64, f64)> = (0..20).map(|i| (100.0 + 10.0 * f64::from(i), 100.0)).collect();
    path.extend((1..20).map(|i| (290.0, 100.0 + 10.0 * f64::from(i))));
    rig.trace(&path);
    let e = rig.only();
    let pts = world(&e);
    // The pointer turned at (290, 100). The pen cut the corner: it never reached x = 290 while at y = 100.
    assert!(pts.iter().all(|p| p.0 < 289.0 || p.1 > 101.0), "{pts:?}");
    assert!(pts.iter().any(|p| p.0 > 250.0 && p.1 > 105.0), "it turns inside the corner: {pts:?}");
}

#[test]
fn the_same_events_draw_the_same_brush_stroke() {
    let run = || {
        let (mut rig, _, _) = rig_with(Tool::Freehand);
        draw_with(&mut rig, "calligraphy", "simulated");
        rig.trace(&wave(150));
        let e = rig.only();
        (world(&e), stroke_of(&e).pressures.clone())
    };
    let (a, b) = (run(), run());
    assert_eq!(a, b);
    assert!(!a.0.is_empty() && a.0.len() == a.1.len());
}

#[test]
#[allow(non_snake_case)]
fn exception_E7_a_brush_stroke_equals_the_plain_command_given_its_points() {
    let (mut gesture, _, _) = rig_with(Tool::Freehand);
    draw_with(&mut gesture, "calligraphy", "simulated");
    gesture.trace(&wave(150));
    let e = gesture.only();
    let d = stroke_of(&e);

    let (mut plain, _, _) = rig_with(Tool::Freehand);
    draw_with(&mut plain, "calligraphy", "simulated");
    let pts: Vec<[f64; 2]> = world(&e).iter().map(|p| [p.0, p.1]).collect();
    plain.run(
        "element.create",
        json!({ "kind": "freehand", "points": pts, "pressures": d.pressures, "highlighter": false, "simplify": 0, "from_tool": true }),
    );
    assert_eq!(gesture.normalized_doc(), plain.normalized_doc());
}

#[test]
fn hostile_timing_never_breaks_the_stroke() {
    // Events at the same time, then back in time, then a day later.
    let (mut rig, _, _) = rig_with(Tool::Freehand);
    draw_with(&mut rig, "calligraphy", "simulated");
    rig.down(100.0, 100.0);
    let mut x = 100.0;
    for jump in [0.0, -500.0, 0.0, 86_400_000.0, -86_400_000.0, 1e-9, 16.0, 16.0] {
        rig.now += jump;
        x += 25.0;
        rig.drag_to(x, 100.0 + (x * 0.1).sin() * 20.0);
    }
    rig.up(x, 100.0);
    let e = rig.only();
    let d = stroke_of(&e);
    assert_eq!(d.pressures.len(), d.points.len());
    assert!(d.pressures.iter().all(|p| p.is_finite() && (0.2..=1.0).contains(p)), "{:?}", d.pressures);
    assert!(world(&e).iter().all(|p| p.0.is_finite() && p.1.is_finite()));
}

#[test]
fn a_click_with_a_brush_still_makes_one_dot() {
    let (mut rig, _, _) = rig_with(Tool::Freehand);
    draw_with(&mut rig, "calligraphy", "simulated");
    rig.click(200.0, 200.0);
    let e = rig.only();
    assert_eq!(stroke_of(&e).points.len(), 1);
}

#[test]
fn a_click_makes_a_dot() {
    let (mut rig, ring, _) = rig_with(Tool::Freehand);
    rig.click(300.0, 300.0);
    assert_eq!(rig.commands_since(ring), ["element.create"]);
    let e = rig.only();
    assert_eq!(stroke_of(&e).points.len(), 1);
    assert_eq!(world(&e), [(300.0, 300.0)]);
    assert!(rig.last.refusals.is_empty());
}

#[test]
fn escape_cancels_a_stroke_without_a_trace() {
    let (mut rig, ring, undo) = rig_with(Tool::Freehand);
    let fingerprint = rig.s.fingerprint();
    rig.down(100.0, 100.0);
    rig.drag_to(150.0, 140.0);
    rig.drag_to(200.0, 100.0);
    assert!(rig.preview().is_some());
    assert!(rig.key(Key::Escape).handled);
    assert!(rig.preview().is_none());
    rig.drag_to(220.0, 100.0);
    rig.up(220.0, 100.0);
    assert!(rig.elements().is_empty());
    assert_eq!(rig.commands_since(ring), Vec::<String>::new());
    assert_eq!(rig.undo_depth(), undo);
    assert_eq!(rig.s.fingerprint(), fingerprint);
    assert_eq!(rig.s.tool.active, Tool::Freehand);
}

#[test]
fn a_second_finger_cancels_a_one_finger_stroke() {
    use drafftink_canvas::{Buttons, InputEvent, PointerEvent};
    let (mut rig, ring, _) = rig_with(Tool::Freehand);
    let send = |rig: &mut Rig, id: u64, phase: PointerPhase, x: f64, y: f64| {
        rig.now += 8.0;
        let mut ev = PointerEvent::touch(id, phase, x, y, rig.now);
        if phase == PointerPhase::Move {
            ev.buttons = Buttons::PRIMARY;
        }
        rig.input(InputEvent::Pointer(ev))
    };
    send(&mut rig, 0, PointerPhase::Down, 100.0, 100.0);
    send(&mut rig, 0, PointerPhase::Move, 140.0, 120.0);
    send(&mut rig, 0, PointerPhase::Move, 180.0, 130.0);
    assert!(rig.preview().is_some());
    send(&mut rig, 1, PointerPhase::Down, 400.0, 300.0);
    assert!(rig.preview().is_none(), "the stroke is dropped");
    send(&mut rig, 1, PointerPhase::Up, 400.0, 300.0);
    send(&mut rig, 0, PointerPhase::Up, 180.0, 130.0);
    assert!(rig.elements().is_empty());
    assert_eq!(rig.commands_since(ring), Vec::<String>::new());
}

#[test]
fn a_recorded_style_stroke_of_five_hundred_samples_is_one_simplified_create() {
    let (mut rig, ring, undo) = rig_with(Tool::Freehand);
    let samples = wave(500);
    rig.trace(&samples);
    assert_eq!(rig.commands_since(ring), ["element.create"]);
    assert_eq!(rig.undo_depth(), undo + 1);
    let e = rig.only();
    let n = stroke_of(&e).points.len();
    assert!(n > 10 && n < 250, "simplified from 500 samples to {n}");
    let pts = world(&e);
    approx_pt(pts[0], samples[0]);
    approx_pt(*pts.last().expect("points"), *samples.last().expect("samples"));
}

fn approx_pt(a: (f64, f64), b: (f64, f64)) {
    assert!((a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6, "{a:?} != {b:?}");
}

#[test]
#[allow(non_snake_case)]
fn exception_E7_stroke_equals_plain_command() {
    // A mouse stroke.
    let samples = wave(120);
    let (mut gesture, _, _) = rig_with(Tool::Freehand);
    gesture.trace(&samples);
    let mut plain = Rig::new();
    let pts: Vec<[f64; 2]> = samples.iter().map(|p| [p.0, p.1]).collect();
    plain.run(
        "element.create",
        json!({ "kind": "freehand", "points": pts, "highlighter": false, "simplify": 2.0, "from_tool": true }),
    );
    assert_eq!(gesture.normalized_doc(), plain.normalized_doc());
    assert_eq!(gesture.s.tool.active, plain.s.tool.active);

    // A pen stroke with pressure.
    let (mut gesture, _, _) = rig_with(Tool::Freehand);
    let pressure = |i: usize| 0.2 + 0.6 * ((i as f32 * 0.1).sin().abs());
    gesture.pen(PointerPhase::Down, samples[0].0, samples[0].1, pressure(0));
    for (i, p) in samples.iter().enumerate().skip(1) {
        gesture.pen(PointerPhase::Move, p.0, p.1, pressure(i));
    }
    let last = samples.last().expect("samples");
    gesture.pen(PointerPhase::Up, last.0, last.1, pressure(samples.len() - 1));
    let mut plain = Rig::new();
    let pressures: Vec<f64> = (0..samples.len()).map(|i| f64::from(pressure(i))).collect();
    plain.run(
        "element.create",
        json!({ "kind": "freehand", "points": pts, "pressures": pressures, "highlighter": false, "simplify": 2.0, "from_tool": true }),
    );
    assert_eq!(gesture.normalized_doc(), plain.normalized_doc());
}

#[test]
#[allow(non_snake_case)]
fn exception_E7_a_highlighter_stroke_equals_the_plain_command() {
    let samples = wave(60);
    let (mut gesture, ring, _) = rig_with(Tool::Highlighter);
    gesture.trace(&samples);
    assert_eq!(gesture.commands_since(ring), ["element.create"]);
    let e = gesture.only();
    assert!(stroke_of(&e).highlighter);
    assert!(e.style.stroke_width >= 12.0);
    assert_eq!(e.style.stroke_color.a, 128);

    let mut plain = Rig::new();
    let pts: Vec<[f64; 2]> = samples.iter().map(|p| [p.0, p.1]).collect();
    plain.run(
        "element.create",
        json!({ "kind": "freehand", "points": pts, "highlighter": true, "simplify": 2.0, "from_tool": true }),
    );
    assert_eq!(gesture.normalized_doc(), plain.normalized_doc());
}

#[test]
fn the_highlighter_preview_is_already_wide_and_translucent() {
    let (mut rig, _, _) = rig_with(Tool::Highlighter);
    rig.down(100.0, 100.0);
    rig.drag_to(160.0, 120.0);
    let p = rig.preview().expect("a preview");
    assert!(p.style.stroke_width >= 12.0);
    assert_eq!(p.style.stroke_color.a, 128);
}

// ---- eraser -----------------------------------------------------------------------------------------------------

/// Boxes A and B on a line at y=150 and an unrelated C far away.
fn erase_rig() -> (Rig, ElementId, ElementId, ElementId) {
    let mut rig = Rig::new();
    let a = rig.rect(100.0, 100.0, 100.0, 100.0);
    let b = rig.rect(300.0, 100.0, 100.0, 100.0);
    let c = rig.rect(600.0, 400.0, 100.0, 100.0);
    rig.tool(Tool::Eraser);
    (rig, a, b, c)
}

fn alive(rig: &Rig, id: &ElementId) -> bool {
    rig.s.element(id).is_some_and(|e| !e.deleted)
}

#[test]
fn the_eraser_fades_what_the_path_touches_and_deletes_it_in_one_step_at_the_release() {
    let (mut rig, a, b, c) = erase_rig();
    let (ring, undo) = (rig.ring_ids().len(), rig.undo_depth());
    rig.down(50.0, 150.0);
    rig.drag_to(150.0, 150.0);
    assert_eq!(rig.parts().faded, std::slice::from_ref(&a), "A is faded as soon as the path touches it");
    assert!(alive(&rig, &a), "nothing is deleted yet");
    rig.drag_to(250.0, 150.0);
    assert_eq!(rig.parts().faded, std::slice::from_ref(&a));
    rig.drag_to(350.0, 150.0);
    let mut faded = rig.parts().faded.clone();
    faded.sort();
    let mut expected = vec![a.clone(), b.clone()];
    expected.sort();
    assert_eq!(faded, expected);
    assert_eq!(rig.i.overlay(&rig.s).faded.len(), 2, "the overlay carries the faded ids");
    assert_eq!(rig.elements().len(), 3);
    assert_eq!(rig.commands_since(ring), Vec::<String>::new());
    assert_eq!(rig.undo_depth(), undo);

    rig.up(350.0, 150.0);
    assert!(rig.parts().faded.is_empty());
    assert_eq!(rig.commands_since(ring), ["element.erase"]);
    assert_eq!(rig.undo_depth(), undo + 1);
    assert!(!alive(&rig, &a) && !alive(&rig, &b));
    assert!(alive(&rig, &c));
    assert_eq!(rig.s.tool.active, Tool::Eraser, "the eraser stays");

    rig.run("history.undo", json!({}));
    assert!(alive(&rig, &a) && alive(&rig, &b), "one undo brings both back");
}

#[test]
fn escape_restores_everything() {
    let (mut rig, a, b, c) = erase_rig();
    let (ring, undo) = (rig.ring_ids().len(), rig.undo_depth());
    let fingerprint = rig.s.fingerprint();
    rig.down(50.0, 150.0);
    rig.drag_to(150.0, 150.0);
    rig.drag_to(350.0, 150.0);
    assert!(!rig.parts().faded.is_empty());
    assert!(rig.key(Key::Escape).handled);
    assert!(rig.parts().faded.is_empty());
    rig.up(350.0, 150.0);
    assert!(alive(&rig, &a) && alive(&rig, &b) && alive(&rig, &c));
    assert_eq!(rig.commands_since(ring), Vec::<String>::new());
    assert_eq!(rig.undo_depth(), undo);
    assert_eq!(rig.s.fingerprint(), fingerprint);
}

#[test]
fn a_tap_erases_at_that_point() {
    let (mut rig, a, b, _) = erase_rig();
    let ring = rig.ring_ids().len();
    rig.click(150.0, 150.0);
    assert_eq!(rig.commands_since(ring), ["element.erase"]);
    assert!(!alive(&rig, &a));
    assert!(alive(&rig, &b));
}

#[test]
fn locked_elements_are_neither_faded_nor_erased() {
    let (mut rig, a, b, _) = erase_rig();
    rig.run("element.lock", json!({ "ids": [a.as_str()], "locked": true }));
    rig.down(50.0, 150.0);
    rig.drag_to(150.0, 150.0);
    rig.drag_to(350.0, 150.0);
    assert_eq!(rig.parts().faded, std::slice::from_ref(&b));
    rig.up(350.0, 150.0);
    assert!(alive(&rig, &a));
    assert!(!alive(&rig, &b));
}

#[test]
fn a_label_fades_with_its_container_and_a_frame_with_its_children() {
    let mut rig = Rig::new();
    let a = rig.rect(100.0, 100.0, 100.0, 100.0);
    let label = crate::testing::add_text_label(&mut rig.s, &a, "hi").expect("a label");
    let child = rig.rect(400.0, 400.0, 40.0, 40.0);
    let frame = {
        rig.run(
            "element.create",
            json!({ "kind": "frame", "bounds": { "x": 380, "y": 380, "w": 200, "h": 200 }, "select": false }),
        )["id"]
            .as_str()
            .and_then(|s| ElementId::new(s).ok())
            .expect("a frame id")
    };
    rig.tool(Tool::Eraser);
    rig.down(90.0, 150.0);
    rig.drag_to(110.0, 150.0);
    let mut faded = rig.parts().faded.clone();
    faded.sort();
    let mut expected = vec![a.clone(), label.clone()];
    expected.sort();
    assert_eq!(faded, expected, "the container and its label");

    rig.drag_to(110.0, 300.0);
    rig.drag_to(380.0, 381.0);
    rig.drag_to(385.0, 385.0);
    let faded = rig.parts().faded.clone();
    assert!(faded.contains(&frame) && faded.contains(&child), "{faded:?}");
    rig.up(385.0, 385.0);
    assert!(!alive(&rig, &a) && !alive(&rig, &label) && !alive(&rig, &frame) && !alive(&rig, &child));
}

#[test]
fn the_eraser_radius_is_in_screen_pixels() {
    use drafftink_geom::kurbo::Point;
    let (mut rig, a, _, _) = erase_rig();
    rig.run("view.set", json!({ "scroll": [150, 150], "zoom": 4 }));
    let view = *rig.s.view();
    let screen = |x: f64, y: f64| view.world_to_screen(Point::new(x, y));
    // 6 world units left of A's edge: 24 pixels away at zoom 4, out of the 10 pixel reach.
    let (from, to) = (screen(94.0, 130.0), screen(94.0, 170.0));
    rig.down(from.x, from.y);
    rig.drag_to(to.x, to.y);
    assert!(rig.parts().faded.is_empty(), "24 px away is out of reach at this zoom");
    rig.up(to.x, to.y);
    assert!(alive(&rig, &a));
    // 2 world units away: 8 pixels.
    let (from, to) = (screen(98.0, 130.0), screen(98.0, 170.0));
    rig.down(from.x, from.y);
    rig.drag_to(to.x, to.y);
    assert_eq!(rig.parts().faded, std::slice::from_ref(&a), "8 px away is within reach");
    rig.up(to.x, to.y);
    assert!(!alive(&rig, &a));
}

#[test]
#[allow(non_snake_case)]
fn exception_E8_erase_equals_plain_command() {
    let samples = [(50.0, 150.0), (90.0, 150.0), (130.0, 150.0), (170.0, 150.0), (350.0, 150.0), (380.0, 160.0)];
    let (mut gesture, _, _, _) = erase_rig();
    gesture.trace(&samples);

    let (mut plain, _, _, _) = erase_rig();
    let path: Vec<[f64; 2]> = samples.iter().map(|p| [p.0, p.1]).collect();
    plain.run("element.erase", json!({ "path": path, "radius": 10.0, "units": "screen" }));

    assert_eq!(gesture.normalized_doc(), plain.normalized_doc());
    assert_eq!(gesture.elements().len(), 1);
    assert_eq!(gesture.undo_depth(), plain.undo_depth());
}

#[test]
fn a_stroke_over_nothing_changes_nothing() {
    let (mut rig, _, _, _) = erase_rig();
    let before = rig.normalized_doc();
    let undo = rig.undo_depth();
    rig.drag((20.0, 20.0), (80.0, 60.0));
    assert_eq!(rig.normalized_doc(), before);
    assert_eq!(rig.undo_depth(), undo);
    assert!(rig.history.iter().all(|o| o.refusals.is_empty()));
}

// ---- laser ------------------------------------------------------------------------------------------------------

#[test]
#[allow(non_snake_case)]
fn exception_E12_laser_leaves_document_unchanged() {
    let mut rig = Rig::new();
    rig.rect(100.0, 100.0, 100.0, 100.0);
    rig.tool(Tool::Laser);
    let doc = rig.normalized_doc();
    let (undo, revision) = (rig.undo_depth(), rig.s.store().revision());
    let ring = rig.ring_ids().len();
    let fingerprint = rig.s.fingerprint();

    rig.down(50.0, 50.0);
    for k in 1..=10 {
        rig.drag_to(50.0 + 20.0 * f64::from(k), 50.0 + 5.0 * f64::from(k));
    }
    assert!(!rig.parts().laser.is_empty(), "a trail shows");
    rig.up(250.0, 100.0);

    assert_eq!(rig.normalized_doc(), doc);
    assert_eq!(rig.undo_depth(), undo);
    assert_eq!(rig.s.store().revision(), revision, "the store never moved");
    assert_eq!(rig.s.fingerprint(), fingerprint);
    // The only command that may be in the ring is the one that presents the trail to peers.
    let new = rig.commands_since(ring);
    assert!(new.iter().all(|c| c == "collab.laser"), "{new:?}");
    assert_eq!(rig.s.tool.active, Tool::Laser, "the laser stays");
}

#[test]
fn the_trail_is_capped_and_has_clock_times() {
    let mut rig = Rig::new();
    rig.tool(Tool::Laser);
    rig.down(10.0, 10.0);
    for k in 1..=80 {
        rig.drag_to(10.0 + 5.0 * f64::from(k), 10.0);
        rig.now -= 15.0; // 1 ms per sample: all of them are fresh
    }
    let trail = rig.parts().laser.clone();
    assert_eq!(trail.len(), 50);
    assert!(trail.windows(2).all(|w| w[0].t_ms <= w[1].t_ms), "oldest first");
    assert_eq!(trail.last().map(|p| p.at.x), Some(410.0));
}

#[test]
fn the_trail_fades_by_the_clock_and_idle_means_idle() {
    let mut rig = Rig::new();
    rig.tool(Tool::Laser);
    assert_eq!(rig.i.next_deadline(), None, "nothing runs before the laser is used");
    rig.drag((100.0, 100.0), (300.0, 200.0));
    let shown = rig.parts().laser.len();
    assert!(shown > 0);
    let deadline = rig.i.next_deadline().expect("a trail needs ticks");
    assert!(deadline <= rig.now + 16.0 + 1e-9 && deadline > rig.now - 1.0, "{deadline} vs {}", rig.now);

    let out = rig.tick(100.0);
    assert!(out.redraw.is_some(), "the fade is redrawn while the trail shows");
    assert_eq!(rig.parts().laser.len(), shown, "the points are not 330 ms old yet");
    assert!(rig.i.next_deadline().is_some());

    let out = rig.tick(250.0);
    assert!(rig.parts().laser.is_empty(), "everything is older than 330 ms now");
    assert!(out.redraw.is_some(), "the last redraw clears the trail");
    assert_eq!(rig.i.next_deadline(), None, "idle gate: no timer after the trail has faded");
    assert_eq!(rig.tick(16.0).redraw, None);
}

#[test]
fn only_points_older_than_the_fade_time_go() {
    let mut rig = Rig::new();
    rig.tool(Tool::Laser);
    rig.down(100.0, 100.0);
    rig.drag_to(120.0, 100.0);
    rig.now += 300.0;
    rig.drag_to(140.0, 100.0);
    rig.tick(40.0);
    let xs: Vec<f64> = rig.parts().laser.iter().map(|p| p.at.x).collect();
    assert_eq!(xs, [140.0], "the first two are older than 330 ms, the newest is not");
}

#[test]
fn laser_points_are_sent_at_most_every_33_ms_and_all_arrive() {
    let mut rig = Rig::new();
    rig.tool(Tool::Laser);
    let work = drafftink_profile::WorkScope::begin();
    rig.down(100.0, 100.0);
    let moves = 8_u32;
    for k in 1..=moves {
        rig.drag_to(100.0 + 10.0 * f64::from(k), 100.0);
    }
    rig.up(180.0, 100.0);
    // `collab.laser` is quiet (not in the ring) and nothing else runs during a laser stroke, so the commands
    // counted are the sends. Events are 16 ms apart: 10 events span 160 ms.
    let sends = work.count(drafftink_profile::Counter::Commands);
    assert!(sends >= 2, "sent in batches: {sends}");
    assert!(sends <= 160 / 33 + 3, "throttled to one send per 33 ms: {sends} sends");
    // Every point arrived in the local trail of the collab state, which the UI reads.
    let view = rig.s.collab_view();
    assert_eq!(view.laser.len(), moves as usize + 1, "no point is lost: {:?}", view.laser);
    let xs: Vec<f64> = view.laser.iter().map(|d| d.x).collect();
    assert_eq!(xs.first(), Some(&100.0));
    assert_eq!(xs.last(), Some(&180.0), "the end is sent at the release");
}

#[test]
fn laser_works_in_view_only_mode() {
    let mut rig = Rig::new();
    rig.run("view.set_view_only", json!({ "on": true }));
    rig.run("tool.set", json!({ "tool": "laser" }));
    rig.drag((100.0, 100.0), (300.0, 200.0));
    assert!(!rig.parts().laser.is_empty());
    assert!(rig.history.iter().all(|o| o.refusals.is_empty()));
    assert!(rig.elements().is_empty());
}

#[test]
fn escape_keeps_the_trail_and_stops_the_sending() {
    let mut rig = Rig::new();
    rig.tool(Tool::Laser);
    rig.down(100.0, 100.0);
    rig.drag_to(150.0, 100.0);
    rig.drag_to(200.0, 100.0);
    let shown = rig.parts().laser.len();
    assert!(rig.key(Key::Escape).handled);
    assert_eq!(rig.parts().laser.len(), shown, "the trail fades by the clock, not by Esc");
    assert!(rig.i.next_deadline().is_some());
    rig.up(200.0, 100.0);
    assert_eq!(rig.parts().laser.len(), shown);
}

#[test]
fn a_tool_change_does_not_drop_the_laser_trail() {
    let mut rig = Rig::new();
    rig.tool(Tool::Laser);
    rig.drag((100.0, 100.0), (300.0, 200.0));
    let shown = rig.parts().laser.len();
    rig.tool(Tool::Rectangle);
    rig.hover(10.0, 10.0);
    assert_eq!(rig.parts().laser.len(), shown);
    rig.tick(400.0);
    assert!(rig.parts().laser.is_empty());
    assert_eq!(rig.i.next_deadline(), None);
}

// ---- text and math ----------------------------------------------------------------------------------------------

fn ui_request(rig: &Rig) -> Option<UiRequest> {
    rig.history.iter().rev().find_map(|o| o.ui_request.clone())
}

fn editing(rig: &Rig) -> Option<(Option<ElementId>, bool)> {
    rig.s.text_edit.as_ref().map(|t| (t.element.clone(), t.draft))
}

#[test]
fn a_click_on_empty_canvas_with_the_text_tool_starts_a_draft_at_the_click() {
    let (mut rig, ring, _) = rig_with(Tool::Text);
    rig.click(300.0, 250.0);
    match ui_request(&rig) {
        Some(UiRequest::OpenTextEditor { element: None, at, width: None }) => assert_eq!((at.x, at.y), (300.0, 250.0)),
        other => panic!("expected an editor over a new draft, got {other:?}"),
    }
    assert_eq!(editing(&rig), Some((None, true)));
    assert!(rig.elements().is_empty(), "a draft is not in the document");
    assert!(rig.commands_since(ring).iter().all(|c| c != "element.create"));
}

#[test]
fn a_click_on_a_text_opens_it() {
    let mut rig = Rig::new();
    let out = rig.run("element.create", json!({ "kind": "text", "text": "hello", "at": [100, 100] }));
    let id = ElementId::new(out["id"].as_str().expect("an id")).expect("valid id");
    rig.tool(Tool::Text);
    rig.click(110.0, 110.0);
    match ui_request(&rig) {
        Some(UiRequest::OpenTextEditor { element: Some(e), .. }) => assert_eq!(e, id),
        other => panic!("expected an editor over the text, got {other:?}"),
    }
    assert_eq!(editing(&rig), Some((Some(id), false)));
}

#[test]
fn a_click_on_a_shape_edits_its_label() {
    let mut rig = Rig::new();
    let a = rig.rect(100.0, 100.0, 100.0, 100.0);
    rig.tool(Tool::Text);
    rig.click(150.0, 150.0);
    match ui_request(&rig) {
        Some(UiRequest::OpenTextEditor { element: Some(e), .. }) => assert_eq!(e, a),
        other => panic!("expected an editor over the shape, got {other:?}"),
    }
}

#[test]
fn a_click_on_a_line_starts_a_new_draft_because_a_line_has_no_label() {
    let mut rig = Rig::new();
    rig.run("element.create", json!({ "kind": "line", "points": [[100, 100], [300, 100]] }));
    rig.tool(Tool::Text);
    rig.click(200.0, 100.0);
    assert!(
        matches!(ui_request(&rig), Some(UiRequest::OpenTextEditor { element: None, .. })),
        "{:?}",
        ui_request(&rig)
    );
}

#[test]
fn a_text_tool_drag_on_empty_canvas_makes_a_fixed_width_box() {
    let (mut rig, _, _) = rig_with(Tool::Text);
    rig.drag((300.0, 250.0), (420.0, 300.0));
    match ui_request(&rig) {
        Some(UiRequest::OpenTextEditor { element: None, at, width: Some(w) }) => {
            assert_eq!((at.x, at.y, w), (300.0, 250.0, 120.0));
        }
        other => panic!("expected an editor over a 120 wide box, got {other:?}"),
    }
    assert_eq!(editing(&rig), Some((None, true)));
    // The text is committed with the width: it wraps and the box does not follow the content.
    let out = rig.run("text.edit_end", json!({ "text": "hello" }));
    let id = ElementId::new(out["id"].as_str().expect("an id")).expect("valid id");
    let e = rig.element(&id);
    assert!(matches!(&e.kind, drafftink_scene::ElementKind::Text(t) if !t.auto_resize));
    assert_eq!((e.transform.x, e.transform.y, e.transform.w), (300.0, 250.0, 120.0));
}

#[test]
fn a_text_tool_drag_to_the_left_and_up_still_starts_at_the_top_left() {
    let (mut rig, _, _) = rig_with(Tool::Text);
    rig.drag((420.0, 300.0), (300.0, 250.0));
    match ui_request(&rig) {
        Some(UiRequest::OpenTextEditor { element: None, at, width: Some(w) }) => {
            assert_eq!((at.x, at.y, w), (300.0, 250.0, 120.0));
        }
        other => panic!("expected an editor over a 120 wide box, got {other:?}"),
    }
}

#[test]
fn a_text_tool_drag_narrower_than_a_box_places_at_the_origin_without_a_width() {
    let (mut rig, _, _) = rig_with(Tool::Text);
    rig.drag((300.0, 250.0), (308.0, 290.0));
    match ui_request(&rig) {
        Some(UiRequest::OpenTextEditor { element: None, at, width: None }) => assert_eq!((at.x, at.y), (300.0, 250.0)),
        other => panic!("expected an editor at the origin, got {other:?}"),
    }
}

#[test]
fn a_math_tool_drag_still_places_the_formula_at_the_origin() {
    let (mut rig, ring, _) = rig_with(Tool::Math);
    rig.drag((300.0, 250.0), (420.0, 300.0));
    assert_eq!(rig.commands_since(ring), ["element.create"]);
    assert_eq!(rig.elements().len(), 1);
}

#[test]
fn a_cancelled_text_tool_drag_starts_nothing() {
    let (mut rig, _, _) = rig_with(Tool::Text);
    rig.down(300.0, 250.0);
    rig.drag_to(420.0, 300.0);
    rig.key(Key::Escape);
    rig.up(420.0, 300.0);
    assert_eq!(ui_request(&rig), None);
    assert_eq!(editing(&rig), None);
}

#[test]
fn a_math_click_on_empty_canvas_creates_the_placeholder_and_opens_no_editor() {
    let (mut rig, ring, undo) = rig_with(Tool::Math);
    rig.click(300.0, 250.0);
    assert_eq!(rig.commands_since(ring), ["element.create"]);
    assert_eq!(rig.undo_depth(), undo + 1);
    let e = rig.only();
    assert_eq!(e.tag(), Kind::Math);
    assert_eq!((e.transform.x, e.transform.y), (300.0, 250.0));
    match &e.kind {
        ElementKind::Math(m) => assert_eq!(m.latex, "x^2 + y^2 = r^2"),
        other => panic!("not math: {other:?}"),
    }
    assert_eq!(rig.selected(), [e.id.as_str()]);
    assert_eq!(ui_request(&rig), None, "the old behaviour: no editor");
    assert_eq!(rig.s.tool.active, Tool::Select);
}

#[test]
fn a_math_click_on_a_formula_opens_the_equation_editor() {
    let mut rig = Rig::new();
    let out = rig.run("element.create", json!({ "kind": "math", "at": [100, 100] }));
    let id = ElementId::new(out["id"].as_str().expect("an id")).expect("valid id");
    rig.tool(Tool::Math);
    rig.click(110.0, 110.0);
    assert_eq!(ui_request(&rig), Some(UiRequest::OpenEquationEditor { element: id.clone() }));
    assert_eq!(editing(&rig), Some((Some(id), false)));
    assert_eq!(rig.elements().len(), 1, "no second formula");
}

#[test]
fn the_text_tool_opens_the_equation_editor_for_a_formula_too() {
    let mut rig = Rig::new();
    let out = rig.run("element.create", json!({ "kind": "math", "at": [100, 100] }));
    let id = ElementId::new(out["id"].as_str().expect("an id")).expect("valid id");
    rig.tool(Tool::Text);
    rig.click(110.0, 110.0);
    assert_eq!(ui_request(&rig), Some(UiRequest::OpenEquationEditor { element: id }));
}
