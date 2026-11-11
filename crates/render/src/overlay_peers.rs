//! The other people in a room: their pointers with name labels, what they have selected, and their laser trails.
//! All in the colour each of them has chosen.

use drafftink_geom::OrientedBox;
use kurbo::{Affine, BezPath, Cap, Circle, Join, Point, Size, Stroke, Vec2};
use peniko::{Color, Fill};
use vello::Scene;

use crate::overlay::{Pen, RemoteCursor};
use crate::overlay_text::Label;

/// Most boxes drawn for one person's selection, and most points for one trail. The description comes from a
/// network peer, so the painter bounds its own work.
const MAX_BOXES: usize = 256;
const MAX_TRAIL_POINTS: usize = 256;
/// Gap between a selected element and the outline of a remote selection, in logical pixels.
const OUTLINE_PAD: f64 = 4.0;
/// Where the name label sits relative to the tip of the pointer, in logical pixels.
const LABEL_OFFSET: Vec2 = Vec2::new(14.0, 16.0);

/// What another person has selected.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteSelection {
    /// Their colour.
    pub color: Color,
    /// One box per selected element, in world space.
    pub boxes: Vec<OrientedBox>,
}

/// One point of a trail.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrailPoint {
    /// World position.
    pub at: Point,
    /// Milliseconds since the point was recorded.
    pub age_ms: f64,
}

/// The fading path of someone's laser pointer.
#[derive(Clone, Debug, PartialEq)]
pub struct LaserTrail {
    /// Their colour.
    pub color: Color,
    /// The path, oldest first.
    pub points: Vec<TrailPoint>,
}

/// Outlines of what others have selected: a solid line in their colour, a few pixels outside each element.
pub(crate) fn paint_selections(scene: &mut Scene, pen: &Pen, selections: &[RemoteSelection]) -> usize {
    let pad = OUTLINE_PAD / pen.zoom.max(1e-9);
    let mut drawn = 0;
    for s in selections {
        for b in s.boxes.iter().filter(|b| b.is_valid()).take(MAX_BOXES) {
            let grown =
                OrientedBox::new(b.center, Size::new(b.size.width + 2.0 * pad, b.size.height + 2.0 * pad), b.angle);
            let corners = grown.corners().map(|p| pen.dev(p));
            let mut path = BezPath::new();
            path.move_to(corners[0]);
            corners[1..].iter().for_each(|p| path.line_to(*p));
            path.close_path();
            scene.stroke(&Stroke::new(2.0 * pen.px).with_join(Join::Round), Affine::IDENTITY, s.color, None, &path);
            drawn += 1;
        }
    }
    drawn
}

/// Laser trails: the older a segment, the thinner and fainter, with a dot at the newest point.
pub(crate) fn paint_trails(scene: &mut Scene, pen: &Pen, trails: &[LaserTrail]) -> usize {
    let fade = pen.style.laser_fade_ms;
    let fade = if fade.is_finite() && fade > 0.0 { fade } else { 1500.0 };
    let mut drawn = 0;
    for t in trails {
        let live: Vec<&TrailPoint> =
            t.points.iter().filter(|p| p.at.x.is_finite() && p.at.y.is_finite() && p.age_ms < fade).collect();
        let skip = live.len().saturating_sub(MAX_TRAIL_POINTS);
        let live = &live[skip..];
        let strength = |p: &TrailPoint| (1.0 - p.age_ms.max(0.0) / fade).clamp(0.0, 1.0);
        for pair in live.windows(2) {
            let [a, b] = pair else { continue };
            let k = strength(b);
            let mut seg = BezPath::new();
            seg.move_to(pen.dev(a.at));
            seg.line_to(pen.dev(b.at));
            let stroke = Stroke::new((1.0 + 4.0 * k) * pen.px).with_caps(Cap::Round);
            scene.stroke(&stroke, Affine::IDENTITY, t.color.with_alpha(k as f32), None, &seg);
        }
        if let Some(head) = live.last() {
            let dot = Circle::new(pen.dev(head.at), 3.0 * pen.px);
            scene.fill(Fill::NonZero, Affine::IDENTITY, t.color.with_alpha(strength(head) as f32), None, &dot);
            drawn += 1;
        }
    }
    drawn
}

/// Pointers: an arrow in the person's colour with a white edge, and their name in a label beside it.
pub(crate) fn paint_cursors(scene: &mut Scene, pen: &mut Pen, cursors: &[RemoteCursor]) -> usize {
    let px = pen.px;
    for cur in cursors {
        let c = pen.dev(cur.at);
        let mut path = BezPath::new();
        path.move_to(c);
        path.line_to(c + Vec2::new(0.0, 18.0 * px));
        path.line_to(c + Vec2::new(12.0 * px, 13.0 * px));
        path.close_path();
        scene.fill(Fill::NonZero, Affine::IDENTITY, cur.color, None, &path);
        scene.stroke(&Stroke::new(1.5 * px), Affine::IDENTITY, Color::WHITE, None, &path);
        if let Some(label) = cur.name.as_deref().and_then(|name| Label::new(pen, name)) {
            label.draw_chip(scene, px, c + LABEL_OFFSET * px, cur.color);
        }
    }
    cursors.len()
}
