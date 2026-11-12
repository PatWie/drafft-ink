//! This module defines a function determining the needed kerning (i.e. negative space) to put between a glyph and its sub-/superscript
//! Example: with italic character like `f` in math mode in e.g. `f_1^2`, the subscript needs to be slightly closer to the letter and the superscript slightly further

use crate::{dimensions::{units::FUnit, Unit}, error::LayoutResult, font::{kerning::Corner, MathFont}};

use super::Layout;


/// Computes the amount of kerning between a base layout and its subscript
pub fn subscript_kern<F : MathFont>(base: &Layout<F>, script: &Layout<F>, base_depth: Unit<FUnit>, script_height: Unit<FUnit>) -> LayoutResult<Unit<FUnit>> {
    let value1 = kern_from_layout(base, base_depth, Corner::BottomRight)? +
    kern_from_layout(script, base_depth, Corner::TopLeft)?;

    let value2 = kern_from_layout(base, script_height, Corner::BottomRight)? +
    kern_from_layout(script, script_height, Corner::TopLeft)?;

    Ok(
        if value1 < value2 
        { value1 }
        else 
        { value2 }
    )
}


// I question the accuracy of this algorithm.  But it's not yet clear to me what
// the relavent values should represent with respect to the "cut-ins" for the kerning.
// for now, I'm just going to port the algorithm I found in LuaTeX and XeTeX.
// If nothing else, it will at least be consistent.


// Horizontal Position:
//     - By default, set flat to base glyph
//     - For superscript, add italics correction from base character
//     - For suprscript:
//         - Calculate bottom of script (after shiftup)
//         - Calculate top of base.
//     - For subscript:
//         - Calculate top of script (after shift down)
//         - Calculate bottom of base
//     - For each script:
//         - Find math kern value at this height for base.
//           (TopRight for superscript, BotRight for subscript)
//         - Find math kern value at this height for sciprt.
//           (BotLeft for subscript, TopRight for superscript)
//         - Add the values together together
//     - Horintal kern is applied to smallest of two results
//       from previous step.


/// Computes the amount of kerning (= horizontal space) between a base glyph and its superscript
pub fn superscript_kern<F : MathFont>(base: &Layout<F>, script: &Layout<F>, base_height: Unit<FUnit>, script_depth: Unit<FUnit>) -> LayoutResult<Unit<FUnit>> {
    let value1 = kern_from_layout(base, base_height, Corner::TopRight)? +
    kern_from_layout(script, base_height, Corner::BottomLeft)?;

    let value2 = kern_from_layout(base, script_depth, Corner::TopRight)? +
    kern_from_layout(script, script_depth, Corner::BottomLeft)?;

    Ok(
        if value1 > value2 
        { value1 }
        else 
        { value2 }
    )
}

fn kern_from_layout<F : MathFont>(layout: &Layout<F>,  height: Unit<FUnit>, side: Corner) -> LayoutResult<Unit<FUnit>> {
    Ok(
        if let Some(glyph) = layout.is_symbol() {
            let font = glyph.font;
            let font_glyph = font.glyph_from_gid(glyph.gid)?;
            font_glyph.font.kern_for(font_glyph.gid, height, side).unwrap_or(Unit::ZERO)
        }
        else {
            Unit::ZERO
        }
    )
}

