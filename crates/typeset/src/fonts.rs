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

/// Every font that is always compiled in, registered with Parley at start-up. The big fonts are
/// [`ExtraFont`]s; the STIX math font is read by ReX directly.
pub(crate) fn bundled() -> [&'static [u8]; 7] {
    [
        GELPEN_LIGHT,
        GELPEN_REGULAR,
        GELPEN_HEAVY,
        GELPEN_SERIF_LIGHT,
        GELPEN_SERIF_MEDIUM,
        GELPEN_SERIF_HEAVY,
        VANILLA_EXTRACT,
    ]
}

/// A large font that a build may leave out of the binary and load later (the web app does: they are 2.6 MB
/// of the 3.5 MB of font data). A build with the Cargo feature `noto-sans` or `stix-math` has the matching
/// fonts built in; otherwise the host gives them to [`crate::Typesetter::provide_font`] when
/// [`crate::Typesetter::wanted_fonts`] asks for them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ExtraFont {
    /// Noto Sans, regular. The glyph fallback of every family, and the family `noto_sans`.
    NotoSansRegular,
    /// Noto Sans, bold.
    NotoSansBold,
    /// Noto Sans, italic.
    NotoSansItalic,
    /// STIX Two Math, the font of formulas.
    StixTwoMath,
}

impl ExtraFont {
    /// Every extra font.
    pub const ALL: [ExtraFont; 4] =
        [ExtraFont::NotoSansRegular, ExtraFont::NotoSansBold, ExtraFont::NotoSansItalic, ExtraFont::StixTwoMath];

    /// The three Noto Sans files, which are always wanted together.
    pub const NOTO_SANS: [ExtraFont; 3] =
        [ExtraFont::NotoSansRegular, ExtraFont::NotoSansBold, ExtraFont::NotoSansItalic];

    /// The file name in `crates/typeset/assets/fonts`, which is also the name of the asset the web build serves.
    pub fn file_name(self) -> &'static str {
        match self {
            ExtraFont::NotoSansRegular => "NotoSans-Regular.ttf",
            ExtraFont::NotoSansBold => "NotoSans-Bold.ttf",
            ExtraFont::NotoSansItalic => "NotoSans-Italic.ttf",
            ExtraFont::StixTwoMath => "STIXTwoMath-Regular.otf",
        }
    }

    /// The bytes when this build has the font built in.
    pub(crate) fn embedded(self) -> Option<&'static [u8]> {
        match self {
            #[cfg(feature = "noto-sans")]
            ExtraFont::NotoSansRegular => Some(include_bytes!("../assets/fonts/NotoSans-Regular.ttf")),
            #[cfg(feature = "noto-sans")]
            ExtraFont::NotoSansBold => Some(include_bytes!("../assets/fonts/NotoSans-Bold.ttf")),
            #[cfg(feature = "noto-sans")]
            ExtraFont::NotoSansItalic => Some(include_bytes!("../assets/fonts/NotoSans-Italic.ttf")),
            #[cfg(feature = "stix-math")]
            ExtraFont::StixTwoMath => Some(include_bytes!("../assets/fonts/STIXTwoMath-Regular.otf")),
            #[cfg(not(feature = "noto-sans"))]
            ExtraFont::NotoSansRegular | ExtraFont::NotoSansBold | ExtraFont::NotoSansItalic => None,
            #[cfg(not(feature = "stix-math"))]
            ExtraFont::StixTwoMath => None,
        }
    }
}

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
