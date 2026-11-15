//! How the paint editor dims the canvas around the picture being painted on.
//!
//! The editor draws the picture it edits on top of everything and puts a translucent veil in the colour of the
//! canvas between the picture and the rest of the scene, so the other elements read as set back and the picture is
//! what the eye goes to. The veil is the page colour of the current theme at a fixed strength, not a black scrim, so it
//! looks right on a white page and on a dark one. The strength is a token ([`PaintVeil`]), not a literal at the call
//! site.

use kurbo::{Affine, Rect};
use peniko::{Color, Fill};
use vello::Scene;

use crate::theme::Theme;
use crate::view::View;

/// How strong the veil is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaintVeil {
    /// How much of the page colour covers the scene under it, 0 to 1.
    pub opacity: f32,
}

impl Theme {
    /// The veil of the paint editor in this theme. About 60 % keeps the other elements legible enough to see where
    /// the picture sits among them. Dark mode starts from a page that is already dark, and the same strength reads
    /// the same there.
    pub fn paint_veil(self) -> PaintVeil {
        match self {
            Theme::Light | Theme::Dark => PaintVeil { opacity: 0.6 },
        }
    }
}

/// Covers the whole canvas with `background` (the page colour in the current theme, `SceneComposer::background`) at
/// the strength of the theme's [`PaintVeil`]. Draw the edited picture over it afterwards.
pub fn fill_veil(scene: &mut Scene, view: &View, theme: Theme, background: Color) {
    let device = view.device_size();
    let whole = Rect::new(0.0, 0.0, device.width, device.height);
    let veil = background.with_alpha(theme.paint_veil().opacity);
    scene.fill(Fill::NonZero, Affine::IDENTITY, veil, None, &whole);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_veil_is_a_translucent_layer_in_both_themes() {
        for theme in [Theme::Light, Theme::Dark] {
            let opacity = theme.paint_veil().opacity;
            assert!((0.4..=0.8).contains(&opacity), "{theme:?}: {opacity}");
        }
    }
}
