//! This module defines functions that gives the most esthetically pleasing spacing between two types of symbols.
//! Functions from this module for instance decide that "f" is followed by less space in "f(" than in "f +".
use std::convert::TryFrom;

use crate::font::TexSymbolType;
use crate::layout::Style;
use crate::dimensions::Unit;
use crate::dimensions::units::Em;


/// Determines the type of an expression to be laid out.
/// Documentation for the variants is quoted from Eijkhout's "TeX By Topic" (p. 114)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AtomType {
    /// ordinary: lowercase Greek characters and those symbols that are ‘just symbols’; the
    /// command \mathord forces this class.
    Ordinary,
    ///  large operator: integral and sum signs, and ‘big’ objects such as \bigcap or
    /// \bigotimes; the command \mathop forces this class. Characters that are large opera-
    /// tors are centred vertically, and they may behave differently in display style from in
    /// the other styles.
    Operator,
    ///  binary operation: plus and minus, and things such as \cap or \otimes; the command
    /// \mathbin forces this class.
    BinOperator,
    ///  relation (also called binary relation): equals, less than, and greater than signs, subset
    /// and superset, perpendicular, parallel; the command \mathrel forces this class.
    Relation,
    ///  opening symbol : opening brace, bracket, parenthesis, angle, floor, ceiling; the command
    /// \mathopen forces this class.
    Open,
    ///  closing symbol : closing brace, bracket, parenthesis, angle, floor, ceiling; the command
    /// \mathclose forces this class.
    Close,
    ///  punctuation: most punctuation marks, but : is a relation, the \colon is a punctuation
    /// colon; the command \mathpunct forces this class.
    Punctuation,
    /// the inner subformulas. No characters can be assigned to this class,
    /// but characters and subformulas can be forced into it by \mathinner. The ⟨generalized fraction⟩s
    /// and \left...\right groups are inner formulas. Inner formulas are surrounded by some white
    /// space.
    Inner,
}

impl TryFrom<TexSymbolType> for AtomType {
    type Error = ();
    fn try_from(value: TexSymbolType) -> Result<Self, Self::Error> {
        // The conversion table here is taken from [SILE typesetter](https://github.com/sile-typesetter/sile/blob/b2cc0841ff603abc335c5e66d8cc3c64b65365eb/packages/math/unicode-symbols.lua#L23)
        // source code
        match value {
            TexSymbolType::Punctuation   => Ok(AtomType::Punctuation),
            TexSymbolType::Ordinary      => Ok(AtomType::Ordinary),
            TexSymbolType::Open          => Ok(AtomType::Open),
            TexSymbolType::Close         => Ok(AtomType::Close),
            TexSymbolType::Binary        => Ok(AtomType::BinOperator),
            TexSymbolType::Relation      => Ok(AtomType::Relation),
            TexSymbolType::Alpha         => Ok(AtomType::Ordinary),
            TexSymbolType::Fence         => Ok(AtomType::Ordinary),
            TexSymbolType::Operator(_)   => Ok(AtomType::Operator),
            TexSymbolType::Inner         => Ok(AtomType::Inner),
            TexSymbolType::Radical       => Ok(AtomType::Open),
            // these ones shouldn't participate in the spacing rules
            TexSymbolType::Accent        => Err(()),
            TexSymbolType::AccentWide    => Err(()),
            TexSymbolType::AccentOverlay => Err(()),
            TexSymbolType::BotAccent     => Err(()),
            TexSymbolType::BotAccentWide => Err(()),
            TexSymbolType::Over          => Err(()),
            TexSymbolType::Under         => Err(()),
            TexSymbolType::Transparent   => Err(()),
        }
    }
}

/// Given the type of two subsequent atoms and the current style, 
/// determines how much spacing should occur between the two
/// symbols.
pub fn atom_space(left: TexSymbolType, right: TexSymbolType, style: Style) -> Spacing {
    let left  = AtomType::try_from(left).ok();
    let right = AtomType::try_from(right).ok();

    if let Some((left, right)) = Option::zip(left, right) {
        if style >= Style::TextCramped {
            match (left, right) {
                (AtomType::Ordinary,      AtomType::Operator)     => Spacing::Thin,
                (AtomType::Ordinary,      AtomType::BinOperator)  => Spacing::Medium,
                (AtomType::Ordinary,      AtomType::Relation)     => Spacing::Thick,
                (AtomType::Ordinary,      AtomType::Inner)        => Spacing::Thin,
                (AtomType::Operator,      AtomType::Ordinary)     => Spacing::Thin,
                (AtomType::Operator,      AtomType::Operator)     => Spacing::Thin,
                (AtomType::Operator,      AtomType::Relation)     => Spacing::Thick,
                (AtomType::Operator,      AtomType::Inner)        => Spacing::Thin,
                (AtomType::BinOperator,   AtomType::Ordinary)     => Spacing::Medium,
                (AtomType::BinOperator,   AtomType::Operator)     => Spacing::Medium,
                (AtomType::BinOperator,   AtomType::Open)         => Spacing::Medium,
                (AtomType::BinOperator,   AtomType::Inner)        => Spacing::Medium,
                (AtomType::Relation,      AtomType::Ordinary)     => Spacing::Thick,
                (AtomType::Relation,      AtomType::Operator)     => Spacing::Thick,
                (AtomType::Relation,      AtomType::Open)         => Spacing::Thick,
                (AtomType::Relation,      AtomType::Inner)        => Spacing::Thick,
                (AtomType::Close,         AtomType::Operator)     => Spacing::Thin,
                (AtomType::Close,         AtomType::BinOperator)  => Spacing::Medium,
                (AtomType::Close,         AtomType::Relation)     => Spacing::Thick,
                (AtomType::Close,         AtomType::Inner)        => Spacing::Thin,

                // Here it is better to list everything but Spacing::Thin
                (AtomType::Inner, AtomType::BinOperator)   => Spacing::Medium,
                (AtomType::Inner, AtomType::Relation) => Spacing::Thick,
                (AtomType::Inner, AtomType::Close)    => Spacing::None,
                (AtomType::Inner, _)                  => Spacing::Thin,

                // Every valid (punct, _) pair is undefined or Thin
                (AtomType::Punctuation, _) => Spacing::Thin,
                _ => Spacing::None,
            }
        } else {
            match (left, right) {
                (AtomType::Ordinary, AtomType::Operator)  => Spacing::Thin,
                (AtomType::Operator, AtomType::Ordinary)  => Spacing::Thin,
                (AtomType::Operator, AtomType::Operator)  => Spacing::Thin,
                (AtomType::Close,    AtomType::Operator)  => Spacing::Thin,
                (AtomType::Inner,    AtomType::Operator)  => Spacing::Thin,
                _ => Spacing::None,
            }
        }
    }
    else {
        Spacing::None
    }
}

/// Different types of space
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Spacing {
    /// no space
    None,
    /// thin space
    Thin,
    /// medium space
    Medium,
    /// thick space
    Thick,
}

impl Spacing {
    /// Returns how much a given type of spaces measure in *em* units
    pub const fn to_length(self) -> Unit<Em> {
        match self {
            Spacing::None   => Unit::<Em>::new(0.0),
            Spacing::Thin   => Unit::<Em>::new(0.1666666666666666666666666), // 1 / 6
            Spacing::Medium => Unit::<Em>::new(0.2222222222222222222222222), // 2 / 9
            Spacing::Thick  => Unit::<Em>::new(0.3333333333333333333333333), // 1 / 3
        }
    }
}
