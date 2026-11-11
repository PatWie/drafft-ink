//! Rectangle, diamond, ellipse and frame: press, drag, release (E6).

use drafftink_edit::create;
use drafftink_geom::guides::SmartGuide;
use drafftink_geom::kurbo::{Point, Rect, Vec2};
use drafftink_scene::Kind;
use serde_json::json;

use super::preview;
use super::snap::{self, Snaps};
use crate::interact::ctx::Ctx;
use crate::interact::events::Ev;
use crate::state::Tool;

/// The kind a shape tool draws.
fn kind_of(tool: Tool) -> Option<Kind> {
    match tool {
        Tool::Rectangle => Some(Kind::Rectangle),
        Tool::Diamond => Some(Kind::Diamond),
        Tool::Ellipse => Some(Kind::Ellipse),
        Tool::Frame => Some(Kind::Frame),
        _ => None,
    }
}

/// The box between a fixed `origin` and a dragged `corner`. Shift makes it a square, Alt makes the origin
/// the centre (Excalidraw).
fn bounds(origin: Point, corner: Point, shift: bool, alt: bool) -> BoxXywh {
    let mut d = corner - origin;
    if shift {
        let side = d.x.abs().max(d.y.abs());
        let sign = |v: f64| if v < 0.0 { -1.0 } else { 1.0 };
        d = Vec2::new(side * sign(d.x), side * sign(d.y));
    }
    let (a, b) = if alt { (origin - d, origin + d) } else { (origin, origin + d) };
    let r = Rect::from_points(a, b);
    (r.x0, r.y0, r.width(), r.height())
}

/// A box as `(x, y, w, h)`.
type BoxXywh = (f64, f64, f64, f64);

/// A shape being dragged out.
#[derive(Debug)]
pub(super) struct BoxDrag {
    kind: Kind,
    /// Where the press happened, snapped to the grid.
    origin: Point,
    /// The box and guides shown last, to skip a move that changes nothing.
    shown: Option<(BoxXywh, Vec<SmartGuide>)>,
}

impl BoxDrag {
    pub(super) fn start(tool: Tool, cx: &Ctx<'_>, ev: &Ev<'_>) -> Option<Self> {
        Some(Self { kind: kind_of(tool)?, origin: snap::start(cx, ev.origin.world), shown: None })
    }

    /// The box for the pointer at `world`, with the guides the snap showed.
    fn current(&self, cx: &Ctx<'_>, world: Point) -> (BoxXywh, Vec<SmartGuide>) {
        let snapped = snap::point(cx, world, None, Snaps::corner(cx));
        let rect = bounds(self.origin, snapped.point, cx.mods.shift, cx.mods.alt);
        (rect, snapped.guides)
    }

    pub(super) fn update(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>) {
        let (rect, guides) = self.current(cx, ev.at.world);
        if self.shown.as_ref().is_some_and(|(r, g)| *r == rect && *g == guides) {
            return;
        }
        preview::set(cx, preview::shape(cx, self.kind, rect));
        preview::set_guides(cx, guides.clone());
        self.shown = Some((rect, guides));
    }

    /// The pointer was released: one `element.create`, unless the box is smaller than the engine allows.
    pub(super) fn finish(self, cx: &mut Ctx<'_>, ev: &Ev<'_>) {
        let (rect, _) = self.current(cx, ev.at.world);
        let (x, y, w, h) = rect;
        if create::check_box_size(self.kind, w, h).is_err() {
            return;
        }
        cx.exec(
            "element.create",
            json!({ "kind": self.kind.name(), "bounds": { "x": x, "y": y, "w": w, "h": h }, "from_tool": true }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_makes_a_square_and_alt_centres_it() {
        let o = Point::new(100.0, 100.0);
        assert_eq!(bounds(o, Point::new(140.0, 120.0), false, false), (100.0, 100.0, 40.0, 20.0));
        assert_eq!(bounds(o, Point::new(140.0, 120.0), true, false), (100.0, 100.0, 40.0, 40.0));
        assert_eq!(bounds(o, Point::new(60.0, 120.0), true, false), (60.0, 100.0, 40.0, 40.0));
        assert_eq!(bounds(o, Point::new(140.0, 120.0), false, true), (60.0, 80.0, 80.0, 40.0));
        assert_eq!(bounds(o, Point::new(140.0, 120.0), true, true), (60.0, 60.0, 80.0, 80.0));
    }
}
