//! Tests that keep `docs/parity.md` consistent with itself.
//!
//! The table has one row per feature (`A-nnn` for the old app, `B-nnn` for Excalidraw). Section 3 repeats the
//! number of rows per status by hand, which goes stale the moment a row changes. These tests parse the file
//! and fail when the counts, the row ids or the ids named in the gap lists no longer agree with the rows.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

/// The statuses a row may have, in the order of the columns of the counts table.
const STATUSES: [&str; 4] = ["done", "partial", "missing", "out of scope"];

/// The dagger that marks a row whose last link is a shell (`docs/parity.md`, "How to read this").
const SHELL_MARK: &str = "†";

/// One feature row of the audit.
#[derive(Debug)]
struct Row {
    id: String,
    status: String,
    where_cell: String,
}

impl Row {
    fn part(&self) -> char {
        self.id.chars().next().unwrap_or('?')
    }
}

fn parity_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../docs/parity.md")
}

fn read_parity() -> Result<String, String> {
    let path = parity_path();
    fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

/// The cells of a markdown table line, without the outer bars and without surrounding blanks.
fn cells(line: &str) -> Vec<&str> {
    let inner = line.trim().trim_start_matches('|');
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    inner.split('|').map(str::trim).collect()
}

/// True for ids such as `A-042` and `B-256`.
fn is_row_id(text: &str) -> bool {
    let mut chars = text.chars();
    matches!(chars.next(), Some('A' | 'B'))
        && chars.next() == Some('-')
        && text.len() == 5
        && chars.all(|c| c.is_ascii_digit())
}

/// Every feature row in the file, in file order. A line is a row when its first cell is a row id.
fn parse_rows(text: &str) -> Result<Vec<Row>, String> {
    let mut rows = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if !line.starts_with('|') {
            continue;
        }
        let cells = cells(line);
        let Some(&first) = cells.first() else { continue };
        if !is_row_id(first) {
            continue;
        }
        if cells.len() < 6 {
            return Err(format!("line {}: row {first} has {} cells, expected 6", n + 1, cells.len()));
        }
        let status = cells[3];
        if !STATUSES.contains(&status) {
            return Err(format!("line {}: row {first} has the status {status:?}", n + 1));
        }
        rows.push(Row { id: first.to_string(), status: status.to_string(), where_cell: cells[4].to_string() });
    }
    Ok(rows)
}

/// The numbers of one line of the counts table in section 3: done, partial, missing, out of scope, rows, with a dagger.
fn parse_count_line(line: &str) -> Result<[usize; 6], String> {
    let cells = cells(line);
    if cells.len() != 7 {
        return Err(format!("counts line {line:?} has {} cells, expected 7", cells.len()));
    }
    let mut numbers = [0usize; 6];
    for (slot, cell) in numbers.iter_mut().zip(&cells[1..]) {
        *slot = cell.parse().map_err(|_| format!("counts line {line:?}: {cell:?} is not a number"))?;
    }
    Ok(numbers)
}

/// The text of section 3 up to the next section heading.
fn section<'a>(text: &'a str, heading: &str) -> Result<&'a str, String> {
    let start = text.find(heading).ok_or_else(|| format!("no section {heading:?}"))?;
    let rest = &text[start + heading.len()..];
    let end = rest.find("\n## ").unwrap_or(rest.len());
    Ok(&rest[..end])
}

/// What the rows say: per part, the number of rows per status, the number of rows and the done rows with a dagger.
fn count_rows(rows: &[Row], part: Option<char>) -> [usize; 6] {
    let mut numbers = [0usize; 6];
    for row in rows.iter().filter(|r| part.is_none_or(|p| r.part() == p)) {
        if let Some(i) = STATUSES.iter().position(|s| *s == row.status) {
            numbers[i] += 1;
        }
        numbers[4] += 1;
        if row.status == "done" && row.where_cell.contains(SHELL_MARK) {
            numbers[5] += 1;
        }
    }
    numbers
}

#[test]
fn counts_table_equals_the_rows_per_status() {
    let text = read_parity().unwrap();
    let rows = parse_rows(&text).unwrap();
    let counts = section(&text, "\n## 3. Counts").unwrap();
    let mut seen = 0;
    for (prefix, part) in [("| A:", Some('A')), ("| B:", Some('B')), ("| Total", None)] {
        let line = counts
            .lines()
            .find(|l| l.starts_with(prefix))
            .unwrap_or_else(|| panic!("the counts table has no line starting with {prefix:?}"));
        let stated = parse_count_line(line).unwrap();
        let actual = count_rows(&rows, part);
        assert_eq!(
            stated, actual,
            "counts line {prefix:?}: [done, partial, missing, out of scope, rows, done with dagger] stated vs counted from the rows"
        );
        seen += 1;
    }
    assert_eq!(seen, 3);
}

#[test]
fn row_ids_are_unique() {
    let text = read_parity().unwrap();
    let rows = parse_rows(&text).unwrap();
    assert!(rows.len() > 300, "only {} rows parsed; the parser lost the tables", rows.len());
    let mut first_seen: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, row) in rows.iter().enumerate() {
        if let Some(earlier) = first_seen.insert(&row.id, i) {
            panic!("row id {} appears twice (the {}th and the {}th row)", row.id, earlier + 1, i + 1);
        }
    }
}

#[test]
fn section_one_holds_the_a_rows_and_section_two_the_b_rows() {
    let text = read_parity().unwrap();
    for (heading, part) in [("\n## 1. Old app features (A)", 'A'), ("\n## 2. Excalidraw features (B)", 'B')] {
        let rows = parse_rows(section(&text, heading).unwrap()).unwrap();
        assert!(!rows.is_empty(), "{heading:?} has no rows");
        let stray: Vec<&str> = rows.iter().filter(|r| r.part() != part).map(|r| r.id.as_str()).collect();
        assert!(stray.is_empty(), "{heading:?} holds rows of the other part: {stray:?}");
    }
}

#[test]
fn every_row_id_named_in_the_gap_lists_is_a_row() {
    let text = read_parity().unwrap();
    let rows = parse_rows(&text).unwrap();
    let known: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
    let gaps = section(&text, "\n## 4. Prioritised gap list").unwrap();
    let mut missing = Vec::new();
    for line in gaps.lines().filter(|l| l.starts_with('|')) {
        // Only the "Gap (rows)" column names rows; the "What is needed" column may cite ids of the old text too.
        let Some(&gap) = cells(line).get(2) else { continue };
        for word in gap.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
            if is_row_id(word) && !known.contains(&word) {
                missing.push(word.to_string());
            }
        }
    }
    assert!(missing.is_empty(), "the gap lists name rows that do not exist: {missing:?}");
}
