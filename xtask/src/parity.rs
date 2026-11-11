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

/// One feature row of the audit.
#[derive(Debug)]
struct Row {
    id: String,
    status: String,
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
        rows.push(Row { id: first.to_string(), status: status.to_string() });
    }
    Ok(rows)
}

/// The numbers of one line of the counts table in section 3: done, partial, missing, out of scope, rows.
fn parse_count_line(line: &str) -> Result<[usize; 5], String> {
    let cells = cells(line);
    if cells.len() != 6 {
        return Err(format!("counts line {line:?} has {} cells, expected 6", cells.len()));
    }
    let mut numbers = [0usize; 5];
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

/// What the rows say: the number of rows per status and the number of rows, for one part or for all.
fn count_rows(rows: &[Row], part: Option<char>) -> [usize; 5] {
    let mut numbers = [0usize; 5];
    for row in rows.iter().filter(|r| part.is_none_or(|p| r.part() == p)) {
        if let Some(i) = STATUSES.iter().position(|s| *s == row.status) {
            numbers[i] += 1;
        }
        numbers[4] += 1;
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
            "counts line {prefix:?}: [done, partial, missing, out of scope, rows] stated vs counted from the rows"
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

/// The row ids named in the "Gap (rows)" column of the tables in section 4.
fn gap_list_ids(text: &str) -> Result<Vec<String>, String> {
    let gaps = section(text, "\n## 4. Prioritised gap list")?;
    let mut ids = Vec::new();
    for line in gaps.lines().filter(|l| l.starts_with('|')) {
        // Only the third column names rows; the "What is needed" column may mention other ids.
        let Some(&gap) = cells(line).get(2) else { continue };
        for word in gap.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
            if is_row_id(word) {
                ids.push(word.to_string());
            }
        }
    }
    Ok(ids)
}

#[test]
fn the_gap_lists_name_rows_that_exist_and_are_not_done() {
    let text = read_parity().unwrap();
    let rows = parse_rows(&text).unwrap();
    let mut bad = Vec::new();
    for id in gap_list_ids(&text).unwrap() {
        match rows.iter().find(|r| r.id == id) {
            None => bad.push(format!("{id} is not a row")),
            Some(row) if row.status == "done" => bad.push(format!("{id} is done")),
            Some(_) => {}
        }
    }
    assert!(bad.is_empty(), "the gap lists are out of date: {bad:?}");
}

#[test]
fn every_partial_or_missing_row_is_in_a_gap_list() {
    let text = read_parity().unwrap();
    let rows = parse_rows(&text).unwrap();
    let named = gap_list_ids(&text).unwrap();
    let left_out: Vec<&str> = rows
        .iter()
        .filter(|r| r.status == "partial" || r.status == "missing")
        .map(|r| r.id.as_str())
        .filter(|id| !named.iter().any(|n| n == id))
        .collect();
    assert!(left_out.is_empty(), "partial or missing rows that no gap list names: {left_out:?}");
}
