//! `cargo xtask exceptions [--check]`: `docs/exceptions.md` against the source.
//!
//! The document is the closed list of inputs that may change state without one `execute` per user action
//! (sections 1 to 4), and of the writes that are not commands (section 5). This gate fails when
//!
//! - an `### E<n>` row names no `exception_E<n>_*` test, or names a test that no source file defines;
//! - a test `exception_E<n>_*` or `exception_N<k>_*` exists for a row that the document does not have;
//! - a row of section 5 names a function or a test that does not exist;
//! - a source file carries a `Non-command write N<k>` doc line with no row in section 5, or a row has no such line;
//! - the registry side fails: a command with `undo = Gesture` that no class A row names, or a final command that
//!   is not registered (`exception_tests` in `crates/engine`, which this gate runs).
//!
//! A row for a feature that is not built says so in its `Not built` line. Its tests cover only what exists.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::{cargo, run};

const DOC: &str = "docs/exceptions.md";
/// Where tests and markers can live.
const SOURCE_DIRS: &[&str] = &["crates", "apps", "e2e", "tests"];
/// The phrase a source doc line carries to mark a non-command write.
const MARKER: &str = "Non-command write N";

/// One row of the document: `E<n>` or `N<k>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// `E2`, `N1`.
    pub id: String,
    /// The test names the row cites (`exception_<id>_...`).
    pub tests: Vec<String>,
    /// For `N` rows: the function names the row cites.
    pub functions: Vec<String>,
}

/// What the source tree contains.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Source {
    /// Names of every `fn` whose name starts with `exception_`.
    pub tests: BTreeSet<String>,
    /// Names of every `fn`.
    pub functions: BTreeSet<String>,
    /// The `N<k>` ids of every `Non-command write N<k>` doc line.
    pub markers: BTreeSet<String>,
}

/// The text of each backtick span on a line.
fn spans(line: &str) -> impl Iterator<Item = &str> {
    line.split('`').skip(1).step_by(2)
}

fn is_row_id(word: &str, letter: char) -> bool {
    word.strip_prefix(letter).is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The rows of the document: `### E<n>` sections and the `| N<k> |` table rows of section 5.
pub fn parse(doc: &str) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    // The index of the `E` row whose section we are in; a heading of any other kind closes it.
    let mut open: Option<usize> = None;
    for line in doc.lines() {
        if line.starts_with('#') {
            let word = line.strip_prefix("### ").and_then(|r| r.split_whitespace().next()).unwrap_or_default();
            open = is_row_id(word, 'E').then(|| {
                rows.push(Row { id: word.to_owned(), tests: Vec::new(), functions: Vec::new() });
                rows.len() - 1
            });
            continue;
        }
        if let Some(row) = open.and_then(|i| rows.get_mut(i))
            && line.starts_with("| Test |")
        {
            row.tests.extend(spans(line).filter(|s| s.starts_with("exception_")).map(str::to_owned));
        }
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        if let [_, id, function, _, test, ..] = cells.as_slice()
            && is_row_id(id, 'N')
        {
            rows.push(Row {
                id: (*id).to_owned(),
                tests: spans(test).map(str::to_owned).collect(),
                functions: spans(function)
                    .filter_map(|s| s.rsplit("::").next())
                    .map(str::to_owned)
                    .collect(),
            });
        }
    }
    rows
}

/// The identifier after `fn ` at each place in `line` where `fn` starts a word.
fn fn_names(line: &str) -> impl Iterator<Item = &str> {
    line.match_indices("fn ").filter_map(|(at, _)| {
        let before = line[..at].chars().next_back();
        if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            return None;
        }
        let rest = &line[at + 3..];
        let end = rest.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(rest.len());
        rest.get(..end).filter(|n| !n.is_empty())
    })
}

/// Adds what one source file contains.
pub fn scan_text(text: &str, into: &mut Source) {
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") {
            if let Some(at) = trimmed.find(MARKER) {
                let digits: String = trimmed[at + MARKER.len()..].chars().take_while(char::is_ascii_digit).collect();
                if !digits.is_empty() {
                    into.markers.insert(format!("N{digits}"));
                }
            }
            continue;
        }
        for name in fn_names(trimmed) {
            into.functions.insert(name.to_owned());
            if name.starts_with("exception_") {
                into.tests.insert(name.to_owned());
            }
        }
    }
}

fn scan_dir(dir: &Path, into: &mut Source) -> Result<(), String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Ok(()),
    };
    for entry in entries {
        let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if path.is_dir() {
            if !matches!(name, "target" | "node_modules" | ".git") {
                scan_dir(&path, into)?;
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            scan_text(&text, into);
        }
    }
    Ok(())
}

/// The problems of the document given the source. Empty means the gate passes this half.
pub fn check(rows: &[Row], source: &Source) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen = BTreeSet::new();
    for row in rows {
        if !seen.insert(row.id.as_str()) {
            problems.push(format!("{DOC}: row {} appears twice", row.id));
        }
        let prefix = format!("exception_{}_", row.id);
        if row.tests.is_empty() {
            problems.push(format!("{DOC}: row {} names no test; add a test called `{prefix}...`", row.id));
        }
        for test in &row.tests {
            if !test.starts_with(&prefix) {
                problems.push(format!("{DOC}: row {} names `{test}`, which does not start with `{prefix}`", row.id));
            } else if !source.tests.contains(test) {
                problems.push(format!("{DOC}: row {} names the test `{test}`, which no source file defines", row.id));
            }
        }
        if row.id.starts_with('N') {
            if row.functions.is_empty() {
                problems.push(format!("{DOC}: row {} names no function", row.id));
            }
            for function in &row.functions {
                if !source.functions.contains(function) {
                    problems.push(format!("{DOC}: row {} names the function `{function}`, which no source file defines", row.id));
                }
            }
            if !source.markers.contains(&row.id) {
                problems.push(format!(
                    "{DOC}: row {} has no `{MARKER}{}` doc line in the source; add it to the function's rustdoc",
                    row.id,
                    &row.id[1..]
                ));
            }
        }
    }
    // A test may be one of several for its row; it needs a row, not a mention.
    let ids: BTreeSet<&str> = rows.iter().map(|r| r.id.as_str()).collect();
    for test in &source.tests {
        let row_of = test.strip_prefix("exception_").and_then(|rest| rest.split('_').next()).unwrap_or_default();
        if !ids.contains(row_of) {
            problems.push(format!("the test `{test}` belongs to row `{row_of}`, which {DOC} does not have"));
        }
    }
    for marker in &source.markers {
        if !ids.contains(marker.as_str()) {
            problems.push(format!("a source file marks a `{MARKER}{}` with no row {marker} in section 5 of {DOC}", &marker[1..]));
        }
    }
    problems
}

/// `cargo xtask exceptions [--check]`.
pub fn run_gate(root: &Path, args: &[String]) -> Result<(), String> {
    if let Some(bad) = args.iter().find(|a| a.as_str() != "--check") {
        return Err(format!("exceptions: unknown argument `{bad}` (the only mode is --check)"));
    }
    let path = root.join(DOC);
    let doc = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let rows = parse(&doc);
    if !rows.iter().any(|r| r.id.starts_with('E')) {
        return Err(format!("{DOC}: no `### E<n>` rows found"));
    }
    let mut source = Source::default();
    for dir in SOURCE_DIRS {
        scan_dir(&root.join(dir), &mut source)?;
    }
    let problems = check(&rows, &source);
    if !problems.is_empty() {
        return Err(format!("{DOC} and the source disagree:\n  {}", problems.join("\n  ")));
    }
    println!(
        "exceptions: {} rows, {} tests, {} non-command writes",
        rows.iter().filter(|r| r.id.starts_with('E')).count(),
        source.tests.len(),
        rows.iter().filter(|r| r.id.starts_with('N')).count()
    );
    // The registry half needs the engine.
    run(cargo(root).args(["test", "-p", "drafftink-engine", "--lib", "--", "exception_tests::"]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC_TEXT: &str = "\
## 2. The exceptions

### E1 Hover

| | |
|---|---|
| Class | B |
| Test | `exception_E1_hover_is_quiet` (idle gate). |

### E2 Move

| Test | `exception_E2_move_equals_plain_command`, `edit/tests/other` |

## 5. Writes that are not commands

| Row | Function | Reason | Test |
|---|---|---|---|
| N1 | `Session::on_write_done` | reason | `exception_N1_done` |
";

    fn source() -> Source {
        let mut s = Source::default();
        scan_text(
            "fn exception_E1_hover_is_quiet() {}\n    fn exception_E2_move_equals_plain_command() {}\n\
             /// Non-command write N1 (docs/exceptions.md).\npub fn on_write_done() {}\nfn exception_N1_done() {}\n",
            &mut s,
        );
        s
    }

    #[test]
    fn rows_come_from_headings_and_section_five() {
        let rows = parse(DOC_TEXT);
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["E1", "E2", "N1"]);
        assert_eq!(rows[1].tests, ["exception_E2_move_equals_plain_command"], "other backticks are not tests");
        assert_eq!(rows[2].functions, ["on_write_done"]);
    }

    #[test]
    fn a_matching_document_and_source_pass() {
        assert_eq!(check(&parse(DOC_TEXT), &source()), Vec::<String>::new());
    }

    #[test]
    fn a_named_test_that_does_not_exist_fails() {
        let mut s = source();
        s.tests.remove("exception_E2_move_equals_plain_command");
        let problems = check(&parse(DOC_TEXT), &s);
        assert!(problems.iter().any(|p| p.contains("E2") && p.contains("no source file defines")), "{problems:?}");
    }

    #[test]
    fn a_row_without_a_test_fails() {
        let doc = DOC_TEXT.replace("| Test | `exception_E2_move_equals_plain_command`, `edit/tests/other` |", "| Test | none |");
        let problems = check(&parse(&doc), &source());
        assert!(problems.iter().any(|p| p.contains("E2") && p.contains("names no test")), "{problems:?}");
    }

    #[test]
    fn a_test_of_a_row_that_does_not_exist_fails() {
        let mut s = source();
        s.tests.insert("exception_E9_extra".into());
        s.tests.insert("exception_E2_another_case_of_a_row".into());
        let problems = check(&parse(DOC_TEXT), &s);
        assert!(problems.iter().any(|p| p.contains("exception_E9_extra")), "{problems:?}");
        assert!(!problems.iter().any(|p| p.contains("another_case")), "a second test of a row is fine: {problems:?}");
    }

    #[test]
    fn a_test_under_the_wrong_row_prefix_fails() {
        let doc = DOC_TEXT.replace("exception_E1_hover_is_quiet", "exception_E3_hover_is_quiet");
        let mut s = source();
        s.tests.insert("exception_E3_hover_is_quiet".into());
        let problems = check(&parse(&doc), &s);
        assert!(problems.iter().any(|p| p.contains("does not start with")), "{problems:?}");
    }

    #[test]
    fn a_marker_without_a_row_and_a_row_without_a_marker_fail() {
        let mut s = source();
        s.markers.insert("N7".into());
        let problems = check(&parse(DOC_TEXT), &s);
        assert!(problems.iter().any(|p| p.contains("N7") && p.contains("no row")), "{problems:?}");
        let mut s = source();
        s.markers.clear();
        let problems = check(&parse(DOC_TEXT), &s);
        assert!(problems.iter().any(|p| p.contains("N1") && p.contains("doc line")), "{problems:?}");
    }

    #[test]
    fn a_function_that_does_not_exist_fails() {
        let mut s = source();
        s.functions.remove("on_write_done");
        let problems = check(&parse(DOC_TEXT), &s);
        assert!(problems.iter().any(|p| p.contains("on_write_done")), "{problems:?}");
    }

    #[test]
    fn comments_and_words_ending_in_fn_are_not_functions() {
        let mut s = Source::default();
        scan_text("// fn exception_E1_in_a_comment() {}\nlet cfn = 1; pub(crate) fn real_one() {}\n", &mut s);
        assert!(s.tests.is_empty());
        assert!(s.functions.contains("real_one") && !s.functions.contains("cfn"));
    }
}
