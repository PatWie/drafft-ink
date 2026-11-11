//! The bundled fonts and the one table that maps a style to a font family name.
//!
//! The old app had this table twice (render and editor) and spelled the names as strings in
//! both. Here it is one function, [`face_for`], and the strings live only in [`BUNDLED`].

use crate::style::{FontFamily, FontWeight};

pub(crate) static GELPEN_LIGHT: &[u8] = include_bytes!("../assets/fonts/GelPenLight.ttf");
pub(crate) static GELPEN_REGULAR: &[u8] = include_bytes!("../assets/fonts/GelPen.ttf");
pub(crate) static GELPEN_HEAVY: &[u8] = include_bytes!("../assets/fonts/GelPenHeavy.ttf");
pub(crate) static GELPEN_SERIF_LIGHT: &[u8] = include_bytes!("../assets/fonts/GelPenSerifLight.ttf");
pub(crate) static GELPEN_SERIF_MEDIUM: &[u8] = include_bytes!("../assets/fonts/GelPenSerifMedium.ttf");
pub(crate) static GELPEN_SERIF_HEAVY: &[u8] = include_bytes!("../assets/fonts/GelPenSerifHeavy.ttf");
pub(crate) static VANILLA_EXTRACT: &[u8] = include_bytes!("../assets/fonts/VanillaExtract.ttf");
pub(crate) static STIX_TWO_MATH: &[u8] = include_bytes!("../assets/fonts/STIXTwoMath-Regular.otf");
#[cfg(feature = "noto-sans")]
pub(crate) static NOTO_SANS_REGULAR: &[u8] = include_bytes!("../assets/fonts/NotoSans-Regular.ttf");
#[cfg(feature = "noto-sans")]
pub(crate) static NOTO_SANS_BOLD: &[u8] = include_bytes!("../assets/fonts/NotoSans-Bold.ttf");
#[cfg(feature = "noto-sans")]
pub(crate) static NOTO_SANS_ITALIC: &[u8] = include_bytes!("../assets/fonts/NotoSans-Italic.ttf");

/// Every bundled text font, registered with Parley at start-up. The STIX math font is not in
/// this list: ReX reads it directly.
pub(crate) fn bundled() -> Vec<&'static [u8]> {
    #[cfg_attr(not(feature = "noto-sans"), allow(unused_mut))]
    let mut v: Vec<&'static [u8]> = vec![
        GELPEN_LIGHT,
        GELPEN_REGULAR,
        GELPEN_HEAVY,
        GELPEN_SERIF_LIGHT,
        GELPEN_SERIF_MEDIUM,
        GELPEN_SERIF_HEAVY,
        VANILLA_EXTRACT,
    ];
    #[cfg(feature = "noto-sans")]
    v.extend([NOTO_SANS_REGULAR, NOTO_SANS_BOLD, NOTO_SANS_ITALIC]);
    v
}

/// Whether Noto Sans is compiled in.
pub const HAS_NOTO_SANS: bool = cfg!(feature = "noto-sans");

/// What Parley is asked for.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Face {
    /// Parley family name. The GelPen weights are separate families because that is how the
    /// font files name themselves.
    pub name: String,
    /// CSS-style weight (400 or 700).
    pub weight: f32,
    pub italic: bool,
}

/// The family name used as a glyph fallback for characters the chosen family lacks.
pub(crate) const FALLBACK_FAMILY: &str = "Noto Sans";

/// Resolve a style to a face. Unknown custom families are returned as given; Parley then falls
/// back to the next entry of the font stack.
pub(crate) fn face_for(family: &FontFamily, weight: FontWeight, italic: bool) -> Face {
    let name = match (family, weight) {
        (FontFamily::GelPen, FontWeight::Light) => "GelPenLight",
        (FontFamily::GelPen, FontWeight::Regular) => "GelPen",
        (FontFamily::GelPen, FontWeight::Heavy) => "GelPenHeavy",
        (FontFamily::GelPenSerif, FontWeight::Light) => "GelPenSerifLight",
        (FontFamily::GelPenSerif, FontWeight::Regular) => "GelPenSerif",
        (FontFamily::GelPenSerif, FontWeight::Heavy) => "GelPenSerifHeavy",
        (FontFamily::VanillaExtract, _) => "Vanilla Extract",
        (FontFamily::NotoSans, _) => FALLBACK_FAMILY,
        (FontFamily::Custom(name), _) => name.as_str(),
    };
    let bold = matches!(family, FontFamily::NotoSans | FontFamily::Custom(_)) && weight == FontWeight::Heavy;
    Face { name: name.to_owned(), weight: if bold { 700.0 } else { 400.0 }, italic }
}
