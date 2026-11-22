//! The highlighter tool: what a drag of the marker turns the highlight on (or off) for.
//!
//! The tool creates no element. A drag collects *targets*: whole elements that the path touches, and for a text
//! the character ranges that the path crosses (found on the real text layout, see [`sweep`]). One call of
//! [`apply`] then writes them all in the caller's transaction, so the drag is one undo step and one change for
//! collaborators.
//!
//! **The seam.** How a highlight is stored is not decided here. [`apply`] checks the targets and calls a
//! [`HighlightWriter`]: "set the highlight of this element" and "set the highlight of these characters of this
//! text". The `highlight` style property wires its own writer in (one `impl HighlightWriter`); nothing else in
//! this module changes. [`MarkerColor`] stands in for the colour type of that property until it lands.

mod sweep;

use std::ops::Range;

use drafftink_scene::{ElementId, ElementKind, Kind, SceneTx, StoreError};

use crate::target::TargetError;

pub use sweep::{CharSweep, byte_range_to_chars};

/// The marker colours. A stand-in for the palette of the `highlight` style property.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum MarkerColor {
    /// Yellow.
    Yellow,
    /// Green.
    Green,
    /// Pink.
    Pink,
    /// Blue.
    Blue,
    /// Orange.
    Orange,
}

impl MarkerColor {
    /// The five colours, in the order the picker shows them.
    pub const ALL: [MarkerColor; 5] =
        [MarkerColor::Yellow, MarkerColor::Green, MarkerColor::Pink, MarkerColor::Blue, MarkerColor::Orange];

    /// The colour as a translucent `(r, g, b, a)`, for the preview of a drag.
    pub fn rgba(self) -> (u8, u8, u8, u8) {
        match self {
            MarkerColor::Yellow => (255, 226, 0, 110),
            MarkerColor::Green => (60, 220, 90, 110),
            MarkerColor::Pink => (255, 105, 180, 110),
            MarkerColor::Blue => (80, 170, 255, 110),
            MarkerColor::Orange => (255, 150, 30, 110),
        }
    }
}

/// True when the marker can highlight elements of this kind: the ones that have a band behind them.
pub fn applies(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::Rectangle | Kind::Diamond | Kind::Ellipse | Kind::Line | Kind::Arrow | Kind::Freehand | Kind::Text
    )
}

/// One element the marker touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The element.
    pub id: ElementId,
    /// For a text: the characters (indices of `char`s, not bytes) to highlight, sorted, apart and not empty.
    /// `None` is the whole element.
    pub chars: Option<Vec<Range<usize>>>,
}

/// Why a highlight could not be written.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum HighlightError {
    /// An element is missing, deleted or locked.
    #[error(transparent)]
    Target(#[from] TargetError),
    /// The element has no band to highlight (a picture, a frame, a formula).
    #[error("{id} is a {}, and a highlight does not apply to it", .kind.name())]
    NotApplicable {
        /// The element.
        id: ElementId,
        /// What it is.
        kind: Kind,
    },
    /// Characters were given for something that is not a text, or past the end of the text.
    #[error("{id} has {len} characters, and {start}..{end} is not inside them")]
    BadRange {
        /// The element.
        id: ElementId,
        /// The first character asked for.
        start: usize,
        /// One past the last.
        end: usize,
        /// The characters the element has (0 when it is not a text).
        len: usize,
    },
    /// The store refused a write.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// There is no writer for the highlight yet.
    #[error("highlights are not available in this build yet")]
    Unavailable,
}

/// Where a highlight is stored. The `highlight` style property implements this once.
pub trait HighlightWriter {
    /// Turns the highlight of the whole element on with `color`, or off with `None`. True when it changed.
    fn set_element<T: SceneTx>(
        &self,
        tx: &mut T,
        id: &ElementId,
        color: Option<MarkerColor>,
    ) -> Result<bool, HighlightError>;

    /// Turns the highlight of the characters `chars` of a text on with `color`, or off with `None`. True when it
    /// changed.
    fn set_chars<T: SceneTx>(
        &self,
        tx: &mut T,
        id: &ElementId,
        chars: Range<usize>,
        color: Option<MarkerColor>,
    ) -> Result<bool, HighlightError>;
}

/// What [`apply`] changed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Applied {
    /// The elements whose highlight changed, in the order of the targets.
    pub changed: Vec<ElementId>,
}

/// Writes the highlight of every target (`color` `None` removes it). Everything is checked before the first
/// write, so a refusal changes nothing; the writes all go through `tx`, so the caller's commit makes them one
/// step.
pub fn apply<T: SceneTx, W: HighlightWriter>(
    tx: &mut T,
    writer: &W,
    targets: &[Target],
    color: Option<MarkerColor>,
) -> Result<Applied, HighlightError> {
    let checked = targets.iter().map(|t| check(&*tx, t)).collect::<Result<Vec<_>, _>>()?;
    let mut changed = Vec::new();
    for (target, ranges) in targets.iter().zip(checked) {
        let did = match ranges {
            None => writer.set_element(tx, &target.id, color)?,
            Some(ranges) => {
                let mut any = false;
                for r in ranges {
                    any |= writer.set_chars(tx, &target.id, r, color)?;
                }
                any
            }
        };
        if did {
            changed.push(target.id.clone());
        }
    }
    Ok(Applied { changed })
}

/// The ranges of a target in order and apart, `None` for a whole element; or why it cannot be written.
fn check<R: drafftink_scene::SceneRead>(scene: &R, t: &Target) -> Result<Option<Vec<Range<usize>>>, HighlightError> {
    let e = scene.get(&t.id).filter(|e| !e.deleted).ok_or_else(|| TargetError::NotFound(t.id.clone()))?;
    if e.locked {
        return Err(TargetError::Locked(vec![t.id.clone()]).into());
    }
    if !applies(e.tag()) {
        return Err(HighlightError::NotApplicable { id: t.id.clone(), kind: e.tag() });
    }
    let Some(ranges) = &t.chars else { return Ok(None) };
    let len = match &e.kind {
        ElementKind::Text(d) => d.content.chars().count(),
        _ => 0,
    };
    if let Some(bad) = ranges.iter().find(|r| r.start >= r.end || r.end > len) {
        return Err(HighlightError::BadRange { id: t.id.clone(), start: bad.start, end: bad.end, len });
    }
    Ok(Some(merge(ranges)))
}

/// The ranges sorted, with the ones that touch or overlap joined.
pub fn merge(ranges: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut sorted = ranges.to_vec();
    sorted.sort_by_key(|r| (r.start, r.end));
    let mut out: Vec<Range<usize>> = Vec::with_capacity(sorted.len());
    for r in sorted {
        match out.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

#[cfg(test)]
mod tests;
