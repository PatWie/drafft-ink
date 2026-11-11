//! The name of a frame, above its top-left corner, at the same size on screen at every zoom.
//!
//! Excalidraw keeps the label a constant size; the old app scaled it with the frame. The label is therefore not
//! part of the frame's cached fragment (which lives in world units): the composer draws it in device space right
//! after the fragment, from the typesetter's cached layout. The text is cut at the frame's width on screen, but
//! never narrower than [`FRAME_LABEL_MIN_WIDTH`].

use drafftink_scene::{Element, ElementKind};
use drafftink_typeset::Typesetter;
use kurbo::{Affine, Point, Rect};
use peniko::Fill;
use vello::Scene;

use crate::overlay_text::draw_glyphs;
use crate::spec::{FRAME_LABEL_GAP, FRAME_LABEL_MIN_WIDTH, FRAME_LABEL_ROOM, frame_label_spec};
use crate::theme::{Theme, scale_alpha};
use crate::view::View;

/// The name shown when a frame has none.
const DEFAULT_NAME: &str = "Frame";
/// A name is cut to this many characters before it is laid out.
const MAX_NAME_CHARS: usize = 256;

/// The text of the label: the first line of the name, or [`DEFAULT_NAME`] for a frame without one. `None` for a
/// frame whose name was set to nothing.
fn label_text(name: Option<&str>) -> Option<String> {
    let line = name.unwrap_or(DEFAULT_NAME).lines().next().unwrap_or("");
    let text: String = line.chars().filter(|c| !c.is_control()).take(MAX_NAME_CHARS).collect();
    (!text.trim().is_empty()).then_some(text)
}

/// Draws the label of `el` if it is a frame. `to_dev` is the frame's local frame to device space.
pub(crate) fn paint_frame_label(
    scene: &mut Scene,
    el: &Element,
    to_dev: Affine,
    view: &View,
    theme: Theme,
    typesetter: &mut Typesetter,
) {
    let ElementKind::Frame(f) = &el.kind else { return };
    let Some(text) = label_text(f.name.as_deref()) else { return };
    let corner = to_dev * Point::ORIGIN;
    if !(corner.x.is_finite() && corner.y.is_finite()) {
        return;
    }
    let px = view.logical_to_device().as_coeffs()[0];
    let layout = typesetter.text(&frame_label_spec(&text));
    let width = (el.transform.w.max(0.0) * view.scale()).max(FRAME_LABEL_MIN_WIDTH * px);
    let clip = Rect::new(corner.x, corner.y - FRAME_LABEL_ROOM * px, corner.x + width, corner.y);
    let origin = Point::new(corner.x, corner.y - (FRAME_LABEL_GAP + layout.size.height) * px);
    // The caption is drawn in the stroke colour, as the rest of the frame is.
    let c = theme.map(el.style.stroke_color);
    let alpha = scale_alpha(c.a, el.style.opacity.clamp(0.0, 1.0));
    let color = peniko::Color::from_rgba8(c.r, c.g, c.b, alpha);
    scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &clip);
    draw_glyphs(scene, &layout, origin, px, color);
    scene.pop_layer();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_label_is_the_first_line_of_the_name_and_a_frame_without_a_name_is_called_frame() {
        assert_eq!(label_text(None).as_deref(), Some("Frame"));
        assert_eq!(label_text(Some("Plan\nsecond")).as_deref(), Some("Plan"));
        assert_eq!(label_text(Some("")), None);
        assert_eq!(label_text(Some("  \t")), None);
        assert_eq!(label_text(Some(&"x".repeat(10_000))).map(|t| t.chars().count()), Some(MAX_NAME_CHARS));
    }
}
