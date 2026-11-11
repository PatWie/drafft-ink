//! Style runs: ranges of characters that have their own colour, bold or italic.

use drafftink_scene::{CharStyle, Color, ElementId, ElementKind, Kind, SceneTx, TextData, TextRun};

use super::{TextBindError, TextMeasure, label_of, live, remeasure_text};
use crate::target::TargetError;

/// Keeps the style runs that lie in the unchanged beginning and end of the text.
pub fn carry_runs(old: &str, new: &str, runs: &[TextRun]) -> Vec<TextRun> {
    let (a, b): (Vec<char>, Vec<char>) = (old.chars().collect(), new.chars().collect());
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix =
        a.iter().rev().zip(b.iter().rev()).take(a.len().min(b.len()) - prefix).take_while(|(x, y)| x == y).count();
    let (old_len, new_len) = (a.len(), b.len());
    let to_u32 = |v: usize| u32::try_from(v).unwrap_or(u32::MAX);
    let placed = |r: &TextRun, start: u32, end: u32| TextRun { start, end, ..r.clone() };
    let mut out = Vec::new();
    for r in runs {
        let (s, e) = (r.start as usize, r.end as usize);
        if e <= prefix {
            out.push(r.clone());
        } else if s < prefix {
            out.push(placed(r, r.start, to_u32(prefix)));
        } else if s >= old_len.saturating_sub(suffix) {
            let shift = |v: usize| (v + new_len).saturating_sub(old_len);
            out.push(placed(r, to_u32(shift(s)), to_u32(shift(e))));
        } else if e > old_len.saturating_sub(suffix) {
            let start = new_len.saturating_sub(suffix);
            out.push(placed(r, to_u32(start), to_u32(new_len.min((e + new_len).saturating_sub(old_len)))));
        }
    }
    out.retain(|r| r.start < r.end && (r.end as usize) <= new_len);
    out
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

/// The live, unlocked text `id`.
fn editable_text<'a, T: SceneTx>(tx: &'a T, id: &ElementId) -> Result<&'a TextData, TextBindError> {
    let e = live(tx, id)?;
    let ElementKind::Text(td) = &e.kind else {
        return Err(TextBindError::NotText { id: id.clone(), kind: e.tag() });
    };
    if e.locked {
        return Err(TextBindError::Target(TargetError::Locked(vec![id.clone()])));
    }
    Ok(td)
}

/// Writes `runs` to the text, unless they are what it has. Returns whether it changed.
fn store_runs<T: SceneTx>(tx: &mut T, id: &ElementId, runs: Vec<TextRun>) -> Result<bool, TextBindError> {
    let unchanged = match &live(tx, id)?.kind {
        ElementKind::Text(td) => td.runs == runs,
        _ => true,
    };
    if unchanged {
        return Ok(false);
    }
    tx.update(id, |e| {
        if let ElementKind::Text(t) = &mut e.kind {
            t.runs = runs;
        }
    })?;
    Ok(true)
}

/// Changes the style of the characters `start..end` of a text with `change`; the other characters keep theirs.
fn restyle_range<T: SceneTx>(
    tx: &mut T,
    id: &ElementId,
    start: u32,
    end: u32,
    change: impl Fn(CharStyle) -> CharStyle,
) -> Result<bool, TextBindError> {
    let td = editable_text(tx, id)?;
    let len = td.char_count();
    check_range(start, end, len)?;
    let mut styles = td.char_styles();
    if let Some(slice) = styles.get_mut(start as usize..end as usize) {
        slice.iter_mut().for_each(|s| *s = change(*s));
    }
    store_runs(tx, id, TextData::runs_from_char_styles(&styles, len))
}

/// Sets, or with `None` clears, the colour of the characters `start..end` of a text (`text.style_range`). Bold and
/// italic stay. Returns false when nothing changed.
pub fn style_range<T: SceneTx>(
    tx: &mut T,
    id: &ElementId,
    start: u32,
    end: u32,
    color: Option<Color>,
) -> Result<bool, TextBindError> {
    restyle_range(tx, id, start, end, |s| CharStyle { color, ..s })
}

/// Makes the characters `start..end` of a text bold and italic, or plain again, as asked: `None` leaves that face as it
/// is (`text.format`). Colours stay. Returns false when nothing changed.
pub fn format_range<T: SceneTx>(
    tx: &mut T,
    id: &ElementId,
    start: u32,
    end: u32,
    bold: Option<bool>,
    italic: Option<bool>,
) -> Result<bool, TextBindError> {
    restyle_range(tx, id, start, end, |s| CharStyle {
        bold: bold.unwrap_or(s.bold),
        italic: italic.unwrap_or(s.italic),
        ..s
    })
}

/// What [`format_texts`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FormatReport {
    /// Texts whose characters changed.
    pub changed: Vec<ElementId>,
    /// Texts that already looked as asked.
    pub unchanged: Vec<ElementId>,
    /// Targets that are neither a text nor a shape or arrow with a label.
    pub skipped: Vec<ElementId>,
}

impl FormatReport {
    /// True when at least one target was a text.
    pub fn applicable(&self) -> bool {
        !self.changed.is_empty() || !self.unchanged.is_empty()
    }
}

/// Makes the text in `ids` bold or italic, or regular again (`None` leaves that face as it is), over the characters
/// `range` or else the whole text. A shape or arrow with a label formats the label. Boxes are measured again, as bold
/// letters are wider. Locked targets refuse the whole call before anything is written.
pub fn format_texts<T: SceneTx, M: TextMeasure + ?Sized>(
    tx: &mut T,
    m: &M,
    ids: &[ElementId],
    range: Option<(u32, u32)>,
    bold: Option<bool>,
    italic: Option<bool>,
) -> Result<FormatReport, TextBindError> {
    let mut report = FormatReport::default();
    let mut targets: Vec<ElementId> = Vec::new();
    for id in ids {
        let e = live(tx, id)?;
        let target = match e.tag() {
            Kind::Text => Some(id.clone()),
            k if k.is_label_container() => label_of(tx, id),
            _ => None,
        };
        match target {
            Some(t) if !targets.contains(&t) => {
                if e.locked || live(tx, &t)?.locked {
                    return Err(TextBindError::Target(TargetError::Locked(vec![id.clone()])));
                }
                targets.push(t);
            }
            Some(_) => {}
            None => report.skipped.push(id.clone()),
        }
    }
    if range.is_some() && targets.len() > 1 {
        return Err(TextBindError::RangeNeedsOneText { n: targets.len() });
    }
    for id in targets {
        let len = editable_text_len(tx, &id)?;
        let (start, end) = range.unwrap_or((0, u32::try_from(len).unwrap_or(u32::MAX)));
        if range.is_none() && len == 0 {
            report.unchanged.push(id);
            continue;
        }
        if format_range(tx, &id, start, end, bold, italic)? {
            remeasure_text(tx, m, &id)?;
            report.changed.push(id);
        } else {
            report.unchanged.push(id);
        }
    }
    Ok(report)
}

/// Replaces all style runs of a text with the editor's final runs (`text.edit_end`). Later runs win where they
/// overlap. Returns false when the runs are unchanged.
pub fn replace_runs<T: SceneTx>(tx: &mut T, id: &ElementId, specs: &[TextRun]) -> Result<bool, TextBindError> {
    let len = editable_text_len(tx, id)?;
    let mut styles = vec![CharStyle::default(); len];
    for r in specs {
        check_range(r.start, r.end, len)?;
        if let Some(slice) = styles.get_mut(r.start as usize..r.end as usize) {
            slice.fill(r.style());
        }
    }
    store_runs(tx, id, TextData::runs_from_char_styles(&styles, len))
}

/// The number of characters of the text `id` (a live text, locked or not: the editor's own result goes through).
fn editable_text_len<T: SceneTx>(tx: &T, id: &ElementId) -> Result<usize, TextBindError> {
    let e = live(tx, id)?;
    match &e.kind {
        ElementKind::Text(td) => Ok(td.char_count()),
        _ => Err(TextBindError::NotText { id: id.clone(), kind: e.tag() }),
    }
}
