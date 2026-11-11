//! Colour runs: ranges of characters that have their own colour.

use drafftink_scene::{Color, ElementId, ElementKind, SceneTx, TextData, TextRun};

use super::{TextBindError, live};
use crate::target::TargetError;

/// Keeps the colour runs that lie in the unchanged beginning and end of the text.
pub fn carry_runs(old: &str, new: &str, runs: &[TextRun]) -> Vec<TextRun> {
    let (a, b): (Vec<char>, Vec<char>) = (old.chars().collect(), new.chars().collect());
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix =
        a.iter().rev().zip(b.iter().rev()).take(a.len().min(b.len()) - prefix).take_while(|(x, y)| x == y).count();
    let (old_len, new_len) = (a.len(), b.len());
    let to_u32 = |v: usize| u32::try_from(v).unwrap_or(u32::MAX);
    let mut out = Vec::new();
    for r in runs {
        let (s, e) = (r.start as usize, r.end as usize);
        if e <= prefix {
            out.push(r.clone());
        } else if s < prefix {
            out.push(TextRun { start: r.start, end: to_u32(prefix), color: r.color });
        } else if s >= old_len.saturating_sub(suffix) {
            let shift = |v: usize| (v + new_len).saturating_sub(old_len);
            out.push(TextRun { start: to_u32(shift(s)), end: to_u32(shift(e)), color: r.color });
        } else if e > old_len.saturating_sub(suffix) {
            let start = new_len.saturating_sub(suffix);
            out.push(TextRun {
                start: to_u32(start),
                end: to_u32(new_len.min((e + new_len).saturating_sub(old_len))),
                color: r.color,
            });
        }
    }
    out.retain(|r| r.start < r.end && (r.end as usize) <= new_len);
    out
}

/// A colour range from the editor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RunSpec {
    /// First character.
    pub start: u32,
    /// One past the last character.
    pub end: u32,
    /// The colour.
    pub color: Color,
}

fn check_range(start: u32, end: u32, len: usize) -> Result<(), TextBindError> {
    if end as usize > len || start as usize > len {
        return Err(TextBindError::BadRange { start, end, len });
    }
    if start >= end {
        return Err(TextBindError::EmptyRange { start, end, len });
    }
    Ok(())
}

/// Sets, or with `None` clears, the colour of the characters `start..end` of a text (`text.style_range`).
/// Returns false when nothing changed.
pub fn style_range<T: SceneTx>(
    tx: &mut T,
    id: &ElementId,
    start: u32,
    end: u32,
    color: Option<Color>,
) -> Result<bool, TextBindError> {
    let e = live(tx, id)?;
    let ElementKind::Text(td) = &e.kind else {
        return Err(TextBindError::NotText { id: id.clone(), kind: e.tag() });
    };
    if e.locked {
        return Err(TextBindError::Target(TargetError::Locked(vec![id.clone()])));
    }
    let len = td.char_count();
    check_range(start, end, len)?;
    let mut colors = td.char_colors();
    if let Some(slice) = colors.get_mut(start as usize..end as usize) {
        slice.fill(color);
    }
    let runs = TextData::runs_from_char_colors(&colors, len);
    if runs == td.runs {
        return Ok(false);
    }
    tx.update(id, |e| {
        if let ElementKind::Text(t) = &mut e.kind {
            t.runs = runs;
        }
    })?;
    Ok(true)
}

/// Replaces all colour runs of a text with the editor's final ranges (`text.edit_end`). Later ranges win where they
/// overlap. Returns false when the runs are unchanged.
pub fn replace_runs<T: SceneTx>(tx: &mut T, id: &ElementId, specs: &[RunSpec]) -> Result<bool, TextBindError> {
    let e = live(tx, id)?;
    let ElementKind::Text(td) = &e.kind else {
        return Err(TextBindError::NotText { id: id.clone(), kind: e.tag() });
    };
    let len = td.char_count();
    let mut colors: Vec<Option<Color>> = vec![None; len];
    for r in specs {
        check_range(r.start, r.end, len)?;
        if let Some(slice) = colors.get_mut(r.start as usize..r.end as usize) {
            slice.fill(Some(r.color));
        }
    }
    let runs: Vec<TextRun> = TextData::runs_from_char_colors(&colors, len);
    if runs == td.runs {
        return Ok(false);
    }
    tx.update(id, |e| {
        if let ElementKind::Text(t) = &mut e.kind {
            t.runs = runs;
        }
    })?;
    Ok(true)
}
