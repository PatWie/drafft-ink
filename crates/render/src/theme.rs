//! Light and dark rendering.
//!
//! Dark mode is a colour mapping applied while a fragment is encoded, not a filter over the finished frame. It
//! reproduces the CSS filter `invert(93%) hue-rotate(180deg)` that Excalidraw uses for its dark theme, so a white
//! canvas becomes `#121212`, black ink becomes a light grey, and hues keep their meaning. Alpha is untouched.
//! The mapping itself is `Color::for_dark_theme` (drafftink-scene), which the SVG export shares.

use drafftink_scene::Color;

/// Which palette elements are drawn with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Theme {
    /// Colours as stored in the document.
    #[default]
    Light,
    /// Colours mapped for a dark canvas.
    Dark,
}

impl Theme {
    /// The colour as it is drawn in this theme.
    pub fn map(self, c: Color) -> Color {
        match self {
            Theme::Light => c,
            Theme::Dark => c.for_dark_theme(),
        }
    }

    /// Index for per-theme tables.
    pub(crate) fn index(self) -> usize {
        match self {
            Theme::Light => 0,
            Theme::Dark => 1,
        }
    }
}

/// `alpha` scaled by `factor`, truncated like the old app's `stroke_with_opacity` so that drawings keep their look.
/// (`Color::with_opacity` in the document model rounds; that is for stored colours.) A factor outside 0..=1 saturates.
pub(crate) fn scale_alpha(alpha: u8, factor: f64) -> u8 {
    (f64::from(alpha) * factor) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_is_identity() {
        let c = Color::rgba(10, 20, 30, 40);
        assert_eq!(Theme::Light.map(c), c);
    }

    #[test]
    fn dark_maps_white_to_the_dark_canvas_and_black_to_light_ink() {
        let w = Theme::Dark.map(Color::WHITE);
        assert_eq!((w.r, w.g, w.b), (18, 18, 18));
        let k = Theme::Dark.map(Color::BLACK);
        assert_eq!((k.r, k.g, k.b), (237, 237, 237));
    }

    #[test]
    fn dark_keeps_alpha_and_stays_in_range() {
        for r in (0..=255u16).step_by(15) {
            for g in (0..=255u16).step_by(15) {
                for b in (0..=255u16).step_by(15) {
                    let c = Color::rgba(r as u8, g as u8, b as u8, 77);
                    assert_eq!(Theme::Dark.map(c).a, 77);
                }
            }
        }
    }

    #[test]
    fn dark_keeps_the_hue_family_of_a_saturated_colour() {
        // Excalidraw red stays reddish after the filter.
        let m = Theme::Dark.map(Color::parse("#e03131").unwrap_or(Color::BLACK));
        assert!(m.r > m.g && m.r > m.b, "{m:?}");
    }
}
