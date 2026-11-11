//! Small pictures for the ribbon's buttons that have no icon in `icons.rs`: a sample of a stroke style, a
//! tile of a fill pattern, the lines of a text alignment. Each one is painted into a square and takes the
//! colour of the button's ink, so it follows the theme and the selected look.

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};

/// How a line is drawn, as the stroke style property names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LineStyle {
    Solid,
    Dashed,
    Dotted,
}

/// A horizontal sample of a stroke style across the middle of `rect`.
pub(super) fn line_sample(p: &Painter, rect: Rect, style: LineStyle, ink: Color32) {
    let y = rect.center().y;
    let (from, to) = (rect.left() + rect.width() * 0.12, rect.right() - rect.width() * 0.12);
    let stroke = Stroke::new(2.0, ink);
    match style {
        LineStyle::Solid => {
            p.line_segment([pos2(from, y), pos2(to, y)], stroke);
        }
        LineStyle::Dashed => {
            p.extend(Shape::dashed_line(&[pos2(from, y), pos2(to, y)], stroke, 4.0, 3.0));
        }
        LineStyle::Dotted => {
            p.extend(Shape::dotted_line(&[pos2(from, y), pos2(to, y)], ink, 3.5, 1.0));
        }
    }
}

/// How a shape is filled, as the fill pattern property names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Pattern {
    Solid,
    Hachure,
    Zigzag,
    CrossHatch,
    Dots,
    Dashed,
    ZigzagLine,
}

impl Pattern {
    /// The patterns, in the order of the ribbon, with the value `style.set` takes and the name shown.
    pub(super) const ALL: [(Pattern, &'static str, &'static str); 7] = [
        (Pattern::Solid, "solid", "Solid"),
        (Pattern::Hachure, "hachure", "Hatch"),
        (Pattern::CrossHatch, "cross_hatch", "Cross-hatch"),
        (Pattern::Dots, "dots", "Dots"),
        (Pattern::Zigzag, "zigzag", "Zigzag"),
        (Pattern::Dashed, "dashed", "Dashed hatch"),
        (Pattern::ZigzagLine, "zigzag_line", "Zigzag lines"),
    ];
}

/// A tile filled with the pattern.
pub(super) fn pattern_tile(p: &Painter, rect: Rect, pattern: Pattern, ink: Color32) {
    let tile = rect.shrink(rect.width() * 0.04);
    let p = p.with_clip_rect(tile);
    let thin = Stroke::new(1.0, ink);
    match pattern {
        Pattern::Solid => {
            p.rect_filled(tile, 2.0, ink);
        }
        Pattern::Hachure => diagonals(&p, tile, thin, 4.5, false),
        Pattern::CrossHatch => {
            diagonals(&p, tile, thin, 5.0, false);
            diagonals(&p, tile, thin, 5.0, true);
        }
        Pattern::Dashed => {
            for line in diagonal_lines(tile, 5.0, false) {
                p.extend(Shape::dashed_line(&line, thin, 2.5, 2.0));
            }
        }
        Pattern::Dots => {
            let step = 5.0;
            let mut y = tile.top() + step / 2.0;
            let mut row = 0;
            while y < tile.bottom() {
                let mut x = tile.left() + if row % 2 == 0 { step / 2.0 } else { step };
                while x < tile.right() {
                    p.circle_filled(pos2(x, y), 0.9, ink);
                    x += step;
                }
                y += step;
                row += 1;
            }
        }
        Pattern::Zigzag => {
            for line in diagonal_lines(tile, 6.0, false) {
                let points = zigzag(line[0], line[1], 2.0);
                p.add(Shape::line(points, thin));
            }
        }
        Pattern::ZigzagLine => {
            let mut y = tile.top() + 3.0;
            while y < tile.bottom() {
                let points = zigzag(pos2(tile.left(), y), pos2(tile.right(), y), 1.6);
                p.add(Shape::line(points, thin));
                y += 5.0;
            }
        }
    }
    p.rect_stroke(tile, 2.0, Stroke::new(1.0, ink), egui::StrokeKind::Inside);
}

/// Lines at 45 degrees across `rect`, `gap` apart, from the lower left to the upper right (or mirrored).
fn diagonal_lines(rect: Rect, gap: f32, mirrored: bool) -> Vec<[Pos2; 2]> {
    let span = rect.width() + rect.height();
    let count = (span / gap).ceil() as usize;
    (0..=count)
        .map(|i| {
            let offset = i as f32 * gap;
            if mirrored {
                [pos2(rect.right() - offset, rect.top()), pos2(rect.right() - offset + rect.height(), rect.bottom())]
            } else {
                [pos2(rect.left() + offset, rect.top()), pos2(rect.left() + offset - rect.height(), rect.bottom())]
            }
        })
        .collect()
}

fn diagonals(p: &Painter, rect: Rect, stroke: Stroke, gap: f32, mirrored: bool) {
    for line in diagonal_lines(rect, gap, mirrored) {
        p.line_segment(line, stroke);
    }
}

/// A zigzag from `from` to `to` with teeth `amplitude` high.
fn zigzag(from: Pos2, to: Pos2, amplitude: f32) -> Vec<Pos2> {
    let length = (to - from).length().max(1.0);
    let direction = (to - from) / length;
    let normal = vec2(-direction.y, direction.x);
    let teeth = (length / 3.0).ceil() as usize;
    (0..=teeth)
        .map(|i| {
            let along = from + direction * (length * i as f32 / teeth as f32);
            along + normal * if i % 2 == 0 { -amplitude } else { amplitude }
        })
        .collect()
}

/// Which side the lines of a text alignment hug.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Align {
    Left,
    Center,
    Right,
}

/// Four lines of text, flush left, centred or flush right.
pub(super) fn text_lines(p: &Painter, rect: Rect, align: Align, ink: Color32) {
    let stroke = Stroke::new(1.5, ink);
    let widths = [1.0_f32, 0.62, 0.9, 0.5];
    let step = rect.height() / 4.0;
    for (i, share) in widths.into_iter().enumerate() {
        let width = rect.width() * share;
        let y = rect.top() + step * (i as f32 + 0.5);
        let left = match align {
            Align::Left => rect.left(),
            Align::Center => rect.center().x - width / 2.0,
            Align::Right => rect.right() - width,
        };
        p.line_segment([pos2(left, y), pos2(left + width, y)], stroke);
    }
}

/// A corner of a shape, sharp or rounded: two sides meeting in the upper left of `rect`.
pub(super) fn corner(p: &Painter, rect: Rect, rounded: bool, ink: Color32) {
    let radius = if rounded { rect.width() * 0.55 } else { 0.0 };
    let (left, top) = (rect.left(), rect.top());
    let mut points = vec![pos2(left, rect.bottom())];
    if rounded {
        let centre = pos2(left + radius, top + radius);
        let steps = 8;
        points.extend((0..=steps).map(|i| {
            let angle = std::f32::consts::PI * (1.0 + i as f32 / steps as f32 * 0.5);
            centre + vec2(angle.cos(), angle.sin()) * radius
        }));
    } else {
        points.push(pos2(left, top));
    }
    points.push(pos2(rect.right(), top));
    p.add(Shape::line(points, Stroke::new(2.0, ink)));
}

/// Where text sits in its box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VAlign {
    Top,
    Middle,
    Bottom,
}

/// A box with two lines of text at the top, in the middle or at the bottom.
pub(super) fn box_lines(p: &Painter, rect: Rect, align: VAlign, ink: Color32) {
    p.rect_stroke(rect, 2.0, Stroke::new(1.0, ink), egui::StrokeKind::Inside);
    let inner = rect.shrink(rect.width() * 0.2);
    let gap = inner.height() * 0.28;
    let top = match align {
        VAlign::Top => inner.top(),
        VAlign::Middle => inner.center().y - gap / 2.0,
        VAlign::Bottom => inner.bottom() - gap,
    };
    let stroke = Stroke::new(1.5, ink);
    for (i, share) in [1.0_f32, 0.6].into_iter().enumerate() {
        let y = top + gap * i as f32;
        p.line_segment([pos2(inner.left(), y), pos2(inner.left() + inner.width() * share, y)], stroke);
    }
}
