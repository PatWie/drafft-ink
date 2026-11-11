//! Colours as data: the quick swatches and the conversion from the palette of `drafftink-scene`.
//!
//! **What a swatch shows.** The canvas draws an element's stored colour through `Color::for_dark_theme` in the dark
//! theme, so the UI must not show the stored colour: a swatch shows the colour as the canvas displays it, and a
//! pick stores the colour whose display is the swatch (`Color::from_dark_theme`, the inverse of the same function).
//! This module is the only place the UI applies either direction. In the light theme both are the identity.

use drafftink_scene::{Color, palette};
use egui::Color32;

/// The families of the quick swatches in the top bar (the old app's choice).
pub const QUICK_FAMILIES: [&str; 6] = ["blue", "red", "emerald", "amber", "purple", "slate"];
/// Shade of a quick stroke swatch.
pub const QUICK_STROKE_SHADE: u16 = 500;
/// Shade of a quick fill swatch.
pub const QUICK_FILL_SHADE: u16 = 100;

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
