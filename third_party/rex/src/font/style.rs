use super::Style;

// BMP codepoint points of each symbol group.
const UPPER_A: u32 = 0x41;
const UPPER_Z: u32 = 0x5A;
const LOWER_A: u32 = 0x61;
const LOWER_Z: u32 = 0x7A;
const UPPER_ALPHA: u32 = 0x391;
const UPPER_OMEGA: u32 = 0x3A9;
const LOWER_ALPHA: u32 = 0x3B1;
const LOWER_OMEGA: u32 = 0x3C9;
const DIGIT_0: u32 = 0x30;
const DIGIT_9: u32 = 0x39;

#[cfg_attr(rustfmt, rustfmt_skip)]
const LATIN_UPPER_LUT: [u32; 28] = [
//  None,              Italic,            Bold,              BoldItalic
    65      - UPPER_A, 0x1D434 - UPPER_A, 0x1D400 - UPPER_A, 0x1D468 - UPPER_A,   // Roman
    0x1D49C - UPPER_A, 0x1D49C - UPPER_A, 0x1D4D0 - UPPER_A, 0x1D4D0 - UPPER_A,   // Script
    0x1D504 - UPPER_A, 0x1D504 - UPPER_A, 0x1D56C - UPPER_A, 0x1D56C - UPPER_A,   // Fractur
    0x1D5A0 - UPPER_A, 0x1D608 - UPPER_A, 0x1D5D4 - UPPER_A, 0x1D63C - UPPER_A,   // SansSerif
    0x1D538 - UPPER_A, 0x1D538 - UPPER_A, 0x1D538 - UPPER_A, 0x1D538 - UPPER_A,   // Blackboard
    0x1D670 - UPPER_A, 0x1D670 - UPPER_A, 0x1D670 - UPPER_A, 0x1D670 - UPPER_A,   // Monospace
    0x1D434 - UPPER_A, 0x1D434 - UPPER_A, 0x1D400 - UPPER_A, 0x1D468 - UPPER_A,   // Normal
];

#[cfg_attr(rustfmt, rustfmt_skip)]
const LATIN_LOWER_LUT: [u32; 28] = [
//  None,              Italic,            Bold,              BoldItalic
    97      - LOWER_A, 0x1D44E - LOWER_A, 0x1D41A - LOWER_A, 0x1D482 - LOWER_A,   // Roman
    0x1D4B6 - LOWER_A, 0x1D4B6 - LOWER_A, 0x1D4EA - LOWER_A, 0x1D4EA - LOWER_A,   // Script
    0x1D51E - LOWER_A, 0x1D51E - LOWER_A, 0x1D586 - LOWER_A, 0x1D586 - LOWER_A,   // Fracture
    0x1D5BA - LOWER_A, 0x1D622 - LOWER_A, 0x1D5EE - LOWER_A, 0x1D656 - LOWER_A,   // SansSerif
    0x1D552 - LOWER_A, 0x1D552 - LOWER_A, 0x1D552 - LOWER_A, 0x1D552 - LOWER_A,   // Blackboard
    0x1D68A - LOWER_A, 0x1D68A - LOWER_A, 0x1D68A - LOWER_A, 0x1D68A - LOWER_A,   // Monospace
    0x1D44E - LOWER_A, 0x1D44E - LOWER_A, 0x1D41A - LOWER_A, 0x1D482 - LOWER_A,   // Normal
];

#[cfg_attr(rustfmt, rustfmt_skip)]
const GREEK_UPPER_LUT: [u32; 28] = [
//  None,                  Italic,                Bold,                  BoldItalic
    0x391   - UPPER_ALPHA, 0x1D6E2 - UPPER_ALPHA, 0x1D6A8 - UPPER_ALPHA, 0x1D71C - UPPER_ALPHA,   // Roman
    0x391   - UPPER_ALPHA, 0x1D6E2 - UPPER_ALPHA, 0x1D6A8 - UPPER_ALPHA, 0x1D71C - UPPER_ALPHA,   // Script
    0x391   - UPPER_ALPHA, 0x1D6E2 - UPPER_ALPHA, 0x1D6A8 - UPPER_ALPHA, 0x1D71C - UPPER_ALPHA,   // Fracture
    0x391   - UPPER_ALPHA, 0x1D6E2 - UPPER_ALPHA, 0x1D756 - UPPER_ALPHA, 0x1D790 - UPPER_ALPHA,   // SansSerif
    0x391   - UPPER_ALPHA, 0x1D6E2 - UPPER_ALPHA, 0x1D6A8 - UPPER_ALPHA, 0x1D71C - UPPER_ALPHA,   // Blackboard
    0x391   - UPPER_ALPHA, 0x1D6E2 - UPPER_ALPHA, 0x1D6A8 - UPPER_ALPHA, 0x1D71C - UPPER_ALPHA,   // Monospace
    0x1D6E2 - UPPER_ALPHA, 0x1D6E2 - UPPER_ALPHA, 0x1D6A8 - UPPER_ALPHA, 0x1D71C - UPPER_ALPHA,   // Normal
];

#[cfg_attr(rustfmt, rustfmt_skip)]
const GREEK_LOWER_LUT: [u32; 28] = [
//  None,                  Italic,                Bold,                  BoldItalic
    0x3B1   - LOWER_ALPHA, 0x1D6FC - LOWER_ALPHA, 0x1D6C2 - LOWER_ALPHA, 0x1D736 - LOWER_ALPHA,   // Roman
    0x3B1   - LOWER_ALPHA, 0x1D6FC - LOWER_ALPHA, 0x1D6C2 - LOWER_ALPHA, 0x1D736 - LOWER_ALPHA,   // Script
    0x3B1   - LOWER_ALPHA, 0x1D6FC - LOWER_ALPHA, 0x1D6C2 - LOWER_ALPHA, 0x1D736 - LOWER_ALPHA,   // Fracture
    0x3B1   - LOWER_ALPHA, 0x1D6FC - LOWER_ALPHA, 0x1D770 - LOWER_ALPHA, 0x1D7AA - LOWER_ALPHA,   // SansSerif
    0x3B1   - LOWER_ALPHA, 0x1D6FC - LOWER_ALPHA, 0x1D6C2 - LOWER_ALPHA, 0x1D736 - LOWER_ALPHA,   // Blackboard
    0x3B1   - LOWER_ALPHA, 0x1D6FC - LOWER_ALPHA, 0x1D6C2 - LOWER_ALPHA, 0x1D736 - LOWER_ALPHA,   // Monospace
    0x1D6FC - LOWER_ALPHA, 0x1D6FC - LOWER_ALPHA, 0x1D6C2 - LOWER_ALPHA, 0x1D736 - LOWER_ALPHA,   // Normal
];

#[cfg_attr(rustfmt, rustfmt_skip)]
const DIGIT_LUT: [u32; 28] = [
//  None,              Italic,            Bold,              BoldItalic
    48      - DIGIT_0, 48      - DIGIT_0, 0x1D7CE - DIGIT_0, 0x1D7CE - DIGIT_0,   // Roman
    48      - DIGIT_0, 48      - DIGIT_0, 0x1D7CE - DIGIT_0, 0x1D7CE - DIGIT_0,   // Script
    48      - DIGIT_0, 48      - DIGIT_0, 0x1D7CE - DIGIT_0, 0x1D7CE - DIGIT_0,   // Fracture
    0x1D7E2 - DIGIT_0, 0x1D7E2 - DIGIT_0, 0x1D7EC - DIGIT_0, 0x1D7EC - DIGIT_0,   // SansSerif
    0x1D7D8 - DIGIT_0, 0x1D7D8 - DIGIT_0, 0x1D7D8 - DIGIT_0, 0x1D7D8 - DIGIT_0,   // Blackboard
    0x1D7F6 - DIGIT_0, 0x1D7F6 - DIGIT_0, 0x1D7F6 - DIGIT_0, 0x1D7F6 - DIGIT_0,   // Monospace
    48      - DIGIT_0, 48      - DIGIT_0, 0x1D7CE - DIGIT_0, 0x1D7CE - DIGIT_0,   // Normal
];

/// Take a codepoint and a style (a weight and family pair), and remaps a given codepoint  
/// to the appropriate mathematical codepoint one in the Unicode table (e.g. in roman italic, maps the "vanilla" 'U+0051 Latin Small Letter A' to 'U+1D44E Mathematical Italic Small A')
pub fn style_symbol(codepoint: char, style: Style) -> char {
    let codepoint = codepoint as u32;
    let cp = match codepoint {
        LOWER_A ..= LOWER_Z => style_lookup(&LATIN_LOWER_LUT, codepoint, style),
        UPPER_A ..= UPPER_Z => style_lookup(&LATIN_UPPER_LUT, codepoint, style),
        UPPER_ALPHA ..= UPPER_OMEGA => style_lookup(&GREEK_UPPER_LUT, codepoint, style),
        LOWER_ALPHA ..= LOWER_OMEGA => style_lookup(&GREEK_LOWER_LUT, codepoint, style),
        DIGIT_0 ..= DIGIT_9 => style_lookup(&DIGIT_LUT, codepoint, style),
        _ => style_other(codepoint, style),
    };
    std::char::from_u32(cp).unwrap()
}

fn style_lookup(lut: &[u32], codepoint: u32, style: Style) -> u32 {
    let y = style.family as usize;
    let x = style.weight as usize;
    let result = codepoint + lut[4 * y + x];
    match unicode_math::MATH_ALPHANUMERIC_TABLE_RESERVED_REPLACEMENTS.binary_search_by_key(&result, |x| x.0) {
        Ok(i)  => unicode_math::MATH_ALPHANUMERIC_TABLE_RESERVED_REPLACEMENTS[i].1,
        Err(_) => result,
    }
}

// TODO: Investigate these symbols.
// Exceptional glyphs that are not found in BMP
// and don't belong in the previous groups
// The handling of these values can be reference
//   codepoint-math-usv.dtx in codepoint-math
//   RenderMathMLToken.cpp in WebKit
//
// const VAR_UPPER_THETA:      u32 = 0x3F4;
// const VAR_DIGAMMA:          u32 = 0x3DC;  ??
// const VAR_EPSILON:          u32 = 0x3F5;
// const VAR_THETA:            u32 = 0x3D1;
// const VAR_KAPPA:            u32 = 0x3F0;
// const VAR_PHI:              u32 = 0x3D5;
// const VAR_RHO:              u32 = 0x3F1;
// const VAR_PI:               u32 = 0x3D6;
// const DIGAMMA:              u32 = 0x3DD; ??
// const PARTIAL_DIFFERENTIAL: u32 = 0x2202;
// const NABLA:                u32 = 0x2207;

// const BOLD_VAR_UPPER_THETA:      u32 = 0x1D6B9;
// const BOLD_VAR_EPSILON:          u32 = 0x1D6DC;
// const BOLD_VAR_THETA:            u32 = 0x1D6DD;
// const BOLD_VAR_KAPPA:            u32 = 0x1D6DE;
// const BOLD_VAR_PHI:              u32 = 0x1D6DF;
// const BOLD_VAR_RHO:              u32 = 0x1D6E0;
// const BOLD_VAR_PI:               u32 = 0x1D6E1;
// const BOLD_NABLA:                u32 = 0x1D6C1;
// const BOLD_PARTIAL_DIFFERENTIAL: u32 = 0x1D6DB;

fn style_other(sym: u32, _: Style) -> u32 {
    match sym {
        0x2D => 0x2212, // Minus sign
        _ => sym,
    }
}
