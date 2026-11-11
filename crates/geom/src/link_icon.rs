//! Where the link icon of an element sits and how far a click reaches it.
//!
//! One rule for the renderer (which draws the icon), the canvas (which reports a click on it as its own target)
//! and the editor (`edit::link`). The geometry follows Excalidraw's `getLinkHandleFromCoords`.

use kurbo::{Point, Rect, Vec2};

/// Side of the link icon in screen pixels (Excalidraw `DEFAULT_LINK_SIZE`).
pub const ICON_SIZE_PX: f64 = 12.0;
/// How far outside the icon a click still hits it, in screen pixels.
pub const ICON_REACH_PX: f64 = 4.0;
/// Gap between the corner of the box and the icon, in screen pixels (also at zoom below 1, see [`icon_rect`]).
const MARGIN_PX: f64 = 4.0;

/// The most world units the icon (with its margin) can stand away from the box corner, at any zoom. Hit tests use
/// it to know how far from an element's bounds an icon can be.
pub const MAX_ICON_DISTANCE: f64 = ICON_SIZE_PX + MARGIN_PX;

/// The zoom the icon geometry uses: the icon shrinks with the zoom above 1 and has a fixed size in world units below.
fn icon_zoom(zoom: f64) -> f64 {
    if zoom.is_finite() && zoom > 1.0 { zoom } else { 1.0 }
}

/// The icon of an element whose unrotated box is `frame` and whose angle is `angle`, in world units.
///
/// It is a square beside the top-right corner of the box, where the north-east resize handle sits when the
/// element is selected. The centre turns with the element about the box centre; the square stays upright. Zoomed in
/// the icon keeps its size on screen; zoomed out it keeps its size in world units, so it shrinks on screen, as in
/// Excalidraw.
pub fn icon_rect(frame: Rect, angle: f64, zoom: f64) -> Rect {
    let z = icon_zoom(zoom);
    let size = ICON_SIZE_PX / z;
    let centering = (ICON_SIZE_PX - 8.0) / (2.0 * z);
    let margin = MARGIN_PX / z;
    let x = frame.x1 + margin - centering;
    let y = frame.y0 - margin - size + centering;
    let centre = frame.center();
    let icon_centre = Point::new(x + size / 2.0, y + size / 2.0);
    let d = icon_centre - centre;
    let (sin, cos) = angle.sin_cos();
    let rotated = centre + Vec2::new(d.x * cos - d.y * sin, d.x * sin + d.y * cos);
    Rect::new(rotated.x - size / 2.0, rotated.y - size / 2.0, rotated.x + size / 2.0, rotated.y + size / 2.0)
}

/// Whether the world point `p` is on the icon, with a small reach around it. False for non-finite input.
pub fn hits_icon(frame: Rect, angle: f64, p: Point, zoom: f64) -> bool {
    if !(p.x.is_finite() && p.y.is_finite() && frame.is_finite() && angle.is_finite()) {
        return false;
    }
    let reach = if zoom.is_finite() && zoom > 0.0 { ICON_REACH_PX / zoom } else { ICON_REACH_PX };
    icon_rect(frame, angle, zoom).inflate(reach, reach).contains(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_is_beside_the_top_right_corner_and_turns_about_the_centre() {
        let frame = Rect::new(0.0, 0.0, 100.0, 100.0);
        let r = icon_rect(frame, 0.0, 1.0);
        assert!((r.center().x - 108.0).abs() < 1e-9 && (r.center().y + 8.0).abs() < 1e-9, "{r:?}");
        let turned = icon_rect(frame, std::f64::consts::FRAC_PI_2, 1.0);
        assert!((turned.center().x - 108.0).abs() < 1e-9 && (turned.center().y - 108.0).abs() < 1e-9, "{turned:?}");
    }

    #[test]
    fn hostile_input_hits_nothing() {
        let frame = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(!hits_icon(frame, 0.0, Point::new(f64::NAN, 0.0), 1.0));
        assert!(!hits_icon(frame, f64::INFINITY, Point::new(0.0, 0.0), 1.0));
        assert!(!hits_icon(Rect::new(f64::NAN, 0.0, 1.0, 1.0), 0.0, Point::ZERO, 1.0));
        let r = icon_rect(frame, 0.0, f64::NAN);
        assert!((r.width() - ICON_SIZE_PX).abs() < 1e-9, "a bad zoom counts as 1");
    }
}
