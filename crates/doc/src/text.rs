//! Text content and per-character style runs (colour, bold, italic) in a `LoroText`.
//!
//! The content is the text itself, so two people typing in one text element merge character by character. A run
//! is made of Loro marks on the characters it covers (`color` = `#rrggbb[aa]`, `bold` and `italic` = true), so runs
//! move with the characters they style when somebody else inserts or deletes in front of them. The model keeps runs as ranges of Unicode scalar
//! values; this module converts both ways.
//!
//! Normal form of runs (the store applies it on every write, so what you read equals what a peer reads): sorted,
//! non-empty, adjacent runs of the same style merged, and plain runs dropped (no colour, regular, upright is the
//! same as no run).

use drafftink_scene::{CharStyle, Color, TextRun};
use loro::{ExpandType, LoroDoc, LoroText, LoroValue, StyleConfig, StyleConfigMap};

use crate::error::DocError;

/// Name of the mark that carries a run colour.
pub(crate) const COLOR_MARK: &str = "color";
/// Name of the mark of a bold run.
pub(crate) const BOLD_MARK: &str = "bold";
/// Name of the mark of an italic run.
pub(crate) const ITALIC_MARK: &str = "italic";

/// Every mark a run can carry.
const MARKS: [&str; 3] = [COLOR_MARK, BOLD_MARK, ITALIC_MARK];

/// Tells the document how colour marks behave at their edges. Marks are not extended by typing next to them, so a
/// character typed after a red word is not red (the editor decides what typed text looks like). Must be called on every `LoroDoc` the store creates (it is not
/// part of the synced data).
pub(crate) fn configure(doc: &LoroDoc) {
    let mut styles = StyleConfigMap::new();
    for mark in MARKS {
        styles.insert(mark.into(), StyleConfig { expand: ExpandType::None });
    }
    doc.config_text_style(styles);
}

/// Puts runs into normal form.
pub(crate) fn normalize_runs(runs: &mut Vec<TextRun>) {
    runs.retain(|r| r.start < r.end && !r.style().is_plain());
    runs.sort_by_key(|r| (r.start, r.end));
    let mut out: Vec<TextRun> = Vec::with_capacity(runs.len());
    for r in runs.drain(..) {
        match out.last_mut() {
            Some(last) if last.end == r.start && last.style() == r.style() => last.end = r.end,
            _ => out.push(r),
        }
    }
    *runs = out;
}

/// Reads the colour runs from a text container.
pub(crate) fn read_runs(text: &LoroText) -> Vec<TextRun> {
    let mut runs: Vec<TextRun> = Vec::new();
    let LoroValue::List(items) = text.get_richtext_value() else { return runs };
    let mut pos: u32 = 0;
    for item in items.iter() {
        let LoroValue::Map(m) = item else { continue };
        let n = match m.get("insert") {
            Some(LoroValue::String(s)) => u32::try_from(s.chars().count()).unwrap_or(u32::MAX),
            _ => 0,
        };
        let attrs = match m.get("attributes") {
            Some(LoroValue::Map(attrs)) => Some(attrs),
            _ => None,
        };
        let color = match attrs.and_then(|a| a.get(COLOR_MARK)) {
            Some(LoroValue::String(c)) => Color::parse(c).ok(),
            _ => None,
        };
        let flag = |mark: &str| matches!(attrs.and_then(|a| a.get(mark)), Some(LoroValue::Bool(true)));
        let style = CharStyle { color, bold: flag(BOLD_MARK), italic: flag(ITALIC_MARK) };
        let end = pos.saturating_add(n);
        if !style.is_plain() {
            match runs.last_mut() {
                Some(last) if last.end == pos && last.style() == style => last.end = end,
                _ => runs.push(TextRun::new(pos, end, style)),
            }
        }
        pos = end;
    }
    runs
}

/// Replaces the text with `new` by deleting and inserting only the part between the common prefix and suffix.
/// Counts Unicode scalar values, like the model.
fn splice(text: &LoroText, old: &str, new: &str) -> Result<(), DocError> {
    let a: Vec<char> = old.chars().collect();
    let b: Vec<char> = new.chars().collect();
    let max_prefix = a.len().min(b.len());
    let prefix = a.iter().zip(b.iter()).take(max_prefix).take_while(|(x, y)| x == y).count();
    let max_suffix = max_prefix - prefix;
    let suffix = a.iter().rev().zip(b.iter().rev()).take(max_suffix).take_while(|(x, y)| x == y).count();
    let del = a.len() - prefix - suffix;
    if del > 0 {
        text.delete(prefix, del)?;
    }
    if b.len() - prefix - suffix > 0 {
        let ins: String = b.get(prefix..b.len() - suffix).unwrap_or_default().iter().collect();
        text.insert(prefix, &ins)?;
    }
    Ok(())
}

/// Makes `text` hold `content` with colour `runs`, writing only the difference. `runs` must be in normal form.
/// Returns whether anything was written.
pub(crate) fn write_text(text: &LoroText, content: &str, runs: &[TextRun]) -> Result<bool, DocError> {
    let old = text.to_string();
    let mut changed = false;
    if old != content {
        splice(text, &old, content)?;
        changed = true;
    }
    let current = read_runs(text);
    if current != runs {
        changed = true;
        if !current.is_empty() {
            for mark in MARKS {
                text.unmark(0..text.len_unicode(), mark)?;
            }
        }
        for r in runs {
            let (s, e) = (usize::try_from(r.start).unwrap_or(usize::MAX), usize::try_from(r.end).unwrap_or(0));
            if let Some(c) = r.color {
                text.mark(s..e, COLOR_MARK, c.to_hex())?;
            }
            if r.bold {
                text.mark(s..e, BOLD_MARK, true)?;
            }
            if r.italic {
                text.mark(s..e, ITALIC_MARK, true)?;
            }
        }
    }
    Ok(changed)
}
