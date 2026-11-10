//! Colours as data: the quick swatches and the conversion from the palette of `drafftink-scene`.

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

/// A palette name such as `blue-500`, or a `#rrggbb` string, as an egui colour.
pub fn parse(text: &str) -> Option<Color32> {
    palette::lookup(text).or_else(|| Color::parse(text).ok()).map(to_egui)
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
