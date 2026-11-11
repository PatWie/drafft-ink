//! Helpers shared by the integration tests.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use drafftink_canvas::{InputEvent, Intent, PointerEvent, PointerPhase};
use drafftink_canvas_js::{Core, StageOptions};
use drafftink_geom::kurbo::Size;
use serde_json::{Value, json};

/// A stage of 800 by 600 pixels at zoom 1: the world origin is at screen (400, 300).
pub fn stage() -> Core {
    stage_with(&StageOptions::default())
}

pub fn stage_with(options: &StageOptions) -> Core {
    Core::new(Size::new(800.0, 600.0), 1.0, options).unwrap()
}

/// A clean (not hand-drawn) filled rectangle.
pub fn rect(id: &str, x: f64, y: f64, w: f64, h: f64) -> Value {
    json!({"id": id, "type": "rectangle", "x": x, "y": y, "w": w, "h": h,
           "style": {"fill_color": "#ffc9c9", "sloppiness": "architect"}})
}

pub fn mouse(core: &mut Core, phase: PointerPhase, x: f64, y: f64, t: f64) -> Vec<Intent> {
    let e = PointerEvent::mouse(phase, x, y, t);
    let e = if phase == PointerPhase::Move && t < 0.0 { e.held() } else { e };
    core.handle_input(&InputEvent::Pointer(e))
}

pub fn press(core: &mut Core, x: f64, y: f64, t: f64) -> Vec<Intent> {
    core.handle_input(&InputEvent::Pointer(PointerEvent::mouse(PointerPhase::Down, x, y, t)))
}

pub fn drag_to(core: &mut Core, x: f64, y: f64, t: f64) -> Vec<Intent> {
    core.handle_input(&InputEvent::Pointer(PointerEvent::mouse(PointerPhase::Move, x, y, t).held()))
}

pub fn release(core: &mut Core, x: f64, y: f64, t: f64) -> Vec<Intent> {
    core.handle_input(&InputEvent::Pointer(PointerEvent::mouse(PointerPhase::Up, x, y, t)))
}

pub fn hover(core: &mut Core, x: f64, y: f64, t: f64) -> Vec<Intent> {
    core.handle_input(&InputEvent::Pointer(PointerEvent::mouse(PointerPhase::Move, x, y, t)))
}

/// Presses at `from`, moves in a few steps to `to`, releases.
pub fn drag(core: &mut Core, from: (f64, f64), to: (f64, f64)) {
    let mut t = 0.0;
    press(core, from.0, from.1, t);
    for i in 1..=5 {
        t += 16.0;
        let f = f64::from(i) / 5.0;
        drag_to(core, from.0 + (to.0 - from.0) * f, from.1 + (to.1 - from.1) * f, t);
    }
    release(core, to.0, to.1, t + 16.0);
}

pub fn xywh(core: &Core, id: &str) -> [f64; 4] {
    let t = core.get(id).unwrap().transform;
    [t.x, t.y, t.w, t.h]
}

/// Compares boxes up to rounding noise of the transform maths.
pub fn assert_box(actual: [f64; 4], expected: [f64; 4]) {
    for (a, e) in actual.iter().zip(expected) {
        assert!((a - e).abs() < 1e-6, "{actual:?} is not {expected:?}");
    }
}

/// The bytes of a PNG of `w` by `h` pixels, one flat colour.
pub fn png_bytes(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().unwrap();
        let pixels: Vec<u8> = (0..w * h).flat_map(|_| [200u8, 40, 40, 255]).collect();
        writer.write_image_data(&pixels).unwrap();
    }
    out
}
