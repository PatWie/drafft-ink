//! Glyph fallback: the bundled fonts that draw the characters a text's own family lacks.
//!
//! Every text style names its family first, then Noto Sans, then the families of [`ExtraFont::FALLBACKS`]. Parley
//! picks, for each character cluster, the first font of that list that has all its glyphs, so the chosen family is
//! used wherever it can draw and a fallback only where it cannot.
//!
//! The fallback fonts are subsets of Noto fonts (see `scripts/build-fallback-fonts.py`), and each code point belongs to
//! at most one of them. [`ExtraFont::fallback_for`] tells which one, so a host that loads the fonts on demand (the
//! web app) fetches only the font that a missing character needs.

use std::collections::BTreeSet;
use std::ops::RangeInclusive;

use parley::PositionedLayoutItem;

use crate::fonts::ExtraFont;
use crate::layout::StyleId;

/// The code points of each fallback font. `scripts/build-fallback-fonts.py` cuts the fonts to the same ranges, and a
/// test checks that no font has a character outside its ranges.
const CLAIMS: [(ExtraFont, &[RangeInclusive<u32>]); 5] = [
    // Geometric shapes (the eyes and mouths of emoticons), miscellaneous symbols, dingbats, symbols and arrows.
    (ExtraFont::FallbackSymbols, &[0x25A0..=0x25FF, 0x2600..=0x27BF, 0x2B00..=0x2BFF]),
    // Arrows, mathematical operators, miscellaneous technical.
    (ExtraFont::FallbackMath, &[0x2190..=0x23FF]),
    // Kannada: ಠ, ಥ, ಡ.
    (ExtraFont::FallbackKannada, &[0x0C80..=0x0CFF]),
    // Arabic and Arabic-Indic digits: ٩(◕‿◕｡)۶.
    (ExtraFont::FallbackArabic, &[0x0600..=0x06FF, 0x0750..=0x077F, 0x08A0..=0x08FF, 0xFB50..=0xFDFF, 0xFE70..=0xFEFF]),
    // CJK punctuation, kana, Hangul jamo, box drawing, vertical forms and the half- and fullwidth forms (｡ ｀ ）).
    (
        ExtraFont::FallbackCjk,
        &[
            0x1100..=0x11FF,
            0x2500..=0x259F,
            0x3000..=0x30FF,
            0x3131..=0x318E,
            0x31F0..=0x31FF,
            0xFE30..=0xFE4F,
            0xFF00..=0xFFEF,
        ],
    ),
];

impl ExtraFont {
    /// The fallback fonts, in the order of the font stack after Noto Sans.
    pub const FALLBACKS: [ExtraFont; 5] = [
        ExtraFont::FallbackSymbols,
        ExtraFont::FallbackMath,
        ExtraFont::FallbackKannada,
        ExtraFont::FallbackArabic,
        ExtraFont::FallbackCjk,
    ];

    /// The fallback font that is responsible for `c`, if there is one.
    pub fn fallback_for(c: char) -> Option<ExtraFont> {
        let cp = u32::from(c);
        CLAIMS.iter().find(|(_, ranges)| ranges.iter().any(|r| r.contains(&cp))).map(|(font, _)| *font)
    }

    /// The code points a fallback font is responsible for; empty for the other fonts.
    pub fn claimed(self) -> &'static [RangeInclusive<u32>] {
        CLAIMS.iter().find(|(font, _)| *font == self).map_or(&[], |(_, ranges)| ranges)
    }

    /// The Parley family name of a fallback font (the font's own family name).
    pub(crate) fn fallback_family(self) -> Option<&'static str> {
        match self {
            ExtraFont::FallbackSymbols => Some("Noto Sans Symbols 2"),
            ExtraFont::FallbackMath => Some("Noto Sans Math"),
            ExtraFont::FallbackKannada => Some("Noto Sans Kannada"),
            ExtraFont::FallbackArabic => Some("Noto Sans Arabic"),
            ExtraFont::FallbackCjk => Some("Noto Sans CJK JP"),
            _ => None,
        }
    }
}

/// True when `text` has a character that only a fallback font draws (Kannada, symbols, CJK punctuation, ...). A text
/// like that cannot be written as `<text>` in a file that must look the same everywhere, so SVG export outlines it.
pub fn needs_glyph_fallback(text: &str) -> bool {
    !text.is_ascii() && text.chars().any(|c| ExtraFont::fallback_for(c).is_some())
}

/// The families of the fallback fonts, after Noto Sans.
pub(crate) fn families() -> impl Iterator<Item = &'static str> {
    ExtraFont::FALLBACKS.into_iter().filter_map(ExtraFont::fallback_family)
}

/// Most distinct missing characters kept per layout; a text of a million unknown characters needs no more.
const MAX_MISSING: usize = 256;

/// Adds to `out` the characters of `run` that have no glyph (id 0) in the font the run was shaped with.
pub(crate) fn collect_missing(item: &PositionedLayoutItem<'_, StyleId>, text: &str, out: &mut BTreeSet<char>) {
    let PositionedLayoutItem::GlyphRun(gr) = item else { return };
    if !gr.positioned_glyphs().any(|g| g.id == 0) {
        return;
    }
    for cluster in gr.run().clusters().filter(|c| c.glyphs().any(|g| g.id == 0)) {
        let chars = text.get(cluster.text_range()).unwrap_or_default().chars();
        out.extend(chars.take(MAX_MISSING.saturating_sub(out.len())));
    }
}
