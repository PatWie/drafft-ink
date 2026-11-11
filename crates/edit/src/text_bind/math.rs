//! Formulas: sizing a math element from its layout. Math cannot be laid out here (the typesetter sits above
//! this crate's layout of text), so the caller passes a [`MathMeasure`] and this module only writes the result.

use drafftink_scene::{ElementId, ElementKind, SceneTx};
use drafftink_typeset::MathError;
use kurbo::Size;

use super::font::anchor_top_left;
use super::{TextBindError, live};
use crate::target::TargetError;

/// Lays out a formula. The engine implements it over the typesetter.
pub trait MathMeasure {
    /// The size of the formula at a font size, or the typesetter's error when it does not parse.
    fn math_box(&self, latex: &str, size: f64) -> Result<Size, MathError>;
}

/// Lays out `latex` at `size` and writes formula, size and box. The rotated top-left corner stays.
pub fn set_math_size<T: SceneTx>(
    tx: &mut T,
    math: &dyn MathMeasure,
    id: &ElementId,
    latex: &str,
    size: f64,
) -> Result<(), TextBindError> {
    if latex.trim().is_empty() {
        return Err(TextBindError::EmptyFormula);
    }
    if latex.chars().count() > drafftink_scene::limits::MAX_LATEX_CHARS {
        return Err(TextBindError::FormulaTooLong { max: drafftink_scene::limits::MAX_LATEX_CHARS });
    }
    let b = math.math_box(latex, size).map_err(|error| {
        let (line, column) = error.line_col(latex);
        TextBindError::InvalidLatex { error, line: line + 1, column: column + 1 }
    })?;
    let (w, h) = (b.width.max(1.0), b.height.max(1.0));
    if !(w.is_finite() && h.is_finite()) {
        return Err(TextBindError::NotFinite);
    }
    let old = live(tx, id)?.transform;
    let (x, y) = anchor_top_left(&old, w, h);
    let latex = latex.to_owned();
    tx.update(id, |e| {
        if let ElementKind::Math(d) = &mut e.kind {
            d.latex = latex;
            d.font_size = size;
        }
        e.transform.x = x;
        e.transform.y = y;
        e.transform.w = w;
        e.transform.h = h;
    })?;
    Ok(())
}

/// Replaces the formula of a math element (`text.set_latex`). On a parse error nothing is written. Returns false
/// when the formula was already as given.
pub fn set_latex<T: SceneTx>(
    tx: &mut T,
    math: &dyn MathMeasure,
    id: &ElementId,
    latex: &str,
) -> Result<bool, TextBindError> {
    let e = live(tx, id)?;
    let ElementKind::Math(d) = &e.kind else {
        return Err(TextBindError::NotMath { id: id.clone(), kind: e.tag() });
    };
    if e.locked {
        return Err(TextBindError::Target(TargetError::Locked(vec![id.clone()])));
    }
    if d.latex == latex {
        return Ok(false);
    }
    let size = d.font_size;
    set_math_size(tx, math, id, latex, size)?;
    Ok(true)
}
