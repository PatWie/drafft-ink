//! The content hash of an element: everything that changes how its fragment looks, and nothing else.
//!
//! Position and rotation are left out on purpose. A fragment is drawn in the element's local frame, so a moved or
//! rotated element keeps its fragment and only gets a new transform. The box size is in, because a resize changes
//! the geometry. Relations (groups, frame, bindings, text container) and labels do not change the picture.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use drafftink_scene::{Color, Element, ElementKind, LinearData, Style};

struct Wide(DefaultHasher, DefaultHasher);

impl Wide {
    fn new() -> Self {
        let mut a = DefaultHasher::new();
        let mut b = DefaultHasher::new();
        0x5eed_u16.hash(&mut a);
        0xfeed_u16.hash(&mut b);
        Self(a, b)
    }

    fn u8(&mut self, v: u8) {
        v.hash(&mut self.0);
        v.hash(&mut self.1);
    }

    fn u32(&mut self, v: u32) {
        v.hash(&mut self.0);
        v.hash(&mut self.1);
    }

    fn f(&mut self, v: f64) {
        // -0.0 and 0.0 draw the same.
        let bits = if v == 0.0 { 0 } else { v.to_bits() };
        bits.hash(&mut self.0);
        bits.hash(&mut self.1);
    }

    fn str(&mut self, s: &str) {
        s.hash(&mut self.0);
        s.hash(&mut self.1);
    }

    fn color(&mut self, c: Color) {
        self.u32(u32::from_be_bytes([c.r, c.g, c.b, c.a]));
    }

    fn finish(&self) -> u128 {
        (u128::from(self.0.finish()) << 64) | u128::from(self.1.finish())
    }
}

fn style(h: &mut Wide, s: &Style) {
    h.color(s.stroke_color);
    h.f(s.stroke_width);
    h.u8(s.stroke_style as u8);
    match s.fill_color {
        Some(c) => {
            h.u8(1);
            h.color(c);
        }
        None => h.u8(0),
    }
    h.u8(s.fill_pattern as u8);
    h.u8(s.sloppiness as u8);
    h.f(s.opacity);
    h.f(s.corner_radius);
    h.u32(s.seed);
}

fn linear(h: &mut Wide, d: &LinearData) {
    h.u32(d.points.len() as u32);
    for p in &d.points {
        h.f(p.x);
        h.f(p.y);
    }
    h.u8(d.path as u8);
    h.u8(d.start_head as u8);
    h.u8(d.end_head as u8);
    match d.head_size {
        Some(v) => {
            h.u8(1);
            h.f(v);
        }
        None => h.u8(0),
    }
    h.u8(u8::from(d.closed));
}

/// A 128-bit digest of the paint-relevant content of `e`.
pub fn paint_hash(e: &Element) -> u128 {
    let mut h = Wide::new();
    let t = &e.transform;
    h.f(t.w);
    h.f(t.h);
    style(&mut h, &e.style);
    match &e.kind {
        ElementKind::Rectangle => h.u8(0),
        ElementKind::Diamond => h.u8(1),
        ElementKind::Ellipse => h.u8(2),
        ElementKind::Line(d) => {
            h.u8(3);
            linear(&mut h, d);
        }
        ElementKind::Arrow(d) => {
            h.u8(4);
            linear(&mut h, d);
        }
        ElementKind::Freehand(f) => {
            h.u8(5);
            h.u32(f.points.len() as u32);
            for p in &f.points {
                h.f(p.x);
                h.f(p.y);
            }
            h.u32(f.pressures.len() as u32);
            for v in &f.pressures {
                h.f(*v);
            }
            h.u8(f.pressure_mode as u8);
            h.u8(f.brush as u8);
            h.u8(u8::from(f.highlighter));
            h.u8(u8::from(f.closed));
        }
        ElementKind::Text(d) => {
            h.u8(6);
            h.str(&d.content);
            h.u32(d.runs.len() as u32);
            for r in &d.runs {
                h.u32(r.start);
                h.u32(r.end);
                match r.color {
                    Some(c) => {
                        h.u8(1);
                        h.color(c);
                    }
                    None => h.u8(0),
                }
            }
            h.u8(d.font.family as u8);
            h.f(d.font.size);
            h.u8(d.font.weight as u8);
            h.u8(d.font.align as u8);
            h.u8(d.font.vertical_align as u8);
            h.f(d.font.line_height);
            h.u8(u8::from(d.auto_resize));
        }
        ElementKind::Math(m) => {
            h.u8(7);
            h.str(&m.latex);
            h.f(m.font_size);
        }
        ElementKind::Image(i) => {
            h.u8(8);
            match &i.file_id {
                Some(f) => {
                    h.u8(1);
                    h.str(f.as_str());
                }
                None => h.u8(0),
            }
            h.u32(i.natural_width);
            h.u32(i.natural_height);
            match &i.crop {
                Some(c) => {
                    h.u8(1);
                    h.f(c.x);
                    h.f(c.y);
                    h.f(c.width);
                    h.f(c.height);
                }
                None => h.u8(0),
            }
        }
        ElementKind::Frame(f) => {
            h.u8(9);
            match &f.name {
                Some(n) => {
                    h.u8(1);
                    h.str(n);
                }
                None => h.u8(0),
            }
            h.u8(u8::from(f.clip));
        }
    }
    h.finish()
}
