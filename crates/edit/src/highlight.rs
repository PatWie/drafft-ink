//! Highlighting a set of elements and character ranges in one step.
//!
//! This is the rule behind `highlight.apply`, which the highlighter tool and the highlight control call. One call
//! is one transaction, so it is one undo step and one sync change, however many elements and ranges it names.
//!
//! * Whole elements: `style.highlight` is set (or cleared). Frames cannot be highlighted; they are reported as
//!   skipped. A text named here is highlighted all over, and the highlights its runs had of their own go.
//! * Character ranges of a text: the runs get a highlight of their own (`text_bind::highlight_range`). Ranges
//!   count Unicode scalar values from 0, `end` exclusive, like [`drafftink_scene::TextRun`].
//!
//! Everything is checked before anything is written, so a failed call changes nothing.

use drafftink_scene::{Element, ElementId, ElementKind, Kind, MarkerColor, SceneRead, SceneTx, StoreError};

use crate::style::{self, Prop, StyleError, StyleProps};
use crate::target::TargetError;
use crate::text_bind::{self, TextBindError};

/// A range of characters of one text element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharRange {
    /// The text element (a label counts: it is a text element).
    pub id: ElementId,
    /// First character, counted in Unicode scalar values from 0.
    pub start: u32,
    /// One past the last character. Must be above `start` and at most the length of the text in characters.
    pub end: u32,
}

/// What [`apply`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HighlightReport {
    /// Elements and texts whose highlight changed.
    pub changed: Vec<ElementId>,
    /// Elements and ranges that already looked as asked.
    pub unchanged: Vec<ElementId>,
    /// Elements that cannot be highlighted (frames).
    pub skipped: Vec<ElementId>,
}

/// Why a highlight was refused.
#[derive(Debug, thiserror::Error)]
pub enum HighlightError {
    /// Nothing was named.
    #[error("nothing to highlight was given: name elements, character ranges, or both")]
    Nothing,
    /// An element rule: missing, deleted, locked, or none of them can be highlighted.
    #[error(transparent)]
    Style(#[from] StyleError),
    /// A range rule: not a text, out of range, empty.
    #[error(transparent)]
    Text(#[from] TextBindError),
    /// The store refused the write.
    #[error(transparent)]
    Store(#[from] StoreError),
}

fn live<'a, R: SceneRead>(r: &'a R, id: &ElementId) -> Result<&'a Element, TargetError> {
    r.get(id).filter(|e| !e.deleted).ok_or_else(|| TargetError::NotFound(id.clone()))
}

/// Sets the highlight of `elements` and of the character `ranges` to `color`, or takes it off with `None`.
///
/// Elements that cannot hold a highlight are skipped and reported; when that leaves nothing to do and no range is
/// named, the call is refused. A locked, deleted or unknown element, a range outside its text, or a range of
/// something that is not a text refuses the whole call before anything is written.
pub fn apply<T: SceneTx>(
    tx: &mut T,
    color: Option<MarkerColor>,
    elements: &[ElementId],
    ranges: &[CharRange],
) -> Result<HighlightReport, HighlightError> {
    if elements.is_empty() && ranges.is_empty() {
        return Err(HighlightError::Nothing);
    }
    let mut report = HighlightReport::default();
    // Check the ranges now, with the same rules the write has, so that a bad one leaves the elements alone too.
    for r in ranges {
        let e = live(tx, &r.id).map_err(TextBindError::from)?;
        if e.locked {
            return Err(TextBindError::Target(TargetError::Locked(vec![r.id.clone()])).into());
        }
        let len = match &e.kind {
            ElementKind::Text(t) => t.char_count(),
            _ => return Err(TextBindError::NotText { id: r.id.clone(), kind: e.tag() }.into()),
        };
        if r.start >= r.end {
            return Err(TextBindError::EmptyRange { start: r.start, end: r.end, len }.into());
        }
        if r.end as usize > len {
            return Err(TextBindError::BadRange { start: r.start, end: r.end, len }.into());
        }
    }
    let (fit, frames): (Vec<ElementId>, Vec<ElementId>) =
        elements.iter().cloned().partition(|id| tx.get(id).is_none_or(|e| e.tag() != Kind::Frame));
    report.skipped = frames;
    if !fit.is_empty() {
        let props = StyleProps { highlight: Some(color), ..StyleProps::default() };
        let done = style::apply(tx, &fit, &props, false)?;
        for id in &fit {
            let note = if done.changed.contains(id) { &mut report.changed } else { &mut report.unchanged };
            note.push(id.clone());
        }
    } else if ranges.is_empty() {
        return Err(StyleError::NotApplicable { props: vec![Prop::Highlight] }.into());
    }
    for r in ranges {
        let note = if text_bind::highlight_range(tx, &r.id, r.start, r.end, color)? {
            &mut report.changed
        } else {
            &mut report.unchanged
        };
        if !note.contains(&r.id) {
            note.push(r.id.clone());
        }
    }
    Ok(report)
}
