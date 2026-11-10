//! Text helpers shared by the flowchart and sequence parsers. Everything here slices by char boundary only and
//! never indexes: input is hostile.

use super::MAX_LABEL_CHARS;

/// A position in a `&str`. `pos` is always a char boundary.
#[derive(Debug, Clone)]
pub(super) struct Cur<'a> {
    s: &'a str,
    pos: usize,
}

impl<'a> Cur<'a> {
    pub(super) fn new(s: &'a str) -> Self {
        Self { s, pos: 0 }
    }

    pub(super) fn set_pos(&mut self, pos: usize) {
        self.pos = pos.min(self.s.len());
        while self.pos < self.s.len() && !self.s.is_char_boundary(self.pos) {
            self.pos += 1;
        }
    }

    /// The text from the position to the end.
    pub(super) fn rest(&self) -> &'a str {
        self.s.get(self.pos..).unwrap_or("")
    }

    pub(super) fn eof(&self) -> bool {
        self.pos >= self.s.len()
    }

    pub(super) fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    pub(super) fn peek2(&self) -> Option<char> {
        self.rest().chars().nth(1)
    }

    pub(super) fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    pub(super) fn starts_with(&self, p: &str) -> bool {
        self.rest().starts_with(p)
    }

    pub(super) fn skip_ws(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }

    /// Consumes a run of `c` and returns its length.
    pub(super) fn eat_run(&mut self, c: char) -> usize {
        let mut n = 0;
        while self.peek() == Some(c) {
            self.bump();
            n += 1;
        }
        n
    }

    /// Consumes `n` bytes. The caller got `n` from a `str` search, so it ends on a boundary.
    pub(super) fn advance(&mut self, n: usize) {
        self.set_pos(self.pos.saturating_add(n));
    }
}

/// A character of a node id: a Unicode letter or digit, or `_`.
pub(super) fn is_id_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Splits off the first whitespace-separated word. Returns it and the trimmed remainder.
pub(super) fn split_word(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    match s.find(char::is_whitespace) {
        Some(i) => (s.get(..i).unwrap_or(""), s.get(i..).unwrap_or("").trim_start()),
        None => (s, ""),
    }
}

/// Cuts `s` to at most `n` chars, appending an ellipsis when something was cut. For messages.
pub(super) fn shorten(s: &str, n: usize) -> String {
    let mut it = s.chars();
    let head: String = it.by_ref().take(n).collect();
    if it.next().is_some() { format!("{head}…") } else { head }
}

/// Byte offset of the first `closer` in `s`. When `s` starts (after spaces) with a `"`, the quoted part is skipped
/// first, so a closer inside the quotes does not count. An unbalanced quote is treated as plain text.
pub(super) fn find_closer(s: &str, closer: &str) -> Option<usize> {
    let lead = s.len() - s.trim_start().len();
    let body = s.get(lead..)?;
    if body.starts_with('"')
        && let Some(q) = body.get(1..).and_then(|b| b.find('"'))
    {
        let after = lead + 1 + q + 1;
        return s.get(after..)?.find(closer).map(|i| after + i);
    }
    s.find(closer)
}

/// Splits one flowchart line into statements at every `;` that is outside quotes and brackets, and drops a `%%`
/// comment (also outside quotes and brackets). The pieces are not trimmed.
pub(super) fn split_statements(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_quote = false;
    let mut in_pipe = false;
    let mut depth = 0usize;
    let mut prev = '\0';
    for (i, c) in line.char_indices() {
        if in_pipe && c != '|' && c != '"' {
            prev = c;
            continue;
        }
        if in_quote {
            if c == '"' {
                in_quote = false;
            }
            prev = c;
            continue;
        }
        match c {
            '"' => in_quote = true,
            '|' if depth == 0 => in_pipe = !in_pipe,
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' => depth = depth.saturating_sub(1),
            // `A>text]`: the `>` of an asymmetric node opens a bracket that `]` closes.
            '>' if depth == 0 && is_id_char(prev) => depth = 1,
            ';' if depth == 0 => {
                out.push(line.get(start..i).unwrap_or(""));
                start = i + 1;
            }
            '%' if depth == 0 && line.get(i..).is_some_and(|r| r.starts_with("%%")) => {
                out.push(line.get(start..i).unwrap_or(""));
                return out;
            }
            _ => {}
        }
        prev = c;
    }
    out.push(line.get(start..).unwrap_or(""));
    out
}

/// Removes a `%%` comment from a line (sequence diagrams). Text inside double quotes is kept.
pub(super) fn strip_comment(line: &str) -> &str {
    let mut in_quote = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_quote = !in_quote,
            '%' if !in_quote && line.get(i..).is_some_and(|r| r.starts_with("%%")) => {
                return line.get(..i).unwrap_or(line);
            }
            _ => {}
        }
    }
    line
}

/// Turns label text into the plain text to show: trims, drops one pair of surrounding double quotes and one pair of
/// markdown backticks, turns `<br>` variants into newlines, decodes common entities, drops control characters,
/// and cuts the result to [`MAX_LABEL_CHARS`] chars.
pub(super) fn clean_label(raw: &str) -> String {
    let mut t = raw.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        t = t.get(1..t.len() - 1).unwrap_or(t).trim();
    }
    if t.len() >= 2 && t.starts_with('`') && t.ends_with('`') {
        t = t.get(1..t.len() - 1).unwrap_or(t).trim();
    }
    let mut out = String::with_capacity(t.len().min(MAX_LABEL_CHARS));
    let mut chars = 0usize;
    let mut i = 0usize;
    while i < t.len() && chars < MAX_LABEL_CHARS {
        let rest = t.get(i..).unwrap_or("");
        let Some(c) = rest.chars().next() else { break };
        if c == '<' {
            if let Some(n) = match_br(rest) {
                out.push('\n');
                chars += 1;
                i += n;
                continue;
            }
        } else if (c == '#' || c == '&')
            && let Some((decoded, n)) = match_entity(rest)
        {
            i += n;
            if let Some(d) = decoded
                && (!d.is_control() || d == '\n')
            {
                out.push(d);
                chars += 1;
            }
            continue;
        }
        i += c.len_utf8();
        if c == '\t' {
            out.push(' ');
            chars += 1;
        } else if c == '\n' || !c.is_control() {
            out.push(c);
            chars += 1;
        }
    }
    out.trim().to_string()
}

/// Matches `<br>`, `<br/>`, `<br />` and spaced variants, in any case. Returns the byte length.
fn match_br(rest: &str) -> Option<usize> {
    let b = rest.as_bytes();
    if !b.first().is_some_and(|c| *c == b'<') {
        return None;
    }
    if !b.get(1).is_some_and(|c| c.eq_ignore_ascii_case(&b'b'))
        || !b.get(2).is_some_and(|c| c.eq_ignore_ascii_case(&b'r'))
    {
        return None;
    }
    let mut p = 3;
    while b.get(p).is_some_and(|c| c.is_ascii_whitespace()) {
        p += 1;
    }
    if b.get(p) == Some(&b'/') {
        p += 1;
        while b.get(p).is_some_and(|c| c.is_ascii_whitespace()) {
            p += 1;
        }
    }
    (b.get(p) == Some(&b'>')).then_some(p + 1)
}

/// Matches `#name;`, `#123;`, `&name;`, `&#123;`, `&#x1F;`. The first value is `None` for a character that is
/// dropped (NUL, surrogates, out of range). Returns the byte length of the entity.
fn match_entity(rest: &str) -> Option<(Option<char>, usize)> {
    let b = rest.as_bytes();
    let marker = *b.first()?;
    let mut p = 1;
    let numeric_marker = marker == b'&' && b.get(1) == Some(&b'#');
    if numeric_marker {
        p = 2;
    }
    let start = p;
    while b.get(p).is_some_and(u8::is_ascii_alphanumeric) && p - start < 10 {
        p += 1;
    }
    if p == start || b.get(p) != Some(&b';') {
        return None;
    }
    let word = rest.get(start..p)?;
    let len = p + 1;
    let all_digits = word.bytes().all(|c| c.is_ascii_digit());
    if all_digits && (marker == b'#' || numeric_marker) {
        let n: u32 = word.parse().ok()?;
        return Some((char::from_u32(n).filter(|c| *c != '\0'), len));
    }
    if numeric_marker {
        let hex = word.strip_prefix(['x', 'X'])?;
        let n = u32::from_str_radix(hex, 16).ok()?;
        return Some((char::from_u32(n).filter(|c| *c != '\0'), len));
    }
    let named = match word {
        "quot" => '"',
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "apos" => '\'',
        "nbsp" => ' ',
        _ => return None,
    };
    Some((Some(named), len))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_cleaned() {
        assert_eq!(clean_label("  \"a<br>b<BR/>c<br />d\"  "), "a\nb\nc\nd");
        assert_eq!(clean_label("say #quot;hi#quot; &amp; #35; &#x41; &lt;"), "say \"hi\" & # A <");
        assert_eq!(clean_label("\"`markdown`\""), "markdown");
        assert_eq!(clean_label("a#b; c #1 d &unknown; e"), "a#b; c #1 d &unknown; e");
        assert_eq!(clean_label("x\u{0}y\u{7}z#0;w"), "xyzw");
        assert_eq!(clean_label("日本語 ✓ 😀"), "日本語 ✓ 😀");
        assert_eq!(clean_label("<br"), "<br");
    }

    #[test]
    fn labels_are_capped() {
        let long = "é".repeat(MAX_LABEL_CHARS * 3);
        assert_eq!(clean_label(&long).chars().count(), MAX_LABEL_CHARS);
    }

    #[test]
    fn statements_split_outside_labels_only() {
        assert_eq!(split_statements("A[a;b] --> B; C"), vec!["A[a;b] --> B", " C"]);
        assert_eq!(split_statements("A[\"x;y\"]; B %% c; d"), vec!["A[\"x;y\"]", " B "]);
        assert_eq!(split_statements("A>x;y] --> B;C"), vec!["A>x;y] --> B", "C"]);
        assert_eq!(split_statements("A-->B"), vec!["A-->B"]);
        assert_eq!(split_statements("%% only"), vec![""]);
        assert_eq!(split_statements("ü;é"), vec!["ü", "é"]);
    }

    #[test]
    fn closer_skips_quotes() {
        assert_eq!(find_closer("\"a]b\"]", "]"), Some(5));
        assert_eq!(find_closer("a]b", "]"), Some(1));
        assert_eq!(find_closer("\"unbalanced]", "]"), Some(11));
        assert_eq!(find_closer("", "]"), None);
    }

    #[test]
    fn cursor_never_splits_chars() {
        let mut c = Cur::new("é😀x");
        c.set_pos(1);
        assert_eq!(c.peek(), Some('😀'));
        c.set_pos(100);
        assert!(c.eof());
        assert_eq!(c.rest(), "");
    }
}
