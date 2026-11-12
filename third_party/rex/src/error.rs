//! Defines different error types related to various phases of rendering a formula.
//!   - [`FontError`] : errors that have to do with the font file provided (missing MATH table, no such glyph).
//!   - [`ParseError`] : syntax error in the formula provided (mismatching brackets, unknown command).
//!   - [`LayoutError`] : errors during the layout phase ; currently, these can only be font errors.

use crate::font::common::GlyphId;
use crate::parser::error::ParseError;
use std::fmt;

/// Result type for the [`LayoutError`]
pub type LayoutResult<T> = ::std::result::Result<T, LayoutError>;
/// Result type for the [`ParseError`]
pub type ParseResult<T> = ::std::result::Result<T, ParseError>;

/// Errors during the layout phase ; currently, these can only be font errors.
#[derive(Debug, Clone, PartialEq)]
pub enum LayoutError {
    /// a font error
    Font(FontError)
}

/// Errors having to do with font file provided
#[derive(Debug, Clone, PartialEq)]
pub enum FontError {
    /// The font does not contain a glyph for the given char.
    MissingGlyphCodepoint(char),
    /// The font does not contain a glyph with that id.
    MissingGlyphGID(GlyphId),
    /// The font lacks a MATH table.
    NoMATHTable,
}

impl From<FontError> for LayoutError {
    fn from(e: FontError) -> Self {
        LayoutError::Font(e)
    }
}



/// A generic error type covering any error that may happen during the process.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// a parse error
    Parse(ParseError),
    /// a layout error (including font errors)
    Layout(LayoutError)
}
impl From<ParseError> for Error {
    fn from(e: ParseError) -> Self {
        Error::Parse(e)
    }
}
impl From<LayoutError> for Error {
    fn from(e:LayoutError) -> Self {
        Error::Layout(e)
    }
}


impl fmt::Display for FontError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use self::FontError::*;
        match *self {
            MissingGlyphCodepoint(cp) =>
                write!(f, "missing glyph for codepoint'{}'", cp),
            MissingGlyphGID(gid) =>
                write!(f, "missing glyph with gid {}", Into::<u16>::into(gid)),
            NoMATHTable =>
                write!(f, "no MATH tables"),
        }
    }
}
