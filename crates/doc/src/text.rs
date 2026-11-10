//! Text content and per-character colour runs in a `LoroText`.
//!
//! The content is the text itself, so two people typing in one text element merge character by character. A colour
//! run is a Loro mark (`color` = `#rrggbb[aa]`) on the characters it covers, so runs move with the characters they
//! colour when somebody else inserts or deletes in front of them. The model keeps runs as ranges of Unicode scalar
//! values; this module converts both ways.
//!
//! Normal form of runs (the store applies it on every write, so what you read equals what a peer reads): sorted,
//! non-empty, adjacent runs of the same colour merged, and runs without a colour dropped (no colour means "use the
//! stroke colour", which is the same as no run).

use drafftink_scene::{Color, TextRun};
use loro::{ExpandType, LoroDoc, LoroText, LoroValue, StyleConfig, StyleConfigMap};

use crate::error::DocError;

/// Name of the mark that carries a run colour.
pub(crate) const COLOR_MARK: &str = "color";

/// Tells the document how colour marks behave at their edges. Marks are not extended by typing next to them, so a
/// character typed after a red word is not red. Must be called on every `LoroDoc` the store creates (it is not
/// part of the synced data).
pub(crate) fn configure(doc: &LoroDoc) {
    let mut styles = StyleConfigMap::new();
    styles.insert(COLOR_MARK.into(), StyleConfig { expand: ExpandType::None });
    doc.config_text_style(styles);
}

/// Puts runs into normal form.
pub(crate) fn normalize_runs(runs: &mut Vec<TextRun>) {
    runs.retain(|r| r.start < r.end && r.color.is_some());
    runs.sort_by_key(|r| (r.start, r.end));
    let mut out: Vec<TextRun> = Vec::with_capacity(runs.len());
    for r in runs.drain(..) {
        match out.last_mut() {
            Some(last) if last.end == r.start && last.color == r.color => last.end = r.end,
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
        let color = match m.get("attributes") {
            Some(LoroValue::Map(attrs)) => match attrs.get(COLOR_MARK) {
                Some(LoroValue::String(c)) => Color::parse(c).ok(),
                _ => None,
            },
            _ => None,
        };
        let end = pos.saturating_add(n);
        if let Some(color) = color {
            match runs.last_mut() {
                Some(last) if last.end == pos && last.color == Some(color) => last.end = end,
                _ => runs.push(TextRun { start: pos, end, color: Some(color) }),
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
            text.unmark(0..text.len_unicode(), COLOR_MARK)?;
        }
        for r in runs {
            if let Some(c) = r.color {
                let (s, e) = (usize::try_from(r.start).unwrap_or(usize::MAX), usize::try_from(r.end).unwrap_or(0));
                text.mark(s..e, COLOR_MARK, c.to_hex())?;
            }
        }
    }
    Ok(changed)
}
