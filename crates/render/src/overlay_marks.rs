//! Small marks on elements: the link icon.

use kurbo::{Affine, BezPath, Cap, Join, Rect, RoundedRect, Shape, Stroke};
use peniko::{Color, Fill};
use vello::Scene;

use crate::overlay::Pen;

/// A link icon to draw, in world space (`drafftink_geom::link_icon::icon_rect`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinkMark {
    /// The upright square of the icon.
    pub rect: Rect,
    /// True for a link to another element of the document (an arrow), false for a web address (a chain).
    pub to_element: bool,
}

/// The glyph is drawn in a unit square; this much of the icon is left as a margin on each side.
const GLYPH_INSET: f64 = 0.2;

/// Two chain links, in the unit square.
fn chain() -> BezPath {
    let link = RoundedRect::new(-0.26, -0.13, 0.26, 0.13, 0.13).to_path(0.01);
    let mut out = BezPath::new();
    for (cx, cy) in [(0.36, 0.64), (0.64, 0.36)] {
        let at = Affine::translate((cx, cy)) * Affine::rotate(-std::f64::consts::FRAC_PI_4);
        out.extend((at * &link).elements().iter().copied());
    }
    out
}

/// An arrow pointing up and to the right, in the unit square.
fn arrow() -> BezPath {
    let mut p = BezPath::new();
    p.move_to((0.1, 0.9));
    p.line_to((0.85, 0.15));
    p.move_to((0.4, 0.15));
    p.line_to((0.85, 0.15));
    p.line_to((0.85, 0.6));
    p
}

/// Draws each icon as a small white tile with the glyph in the accent colour.
pub(crate) fn paint_link_marks(scene: &mut Scene, pen: &Pen, marks: &[LinkMark]) -> usize {
    let mut drawn = 0;
    for m in marks.iter().filter(|m| m.rect.is_finite()) {
        let (a, b) = (pen.dev(m.rect.origin()), pen.dev(kurbo::Point::new(m.rect.x1, m.rect.y1)));
        let tile = Rect::from_points(a, b);
        if tile.width() <= 0.0 || tile.height() <= 0.0 {
            continue;
        }
        // A tile smaller than the corner radius would be drawn with fewer curves; keep the radius below a quarter of it.
        let radius = (2.0 * pen.px).min(tile.width().min(tile.height()) / 4.0);
        let round = RoundedRect::from_rect(tile, radius);
        scene.fill(Fill::NonZero, Affine::IDENTITY, Color::WHITE.with_alpha(0.92), None, &round);
        scene.stroke(&Stroke::new(pen.px), Affine::IDENTITY, pen.style.accent.with_alpha(0.5), None, &round);
        let side = tile.width().min(tile.height());
        let inner = side * (1.0 - 2.0 * GLYPH_INSET);
        let to_tile =
            Affine::translate((tile.x0 + side * GLYPH_INSET, tile.y0 + side * GLYPH_INSET)) * Affine::scale(inner);
        let glyph = if m.to_element { arrow() } else { chain() };
        let stroke = Stroke::new((1.3 * pen.px / inner).max(0.0)).with_caps(Cap::Round).with_join(Join::Round);
        // The stroke width is in the glyph's unit square, so it scales with the tile.
        scene.stroke(&stroke, to_tile, pen.style.accent, None, &glyph);
        drawn += 1;
    }
    drawn
}
