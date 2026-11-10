//! Sequence diagram parser. Line based; block keywords are recognised before anything else so that their text is
//! never taken for a message.

use std::collections::HashMap;

use super::text::{clean_label, shorten, split_word, strip_comment};
use super::types::{Activation, Event, MessageHead, NotePlacement, Participant, SequenceDiagram};
use super::{FORMAT, MAX_ID_CHARS, MAX_TOTAL_TEXT_BYTES, Prepared, push_warning};
use crate::error::IoError;
use crate::limits::MAX_DIAGRAM_ITEMS;

/// Words that open, continue, close or configure a block and never start a message.
const SKIPPED: &[&str] = &[
    "autonumber",
    "title",
    "loop",
    "alt",
    "else",
    "opt",
    "par",
    "par_over",
    "and",
    "critical",
    "option",
    "break",
    "rect",
    "box",
    "end",
    "activate",
    "deactivate",
    "destroy",
    "link",
    "links",
    "properties",
    "details",
    "accTitle",
    "accDescr",
];

type Arrow = (&'static str, bool, MessageHead, bool);

/// Arrow tokens, longest first at each position: (text, dashed, head, bidirectional).
const ARROWS: &[Arrow] = &[
    ("<<-->>", true, MessageHead::Arrow, true),
    ("<<->>", false, MessageHead::Arrow, true),
    ("-->>", true, MessageHead::Arrow, false),
    ("->>", false, MessageHead::Arrow, false),
    ("--x", true, MessageHead::Cross, false),
    ("-x", false, MessageHead::Cross, false),
    ("--)", true, MessageHead::Open, false),
    ("-)", false, MessageHead::Open, false),
    ("-->", true, MessageHead::None, false),
    ("->", false, MessageHead::None, false),
];

struct Builder {
    participants: Vec<Participant>,
    index: HashMap<String, usize>,
    events: Vec<Event>,
    text_bytes: usize,
    warnings: Vec<String>,
}

pub(super) fn parse(p: &Prepared<'_>) -> Result<SequenceDiagram, IoError> {
    let mut b = Builder {
        participants: Vec::new(),
        index: HashMap::new(),
        events: Vec::new(),
        text_bytes: 0,
        warnings: Vec::new(),
    };
    for line in &p.lines {
        b.line(strip_comment(line).trim())?;
    }
    if b.participants.is_empty() && b.events.is_empty() {
        return Err(IoError::invalid(FORMAT, "the sequence diagram has no participants and no messages"));
    }
    Ok(SequenceDiagram { participants: b.participants, events: b.events, warnings: b.warnings })
}

impl Builder {
    fn warn(&mut self, message: String) {
        push_warning(&mut self.warnings, message);
    }

    fn charge(&mut self, bytes: usize) -> Result<(), IoError> {
        self.text_bytes = self.text_bytes.saturating_add(bytes);
        if self.text_bytes > MAX_TOTAL_TEXT_BYTES {
            return Err(IoError::Limit {
                what: "text of the diagram (ids and labels)",
                max: format!("{MAX_TOTAL_TEXT_BYTES} bytes"),
            });
        }
        Ok(())
    }

    fn check_items(&self) -> Result<(), IoError> {
        if self.participants.len() + self.events.len() >= MAX_DIAGRAM_ITEMS {
            return Err(IoError::Limit {
                what: "diagram (participants and events)",
                max: format!("{MAX_DIAGRAM_ITEMS} items"),
            });
        }
        Ok(())
    }

    fn line(&mut self, line: &str) -> Result<(), IoError> {
        if line.is_empty() {
            return Ok(());
        }
        let word_end = line.find(|c: char| c.is_whitespace() || c == ':').unwrap_or(line.len());
        let word = line.get(..word_end).unwrap_or("");
        let rest = line.get(word_end..).unwrap_or("").trim_start_matches(':').trim();
        if SKIPPED.contains(&word) {
            return Ok(());
        }
        match word {
            "participant" => return self.declare(rest, false),
            "actor" => return self.declare(rest, true),
            "create" => {
                let (kind, tail) = split_word(rest);
                return match kind {
                    "participant" => self.declare(tail, false),
                    "actor" => self.declare(tail, true),
                    _ => Ok(()),
                };
            }
            _ => {}
        }
        if word.eq_ignore_ascii_case("note") {
            return self.note(rest);
        }
        self.message(line)
    }

    /// Returns the index of the participant, adding it when unknown.
    fn ensure(&mut self, id: &str) -> Result<usize, IoError> {
        if let Some(&i) = self.index.get(id) {
            return Ok(i);
        }
        if id.chars().count() > MAX_ID_CHARS {
            return Err(IoError::Limit { what: "identifier", max: format!("{MAX_ID_CHARS} characters") });
        }
        self.check_items()?;
        self.charge(id.len() * 2)?;
        self.participants.push(Participant { id: id.to_string(), label: id.to_string(), is_actor: false });
        let i = self.participants.len() - 1;
        self.index.insert(id.to_string(), i);
        Ok(i)
    }

    /// `X`, `X as Label`, and Mermaid's `X@{ "type": "actor" }`.
    fn declare(&mut self, rest: &str, actor: bool) -> Result<(), IoError> {
        let rest = match rest.find("@{") {
            Some(i) => rest.get(..i).unwrap_or(rest).trim(),
            None => rest,
        };
        let (id, label) = match find_as(rest) {
            Some((a, b)) => (rest.get(..a).unwrap_or("").trim(), Some(clean_label(rest.get(b..).unwrap_or("")))),
            None => (rest.trim(), None),
        };
        if id.is_empty() {
            self.warn("a participant declaration without a name was ignored".to_string());
            return Ok(());
        }
        let i = self.ensure(id)?;
        let mut extra = 0;
        if let Some(p) = self.participants.get_mut(i) {
            if let Some(l) = label
                && !l.is_empty()
            {
                extra = l.len();
                p.label = l;
            }
            p.is_actor |= actor;
        }
        self.charge(extra)
    }

    /// `over A,B: text`, `left of A: text`, `right of A: text`.
    fn note(&mut self, rest: &str) -> Result<(), IoError> {
        let (placement, after) = if let Some(r) = strip_word(rest, "over") {
            (NotePlacement::Over, r)
        } else if let Some(r) = strip_word(rest, "left").and_then(|r| strip_word(r, "of")) {
            (NotePlacement::LeftOf, r)
        } else if let Some(r) = strip_word(rest, "right").and_then(|r| strip_word(r, "of")) {
            (NotePlacement::RightOf, r)
        } else {
            self.warn(format!("a note without `over`, `left of` or `right of` was ignored: {}", shorten(rest, 40)));
            return Ok(());
        };
        let (names, text) = match after.split_once(':') {
            Some((n, t)) => (n, clean_label(t)),
            None => (after, String::new()),
        };
        let mut ids = Vec::new();
        for name in names.split(',').map(str::trim).filter(|n| !n.is_empty()) {
            self.ensure(name)?;
            ids.push(name.to_string());
        }
        if ids.is_empty() {
            self.warn("a note without a participant was ignored".to_string());
            return Ok(());
        }
        self.check_items()?;
        self.charge(text.len() + ids.iter().map(String::len).sum::<usize>())?;
        self.events.push(Event::Note { placement, participants: ids, text });
        Ok(())
    }

    fn message(&mut self, line: &str) -> Result<(), IoError> {
        // The arrow is searched left of the first colon only: the text may contain arrows.
        let (head_part, text) = match line.split_once(':') {
            Some((h, t)) => (h, clean_label(t)),
            None => (line, String::new()),
        };
        let Some((at, token)) = find_arrow(head_part) else {
            self.warn(format!("skipped a line that is no message, note or declaration: {}", shorten(line, 60)));
            return Ok(());
        };
        let (token_text, dashed, head, bidirectional) = *token;
        let from = head_part.get(..at).unwrap_or("").trim();
        let mut to = head_part.get(at + token_text.len()..).unwrap_or("");
        let activation = match to.chars().next() {
            Some('+') => Activation::Activate,
            Some('-') => Activation::Deactivate,
            _ => Activation::None,
        };
        if activation != Activation::None {
            to = to.get(1..).unwrap_or("");
        }
        let to = to.trim();
        if from.is_empty() || to.is_empty() {
            self.warn(format!("skipped a message without sender or receiver: {}", shorten(line, 60)));
            return Ok(());
        }
        self.ensure(from)?;
        self.ensure(to)?;
        self.check_items()?;
        self.charge(from.len() + to.len() + text.len())?;
        self.events.push(Event::Message {
            from: from.to_string(),
            to: to.to_string(),
            text,
            dashed,
            head,
            bidirectional,
            activation,
        });
        Ok(())
    }
}

/// The leftmost arrow token in `s` (longest at that position): byte offset and the token.
fn find_arrow(s: &str) -> Option<(usize, &'static Arrow)> {
    for (i, _) in s.char_indices() {
        let tail = s.get(i..)?;
        for a in ARROWS {
            if tail.starts_with(a.0) {
                return Some((i, a));
            }
        }
    }
    None
}

/// The byte range of a whitespace-delimited `as`: (end of the id, start of the label).
fn find_as(s: &str) -> Option<(usize, usize)> {
    for (i, _) in s.match_indices("as") {
        let before = s.get(..i)?;
        let after = s.get(i + 2..)?;
        if before.ends_with(char::is_whitespace) && after.starts_with(char::is_whitespace) {
            return Some((i, i + 2));
        }
    }
    None
}

/// `rest` without a leading word `w` (case-insensitive) and the whitespace after it.
fn strip_word<'a>(rest: &'a str, w: &str) -> Option<&'a str> {
    let rest = rest.trim_start();
    let head = rest.get(..w.len())?;
    if !head.eq_ignore_ascii_case(w) {
        return None;
    }
    let tail = rest.get(w.len()..)?;
    if tail.is_empty() || tail.starts_with(char::is_whitespace) { Some(tail.trim_start()) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mermaid::parse_sequence;

    #[test]
    fn arrows_are_found_leftmost_longest() {
        for (src, dashed, head, bi) in [
            ("A->>B", false, MessageHead::Arrow, false),
            ("A-->>B", true, MessageHead::Arrow, false),
            ("A->B", false, MessageHead::None, false),
            ("A-->B", true, MessageHead::None, false),
            ("A-xB", false, MessageHead::Cross, false),
            ("A--xB", true, MessageHead::Cross, false),
            ("A-)B", false, MessageHead::Open, false),
            ("A--)B", true, MessageHead::Open, false),
            ("A<<->>B", false, MessageHead::Arrow, true),
            ("A<<-->>B", true, MessageHead::Arrow, true),
        ] {
            let found = find_arrow(src).map(|(_, t)| (t.1, t.2, t.3));
            assert_eq!(found, Some((dashed, head, bi)), "{src}");
        }
        assert!(find_arrow("A plain line").is_none());
    }

    #[test]
    fn activation_markers_are_stripped() {
        let d = parse_sequence("sequenceDiagram\nA->>+B: hi\nB-->>-A: ok\n").unwrap();
        let ids: Vec<&str> = d.participants.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["A", "B"]);
        assert!(matches!(&d.events[0], Event::Message { activation: Activation::Activate, to, .. } if to == "B"));
        assert!(matches!(&d.events[1], Event::Message { activation: Activation::Deactivate, to, .. } if to == "A"));
    }

    #[test]
    fn find_as_needs_whole_word() {
        assert_eq!(find_as("alias as Label"), Some((6, 8)));
        assert_eq!(find_as("class"), None);
        assert_eq!(find_as("A as B"), Some((2, 4)));
    }
}
