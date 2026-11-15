//! The part of an arrow's line that its label hides.
//!
//! Excalidraw stops the line of an arrow where the label sits, so that the text reads on a clear spot and not on
//! top of a stroke. This module says where: the label's box, grown by [`LABEL_CUT_PADDING`], in the arrow's own
//! frame. The renderers (`drafftink-render`, the SVG export) draw the arrow with that region left out. Only what is
//! painted changes: the points, the hit test and the bindings of the arrow are whole.

use drafftink_geom::OrientedBox;
use kurbo::{BezPath, Point, Rect, Size};

use crate::element::{Element, Kind};

/// Room left clear around the label's box, in world units.
pub const LABEL_CUT_PADDING: f64 = 4.0;

/// The region of an arrow that is not drawn because its label is there: a quadrilateral in the arrow's local frame
/// (the label may be rotated, and so may the arrow).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LabelCut {
    corners: [Point; 4],
}

impl LabelCut {
    /// The cut that `label` makes in `arrow`, or `None` when it makes none: `arrow` is not an arrow, `label` is not
    /// a live text bound to it, has no visible text, or has a box that is not a finite size.
    pub fn of(arrow: &Element, label: &Element) -> Option<Self> {
        let text = label.text_data()?;
        if arrow.tag() != Kind::Arrow
            || arrow.deleted
            || label.deleted
            || text.container_id.as_ref() != Some(&arrow.id)
            || text.content.trim().is_empty()
        {
            return None;
        }
        let t = &label.transform;
        let finite = [t.x, t.y, t.w, t.h, t.angle].iter().all(|v| v.is_finite());
        if !finite || t.w <= 0.0 || t.h <= 0.0 {
            return None;
        }
        let grown = Size::new(t.w + 2.0 * LABEL_CUT_PADDING, t.h + 2.0 * LABEL_CUT_PADDING);
        let corners = OrientedBox::new(t.center(), grown, t.angle).corners().map(|p| arrow.transform.world_to_local(p));
        corners.iter().all(|p| p.x.is_finite() && p.y.is_finite()).then_some(Self { corners })
    }

    /// The four corners, in the arrow's local frame.
    pub fn corners(&self) -> [Point; 4] {
        self.corners
    }

    /// The box of the cut, in the arrow's local frame.
    pub fn bounds(&self) -> Rect {
        let [first, rest @ ..] = self.corners;
        rest.into_iter().fold(Rect::from_points(first, first), |r, p| r.union_pt(p))
    }

    /// A path that covers `area` except the cut: `area` as an outer rectangle and the cut as a hole whose winding is
    /// the opposite, so the non-zero rule leaves it out. As a clip it keeps everything but the label's place; as
    /// the `d` of an SVG `clipPath` it does the same under either fill rule. `area` is grown to hold the cut.
    pub fn outside(&self, area: Rect) -> BezPath {
        let area = area.union(self.bounds());
        let mut path = BezPath::new();
        path.move_to(Point::new(area.x0, area.y0));
        path.line_to(Point::new(area.x1, area.y0));
        path.line_to(Point::new(area.x1, area.y1));
        path.line_to(Point::new(area.x0, area.y1));
        path.close_path();
        // The outer rectangle runs top-left, top-right, bottom-right, bottom-left. The hole must run the other way.
        let mut hole = self.corners;
        if signed_area(&hole) > 0.0 {
            hole.reverse();
        }
        path.move_to(hole[0]);
        for p in &hole[1..] {
            path.line_to(*p);
        }
        path.close_path();
        path
    }
}

/// Twice the signed area of a polygon: positive when it runs top-left, top-right, bottom-right, bottom-left on a
/// y-down plane (clockwise on screen), as the outer rectangle of [`LabelCut::outside`] does.
fn signed_area(p: &[Point; 4]) -> f64 {
    (0..4).map(|i| (p[i].x * p[(i + 1) % 4].y) - (p[(i + 1) % 4].x * p[i].y)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::element::{ElementKind, LinearKind, TextData};
    use crate::id::ElementId;
    use crate::zindex::ZIndex;

    fn arrow() -> Element {
        let id = ElementId::new("a").unwrap();
        Element::linear(id, LinearKind::Arrow, ZIndex::first(), &[Point::new(0.0, 0.0), Point::new(200.0, 0.0)])
            .unwrap()
    }

    fn label(of: &Element, x: f64, y: f64, w: f64, h: f64) -> Element {
        let mut t = TextData::new("hello");
        t.container_id = Some(of.id.clone());
        let id = ElementId::new("l").unwrap();
        Element::new(id, ElementKind::Text(t), ZIndex::first()).with_bounds(x, y, w, h)
    }

    #[test]
    fn the_cut_is_the_label_box_with_its_padding_in_the_arrows_frame() {
        let a = arrow();
        let cut = LabelCut::of(&a, &label(&a, 80.0, -10.0, 40.0, 20.0)).unwrap();
        let b = cut.bounds();
        let (x0, y0) = (80.0 - LABEL_CUT_PADDING - a.transform.x, -10.0 - LABEL_CUT_PADDING - a.transform.y);
        assert!((b.x0 - x0).abs() < 1e-9 && (b.y0 - y0).abs() < 1e-9, "{b:?}");
        assert!((b.width() - (40.0 + 2.0 * LABEL_CUT_PADDING)).abs() < 1e-9);
        assert!((b.height() - (20.0 + 2.0 * LABEL_CUT_PADDING)).abs() < 1e-9);
    }

    #[test]
    fn no_text_another_container_or_a_dead_label_cuts_nothing() {
        let a = arrow();
        let mut l = label(&a, 80.0, -10.0, 40.0, 20.0);
        assert!(LabelCut::of(&a, &l).is_some());
        let mut blank = l.clone();
        if let ElementKind::Text(t) = &mut blank.kind {
            t.content = "  ".into();
        }
        assert!(LabelCut::of(&a, &blank).is_none());
        let mut other = l.clone();
        if let ElementKind::Text(t) = &mut other.kind {
            t.container_id = Some(ElementId::new("zzz").unwrap());
        }
        assert!(LabelCut::of(&a, &other).is_none());
        l.deleted = true;
        assert!(LabelCut::of(&a, &l).is_none());
        let rect = Element::new(ElementId::new("r").unwrap(), ElementKind::Rectangle, ZIndex::first());
        assert!(LabelCut::of(&rect, &label(&rect, 0.0, 0.0, 10.0, 10.0)).is_none(), "only arrows are cut");
        assert!(LabelCut::of(&a, &label(&a, 0.0, 0.0, 0.0, 10.0)).is_none());
        assert!(LabelCut::of(&a, &label(&a, f64::NAN, 0.0, 10.0, 10.0)).is_none());
    }

    #[test]
    fn the_hole_runs_against_the_outer_rectangle_whatever_the_order_of_the_corners() {
        let a = arrow();
        let cut = LabelCut::of(&a, &label(&a, 80.0, -10.0, 40.0, 20.0)).unwrap();
        let path = cut.outside(Rect::new(-50.0, -50.0, 300.0, 50.0));
        let area = |pts: &[Point]| -> f64 {
            (0..pts.len()).map(|i| pts[i].x * pts[(i + 1) % pts.len()].y - pts[(i + 1) % pts.len()].x * pts[i].y).sum()
        };
        let mut rings: Vec<Vec<Point>> = Vec::new();
        for el in path.elements() {
            match el {
                kurbo::PathEl::MoveTo(p) => rings.push(vec![*p]),
                kurbo::PathEl::LineTo(p) => rings.last_mut().unwrap().push(*p),
                _ => {}
            }
        }
        assert_eq!(rings.len(), 2);
        assert!(area(&rings[0]) * area(&rings[1]) < 0.0, "opposite winding: {rings:?}");
    }
}
