//! Colours as data: the quick swatches and the conversion from the palette of `drafftink-scene`.
//!
//! **What a swatch shows.** The canvas draws an element's stored colour through `Color::for_dark_theme` in the dark
//! theme, so the UI must not show the stored colour: a swatch shows the colour as the canvas displays it, and a
//! pick stores the colour whose display is the swatch (`Color::from_dark_theme`, the inverse of the same function).
//! This module is the only place the UI applies either direction. In the light theme both are the identity.

use drafftink_scene::{Color, MarkerColor, MarkerShade, Tone, palette};
use egui::Color32;

/// The families of the quick swatches in the top bar (the old app's choice).
pub const QUICK_FAMILIES: [&str; 6] = ["blue", "red", "emerald", "amber", "purple", "slate"];
/// Shade of a quick stroke swatch.
pub const QUICK_STROKE_SHADE: u16 = 500;
/// Shade of a quick fill swatch.
pub const QUICK_FILL_SHADE: u16 = 100;

/// Shade of a quick marker swatch: the marker shade that the old five markers became, and the one the button puts on.
pub const QUICK_MARKER_SHADE: MarkerShade = MarkerShade::Standard;

/// The markers of the quick column of the highlight popover: yellow, the default marker, then the families of the
/// quick stroke and fill swatches, all in [`QUICK_MARKER_SHADE`].
pub fn quick_markers() -> Vec<MarkerColor> {
    std::iter::once("yellow")
        .chain(QUICK_FAMILIES)
        .filter_map(|family| MarkerColor::new(family, QUICK_MARKER_SHADE))
        .collect()
}

/// A marker as the popover offers it: a pick stores its name (`yellow-400`, a token that each theme resolves), and
/// the swatch shows the ink the canvas of this theme draws the band with (the band itself is that ink at half
/// opacity, too pale to tell apart in a 16 px cell), not the palette colour of the name, which the dark theme would
/// filter.
pub fn marker_offer(marker: MarkerColor, dark: bool) -> Offer {
    let tone = if dark { Tone::Dark } else { Tone::Light };
    Offer { stored: marker.name(), shown: to_egui(marker.rgb(tone)) }
}

/// A scene colour as an egui colour.
pub fn to_egui(c: Color) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

/// A palette name such as `blue-500`, or a `#rrggbb` string, as an egui colour, as it is (no theme applied).
pub fn parse(text: &str) -> Option<Color32> {
    resolve(text).map(to_egui)
}

/// A palette name or a colour string as a scene colour.
pub fn resolve(text: &str) -> Option<Color> {
    palette::lookup(text).or_else(|| Color::parse(text).ok())
}

/// The colour as the canvas displays a stored colour: what a swatch for an element's colour shows.
pub fn shown(stored: Color, dark: bool) -> Color {
    if dark { stored.for_dark_theme() } else { stored }
}

/// The colour to store so that the canvas displays `wanted`: what a pick of a displayed colour stores.
pub fn stored(wanted: Color, dark: bool) -> Color {
    if dark { wanted.from_dark_theme() } else { wanted }
}

/// A stored colour (a palette name or a colour string) as its swatch shows it on this theme.
pub fn displayed(stored_text: &str, dark: bool) -> Option<Color32> {
    resolve(stored_text).map(|c| to_egui(shown(c, dark)))
}

/// A colour on offer in a palette: the text to store when it is picked, and the swatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    /// What a pick stores.
    pub stored: String,
    /// What the swatch shows, which is also what the canvas draws for `stored`.
    pub shown: Color32,
}

/// A palette entry as a stored colour: a pick stores `text` (so the document holds the same colour in both
/// themes) and the swatch shows what the canvas draws for it. In the dark theme the whole palette is therefore
/// shown through the filter, and every entry stays reachable.
pub fn offer(text: &str, dark: bool) -> Option<Offer> {
    let stored = resolve(text)?;
    Some(Offer { stored: text.to_owned(), shown: to_egui(shown(stored, dark)) })
}

/// A colour as the person wants to see it on the canvas, for the entries that are named by how they look (Black,
/// White) and for a typed colour. In the dark theme the pick stores the inverse of the display and the swatch shows
/// what that draws, which for a colour the filter cannot reach (white, pure hues) is the nearest one it can.
pub fn offer_as_seen(text: &str, dark: bool) -> Option<Offer> {
    let wanted = resolve(text)?;
    if !dark {
        return Some(Offer { stored: text.to_owned(), shown: to_egui(wanted) });
    }
    let stored = wanted.from_dark_theme();
    Some(Offer { stored: stored.to_hex(), shown: to_egui(stored.for_dark_theme()) })
}
/// The palette name of a quick swatch.
pub fn quick_name(family: &str, shade: u16) -> String {
    format!("{family}-{shade}")
}

/// True when two colour strings (names or hex) are the same colour, alpha included.
pub fn same(a: &str, b: &str) -> bool {
    match (parse(a), parse(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

#[cfg(test)]
mod marker_tests {
    use super::*;

    #[test]
    fn the_quick_markers_are_yellow_then_the_quick_families_in_the_standard_shade() {
        let names: Vec<String> = quick_markers().iter().map(|m| m.name()).collect();
        assert_eq!(names, ["yellow-400", "blue-400", "red-400", "emerald-400", "amber-400", "purple-400", "slate-400"]);
        assert_eq!(quick_markers()[0], MarkerColor::YELLOW, "the default and last used marker stays yellow");
    }

    #[test]
    fn a_marker_cell_stores_the_name_and_shows_the_ink_of_the_theme() {
        let m = MarkerColor::new("blue", MarkerShade::Soft).unwrap();
        let light = marker_offer(m, false);
        let dark = marker_offer(m, true);
        assert_eq!((light.stored.as_str(), dark.stored.as_str()), ("blue-300", "blue-300"), "one token, both themes");
        assert_eq!(light.shown, to_egui(m.rgb(Tone::Light)));
        assert_eq!(dark.shown, to_egui(m.rgb(Tone::Dark)));
        assert_ne!(light.shown, dark.shown);
        assert_eq!(
            resolve(&light.stored),
            Some(palette_blue_300()),
            "the light swatch is the palette colour of the name"
        );
    }

    fn palette_blue_300() -> Color {
        palette::lookup("blue-300").unwrap()
    }
}
