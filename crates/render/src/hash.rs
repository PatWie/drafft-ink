//! The content hash of an element: everything that changes how its fragment looks, and nothing else.
//!
//! Position and rotation are left out on purpose. A fragment is drawn in the element's local frame, so a moved or
//! rotated element keeps its fragment and only gets a new transform. The box size is in, because a resize changes
//! the geometry. Relations (groups, frame, bindings, text container) and labels do not change the picture.

use std::hash::Hash;

use drafftink_geom::hash::{Bits, Hash128};
use drafftink_scene::{
    Crop, Element, ElementKind, FrameData, FreehandData, ImageData, LinearData, MathData, Style, TextData, TextRun,
    TextStyle, Transform,
};
use kurbo::Point;

// Every function below destructures its input without `..`. A new field in the model is a compile error here until
// it is either hashed or named with `: _` (and a reason). That is what keeps a cache from serving a stale picture.

fn points(h: &mut Hash128, pts: &[Point]) {
    pts.len().hash(h);
    for p in pts {
        (Bits(p.x), Bits(p.y)).hash(h);
    }
}

fn style(h: &mut Hash128, s: &Style) {
    let Style {
        stroke_color,
        stroke_width,
        stroke_style,
        fill_color,
        fill_pattern,
        sloppiness,
        opacity,
        corner_radius,
        seed,
    } = s;
    (stroke_color, Bits(*stroke_width), stroke_style, fill_color, fill_pattern, sloppiness).hash(h);
    (Bits(*opacity), Bits(*corner_radius), seed).hash(h);
}

fn linear(h: &mut Hash128, d: &LinearData) {
    let LinearData {
        points: pts,
        path,
        start_binding: _, // relations: the points are already routed
        end_binding: _,
        start_head,
        end_head,
        head_size,
        fixed_segments: _, // routing input; the result is in `points`
        start_is_special: _,
        end_is_special: _,
        closed,
    } = d;
    points(h, pts);
    (path, start_head, end_head, head_size.map(Bits), closed).hash(h);
}

fn freehand(h: &mut Hash128, f: &FreehandData) {
    let FreehandData { points: pts, pressures, pressure_mode, brush, highlighter, closed } = f;
    points(h, pts);
    pressures.len().hash(h);
    for v in pressures {
        Bits(*v).hash(h);
    }
    (pressure_mode, brush, highlighter, closed).hash(h);
}

fn text(h: &mut Hash128, d: &TextData) {
    let TextData {
        content,
        runs,
        font,
        auto_resize,
        container_id: _,   // relation
        label_position: _, // placement, not paint
    } = d;
    content.hash(h);
    runs.len().hash(h);
    for TextRun { start, end, color } in runs {
        (start, end, color).hash(h);
    }
    let TextStyle { family, size, weight, align, vertical_align, line_height, inline_math } = font;
    (family, Bits(*size), weight, align, vertical_align, Bits(*line_height), auto_resize, inline_math).hash(h);
}

fn math(h: &mut Hash128, m: &MathData) {
    let MathData { latex, font_size } = m;
    (latex, Bits(*font_size)).hash(h);
}

fn image(h: &mut Hash128, i: &ImageData) {
    let ImageData { file_id, natural_width, natural_height, crop } = i;
    (file_id, natural_width, natural_height).hash(h);
    crop.as_ref()
        .map(|c| {
            let Crop { x, y, width, height } = c;
            (Bits(*x), Bits(*y), Bits(*width), Bits(*height))
        })
        .hash(h);
}

fn frame(h: &mut Hash128, f: &FrameData) {
    let FrameData { name, clip } = f;
    (name, clip).hash(h);
}

fn kind(h: &mut Hash128, k: &ElementKind) {
    std::mem::discriminant(k).hash(h);
    match k {
        ElementKind::Rectangle | ElementKind::Diamond | ElementKind::Ellipse => {}
        ElementKind::Line(d) | ElementKind::Arrow(d) => linear(h, d),
        ElementKind::Freehand(f) => freehand(h, f),
        ElementKind::Text(d) => text(h, d),
        ElementKind::Math(m) => math(h, m),
        ElementKind::Image(i) => image(h, i),
        ElementKind::Frame(f) => frame(h, f),
    }
}

/// A 128-bit digest of the paint-relevant content of `e`.
pub fn paint_hash(e: &Element) -> u128 {
    let Element {
        id: _, // identity
        kind: k,
        transform,
        style: s,
        index: _, // draw order, not looks
        group_ids: _,
        frame_id: _,
        locked: _,
        deleted: _,
        name: _,
        link: _,
        custom: _,
        version: _,
    } = e;
    // Position, rotation and flip move the finished fragment; only the size changes the geometry.
    let Transform { x: _, y: _, w, h: height, angle: _, flip_x: _, flip_y: _ } = transform;
    let mut h = Hash128::new();
    (Bits(*w), Bits(*height)).hash(&mut h);
    style(&mut h, s);
    kind(&mut h, k);
    h.finish128()
}
