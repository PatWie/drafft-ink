//! The arrowheads of the ribbon: the table of the choices, the picture of each, and the picker that lists them.
//!
//! Every head the renderer draws is here. A choice's picture is the head at the tip of a short shaft, drawn from
//! the same geometry as the canvas (`sketch::arrowhead` for the plain heads, `render::painter::cardinality_path`
//! for the entity-relationship markers), so what the picker shows is what the arrow gets.

use drafftink_scene::Head;
use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2, pos2, vec2};

/// One arrowhead on offer.
#[derive(Debug, Clone, Copy)]
pub(super) struct HeadChoice {
    /// The head.
    pub head: Head,
    /// Its name in the picker, in tooltips and in accessible names.
    pub name: &'static str,
}

/// The choices, in the order of the picker: nothing, the open and filled heads, their outlines, and the markers of
/// entity-relationship diagrams.
pub(super) const HEADS: [HeadChoice; 15] = [
    HeadChoice { head: Head::None, name: "None" },
    HeadChoice { head: Head::Arrow, name: "Arrow" },
    HeadChoice { head: Head::Bar, name: "Bar" },
    HeadChoice { head: Head::Circle, name: "Dot" },
    HeadChoice { head: Head::CircleOutline, name: "Hollow dot" },
    HeadChoice { head: Head::Triangle, name: "Triangle" },
    HeadChoice { head: Head::TriangleOutline, name: "Hollow triangle" },
    HeadChoice { head: Head::Diamond, name: "Diamond" },
    HeadChoice { head: Head::DiamondOutline, name: "Hollow diamond" },
    HeadChoice { head: Head::CardinalityOne, name: "One" },
    HeadChoice { head: Head::CardinalityMany, name: "Many" },
    HeadChoice { head: Head::CardinalityOneOrMany, name: "One or many" },
    HeadChoice { head: Head::CardinalityExactlyOne, name: "Exactly one" },
    HeadChoice { head: Head::CardinalityZeroOrOne, name: "Zero or one" },
    HeadChoice { head: Head::CardinalityZeroOrMany, name: "Zero or many" },
];

/// How many choices a row of the picker holds.
pub(super) const COLUMNS: usize = 5;

impl HeadChoice {
    /// The name the commands know the head by (`arrow.set_head`, `tool.set_arrow`): `triangle_outline`, `none`.
    pub(super) fn value(self) -> String {
        match serde_json::to_value(self.head) {
            Ok(serde_json::Value::String(name)) => name,
            _ => "none".to_owned(),
        }
    }

    /// The choice a command name stands for, if it is one.
    pub(super) fn named(value: &str) -> Option<Self> {
        HEADS.iter().copied().find(|c| c.value() == value)
    }
}

/// Which end of the shaft the head is drawn at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum End {
    /// At the left: the start of an arrow.
    Start,
    /// At the right: the end of an arrow.
    End,
}

/// The stroke thickness of a picture.
const THICKNESS: f32 = 1.5;

/// A short shaft with `head` at one end of `rect`, in `ink`. The shaft runs along the middle of the rectangle.
pub(super) fn head_sample(p: &Painter, rect: Rect, head: Head, end: End, ink: Color32) {
    let pen = Stroke::new(THICKNESS, ink);
    let y = rect.center().y;
    let margin = 2.0;
    // `dir` points away from the line, out of the tip.
    let (tip, dir, far) = match end {
        End::End => (pos2(rect.right() - margin, y), vec2(1.0, 0.0), pos2(rect.left() + margin, y)),
        End::Start => (pos2(rect.left() + margin, y), vec2(-1.0, 0.0), pos2(rect.right() - margin, y)),
    };
    let size = (rect.height() * 0.6).min(rect.width() * 0.4).max(4.0);
    let perp = vec2(-dir.y, dir.x);
    let back = tip - dir * size;
    let at = |behind: f32, across: f32| tip - dir * behind + perp * across;
    let shaft = |to: Pos2| {
        p.line_segment([far, to], pen);
    };
    let polygon = |points: Vec<Pos2>, filled: bool| {
        let fill = if filled { ink } else { Color32::TRANSPARENT };
        p.add(Shape::convex_polygon(points, fill, pen));
    };
    match head {
        Head::None => shaft(tip),
        Head::Arrow => {
            shaft(tip);
            p.line_segment([tip, back + perp * (size * 0.5)], pen);
            p.line_segment([tip, back - perp * (size * 0.5)], pen);
        }
        Head::Bar => {
            shaft(tip);
            p.line_segment([tip + perp * (size * 0.5), tip - perp * (size * 0.5)], pen);
        }
        Head::Circle | Head::CircleOutline => {
            let diameter = size * 0.7;
            shaft(tip - dir * diameter);
            p.circle(tip - dir * (diameter / 2.0), diameter / 2.0, fill_of(head, ink), pen);
        }
        Head::Triangle | Head::TriangleOutline => {
            shaft(back);
            polygon(vec![tip, at(size, size * 0.5), at(size, -size * 0.5)], head == Head::Triangle);
        }
        Head::Diamond | Head::DiamondOutline => {
            shaft(back);
            let points = vec![tip, at(size * 0.5, size * 0.3), back, at(size * 0.5, -size * 0.3)];
            polygon(points, head == Head::Diamond);
        }
        Head::CardinalityOne
        | Head::CardinalityMany
        | Head::CardinalityOneOrMany
        | Head::CardinalityExactlyOne
        | Head::CardinalityZeroOrOne
        | Head::CardinalityZeroOrMany => {
            shaft(tip);
            cardinality(p, head, Marker { tip, dir, perp, size }, pen);
        }
    }
}

fn fill_of(head: Head, ink: Color32) -> Color32 {
    if head == Head::Circle { ink } else { Color32::TRANSPARENT }
}

/// Where a marker sits: its tip, the direction out of the tip, and its size.
struct Marker {
    tip: Pos2,
    dir: Vec2,
    perp: Vec2,
    size: f32,
}

/// The strokes of an entity-relationship marker, as `render::painter::cardinality_path` lays them out.
fn cardinality(p: &Painter, head: Head, m: Marker, pen: Stroke) {
    let Marker { tip, dir, perp, size } = m;
    let at = |back: f32| tip - dir * back;
    let bar = |back: f32| {
        p.line_segment([at(back) + perp * (size * 0.4), at(back) - perp * (size * 0.4)], pen);
    };
    let foot = || {
        let root = at(size * 0.9);
        for end in [tip + perp * (size * 0.5), tip, tip - perp * (size * 0.5)] {
            p.line_segment([root, end], pen);
        }
    };
    let ring = |back: f32| {
        p.circle_stroke(at(back), size * 0.28, pen);
    };
    match head {
        Head::CardinalityOne => bar(size * 0.5),
        Head::CardinalityExactlyOne => {
            bar(size * 0.35);
            bar(size * 0.65);
        }
        Head::CardinalityMany => foot(),
        Head::CardinalityOneOrMany => {
            foot();
            bar(size * 1.15);
        }
        Head::CardinalityZeroOrOne => {
            bar(size * 0.35);
            ring(size * 0.85);
        }
        Head::CardinalityZeroOrMany => {
            foot();
            ring(size * 1.3);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_head_of_the_scene_is_on_offer_once() {
        let values: Vec<String> = HEADS.iter().map(|c| c.value()).collect();
        let mut unique = values.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), HEADS.len(), "no head twice: {values:?}");
        assert_eq!(values.first().map(String::as_str), Some("none"));
        for value in &values {
            let parsed: Result<Head, _> = serde_json::from_value(serde_json::json!(value));
            assert!(parsed.is_ok(), "{value} is a head the scene reads");
        }
        // A new `Head` makes this match fail to compile, which is the reminder to offer it.
        for choice in HEADS {
            match choice.head {
                Head::None
                | Head::Arrow
                | Head::Bar
                | Head::Circle
                | Head::CircleOutline
                | Head::Triangle
                | Head::TriangleOutline
                | Head::Diamond
                | Head::DiamondOutline
                | Head::CardinalityOne
                | Head::CardinalityMany
                | Head::CardinalityOneOrMany
                | Head::CardinalityExactlyOne
                | Head::CardinalityZeroOrOne
                | Head::CardinalityZeroOrMany => {}
            }
        }
    }

    /// The shapes `head_sample` paints for one head, as text.
    fn shapes_of(head: Head, end: End) -> String {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(60.0, 24.0));
            head_sample(ui.painter(), rect, head, end, Color32::WHITE);
        });
        output.textures_delta.clear();
        format!("{:?}", output.shapes.iter().map(|s| &s.shape).collect::<Vec<_>>())
    }

    #[test]
    fn every_head_has_a_picture_of_its_own_at_either_end() {
        for end in [End::Start, End::End] {
            let pictures: Vec<String> = HEADS.iter().map(|c| shapes_of(c.head, end)).collect();
            for (i, a) in pictures.iter().enumerate() {
                assert!(!a.is_empty());
                for (j, b) in pictures.iter().enumerate().skip(i + 1) {
                    assert_ne!(a, b, "{} and {} look the same at {end:?}", HEADS[i].name, HEADS[j].name);
                }
            }
        }
        assert_ne!(
            shapes_of(Head::Triangle, End::Start),
            shapes_of(Head::Triangle, End::End),
            "the head turns with the end"
        );
    }

    #[test]
    fn a_command_name_finds_its_choice() {
        assert_eq!(HeadChoice::named("triangle_outline").map(|c| c.name), Some("Hollow triangle"));
        assert_eq!(HeadChoice::named("cardinality_zero_or_many").map(|c| c.head), Some(Head::CardinalityZeroOrMany));
        assert!(HeadChoice::named("mixed").is_none());
    }

    #[test]
    fn the_names_are_distinct_for_tooltips() {
        let mut names: Vec<&str> = HEADS.iter().map(|c| c.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), HEADS.len());
    }
}
