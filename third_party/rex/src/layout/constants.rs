//! Length constants for layout

use crate::dimensions::{units::{Em, Pt}, Unit};

/// Determines how large `\big` makes a delimiter.
/// More precisely, in TeX, `\big(` is actually `\left( ... \right.` where the `...` is a box of a certain height and null width. 
/// [`BIG_HEIGHT`] tells the size of the enclosed box.
// In "Tex By Topic" by van Eijhkout (section 21.2.4), this value is 8.5pt. 
// However, experiments reveal that the size is font size dependent and that the value above is only valid for 10pt font size. 
// Hence the value here is chosen to ensure 8.5pt at a 10pt font size.
pub const BIG_HEIGHT : Unit<Em> = Unit::<Em>::new(0.85);

// From [https://tex.stackexchange.com/questions/48276/latex-specify-font-point-size] & `info latex`
/// Desired distance between two baselines of an array. 
/// If one line happens to contain extra high items, then the distance between baselines may prove bigger (cf [`LINE_SKIP_LIMIT_ARRAY`]).
/// "A rule of thumb is that the baselineskip should be 1.2 times the font size." (`info latex`)
pub const BASELINE_SKIP : Unit<Em> = Unit::<Em>::new(1.2);




// Values obtained by writing `\the\lineskiplimit` and `\the\lineskip` in an array environment
/// If the distance between the depth of a line (how far below the baseline the text on that line goes) and the height of the following line (how far above the baseline the text on that line goes)
/// is smaller than this value, [`LINE_SKIP_ARRAY`] is inserted.  
/// Cf Tex by Topic (chap. 15)
pub const LINE_SKIP_LIMIT_ARRAY : Unit<Pt> = Unit::new(0.);
/// Extra space to insert when lines are too close to one another. Cf [`LINE_SKIP_LIMIT_ARRAY`].
pub const LINE_SKIP_ARRAY : Unit<Pt> = Unit::new(1.);


/// Additional line height added to [`BASELINE_SKIP`] in `\begin{aligned}` environment.
pub const JOT : Unit<Pt> = Unit::new(3.);


// The values below are gathered from the definition of the corresponding commands in "article.cls" on a default LateX installation
/// For a row in an array, corresponds to the fraction of the row's height (~ [`BASELINE_SKIP`]) which is above the baseline on which characters sit.
pub const STRUT_HEIGHT      : f64 = 0.7;         // \strutbox height = 0.7\baseline
/// For a row in an array, corresponds to the fraction of the row's height (~ [`BASELINE_SKIP`]) which is below the baseline on which characters sit.
pub const STRUT_DEPTH       : f64 = 0.3;         // \strutbox depth  = 0.3\baseline

/// Half the space between two columns of an array, corresponds to LaTeX (`\arraycolsep`)
/// > "\arraycolsep : Half the width of the default horizontal space between columns in an array environment"
/// From Lamport - LateX a document preparation system (2end edition) - p. 207  
pub const COLUMN_SEP        : Unit<Pt> = Unit::<Pt>::new(5.0) ;  // \arraycolsep

/// Width of the vertical bar that separates columns
pub const RULE_WIDTH        : Unit<Pt> = Unit::<Pt>::new(0.4) ;  // \arrayrulewidth

/// Space between two consecutive vertical bars in an array (e.g. `\begin{array}{c||c} .. \end{array}`)
pub const DOUBLE_RULE_SEP   : Unit<Pt> = Unit::<Pt>::new(2.0) ;  // \doublerulesep
