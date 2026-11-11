//! Fingers and a stylus on the canvas, from the shell's pointer samples to the document. The samples are the ones the
//! web host sends for `pointerType` "touch" and "pen" (`crates/gpu/src/web/pointer.rs`); the path from there on is the
//! real one: `input::pointer_events`, the recognizer, the tools and the command registry.

use drafftink_engine::ports::fake::Fakes;
use drafftink_gpu::{PointerKind, PointerPoint, PointerSample};
use drafftink_scene::{Element, Kind};
use serde_json::json;

use super::*;
use crate::platform::{NoHost, Platform, SharedClipboard};

const SIZE: Size = Size::new(1180.0, 820.0);

/// An app on an iPad-sized canvas, with a fake clock.
struct Rig {
    app: App,
    fakes: Fakes,
}

impl Rig {
    fn new() -> Self {
        let fakes = Fakes::new();
        let platform = Platform {
            ports: fakes.ports(),
            host: Box::new(NoHost),
            wake: WakeCell::new(),
            clipboard: SharedClipboard::new(),
            typesetter: drafftink_engine::services::LazyTypesetter::shared(),
            bridge: None,
            mcp_http: None,
            is_mac: true,
        };
        let mut app = App::new(platform, AppConfig::default());
        app.canvas_rect =
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(SIZE.width as f32, SIZE.height as f32));
        app.sync_viewport(SIZE, 2.0);
        let mut rig = Self { app, fakes };
        // The first document is the welcome scene; these tests count what they draw.
        rig.run("doc.new", json!({}));
        rig.run("view.set", json!({ "scroll": [0, 0], "zoom": 1 }));
        rig.tool("select");
        // The first event brings the interaction layer up to date with the session (the camera, the tool); a
        // frame does it in the app.
        rig.send(PointerPhase::Hover, PointerKind::Mouse, 0, (1.0, 1.0), None);
        rig
    }

    fn run(&mut self, id: &str, args: serde_json::Value) -> serde_json::Value {
        match self.app.session.execute(id, args) {
            Ok(v) => v,
            Err(r) => panic!("{id} refused: {} ({})", r.message, r.code),
        }
    }

    fn tool(&mut self, tool: &str) {
        self.run("tool.set", json!({ "tool": tool }));
    }

    fn wait(&mut self, ms: u64) {
        self.fakes.clock.advance_ms(ms);
    }

    /// One sample, a few milliseconds after the previous one. Nothing is over egui: the canvas gets everything.
    fn send(&mut self, phase: PointerPhase, kind: PointerKind, id: u64, at: (f32, f32), pressure: Option<f32>) {
        self.wait(8);
        let point = PointerPoint { pos: egui::pos2(at.0, at.1), pressure, time_ms: self.app.now_ms() };
        let mut sample = PointerSample::new(phase, kind, point, egui::Modifiers::NONE);
        sample.id = id;
        sample.button = matches!(phase, PointerPhase::Down | PointerPhase::Up).then_some(0);
        self.app.canvas_pointer(&sample, false);
    }

    fn finger(&mut self, phase: PointerPhase, id: u64, at: (f32, f32)) {
        self.send(phase, PointerKind::Touch, id, at, None);
    }

    fn pen(&mut self, phase: PointerPhase, at: (f32, f32), pressure: f32) {
        self.send(phase, PointerKind::Pen, 7, at, Some(pressure));
    }

    /// A finger from `from` to `to` in small steps.
    fn finger_drag(&mut self, id: u64, from: (f32, f32), to: (f32, f32)) {
        self.finger(PointerPhase::Down, id, from);
        for i in 1..=10 {
            let t = i as f32 / 10.0;
            self.finger(PointerPhase::Move, id, (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t));
        }
        self.finger(PointerPhase::Up, id, to);
    }

    /// The live elements, back to front.
    fn elements(&self) -> Vec<Element> {
        self.app.session.store().iter().cloned().collect()
    }

    fn kinds(&self) -> Vec<Kind> {
        self.elements().iter().map(Element::tag).collect()
    }

    /// A filled rectangle that covers 400..600 by 300..420 on the screen. The camera starts with the world origin at
    /// the middle of the canvas.
    fn rectangle(&mut self) {
        let (x, y) = (400.0 - SIZE.width / 2.0, 300.0 - SIZE.height / 2.0);
        self.run(
            "element.create",
            json!({ "kind": "rectangle", "bounds": { "x": x, "y": y, "w": 200, "h": 120 }, "style": { "fill_color": "#ffd8a8" } }),
        );
    }

    fn camera(&self) -> Camera {
        self.app.interaction.camera()
    }

    /// The pressures of the only element, which must be a stroke.
    fn pressures(&self) -> Vec<f64> {
        let elements = self.elements();
        let [stroke] = elements.as_slice() else { panic!("one stroke: {elements:?}") };
        match &stroke.kind {
            ElementKind::Freehand(data) => data.pressures.clone(),
            other => panic!("not a stroke: {other:?}"),
        }
    }
}

#[test]
fn a_one_finger_drag_draws_with_the_active_tool() {
    let mut rig = Rig::new();
    rig.tool("rectangle");
    rig.finger_drag(1, (300.0, 300.0), (520.0, 420.0));
    assert_eq!(rig.kinds(), [Kind::Rectangle]);
}

#[test]
fn a_one_finger_drag_with_freehand_draws_a_stroke_without_pressures() {
    let mut rig = Rig::new();
    rig.tool("freehand");
    rig.finger_drag(1, (300.0, 300.0), (600.0, 340.0));
    assert!(rig.pressures().is_empty(), "a finger has no pressure; the stroke simulates one");
}

#[test]
fn a_pen_drag_draws_and_its_pressure_reaches_the_stroke() {
    let mut rig = Rig::new();
    rig.tool("freehand");
    rig.pen(PointerPhase::Down, (300.0, 300.0), 0.2);
    for (i, pressure) in [0.4_f32, 0.6, 0.95, 0.7, 0.3].into_iter().enumerate() {
        let x = 330.0 + 30.0 * i as f32;
        rig.pen(PointerPhase::Move, (x, 300.0 + 12.0 * (i % 2) as f32), pressure);
    }
    rig.pen(PointerPhase::Up, (480.0, 300.0), 0.2);

    let pressures = rig.pressures();
    assert!(pressures.len() >= 6, "{pressures:?}");
    assert!(pressures.iter().any(|p| (*p - 0.95).abs() < 1e-6), "the pen's own pressure: {pressures:?}");
}

#[test]
fn the_points_a_browser_merged_into_one_move_keep_their_pressure() {
    let mut rig = Rig::new();
    rig.tool("freehand");
    rig.pen(PointerPhase::Down, (300.0, 300.0), 0.3);
    rig.wait(30);
    let now = rig.app.now_ms();
    let mut sample = PointerSample::new(
        PointerPhase::Move,
        PointerKind::Pen,
        PointerPoint { pos: egui::pos2(380.0, 300.0), pressure: Some(0.5), time_ms: now },
        egui::Modifiers::NONE,
    );
    sample.id = 7;
    // The browser merged three moves of the last 24 milliseconds into this one, 8 milliseconds apart.
    // (Not on a straight line: the stored stroke is simplified, and a straight run keeps only its ends.)
    sample.coalesced = [(320.0, 330.0, 0.9_f32, 24.0), (340.0, 290.0, 0.8, 16.0), (360.0, 340.0, 0.7, 8.0)]
        .into_iter()
        .map(|(x, y, p, ago)| PointerPoint { pos: egui::pos2(x, y), pressure: Some(p), time_ms: now - ago })
        .collect();
    rig.app.canvas_pointer(&sample, false);
    rig.pen(PointerPhase::Up, (380.0, 300.0), 0.0);

    let pressures = rig.pressures();
    assert!(pressures.iter().any(|p| (*p - 0.9).abs() < 1e-6), "{pressures:?}");
    assert!(pressures.iter().any(|p| (*p - 0.7).abs() < 1e-6), "{pressures:?}");
}

#[test]
fn a_tap_on_an_element_selects_it() {
    let mut rig = Rig::new();
    rig.rectangle();
    rig.finger(PointerPhase::Down, 1, (500.0, 360.0));
    rig.finger(PointerPhase::Up, 1, (500.0, 360.0));
    assert_eq!(rig.app.session.selection().len(), 1, "the tapped rectangle is selected");
}

#[test]
fn a_one_finger_drag_with_select_moves_the_element() {
    let mut rig = Rig::new();
    rig.rectangle();
    let before = rig.elements()[0].transform;
    rig.finger_drag(1, (500.0, 360.0), (560.0, 400.0));
    let after = rig.elements();
    assert_eq!(after.len(), 1);
    assert_ne!(after[0].transform, before, "the element moved");
}

#[test]
fn a_finger_held_on_an_element_opens_the_context_menu() {
    let mut rig = Rig::new();
    rig.rectangle();
    rig.finger(PointerPhase::Down, 1, (500.0, 360.0));
    assert!(rig.app.interaction.next_deadline().is_some(), "the press asks for a wake-up at the long-press time");
    // The shell wakes the app at that time and the frame ticks the interaction layer (`App::frame`); here the next
    // sample of the held finger, a little later and still inside the slop, does the same.
    rig.wait(600);
    rig.finger(PointerPhase::Move, 1, (501.0, 360.0));
    assert!(rig.app.ui.state.context_menu.is_some(), "a long press is a right click");
    rig.finger(PointerPhase::Up, 1, (501.0, 360.0));
    assert!(rig.app.ui.state.context_menu.is_some(), "and the release does not close it");
}

#[test]
fn two_fingers_pan_and_pinch_and_draw_nothing() {
    let mut rig = Rig::new();
    rig.tool("freehand");
    let before = rig.camera();
    rig.finger(PointerPhase::Down, 1, (500.0, 400.0));
    rig.finger(PointerPhase::Down, 2, (600.0, 400.0));
    for i in 1..=10 {
        // The fingers move apart and the pair moves to the right.
        let d = i as f32 * 10.0;
        rig.finger(PointerPhase::Move, 1, (500.0 + d, 400.0));
        rig.finger(PointerPhase::Move, 2, (600.0 + 3.0 * d, 400.0));
    }
    rig.finger(PointerPhase::Up, 2, (900.0, 400.0));
    rig.finger(PointerPhase::Up, 1, (600.0, 400.0));

    let after = rig.camera();
    assert!(after.zoom > before.zoom, "pinching out zooms in: {} -> {}", before.zoom, after.zoom);
    assert_ne!(after.scroll, before.scroll, "the view moved with the fingers");
    assert!(rig.kinds().is_empty(), "no stroke");
    assert!(!rig.app.interaction.is_busy(), "the gesture ended");
}

#[test]
fn a_finger_that_lands_with_the_pen_down_does_not_draw_and_the_stroke_stays_whole() {
    let mut rig = Rig::new();
    rig.tool("freehand");
    rig.pen(PointerPhase::Down, (300.0, 300.0), 0.5);
    rig.finger(PointerPhase::Down, 1, (320.0, 500.0));
    for i in 1..=5 {
        rig.pen(PointerPhase::Move, (300.0 + 30.0 * i as f32, 300.0), 0.5);
        rig.finger(PointerPhase::Move, 1, (320.0 + 10.0 * i as f32, 500.0));
    }
    rig.finger(PointerPhase::Up, 1, (370.0, 500.0));
    rig.pen(PointerPhase::Up, (450.0, 300.0), 0.0);
    assert_eq!(rig.kinds(), [Kind::Freehand], "only the pen's stroke");
}

#[test]
fn a_palm_that_landed_before_the_pen_is_dropped_when_the_pen_touches_down() {
    let mut rig = Rig::new();
    rig.tool("freehand");
    rig.finger(PointerPhase::Down, 1, (700.0, 600.0));
    rig.finger(PointerPhase::Move, 1, (704.0, 603.0));
    rig.pen(PointerPhase::Down, (300.0, 300.0), 0.5);
    for i in 1..=5 {
        rig.pen(PointerPhase::Move, (300.0 + 30.0 * i as f32, 300.0), 0.5);
    }
    rig.pen(PointerPhase::Up, (450.0, 300.0), 0.0);
    rig.finger(PointerPhase::Up, 1, (704.0, 603.0));
    assert_eq!(rig.kinds(), [Kind::Freehand], "the pen's stroke, and no mark from the palm");
}

#[test]
fn after_the_pen_a_finger_only_pans_until_the_grace_time_is_over() {
    let mut rig = Rig::new();
    rig.tool("freehand");
    rig.pen(PointerPhase::Hover, (300.0, 300.0), 0.0);
    let before = rig.camera();
    rig.finger_drag(1, (600.0, 500.0), (500.0, 450.0));
    assert!(rig.kinds().is_empty(), "a finger next to the pen draws nothing");
    assert_ne!(rig.camera().scroll, before.scroll, "it pans the view");

    rig.wait(2_500);
    rig.finger_drag(2, (300.0, 600.0), (500.0, 650.0));
    assert_eq!(rig.kinds(), [Kind::Freehand], "with the pen away for a while the finger draws again");
}

#[test]
fn a_finger_alone_does_not_pan_when_a_tool_is_active() {
    let mut rig = Rig::new();
    rig.tool("rectangle");
    let before = rig.camera();
    rig.finger_drag(1, (300.0, 300.0), (520.0, 420.0));
    assert_eq!(rig.camera(), before);
}
