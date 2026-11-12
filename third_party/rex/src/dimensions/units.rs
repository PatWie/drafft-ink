//! Defines all the units relevant to rendering: font units, em, points, inches, pixels
//!
//! This module defines common units and conventional conversion factors between these (between pt and inches, inches and pixels, etc).
//! The conversions not given here are font-dependent or font size-dependent:
//!    - setting the conversion factor between [`Em`] to [`Pt`] is precisely what specifying a font size is about (cf [`FontSize`]).
//!    - the factor between [`FUnit`] and [`Em`] is specified in the font file in OpenType
// TODO: define font size

// ------------------------- BASIC UNITS --------------------------------

/// Smallest virtual units that the font file can address (so every dimension in the font file is given as an integer number in FUnit)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FUnit;

/// A virtual unit, bigger than [`FUnit`], approximately but not necessarily equal to the width of upper-case M.
///
///  It is conventionally equal one or more of the following quantities:
///
///  - width of an em-dash and an em-space character
///  - line separation
///  - (historically but no longer true), width of an upper case M
///
/// In actuality, font designers need not abide by any of these conventions ; they do whatever they want.
/// The correspondance between em and FUnit is specified in the font file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Em;


/// Specifying a font size is by definition specifying how many points there is in an em.
/// 12 pt size means "1 em = 12 pt"
///
/// Standardly, 1pt is set to 1 / 72 inch (DeskTop Publishing Point). Inches are physical units (i.e. you can measure with a ruler). 
/// Our convention of specifying sizes in pt implies that we are deciding how large em (and characters) should appear on screen at 100% zoom.
/// Now, it is difficult to convert from physical units to pixels (which is what rendering cares about) without knowledge of the display used so we only guarantee the standard of 96 PPI screen.
/// Cf [`Inch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pt;

/// This is the dimension relevant for printing ; this is a physical dimension used for e.g paper or the size of a computer screen (e.g. 20'' monitor)
/// 
/// To connect the numeric measures to the physical measures, several measures are used
/// 
///   - **dots per inch** : on a printer, the number of colored dots to spit in an inch of length
///   - **pixels per inch** : on a screen, the number of pixels there is in an inch ; so, if a monitor is set to a resolution of 1920 x 1080 and has a 20 inch diagonal (20''),
///     the PPI will be 1920 / 20 = 96 PPI
///
/// We need PPI for render (cf [`Pt`]). Because we don't know what screen the person is using we assume a standard PPI of 96 throughout the crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Inch;


/// Final texture pixel
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Px;


/// Font size is expressed in pt / em 
pub type FontSize = Ratio<Pt, Em>;

// ------------------- UNIT COMBINATORS -------------------------

/// If `U` is a unit and `V` is a unit, `Ratio<U, V>` is the unit `U . V⁻¹`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ratio<U, V> {
	_numerator    : std::marker::PhantomData<U>,
	_denominator  : std::marker::PhantomData<V>,
}


impl<U, V> Ratio<U, V> {
	/// Creates new ratio unit
	pub const fn new() -> Self { 
		Self { 
			_numerator:   std::marker::PhantomData, 
			_denominator: std::marker::PhantomData 
		} 
	}
}