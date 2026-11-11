//! A small backtracking regular expression matcher for `element.query`.
//!
//! Supported: literals, `.`, classes (`[a-z]`, `[^0-9]`, with `\d \w \s` inside), `\d \D \w \W \s \S \b`,
//! escapes of punctuation, `^` and `$`, groups `( )` (not captured), alternation `|`, and the quantifiers `*`, `+`,
//! `?` and `{n}`, `{n,}`, `{n,m}` (a trailing `?` for lazy is accepted and ignored). A pattern matches anywhere in
//! the text. Matching has a step budget, so a hostile pattern ends quickly with "no match" instead of hanging.

use std::cell::Cell;

/// Longest pattern accepted, in characters.
const MAX_PATTERN: usize = 256;
/// Deepest nesting of groups.
const MAX_DEPTH: usize = 16;
/// Matching steps allowed per `is_match` call.
const STEP_BUDGET: usize = 2_000_000;
/// Largest repeat count in `{n,m}`.
const MAX_REPEAT: u32 = 1000;

/// Why a pattern was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegexError {
    /// Too long.
    #[error("the pattern is longer than {MAX_PATTERN} characters")]
    TooLong,
    /// Groups nest too deep.
    #[error("groups are nested more than {MAX_DEPTH} deep")]
    TooDeep,
    /// A syntax error.
    #[error("{0}")]
    Syntax(String),
}

#[derive(Debug, Clone)]
enum Atom {
    Char(char),
    Any,
    Class { negated: bool, items: Vec<ClassItem> },
    Start,
    End,
    WordBoundary,
    Group(Vec<Vec<Item>>),
}

#[derive(Debug, Clone)]
enum ClassItem {
    Range(char, char),
    Digit(bool),
    Word(bool),
    Space(bool),
}

#[derive(Debug, Clone)]
struct Item {
    atom: Atom,
    min: u32,
    max: u32,
}

/// Whether letters must match in case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Case {
    /// `a` and `A` differ.
    Sensitive,
    /// `a` and `A` match.
    Insensitive,
}

/// A compiled pattern.
#[derive(Debug, Clone)]
pub struct Regex {
    alts: Vec<Vec<Item>>,
    ignore_case: bool,
}

struct Parser<'a> {
    chars: &'a [char],
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn alternation(&mut self, depth: usize) -> Result<Vec<Vec<Item>>, RegexError> {
        if depth > MAX_DEPTH {
            return Err(RegexError::TooDeep);
        }
        let mut alts = vec![Vec::new()];
        while let Some(c) = self.peek() {
            match c {
                ')' if depth > 0 => break,
                ')' => return Err(RegexError::Syntax("unmatched `)`".into())),
                '|' => {
                    self.pos += 1;
                    alts.push(Vec::new());
                }
                _ => {
                    let atom = self.atom(depth)?;
                    let (min, max) = self.quantifier()?;
                    if let Some(last) = alts.last_mut() {
                        last.push(Item { atom, min, max });
                    }
                }
            }
        }
        Ok(alts)
    }

    fn atom(&mut self, depth: usize) -> Result<Atom, RegexError> {
        match self.next() {
            Some('(') => {
                if self.peek() == Some('?') {
                    // `(?:` is accepted, other `(?` forms are not.
                    if self.chars.get(self.pos + 1) == Some(&':') {
                        self.pos += 2;
                    } else {
                        return Err(RegexError::Syntax("only `(?:...)` groups are supported".into()));
                    }
                }
                let inner = self.alternation(depth + 1)?;
                if self.next() != Some(')') {
                    return Err(RegexError::Syntax("missing `)`".into()));
                }
                Ok(Atom::Group(inner))
            }
            Some('[') => self.class(),
            Some('.') => Ok(Atom::Any),
            Some('^') => Ok(Atom::Start),
            Some('$') => Ok(Atom::End),
            Some('\\') => match self.next() {
                Some('d') => Ok(class_of(ClassItem::Digit(true))),
                Some('D') => Ok(class_of(ClassItem::Digit(false))),
                Some('w') => Ok(class_of(ClassItem::Word(true))),
                Some('W') => Ok(class_of(ClassItem::Word(false))),
                Some('s') => Ok(class_of(ClassItem::Space(true))),
                Some('S') => Ok(class_of(ClassItem::Space(false))),
                Some('b') => Ok(Atom::WordBoundary),
                Some('n') => Ok(Atom::Char('\n')),
                Some('t') => Ok(Atom::Char('\t')),
                Some(c) if c.is_alphanumeric() => Err(RegexError::Syntax(format!("unknown escape `\\{c}`"))),
                Some(c) => Ok(Atom::Char(c)),
                None => Err(RegexError::Syntax("the pattern ends with `\\`".into())),
            },
            Some(c @ ('*' | '+' | '?')) => Err(RegexError::Syntax(format!("`{c}` has nothing to repeat"))),
            Some(c) => Ok(Atom::Char(c)),
            None => Err(RegexError::Syntax("unexpected end".into())),
        }
    }

    fn class(&mut self) -> Result<Atom, RegexError> {
        let negated = self.peek() == Some('^');
        if negated {
            self.pos += 1;
        }
        let mut items = Vec::new();
        let mut first = true;
        loop {
            let Some(c) = self.next() else { return Err(RegexError::Syntax("missing `]`".into())) };
            if c == ']' && !first {
                break;
            }
            first = false;
            let lo = if c == '\\' {
                match self.next() {
                    Some('d') => {
                        items.push(ClassItem::Digit(true));
                        continue;
                    }
                    Some('D') => {
                        items.push(ClassItem::Digit(false));
                        continue;
                    }
                    Some('w') => {
                        items.push(ClassItem::Word(true));
                        continue;
                    }
                    Some('W') => {
                        items.push(ClassItem::Word(false));
                        continue;
                    }
                    Some('s') => {
                        items.push(ClassItem::Space(true));
                        continue;
                    }
                    Some('S') => {
                        items.push(ClassItem::Space(false));
                        continue;
                    }
                    Some('n') => '\n',
                    Some('t') => '\t',
                    Some(x) => x,
                    None => return Err(RegexError::Syntax("missing `]`".into())),
                }
            } else {
                c
            };
            if self.peek() == Some('-') && self.chars.get(self.pos + 1).is_some_and(|n| *n != ']') {
                self.pos += 1;
                let hi = match self.next() {
                    Some('\\') => self.next().unwrap_or('\\'),
                    Some(h) => h,
                    None => return Err(RegexError::Syntax("missing `]`".into())),
                };
                if hi < lo {
                    return Err(RegexError::Syntax(format!("the range `{lo}-{hi}` is backwards")));
                }
                items.push(ClassItem::Range(lo, hi));
            } else {
                items.push(ClassItem::Range(lo, lo));
            }
        }
        Ok(Atom::Class { negated, items })
    }

    fn quantifier(&mut self) -> Result<(u32, u32), RegexError> {
        let q = match self.peek() {
            Some('*') => (0, u32::MAX),
            Some('+') => (1, u32::MAX),
            Some('?') => (0, 1),
            Some('{') => {
                let close = self.chars[self.pos..].iter().position(|c| *c == '}');
                let Some(close) = close else { return Ok((1, 1)) };
                let body: String = self.chars[self.pos + 1..self.pos + close].iter().collect();
                let parsed = match body.split_once(',') {
                    None => body.parse::<u32>().ok().map(|n| (n, n)),
                    Some((a, "")) => a.parse::<u32>().ok().map(|n| (n, u32::MAX)),
                    Some((a, b)) => a.parse::<u32>().ok().zip(b.parse::<u32>().ok()),
                };
                let Some((lo, hi)) = parsed else { return Ok((1, 1)) };
                if lo > hi || lo > MAX_REPEAT || (hi != u32::MAX && hi > MAX_REPEAT) {
                    return Err(RegexError::Syntax(format!(
                        "the repeat `{{{body}}}` is not valid (counts up to {MAX_REPEAT})"
                    )));
                }
                self.pos += close;
                (lo, hi)
            }
            _ => return Ok((1, 1)),
        };
        self.pos += 1;
        if self.peek() == Some('?') {
            self.pos += 1; // lazy: treated as greedy
        }
        Ok(q)
    }
}

fn class_of(i: ClassItem) -> Atom {
    Atom::Class { negated: false, items: vec![i] }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

struct Run<'a> {
    text: &'a [char],
    ignore_case: bool,
    steps: Cell<usize>,
}

impl Run<'_> {
    fn tick(&self) -> bool {
        let s = self.steps.get() + 1;
        self.steps.set(s);
        s <= STEP_BUDGET
    }

    fn class_has(&self, negated: bool, items: &[ClassItem], c: char) -> bool {
        let test = |c: char| {
            items.iter().any(|i| match i {
                ClassItem::Range(a, b) => *a <= c && c <= *b,
                ClassItem::Digit(yes) => c.is_ascii_digit() == *yes,
                ClassItem::Word(yes) => is_word(c) == *yes,
                ClassItem::Space(yes) => c.is_whitespace() == *yes,
            })
        };
        let hit = test(c) || (self.ignore_case && (test(fold(c)) || c.to_uppercase().next().is_some_and(test)));
        hit != negated
    }

    /// Does a single-character atom match the character at `pos`?
    fn one(&self, atom: &Atom, pos: usize) -> bool {
        let Some(&c) = self.text.get(pos) else { return false };
        match atom {
            Atom::Char(x) => *x == c || (self.ignore_case && fold(*x) == fold(c)),
            Atom::Any => c != '\n',
            Atom::Class { negated, items } => self.class_has(*negated, items, c),
            _ => false,
        }
    }

    fn zero_width(&self, atom: &Atom, pos: usize) -> Option<bool> {
        match atom {
            Atom::Start => Some(pos == 0),
            Atom::End => Some(pos == self.text.len()),
            Atom::WordBoundary => {
                let before = pos > 0 && self.text.get(pos - 1).is_some_and(|c| is_word(*c));
                let after = self.text.get(pos).is_some_and(|c| is_word(*c));
                Some(before != after)
            }
            _ => None,
        }
    }

    fn seq(&self, items: &[Item], pos: usize, k: &mut dyn FnMut(usize) -> bool) -> bool {
        if !self.tick() {
            return false;
        }
        let Some((item, rest)) = items.split_first() else { return k(pos) };
        if let Some(ok) = self.zero_width(&item.atom, pos) {
            return ok && self.seq(rest, pos, k);
        }
        match &item.atom {
            Atom::Group(alts) => self.group(alts, item, rest, pos, 0, k),
            _ => {
                // Greedy run of single characters, then back off.
                let mut n = 0u32;
                while n < item.max && self.one(&item.atom, pos + n as usize) {
                    n += 1;
                    if !self.tick() {
                        return false;
                    }
                }
                loop {
                    if n < item.min {
                        return false;
                    }
                    if self.seq(rest, pos + n as usize, k) {
                        return true;
                    }
                    if n == 0 {
                        return false;
                    }
                    n -= 1;
                }
            }
        }
    }

    fn group(
        &self,
        alts: &[Vec<Item>],
        item: &Item,
        rest: &[Item],
        pos: usize,
        count: u32,
        k: &mut dyn FnMut(usize) -> bool,
    ) -> bool {
        if !self.tick() {
            return false;
        }
        if count < item.max {
            for alt in alts {
                let mut again = |p: usize| {
                    // A repeat that consumed nothing cannot make progress.
                    if p == pos && count >= item.min {
                        return false;
                    }
                    self.group(alts, item, rest, p, count + 1, k)
                };
                if self.seq(alt, pos, &mut again) {
                    return true;
                }
            }
        }
        count >= item.min && self.seq(rest, pos, k)
    }
}

impl Regex {
    /// Compiles `pattern`. With [`Case::Insensitive`], letters match in either case.
    pub fn new(pattern: &str, case: Case) -> Result<Regex, RegexError> {
        let ignore_case = case == Case::Insensitive;
        let chars: Vec<char> = pattern.chars().collect();
        if chars.len() > MAX_PATTERN {
            return Err(RegexError::TooLong);
        }
        let mut p = Parser { chars: &chars, pos: 0 };
        let alts = p.alternation(0)?;
        Ok(Regex { alts, ignore_case })
    }

    /// True when the pattern matches anywhere in `text`. A pattern that needs too many steps does not match.
    pub fn is_match(&self, text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        let run = Run { text: &chars, ignore_case: self.ignore_case, steps: Cell::new(0) };
        let top = [Item { atom: Atom::Group(self.alts.clone()), min: 1, max: 1 }];
        (0..=chars.len()).any(|start| run.seq(&top, start, &mut |_| true))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn m(p: &str, t: &str) -> bool {
        Regex::new(p, Case::Insensitive).unwrap().is_match(t)
    }

    #[test]
    fn literals_anchors_and_dot() {
        assert!(m("abc", "xxABCxx"));
        assert!(!m("^abc", "xabc"));
        assert!(m("^abc$", "abc"));
        assert!(!m("^abc$", "abcd"));
        assert!(m("a.c", "abc") && !m("a.c", "ac"));
    }

    #[test]
    fn quantifiers() {
        assert!(m("ab*c", "ac") && m("ab*c", "abbbc"));
        assert!(m("ab+c", "abc") && !m("ab+c", "ac"));
        assert!(m("ab?c", "ac") && m("ab?c", "abc") && !m("^ab?c$", "abbc"));
        assert!(m("^a{2,3}$", "aa") && m("^a{2,3}$", "aaa") && !m("^a{2,3}$", "a") && !m("^a{2,3}$", "aaaa"));
        assert!(m("^a{2}$", "aa") && m("^a{2,}$", "aaaaa"));
        assert!(m("a.*?c", "abbbc"));
    }

    #[test]
    fn classes_and_escapes() {
        assert!(m("[a-c]+", "xxb") && !m("^[a-c]+$", "abd"));
        assert!(m("^[^0-9]+$", "abc") && !m("^[^0-9]+$", "ab1"));
        assert!(m(r"\d+", "v12") && !m(r"\d", "abc"));
        assert!(m(r"^\w+$", "a_1") && !m(r"^\w+$", "a b"));
        assert!(m(r"a\.b", "a.b") && !m(r"a\.b", "axb"));
        assert!(m(r"[\d.]+", "3.14"));
        assert!(m(r"\bcat\b", "a cat sat") && !m(r"\bcat\b", "concat"));
        assert!(m(r"[]a]", "]"));
    }

    #[test]
    fn groups_and_alternation() {
        assert!(m("^(foo|bar)$", "bar") && !m("^(foo|bar)$", "baz"));
        assert!(m("^(ab)+$", "ababab") && !m("^(ab)+$", "aba"));
        assert!(m("cat|dog", "hotdog"));
        assert!(m("^(?:a|b)c$", "bc"));
        assert!(m("^(a*)*$", "aaa"), "an empty repeat must not loop");
    }

    #[test]
    fn case_insensitive() {
        assert!(m("START", "start here"));
        assert!(m("[a-z]+", "ABC"));
        assert!(!Regex::new("START", Case::Sensitive).unwrap().is_match("start"));
    }

    #[test]
    fn bad_patterns_are_errors() {
        for p in ["(", ")", "[a", "*a", "a{3,2}", "a{5000}", r"\q", "[z-a]", "(?=a)", "a\\"] {
            assert!(Regex::new(p, Case::Insensitive).is_err(), "{p}");
        }
        assert_eq!(Regex::new(&"a".repeat(300), Case::Insensitive).unwrap_err(), RegexError::TooLong);
        let deep = format!("{}a{}", "(".repeat(40), ")".repeat(40));
        assert_eq!(Regex::new(&deep, Case::Insensitive).unwrap_err(), RegexError::TooDeep);
    }

    #[test]
    fn hostile_patterns_end_quickly() {
        // Classic catastrophic backtracking: the step budget turns it into "no match".
        let re = Regex::new("^(a+)+$", Case::Insensitive).unwrap();
        let text = format!("{}b", "a".repeat(40));
        assert!(!re.is_match(&text));
    }

    #[test]
    fn unicode_text() {
        assert!(m("é+", "café") && m("^.{4}$", "café"));
    }
}
