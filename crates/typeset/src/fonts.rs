//! The bundled fonts and the one table that maps a style to a font family name.
//!
//! The old app had this table twice (render and editor) and spelled the names as strings in
//! both. Here it is one function, [`face_for`], and the strings live only in [`BUNDLED`].

use crate::style::{FontFamily, FontWeight};

/// True when this build has the hand-drawn fonts (GelPen, GelPen Serif, Vanilla Extract) built in: the Cargo feature
/// `hand-fonts`. Without them those three families are drawn with Noto Sans (see [`font_file`]).
pub const HAND_FONTS: bool = cfg!(feature = "hand-fonts");

/// The hand-drawn font files. They are by Shara Weber, are not covered by the AGPL and are never part of a package on
/// crates.io (`docs/publishing.md`), so every use of them sits behind the feature `hand-fonts`.
#[cfg(feature = "hand-fonts")]
mod hand {
    use crate::style::{FontFamily, FontWeight};

    static GELPEN_LIGHT: &[u8] = include_bytes!("../assets/fonts/GelPenLight.ttf");
    static GELPEN_REGULAR: &[u8] = include_bytes!("../assets/fonts/GelPen.ttf");
    static GELPEN_HEAVY: &[u8] = include_bytes!("../assets/fonts/GelPenHeavy.ttf");
    static GELPEN_SERIF_LIGHT: &[u8] = include_bytes!("../assets/fonts/GelPenSerifLight.ttf");
    static GELPEN_SERIF_MEDIUM: &[u8] = include_bytes!("../assets/fonts/GelPenSerifMedium.ttf");
    static GELPEN_SERIF_HEAVY: &[u8] = include_bytes!("../assets/fonts/GelPenSerifHeavy.ttf");
    static VANILLA_EXTRACT: &[u8] = include_bytes!("../assets/fonts/VanillaExtract.ttf");

    pub(super) fn file(family: &FontFamily, weight: FontWeight) -> Option<&'static [u8]> {
        match (family, weight) {
            (FontFamily::GelPen, FontWeight::Light) => Some(GELPEN_LIGHT),
            (FontFamily::GelPen, FontWeight::Regular) => Some(GELPEN_REGULAR),
            (FontFamily::GelPen, FontWeight::Heavy) => Some(GELPEN_HEAVY),
            (FontFamily::GelPenSerif, FontWeight::Light) => Some(GELPEN_SERIF_LIGHT),
            (FontFamily::GelPenSerif, FontWeight::Regular) => Some(GELPEN_SERIF_MEDIUM),
            (FontFamily::GelPenSerif, FontWeight::Heavy) => Some(GELPEN_SERIF_HEAVY),
            (FontFamily::VanillaExtract, _) => Some(VANILLA_EXTRACT),
            _ => None,
        }
    }

    pub(super) fn all() -> [&'static [u8]; 7] {
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

    pub(super) fn text_for_math() -> Option<&'static [u8]> {
        Some(GELPEN_REGULAR)
    }
}

/// Without the feature `hand-fonts` there are no hand-drawn files.
#[cfg(not(feature = "hand-fonts"))]
mod hand {
    use crate::style::{FontFamily, FontWeight};

    pub(super) fn file(_: &FontFamily, _: FontWeight) -> Option<&'static [u8]> {
        None
    }

    pub(super) fn all() -> [&'static [u8]; 0] {
        []
    }

    /// The stand-in for the hand-drawn text font of a formula: Noto Sans, when it is built in.
    pub(super) fn text_for_math() -> Option<&'static [u8]> {
        super::ExtraFont::NotoSansRegular.embedded()
    }
}

/// True for the three hand-drawn families.
fn is_hand_drawn(family: &FontFamily) -> bool {
    matches!(family, FontFamily::GelPen | FontFamily::GelPenSerif | FontFamily::VanillaExtract)
}

/// The font file that draws `family` at `weight`, when this build has it. With the Cargo feature `hand-fonts` the
/// hand-drawn families (GelPen, GelPen Serif, Vanilla Extract) have their own files; without it they are drawn with Noto
/// Sans, like the family `noto_sans`. Noto Sans and IBM Plex Sans are there when they are built in (the Cargo features
/// `noto-sans` and `plex-sans`). A custom family has none. The bytes are the statics the typesetter registers, so a host
/// that needs the file (the UI fonts, an SVG export) shares them instead of embedding a second copy.
pub fn font_file(family: &FontFamily, weight: FontWeight) -> Option<&'static [u8]> {
    if let Some(bytes) = hand::file(family, weight) {
        return Some(bytes);
    }
    match (family, weight) {
        (FontFamily::NotoSans, FontWeight::Heavy) => ExtraFont::NotoSansBold.embedded(),
        (FontFamily::NotoSans, _) => ExtraFont::NotoSansRegular.embedded(),
        (hand, FontWeight::Heavy) if is_hand_drawn(hand) => ExtraFont::NotoSansBold.embedded(),
        (hand, _) if is_hand_drawn(hand) => ExtraFont::NotoSansRegular.embedded(),
        (FontFamily::PlexSans, _) => ExtraFont::PlexSans.embedded(),
        _ => None,
    }
}

/// Every hand-drawn font file this build has, registered with Parley at start-up (none without the feature
/// `hand-fonts`). The other fonts are [`ExtraFont`]s; the STIX math font is read by ReX directly.
pub(crate) fn bundled() -> impl Iterator<Item = &'static [u8]> {
    hand::all().into_iter()
}

/// The font that draws the letters and digits of a formula (the hand-drawn GelPen Regular, or Noto Sans without it).
pub(crate) fn math_text_font() -> Option<&'static [u8]> {
    hand::text_for_math()
}

/// A large font that a build may leave out of the binary and load later (the web app does: they are 2.6 MB
/// of the 3.5 MB of font data). A build with the Cargo feature `noto-sans`, `plex-sans`, `stix-math` or `fallback-fonts` has the matching
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
    /// IBM Plex Sans, the variable font (weights 100 to 700) of the family `plex_sans`.
    PlexSans,
    /// STIX Two Math, the font of formulas.
    StixTwoMath,
    /// Noto Sans Symbols 2 (and Symbols) cut to geometric shapes, miscellaneous symbols, dingbats and arrows: ◕ ☺ ★ ♥ ✓.
    FallbackSymbols,
    /// Noto Sans Math cut to arrows, mathematical operators and miscellaneous technical: → ∀ ≦ ⌘.
    FallbackMath,
    /// Noto Sans Kannada: ಠ.
    FallbackKannada,
    /// Noto Sans Arabic cut to the Arabic blocks: ٩ ۶.
    FallbackArabic,
    /// Noto Sans CJK JP cut to punctuation, kana, Hangul jamo, box drawing and the half- and fullwidth forms: ｡ 、 ツ ─.
    FallbackCjk,
}

impl ExtraFont {
    /// Every extra font.
    pub const ALL: [ExtraFont; 10] = [
        ExtraFont::NotoSansRegular,
        ExtraFont::NotoSansBold,
        ExtraFont::NotoSansItalic,
        ExtraFont::PlexSans,
        ExtraFont::StixTwoMath,
        ExtraFont::FallbackSymbols,
        ExtraFont::FallbackMath,
        ExtraFont::FallbackKannada,
        ExtraFont::FallbackArabic,
        ExtraFont::FallbackCjk,
    ];

    /// The three Noto Sans files, which are always wanted together.
    pub const NOTO_SANS: [ExtraFont; 3] =
        [ExtraFont::NotoSansRegular, ExtraFont::NotoSansBold, ExtraFont::NotoSansItalic];

    /// The file name in `crates/typeset/assets/fonts`, which is also the name of the asset the web build serves.
    pub fn file_name(self) -> &'static str {
        match self {
            ExtraFont::NotoSansRegular => "NotoSans-Regular.ttf",
            ExtraFont::NotoSansBold => "NotoSans-Bold.ttf",
            ExtraFont::NotoSansItalic => "NotoSans-Italic.ttf",
            ExtraFont::PlexSans => "IBMPlexSans-Variable.ttf",
            ExtraFont::StixTwoMath => "STIXTwoMath-Regular.otf",
            ExtraFont::FallbackSymbols => "NotoSansSymbols2-Fallback.ttf",
            ExtraFont::FallbackMath => "NotoSansMath-Fallback.ttf",
            ExtraFont::FallbackKannada => "NotoSansKannada-Fallback.ttf",
            ExtraFont::FallbackArabic => "NotoSansArabic-Fallback.ttf",
            ExtraFont::FallbackCjk => "NotoSansCJK-Fallback.otf",
        }
    }

    /// The bytes when this build has the font built in. The UI shares them for its own glyph fallback.
    pub fn embedded(self) -> Option<&'static [u8]> {
        match self {
            #[cfg(feature = "noto-sans")]
            ExtraFont::NotoSansRegular => Some(include_bytes!("../assets/fonts/NotoSans-Regular.ttf")),
            #[cfg(feature = "noto-sans")]
            ExtraFont::NotoSansBold => Some(include_bytes!("../assets/fonts/NotoSans-Bold.ttf")),
            #[cfg(feature = "noto-sans")]
            ExtraFont::NotoSansItalic => Some(include_bytes!("../assets/fonts/NotoSans-Italic.ttf")),
            #[cfg(feature = "plex-sans")]
            ExtraFont::PlexSans => Some(include_bytes!("../assets/fonts/IBMPlexSans-Variable.ttf")),
            #[cfg(feature = "stix-math")]
            ExtraFont::StixTwoMath => Some(include_bytes!("../assets/fonts/STIXTwoMath-Regular.otf")),
            #[cfg(feature = "fallback-fonts")]
            ExtraFont::FallbackSymbols => Some(include_bytes!("../assets/fonts/NotoSansSymbols2-Fallback.ttf")),
            #[cfg(feature = "fallback-fonts")]
            ExtraFont::FallbackMath => Some(include_bytes!("../assets/fonts/NotoSansMath-Fallback.ttf")),
            #[cfg(feature = "fallback-fonts")]
            ExtraFont::FallbackKannada => Some(include_bytes!("../assets/fonts/NotoSansKannada-Fallback.ttf")),
            #[cfg(feature = "fallback-fonts")]
            ExtraFont::FallbackArabic => Some(include_bytes!("../assets/fonts/NotoSansArabic-Fallback.ttf")),
            #[cfg(feature = "fallback-fonts")]
            ExtraFont::FallbackCjk => Some(include_bytes!("../assets/fonts/NotoSansCJK-Fallback.otf")),
            #[cfg(not(feature = "fallback-fonts"))]
            ExtraFont::FallbackSymbols
            | ExtraFont::FallbackMath
            | ExtraFont::FallbackKannada
            | ExtraFont::FallbackArabic
            | ExtraFont::FallbackCjk => None,
            #[cfg(not(feature = "noto-sans"))]
            ExtraFont::NotoSansRegular | ExtraFont::NotoSansBold | ExtraFont::NotoSansItalic => None,
            #[cfg(not(feature = "plex-sans"))]
            ExtraFont::PlexSans => None,
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

/// The Parley family name of IBM Plex Sans (the font's own family name).
const PLEX_SANS_FAMILY: &str = "IBM Plex Sans";

/// The family name used as a glyph fallback for characters the chosen family lacks.
pub(crate) const FALLBACK_FAMILY: &str = "Noto Sans";

/// Resolve a style to a face. Unknown custom families are returned as given; Parley then falls
/// back to the next entry of the font stack.
pub(crate) fn face_for(family: &FontFamily, weight: FontWeight, italic: bool) -> Face {
    let name = match (family, weight) {
        (hand, _) if !HAND_FONTS && is_hand_drawn(hand) => FALLBACK_FAMILY,
        (FontFamily::GelPen, FontWeight::Light) => "GelPenLight",
        (FontFamily::GelPen, FontWeight::Regular) => "GelPen",
        (FontFamily::GelPen, FontWeight::Heavy) => "GelPenHeavy",
        (FontFamily::GelPenSerif, FontWeight::Light) => "GelPenSerifLight",
        (FontFamily::GelPenSerif, FontWeight::Regular) => "GelPenSerif",
        (FontFamily::GelPenSerif, FontWeight::Heavy) => "GelPenSerifHeavy",
        (FontFamily::VanillaExtract, _) => "Vanilla Extract",
        (FontFamily::NotoSans, _) => FALLBACK_FAMILY,
        (FontFamily::PlexSans, _) => PLEX_SANS_FAMILY,
        (FontFamily::Custom(name), _) => name.as_str(),
    };
    let weight = match (family, weight) {
        (FontFamily::PlexSans | FontFamily::Custom(_), FontWeight::Light) => 300.0,
        (FontFamily::NotoSans | FontFamily::PlexSans | FontFamily::Custom(_), FontWeight::Heavy) => 700.0,
        (hand, FontWeight::Heavy) if !HAND_FONTS && is_hand_drawn(hand) => 700.0,
        _ => 400.0,
    };
    Face { name: name.to_owned(), weight, italic }
}
