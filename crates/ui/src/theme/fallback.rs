//! Glyph fallback for the egui text: the fonts that draw the characters the UI fonts lack (Kannada, symbols, CJK
//! punctuation), so that a document name or a library item named `ಠ_ಠ` is not a row of boxes.
//!
//! These are Noto Sans (for characters like the undertie in `◕‿◕`) and the typesetter's [`ExtraFont::FALLBACKS`]. A
//! build that carries them (the desktop) shares the typesetter's bytes. The web build fetches a font when a user's text
//! needs it: the surfaces that show user text call [`note_text`], which asks egui which characters it cannot draw, the
//! host fetches [`wanted`] and gives the file to [`provide`], and the next frame installs it. The app also gives the
//! UI every one of these fonts that the canvas text has fetched.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use drafftink_typeset::ExtraFont;

#[derive(Default)]
struct State {
    /// The files fetched at run time. The bytes stay for the life of the page.
    provided: BTreeMap<ExtraFont, &'static [u8]>,
    /// Fonts that a text shown by the UI needed and that this build does not have yet.
    wanted: BTreeSet<ExtraFont>,
}

/// The fonts the UI uses as its glyph fallback, in the order of the font stack.
const FONTS: [ExtraFont; 6] = [
    ExtraFont::NotoSansRegular,
    ExtraFont::FallbackSymbols,
    ExtraFont::FallbackMath,
    ExtraFont::FallbackKannada,
    ExtraFont::FallbackArabic,
    ExtraFont::FallbackCjk,
];

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn file(state: &State, font: ExtraFont) -> Option<&'static [u8]> {
    font.embedded().or_else(|| state.provided.get(&font).copied())
}

/// True for the fonts the UI uses as its glyph fallback; [`provide`] ignores the others.
pub fn uses(font: ExtraFont) -> bool {
    FONTS.contains(&font)
}

/// Gives the UI the file of a fallback font; the next frame installs it. Other fonts, and fonts the UI has, are ignored.
pub fn provide(font: ExtraFont, bytes: Vec<u8>) {
    STATE.with_borrow_mut(|s| {
        s.wanted.remove(&font);
        if uses(font) && file(s, font).is_none() {
            s.provided.insert(font, Box::leak(bytes.into_boxed_slice()));
        }
    });
}

/// The fonts that draw `missing`, the characters egui cannot draw, except those `has` says are there already: the
/// fallback font that is responsible for each, or Noto Sans.
fn needed(missing: impl IntoIterator<Item = char>, has: impl Fn(ExtraFont) -> bool) -> BTreeSet<ExtraFont> {
    let font = |c| ExtraFont::fallback_for(c).unwrap_or(ExtraFont::NotoSansRegular);
    missing.into_iter().map(font).filter(|font| !has(*font)).collect()
}

/// Notes that the UI is about to show `text`: a character that no font of `ctx` draws makes the font that has it
/// wanted, unless this build has that font already. Cheap for text that is plain ASCII.
pub fn note_text(ctx: &egui::Context, text: &str) {
    if text.is_ascii() {
        return;
    }
    let probe = egui::FontId::proportional(1.0);
    let missing: Vec<char> =
        ctx.fonts_mut(|fonts| text.chars().filter(|c| !c.is_control() && !fonts.has_glyph(&probe, *c)).collect());
    if missing.is_empty() {
        return;
    }
    STATE.with_borrow_mut(|s| {
        let wanted = needed(missing, |font| file(s, font).is_some());
        s.wanted.extend(wanted);
    });
}

/// The fallback fonts that a shown text needs and this build does not have yet. The host fetches them and calls
/// [`provide`].
pub fn wanted() -> Vec<ExtraFont> {
    STATE.with_borrow(|s| s.wanted.iter().copied().collect())
}

/// How many fonts have arrived: the installed egui fonts are out of date when this changes.
pub(super) fn version() -> usize {
    STATE.with_borrow(|s| s.provided.len())
}

/// The fallback fonts this build has: `(egui font name, bytes)`, in the order of the font stack.
pub(super) fn fonts() -> Vec<(&'static str, &'static [u8])> {
    STATE.with_borrow(|s| FONTS.into_iter().filter_map(|font| Some((font.file_name(), file(s, font)?))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_characters_want_the_font_that_draws_them_unless_it_is_there() {
        let none = |_: ExtraFont| false;
        assert!(needed([], none).is_empty());
        assert_eq!(needed(['ಠ'], none), BTreeSet::from([ExtraFont::FallbackKannada]));
        assert_eq!(
            needed("｡◕‿".chars(), none),
            BTreeSet::from([ExtraFont::FallbackCjk, ExtraFont::FallbackSymbols, ExtraFont::NotoSansRegular]),
            "the undertie is in none of the small fonts, so Noto Sans draws it"
        );
        assert!(
            needed(['ಠ'], |font| font == ExtraFont::FallbackKannada).is_empty(),
            "a font that is there is not wanted"
        );
    }

    #[test]
    fn every_font_of_the_stack_is_a_font_the_typesetter_knows() {
        for font in FONTS {
            assert!(!font.file_name().is_empty());
        }
    }
}
