//! The eyedropper: picking a colour from the canvas for the stroke or the fill.
//!
//! The button next to the colour swatches runs a UI verb (`ui.pick_color`). Where the platform has a picker of its
//! own for any colour on the screen (the browser's `EyeDropper`), the verb asks the app for it
//! ([`HostRequest::PickScreenColor`]). Everywhere else the verb starts *pick mode*: the app reads the pixel of the
//! canvas under the pointer, shows it in a swatch that follows the pointer ([`Preview`]), and the release of the
//! pointer picks it. A pick is one `style.set`, so one undo step, and with nothing selected it sets the colour of
//! new elements. Esc leaves pick mode like it closes any overlay.
//!
//! The pixel is the colour as the canvas draws it. In the dark theme the canvas draws stored colours through a
//! filter, so the colour to store is the inverse of the pixel ([`crate::colors::stored`]), the same as for a colour
//! typed in the hex field.

use drafftink_scene::Color;
use serde_json::json;

use crate::action::Call;
use crate::colors;
use crate::state::{ColorTarget, UiState};
use drafftink_engine::Origin;

/// Pick mode: which colour it sets and what is under the pointer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Eyedropper {
    /// The colour a pick sets: the stroke or the fill.
    pub target: ColorTarget,
    /// The colour under the pointer, once the app has read it.
    pub preview: Option<Preview>,
}

/// The colour of the canvas under the pointer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Preview {
    /// Where the pointer is, in egui points.
    pub at: egui::Pos2,
    /// The colour the canvas shows there.
    pub shown: Color,
}

impl UiState {
    /// Pick mode, when it is on.
    pub fn eyedropper(&self) -> Option<&Eyedropper> {
        self.eyedropper.as_ref()
    }

    /// The platform has a picker of its own for colours anywhere on the screen, so the button asks the app for it
    /// instead of starting pick mode (the browser's `EyeDropper`).
    pub fn set_screen_color_picker(&mut self, available: bool) {
        self.screen_picker = available;
    }

    /// True when the button asks the app for the platform's picker.
    pub fn has_screen_color_picker(&self) -> bool {
        self.screen_picker
    }

    /// Starts pick mode for `target`, or leaves it when it is on for that target already (the button toggles).
    pub(crate) fn toggle_eyedropper(&mut self, target: ColorTarget) {
        let leaving = self.eyedropper.is_some_and(|e| e.target == target);
        // The style popover of a narrow strip, which holds the button, stays: the person picks from the canvas
        // under it, and the popover is not in the way of the pixels.
        let keep_style = self.style_open;
        self.close_overlays();
        self.style_open = keep_style;
        self.eyedropper = (!leaving).then_some(Eyedropper { target, preview: None });
    }

    /// The app read the colour of the canvas under the pointer: the swatch follows it. `None` (no pixel) clears it.
    /// True when the picture of pick mode changed.
    pub fn set_eyedropper_preview(&mut self, at: egui::Pos2, rgba: Option<[u8; 4]>) -> bool {
        let Some(mode) = self.eyedropper.as_mut() else { return false };
        let preview = rgba.map(|[r, g, b, _]| Preview { at, shown: Color::rgb(r, g, b) });
        let changed = mode.preview != preview;
        mode.preview = preview;
        changed
    }

    /// A colour was chosen, from the canvas (`rgba` is the pixel as drawn) or from the platform's picker. Leaves pick
    /// mode and returns the call that sets it: `style.set` for the colour of the target. The colour is also the one
    /// last picked, which the strip keeps as a swatch. `None` for the background, which is not picked this way.
    pub fn finish_eyedropper(&mut self, target: ColorTarget, rgb: [u8; 3]) -> Option<Call> {
        self.eyedropper = None;
        let prop = match target {
            ColorTarget::Stroke => "stroke_color",
            ColorTarget::Fill => "fill_color",
            ColorTarget::Background => return None,
        };
        let [r, g, b] = rgb;
        let stored = colors::stored(Color::rgb(r, g, b), self.dark).to_hex();
        self.last_picked.remember(target, stored.clone());
        Some(Call::new(Origin::Ui, "style.set", json!({ "props": { prop: stored } })))
    }

    /// The target of pick mode, when it is on.
    pub fn eyedropper_target(&self) -> Option<ColorTarget> {
        self.eyedropper.map(|e| e.target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_button_toggles_pick_mode_per_target() {
        let mut st = UiState::new();
        st.toggle_eyedropper(ColorTarget::Stroke);
        assert_eq!(st.eyedropper_target(), Some(ColorTarget::Stroke));
        st.toggle_eyedropper(ColorTarget::Fill);
        assert_eq!(st.eyedropper_target(), Some(ColorTarget::Fill), "the other button switches the target");
        st.toggle_eyedropper(ColorTarget::Fill);
        assert_eq!(st.eyedropper_target(), None, "the same button leaves it");
    }

    #[test]
    fn esc_and_every_other_overlay_leave_pick_mode() {
        let mut st = UiState::new();
        st.toggle_eyedropper(ColorTarget::Stroke);
        assert!(st.any_overlay_open(), "so that Esc closes it first");
        assert!(st.close_overlays());
        assert_eq!(st.eyedropper_target(), None);
    }

    #[test]
    fn a_pick_is_a_style_set_of_the_colour_and_leaves_pick_mode() {
        let mut st = UiState::new();
        st.toggle_eyedropper(ColorTarget::Fill);
        let call = st.finish_eyedropper(ColorTarget::Fill, [0x3b, 0x82, 0xf6]).unwrap();
        assert_eq!((call.id.as_str(), call.args), ("style.set", json!({ "props": { "fill_color": "#3b82f6" } })));
        assert_eq!(st.eyedropper(), None);
        assert_eq!(st.last_picked.of(ColorTarget::Fill), Some("#3b82f6"), "it is the colour last picked");
        assert_eq!(st.last_picked.of(ColorTarget::Stroke), None);
    }

    #[test]
    fn in_the_dark_theme_the_stored_colour_is_the_one_that_draws_as_the_pixel() {
        let mut st = UiState::new();
        st.dark = true;
        let pixel = Color::rgb(0x3b, 0x82, 0xf6);
        let call = st.finish_eyedropper(ColorTarget::Stroke, [pixel.r, pixel.g, pixel.b]).unwrap();
        let stored = call.args["props"]["stroke_color"].as_str().and_then(|s| Color::parse(s).ok()).unwrap();
        assert_eq!(stored, pixel.from_dark_theme(), "the inverse of the filter the canvas draws through");
        assert_ne!(stored, pixel, "not the pixel itself");
        // In the light theme the pixel is the colour.
        let mut light = UiState::new();
        let call = light.finish_eyedropper(ColorTarget::Stroke, [pixel.r, pixel.g, pixel.b]).unwrap();
        assert_eq!(call.args["props"]["stroke_color"], pixel.to_hex());
    }

    #[test]
    fn the_background_is_not_picked_from_the_canvas() {
        let mut st = UiState::new();
        assert!(st.finish_eyedropper(ColorTarget::Background, [1, 2, 3]).is_none());
    }

    #[test]
    fn the_preview_follows_the_pixel_and_reports_a_change_only_when_there_is_one() {
        let mut st = UiState::new();
        assert!(!st.set_eyedropper_preview(egui::pos2(1.0, 2.0), Some([1, 2, 3, 255])), "not in pick mode");
        st.toggle_eyedropper(ColorTarget::Stroke);
        assert!(st.set_eyedropper_preview(egui::pos2(1.0, 2.0), Some([1, 2, 3, 255])));
        assert!(!st.set_eyedropper_preview(egui::pos2(1.0, 2.0), Some([1, 2, 3, 255])), "nothing moved");
        assert!(st.set_eyedropper_preview(egui::pos2(5.0, 2.0), Some([1, 2, 3, 255])));
        assert!(st.set_eyedropper_preview(egui::pos2(5.0, 2.0), None), "no pixel clears it");
        assert_eq!(st.eyedropper().and_then(|e| e.preview), None);
    }
}
