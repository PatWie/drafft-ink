/// Obtain kerning measurements for subscripts and superscripts.
/// The kerning is the amount of space that one must add between the symbol and its super-/sub-script.
#[deny(missing_docs)]
pub mod kerning;

/// Different implementations of the 'MathFont' trait for various font parsing crates, like 'ttf-parser'.
#[deny(missing_docs)]
pub mod backend;
/// Contains types and utilities related to fonts.
/// In particular, defines utilities related to extended glyphs, i.e. glyphs like '}' and '→', which can be made bigger.
#[deny(missing_docs)]
pub mod common;
mod style;
//mod unit;

pub use unicode_math::TexSymbolType;
pub use style::style_symbol;


pub use crate::font::common::{Direction, VariantGlyph};

use crate::font::common::{GlyphId, ScriptLevel};
use crate::dimensions::Unit;
use crate::dimensions::units::{Em, FUnit, Ratio};
use crate::error::FontError;

use self::kerning::Corner;


pub trait MathFont : Sized {
    fn glyph_index(&self, codepoint: char) -> Option<crate::font::common::GlyphId>;
    fn glyph_from_gid<'f>(&'f self, glyph_id : GlyphId) -> Result<Glyph<'f, Self>, FontError>;
    fn kern_for(&self, glyph_id : GlyphId, height : Unit<FUnit>, side : Corner) -> Option<Unit<FUnit>>;

    fn italics(&self, glyph_id : GlyphId) -> i16;
    fn attachment(&self, glyph_id : GlyphId) -> i16; 
    fn constants(&self, font_units_to_em: Unit<Ratio<Em, FUnit>>) -> FontConstants;
    fn font_units_to_em(&self) -> Unit<Ratio<Em, FUnit>>;


    fn horz_variant(&self, gid: GlyphId, width: Unit<FUnit>)  -> VariantGlyph;
    fn vert_variant(&self, gid: GlyphId, height: Unit<FUnit>) -> VariantGlyph;

    fn glyph<'f>(& 'f self, codepoint: char) -> Result<Glyph<'f, Self>, FontError> {
        let gid = self.glyph_index(codepoint).ok_or(FontError::MissingGlyphCodepoint(codepoint))?;
        self.glyph_from_gid(gid)
    }

    /// If the font supports `ssty`, this function offers to replace a certain glyph with one more appropriate for sub- and super-scripts
    /// As per the spec, a font may offer substitutions for non-nested superscripts (level 1) and for superscripts of superscripts (level 2)
    /// The `script_level_two` boolean parameter specifies whether a level 1 substitute is requested (false) or a level 2 substitute (true)
    #[allow(unused_variables)]
    fn glyph_script_alternate(&self, gid: GlyphId, script_level : ScriptLevel) -> Option<GlyphId> {
        None
    }


}


pub struct FontMetricsCache {
    constants: FontConstants,
    units_per_em: Unit<Ratio<FUnit, Em>>,
}

impl Clone for FontMetricsCache {
    fn clone(&self) -> Self {
        Self {
            constants:    self.constants.clone(),
            units_per_em: self.units_per_em,
        }
    }
}

impl FontMetricsCache {
    pub fn new<'f, F : MathFont>(font: &'f F) -> Self {
        let font_units_to_em = font.font_units_to_em();
        let units_per_em = font_units_to_em.recip();
        let constants = font.constants(font_units_to_em);

        Self {
            units_per_em,
            constants
        }
    }

    pub fn constants(&self) -> &FontConstants {
        &self.constants
    }

    pub fn units_per_em(&self) -> Unit<Ratio<FUnit, Em>> {
        self.units_per_em
    }
}


#[derive(Clone)]
pub struct FontConstants {
    pub subscript_shift_down: Unit<Em>,
    pub subscript_top_max: Unit<Em>,
    pub subscript_baseline_drop_min: Unit<Em>,

    pub superscript_baseline_drop_max: Unit<Em>,
    pub superscript_bottom_min: Unit<Em>,
    pub superscript_shift_up_cramped: Unit<Em>,
    pub superscript_shift_up: Unit<Em>,
    pub sub_superscript_gap_min: Unit<Em>,

    pub upper_limit_baseline_rise_min: Unit<Em>,
    pub upper_limit_gap_min: Unit<Em>,
    pub lower_limit_gap_min: Unit<Em>,
    pub lower_limit_baseline_drop_min: Unit<Em>,

    pub fraction_rule_thickness: Unit<Em>,
    pub fraction_numerator_display_style_shift_up: Unit<Em>,
    pub fraction_denominator_display_style_shift_down: Unit<Em>,
    pub fraction_num_display_style_gap_min: Unit<Em>,
    pub fraction_denom_display_style_gap_min: Unit<Em>,
    pub fraction_numerator_shift_up: Unit<Em>,
    pub fraction_denominator_shift_down: Unit<Em>,
    pub fraction_numerator_gap_min: Unit<Em>,
    pub fraction_denominator_gap_min: Unit<Em>,

    pub axis_height: Unit<Em>,
    pub accent_base_height: Unit<Em>,

    pub delimited_sub_formula_min_height: Unit<Em>,
    pub display_operator_min_height: Unit<Em>,

    pub radical_display_style_vertical_gap: Unit<Em>,
    pub radical_vertical_gap: Unit<Em>,
    pub radical_rule_thickness: Unit<Em>,
    pub radical_extra_ascender: Unit<Em>,
    pub radical_kern_before_degree: Unit<Em>,
    pub radical_kern_after_degree: Unit<Em>,
    /// Percentage (0–100) of the radical height to raise the degree bottom.
    pub radical_degree_bottom_raise_percent: i16,

    pub stack_display_style_gap_min: Unit<Em>,
    pub stack_top_display_style_shift_up: Unit<Em>,
    pub stack_top_shift_up: Unit<Em>,
    pub stack_bottom_shift_down: Unit<Em>,
    pub stack_gap_min: Unit<Em>,

    pub delimiter_factor: f64,
    pub delimiter_short_fall: Unit<Em>,
    pub null_delimiter_space: Unit<Em>,

    pub underbar_vertical_gap: Unit<Em>,
    pub underbar_rule_thickness: Unit<Em>,
    pub underbar_extra_descender: Unit<Em>,

    pub script_percent_scale_down: f64,
    pub script_script_percent_scale_down: f64,
}


pub struct Glyph<'f, F> {
    pub font: &'f F,
    pub gid:  GlyphId,
    // x_min, y_min, x_max, y_max
    pub bbox: (Unit<FUnit>, Unit<FUnit>, Unit<FUnit>, Unit<FUnit>),
    pub advance: Unit<FUnit>,
    pub lsb: Unit<FUnit>,
    pub italics: Unit<FUnit>,
    pub attachment: Unit<FUnit>,
}
impl<'f, F> Glyph<'f, F> {
    pub fn height(&self) -> Unit<FUnit> {
        self.bbox.3
    }
    pub fn depth(&self) -> Unit<FUnit> {
        self.bbox.1
    }
}

#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
pub struct Style {
    pub family: Family,
    pub weight: Weight,
}


impl Style {
    pub fn new() -> Style {
        Style::default()
    }

    pub fn with_family(self, fam: Family) -> Style {
        Style {
            family: fam,
            ..self
        }
    }

    pub fn with_weight(self, weight: Weight) -> Style {
        Style {
            weight: weight,
            ..self
        }
    }

    pub fn with_bold(self) -> Style {
        Style {
            weight: self.weight.with_bold(),
            ..self
        }
    }

    pub fn with_italics(self) -> Style {
        Style {
            weight: self.weight.with_italics(),
            ..self
        }
    }
}

// NB: Changing the order of these variants requires
//     changing the LUP in fontselection
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Family {
    Roman,
    Script,
    Fraktur,
    SansSerif,
    Blackboard,
    Monospace,
    Normal,
}

// NB: Changing the order of these variants requires
//     changing the LUP in fontselection
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Weight {
    None,
    Italic,
    Bold,
    BoldItalic,
}

impl Weight {
    fn with_bold(self) -> Self {
        match self {
            Weight::Italic | Weight::BoldItalic => Weight::BoldItalic,
            _ => Weight::Bold,
        }
    }

    fn with_italics(self) -> Self {
        match self {
            Weight::Bold | Weight::BoldItalic => Weight::BoldItalic,
            _ => Weight::Italic,
        }
    }
}

impl Default for Family {
    fn default() -> Family {
        Family::Normal
    }
}

impl Default for Weight {
    fn default() -> Weight {
        Weight::None
    }
}
