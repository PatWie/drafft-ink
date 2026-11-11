//! `catalog_matches_registry` and its relatives (docs/commands.md §1.2, "Registry tests", item 7).
//!
//! These tests read `docs/commands.md` at run time and compare it with the registered specs: the ids in
//! §2, the identity tables, the facts of Appendix A (§10) and the declared refusal codes. A catalog id
//! that no family has registered yet must be listed in `crates/engine/unregistered.txt`; a family removes
//! its lines from that file when it registers its commands.

use std::collections::{BTreeMap, BTreeSet};

use crate::refusal::is_shared_code;
use crate::registry::Registry;
use crate::spec::{
    Batch, CommandSpec, DocWrite, Facts, Gate, Host, HostEffect, KeyRepeat, Redraw, RefusalToast, RingMode, Selection,
    TextGate, Undo,
};

const COMMANDS_MD: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/commands.md");
const UNREGISTERED_TXT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/unregistered.txt");

/// The heading line that opens the families section.
const FAMILIES_HEADING: &str = "## 2. Families";
/// The prefix of the heading that closes the families section.
const FAMILIES_END_PREFIX: &str = "## 3.";
/// The prefix of the heading of Appendix A.
const APPENDIX_PREFIX: &str = "## 10.";
/// Number of columns in an Appendix A row.
const APPENDIX_COLUMNS: usize = 12;

// ---------------------------------------------------------------------------------------------------
// Markdown helpers
// ---------------------------------------------------------------------------------------------------

/// Splits one markdown table row into trimmed cells. A pipe written `\|` stays inside its cell (as `|`).
/// Returns `None` when the line is not a table row (it must start with `|`).
pub(crate) fn split_row(line: &str) -> Option<Vec<String>> {
    let line = line.trim();
    let body = line.strip_prefix('|')?;
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                cur.push('|');
                chars.next();
            }
            '|' => cells.push(std::mem::take(&mut cur).trim().to_owned()),
            _ => cur.push(c),
        }
    }
    // Text after the last pipe belongs to a cell only when the row has no closing pipe.
    if !cur.trim().is_empty() {
        cells.push(cur.trim().to_owned());
    }
    Some(cells)
}

/// The id in a first cell written `` `family.verb` ``, or `None` when the cell is not fully backticked.
pub(crate) fn backticked(cell: &str) -> Option<&str> {
    let inner = cell.strip_prefix('`')?.strip_suffix('`')?;
    (!inner.is_empty() && !inner.contains('`') && !inner.contains(char::is_whitespace)).then_some(inner)
}

/// The lines after the heading line `start` (exact match) up to the next line that starts with
/// `end_prefix`, or to the end of the text when `end_prefix` is empty or never found.
pub(crate) fn section<'a>(md: &'a str, start: &str, end_prefix: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in md.lines() {
        if !inside {
            inside = line.trim_end() == start;
        } else if !end_prefix.is_empty() && line.starts_with(end_prefix) {
            break;
        } else {
            out.push(line);
        }
    }
    out
}

/// The lines after the first heading line that starts with `start_prefix`, up to the end of the text.
fn section_from_prefix<'a>(md: &'a str, start_prefix: &str) -> Vec<&'a str> {
    md.lines().skip_while(|l| !l.starts_with(start_prefix)).skip(1).collect()
}

/// Every table row of `lines` whose first cell is a backticked id, in order: `(id, cells)`. A backticked
/// first cell without a dot (the style property table of §2.7) is not an id and is skipped.
pub(crate) fn id_rows(lines: &[&str]) -> Vec<(String, Vec<String>)> {
    lines
        .iter()
        .filter_map(|l| split_row(l))
        .filter_map(|cells| {
            let id = backticked(cells.first()?).filter(|id| id.contains('.'))?.to_owned();
            Some((id, cells))
        })
        .collect()
}

/// The rows of §2 of the catalog: the identity row and the details row of each id.
#[derive(Debug, Default)]
pub(crate) struct Catalog {
    /// How often each id appears as a first cell.
    pub(crate) counts: BTreeMap<String, usize>,
    /// The first row of each id (identity table: id, title, undo, keys, source).
    pub(crate) identity: BTreeMap<String, Vec<String>>,
    /// The second row of each id (details table: id, args, availability and refusals, effect).
    pub(crate) details: BTreeMap<String, Vec<String>>,
}

impl Catalog {
    /// Parses §2 of the catalog text.
    pub(crate) fn parse(md: &str) -> Catalog {
        let lines = section(md, FAMILIES_HEADING, FAMILIES_END_PREFIX);
        let mut cat = Catalog::default();
        for (id, cells) in id_rows(&lines) {
            let n = cat.counts.entry(id.clone()).or_insert(0);
            *n += 1;
            match *n {
                1 => {
                    cat.identity.insert(id, cells);
                }
                2 => {
                    cat.details.insert(id, cells);
                }
                _ => {}
            }
        }
        cat
    }

    /// Every id in §2.
    pub(crate) fn ids(&self) -> BTreeSet<&str> {
        self.counts.keys().map(String::as_str).collect()
    }
}

/// The Appendix A rows (§10) by id. Ids that appear twice are returned in the second value.
fn parse_appendix(md: &str) -> (BTreeMap<String, Vec<String>>, Vec<String>) {
    let lines = section_from_prefix(md, APPENDIX_PREFIX);
    let mut rows = BTreeMap::new();
    let mut duplicates = Vec::new();
    for (id, cells) in id_rows(&lines) {
        if cells.len() != APPENDIX_COLUMNS {
            continue;
        }
        if rows.insert(id.clone(), cells).is_some() {
            duplicates.push(id);
        }
    }
    (rows, duplicates)
}

fn read_catalog_text() -> String {
    std::fs::read_to_string(COMMANDS_MD).unwrap_or_else(|e| panic!("cannot read {COMMANDS_MD}: {e}"))
}

/// The ids listed in `unregistered.txt`, in file order, without comments and blank lines.
fn parse_unregistered(text: &str) -> Vec<String> {
    text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(str::to_owned).collect()
}

fn registered() -> Vec<&'static CommandSpec> {
    Registry::global().all().to_vec()
}

fn is_id_shape(id: &str) -> bool {
    let mut parts = id.split('.');
    let (Some(family), Some(verb), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c == '_');
    ok(family) && ok(verb)
}

fn lower_snake(code: &str) -> bool {
    code.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && code.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !code.ends_with('_')
        && !code.contains("__")
}

// ---------------------------------------------------------------------------------------------------
// 1. Ids
// ---------------------------------------------------------------------------------------------------

#[test]
fn catalog_matches_registry() {
    let md = read_catalog_text();
    let cat = Catalog::parse(&md);
    let catalog_ids = cat.ids();
    let reg = registered();
    let reg_ids: BTreeSet<&str> = reg.iter().map(|s| s.id).collect();
    let mut problems: Vec<String> = Vec::new();

    if catalog_ids.is_empty() {
        problems.push(format!(
            "no catalog rows found between `{FAMILIES_HEADING}` and `{FAMILIES_END_PREFIX}` in docs/commands.md"
        ));
    }

    // Each id appears once in the identity table and once in the details table.
    let offenders: Vec<String> =
        cat.counts.iter().filter(|(_, n)| **n != 2).map(|(id, n)| format!("`{id}` appears {n} times")).collect();
    if !offenders.is_empty() {
        problems.push(format!(
            "each catalog id must appear exactly twice in §2 (identity table and details table):\n  {}",
            offenders.join("\n  ")
        ));
    }
    let bad_shape: Vec<&str> = catalog_ids.iter().copied().filter(|id| !is_id_shape(id)).collect();
    if !bad_shape.is_empty() {
        problems.push(format!("catalog first cells that are not `family.verb` ids: {}", bad_shape.join(", ")));
    }

    // Registered ids are in the catalog.
    let missing: Vec<&str> = reg_ids.iter().copied().filter(|id| !catalog_ids.contains(id)).collect();
    if !missing.is_empty() {
        problems.push(format!(
            "registered ids that are not in docs/commands.md §2 (add their rows, or fix the id):\n  {}",
            missing.join("\n  ")
        ));
    }

    // unregistered.txt: sorted, no duplicates, only catalog ids that are not registered.
    let text =
        std::fs::read_to_string(UNREGISTERED_TXT).unwrap_or_else(|e| panic!("cannot read {UNREGISTERED_TXT}: {e}"));
    let listed = parse_unregistered(&text);
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for id in &listed {
        if !seen.insert(id.as_str()) {
            problems.push(format!("unregistered.txt lists `{id}` twice: remove the duplicate line"));
        }
    }
    if let Some(pair) = listed.windows(2).find(|w| w[0] > w[1]) {
        problems
            .push(format!("unregistered.txt is not sorted: `{}` comes before `{}`; sort the lines", pair[0], pair[1]));
    }
    for id in &seen {
        if reg_ids.contains(id) {
            problems.push(format!("unregistered.txt lists `{id}`, which is registered: remove its line"));
        } else if !catalog_ids.contains(id) {
            problems.push(format!(
                "unregistered.txt lists `{id}`, which is not in the catalog: remove its line or fix the id"
            ));
        }
    }
    let unlisted: Vec<&str> =
        catalog_ids.iter().copied().filter(|id| !reg_ids.contains(id) && !seen.contains(id)).collect();
    if !unlisted.is_empty() {
        problems.push(format!(
            "catalog ids that are neither registered nor listed in crates/engine/unregistered.txt \
             (register them, or add the lines in sorted order):\n{}",
            unlisted.join("\n")
        ));
    }

    assert!(problems.is_empty(), "catalog and registry disagree:\n{}", problems.join("\n"));
}

// ---------------------------------------------------------------------------------------------------
// 2. Identity tables
// ---------------------------------------------------------------------------------------------------

fn undo_of_cell(cell: &str) -> Option<Undo> {
    [Undo::Undoable, Undo::NoUndo, Undo::Gesture].into_iter().find(|u| cell.starts_with(u.name()))
}

#[test]
fn identity_table_matches_registry() {
    let md = read_catalog_text();
    let cat = Catalog::parse(&md);
    let mut problems: Vec<String> = Vec::new();
    for spec in registered() {
        let Some(row) = cat.identity.get(spec.id) else {
            problems.push(format!("{}: no identity row in docs/commands.md §2", spec.id));
            continue;
        };
        let (Some(title), Some(undo)) = (row.get(1), row.get(2)) else {
            problems.push(format!(
                "{}: identity row has {} cells, expected 5 (id | title | undo | keys | source)",
                spec.id,
                row.len()
            ));
            continue;
        };
        if title != spec.title {
            problems.push(format!("{}: title: catalog `{}`, registry `{}`", spec.id, title, spec.title));
        }
        match undo_of_cell(undo) {
            None => {
                problems.push(format!("{}: undo cell `{}` must start with Undoable, NoUndo or Gesture", spec.id, undo))
            }
            Some(u) if u != spec.undo => {
                problems.push(format!(
                    "{}: undo: catalog `{}` (cell `{}`), registry `{}`",
                    spec.id,
                    u.name(),
                    undo,
                    spec.undo.name()
                ));
            }
            Some(_) => {}
        }
    }
    assert!(problems.is_empty(), "identity tables and registry disagree:\n{}", problems.join("\n"));
}

// ---------------------------------------------------------------------------------------------------
// 3. Appendix A facts
// ---------------------------------------------------------------------------------------------------

/// A comma list such as `D,O,B` or `Files,Storage` as a set of lower-case items; `-` is the empty set.
fn item_set(cell: &str) -> BTreeSet<String> {
    cell.split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty() && s != "-").collect()
}

fn show_set(set: &BTreeSet<String>) -> String {
    if set.is_empty() { "-".to_owned() } else { set.iter().cloned().collect::<Vec<_>>().join(",") }
}

fn selection_word(s: Selection) -> &'static str {
    match s {
        Selection::No => "-",
        Selection::Soft => "soft",
        Selection::Required => "req",
    }
}

fn batch_word(b: Batch) -> &'static str {
    match b {
        Batch::Allowed => "ok",
        Batch::Last => "last",
        Batch::Never => "never",
    }
}

fn gate_word(g: Gate) -> &'static str {
    match g {
        Gate::Allowed => "ok",
        Gate::Refused => "no",
    }
}

fn text_gate_word(g: TextGate) -> &'static str {
    match g {
        TextGate::Allowed => "ok",
        TextGate::Refused => "no",
        TextGate::Commit => "commit",
    }
}

fn redraw_word(r: Redraw) -> &'static str {
    match r {
        Redraw::None => "-",
        Redraw::Overlay => "overlay",
        Redraw::Camera => "camera",
        Redraw::Scene => "scene",
    }
}

fn ring_word(r: RingMode) -> &'static str {
    match r {
        RingMode::Always => "always",
        RingMode::Quiet => "quiet",
    }
}

fn yn(b: bool) -> &'static str {
    if b { "y" } else { "n" }
}

/// Compares a spec with its Appendix A row; pushes one line per mismatching column.
///
/// `Facts` is destructured without `..`: a new fact cannot be added to a registration without deciding here
/// how it is compared with the catalog.
fn compare_facts(spec: &CommandSpec, row: &[String], problems: &mut Vec<String>) {
    let Facts {
        // The identity tables of §2 carry the undo class (`catalog_matches_registry`, `identity_tables_match`).
        undo: _,
        reads_only,
        mutates,
        needs_selection,
        ports,
        batch,
        view_only,
        while_gesture,
        while_text_edit,
        redraw,
        ring,
        palette,
        agent,
        // Appendix A2 (`facts_match_appendix_a2`).
        doc_write: _,
    } = spec.facts();
    let cell = |i: usize| row.get(i).map_or("", String::as_str);
    let mut text = |col: &str, i: usize, reg: &str| {
        if cell(i) != reg {
            problems.push(format!("{} | {col} | catalog `{}` | registry `{reg}`", spec.id, cell(i)));
        }
    };
    text("selection", 2, selection_word(needs_selection));
    text("batch", 4, batch_word(batch));
    text("view-only", 5, gate_word(view_only));
    text("during gesture", 6, gate_word(while_gesture));
    text("during text edit", 7, text_gate_word(while_text_edit));
    text("redraw", 8, redraw_word(redraw));
    text("ring", 9, ring_word(ring));
    text("palette", 10, yn(palette));
    text("agent", 11, yn(agent));
    // `reads_only` is the empty `mutates` column.
    text("mutates (reads_only)", 1, if reads_only { "-" } else { cell(1) });
    if !reads_only && item_set(cell(1)).is_empty() {
        problems.push(format!("{} | mutates | catalog `-` | registry is not reads_only", spec.id));
    }

    let mut set = |col: &str, i: usize, reg: BTreeSet<String>| {
        let cat = item_set(cell(i));
        if cat != reg {
            problems.push(format!(
                "{} | {col} | catalog `{}` | registry `{}`",
                spec.id,
                show_set(&cat),
                show_set(&reg)
            ));
        }
    };
    set("mutates", 1, item_set(&mutates.letters()));
    set("ports", 3, ports.names().into_iter().map(str::to_owned).collect());
}

#[test]
fn facts_match_appendix_a() {
    let md = read_catalog_text();
    let (rows, duplicates) = parse_appendix(&md);
    let mut problems: Vec<String> = Vec::new();
    if rows.is_empty() {
        problems.push(format!("no Appendix A rows ({APPENDIX_COLUMNS} columns, first cell a backticked id) found after `{APPENDIX_PREFIX}`"));
    }
    let reg = registered();
    for id in duplicates {
        if reg.iter().any(|s| s.id == id) {
            problems.push(format!("{id}: appears more than once in Appendix A"));
        }
    }
    for spec in reg {
        match rows.get(spec.id) {
            None => {
                problems.push(format!("{} | (row) | no Appendix A row in docs/commands.md §10 | registered", spec.id))
            }
            Some(row) => compare_facts(spec, row, &mut problems),
        }
    }
    assert!(
        problems.is_empty(),
        "Appendix A (docs/commands.md §10) and the registry disagree (id | column | catalog | registry):\n{}",
        problems.join("\n")
    );
}

// ---------------------------------------------------------------------------------------------------
// 3b. Appendix A2: document, host and agent-tool facts
// ---------------------------------------------------------------------------------------------------

/// The heading that opens Appendix A2.
const A2_PREFIX: &str = "### Appendix A2";
/// Number of columns in an Appendix A2 row.
const A2_COLUMNS: usize = 7;

fn toast_word(t: RefusalToast) -> &'static str {
    match t {
        RefusalToast::Show => "show",
        RefusalToast::Silent => "silent",
    }
}

fn repeat_word(r: KeyRepeat) -> &'static str {
    match r {
        KeyRepeat::Repeats => "repeats",
        KeyRepeat::FirstPressOnly => "first_press_only",
    }
}

/// The cells of Appendix A2 for a registration: doc write, host effect, refusal toast, key repeat, editor takes
/// style, own agent tool.
fn a2_cells(spec: &CommandSpec) -> Vec<String> {
    let Host { effect, refusal, repeat, text_editor_takes_style } = spec.host;
    vec![
        spec.doc_write.name().to_owned(),
        effect.name().to_owned(),
        toast_word(refusal).to_owned(),
        repeat_word(repeat).to_owned(),
        yn(text_editor_takes_style).to_owned(),
        yn(spec.own_agent_tool).to_owned(),
    ]
}

/// What a registration has when its Appendix A2 row is absent.
fn a2_default(spec: &CommandSpec) -> Vec<String> {
    let doc = if spec.mutates.contains(crate::spec::Mutates::DOC) { DocWrite::Edit } else { DocWrite::NotDocument };
    vec![
        doc.name().to_owned(),
        HostEffect::None.name().to_owned(),
        toast_word(RefusalToast::Show).to_owned(),
        repeat_word(KeyRepeat::Repeats).to_owned(),
        yn(false).to_owned(),
        yn(false).to_owned(),
    ]
}

fn parse_a2(md: &str) -> (BTreeMap<String, Vec<String>>, Vec<String>) {
    let lines = section_from_prefix(md, A2_PREFIX);
    let mut rows = BTreeMap::new();
    let mut duplicates = Vec::new();
    for (id, cells) in id_rows(&lines) {
        if cells.len() != A2_COLUMNS {
            continue;
        }
        let values = cells.get(1..).unwrap_or_default().to_vec();
        if rows.insert(id.clone(), values).is_some() {
            duplicates.push(id);
        }
    }
    (rows, duplicates)
}

#[test]
fn facts_match_appendix_a2() {
    let md = read_catalog_text();
    let (rows, duplicates) = parse_a2(&md);
    let mut problems: Vec<String> = Vec::new();
    if rows.is_empty() {
        problems.push(format!("no Appendix A2 rows ({A2_COLUMNS} columns) found after `{A2_PREFIX}`"));
    }
    for id in duplicates {
        problems.push(format!("{id}: appears more than once in Appendix A2"));
    }
    let reg = registered();
    for id in rows.keys() {
        if !reg.iter().any(|s| s.id == id) {
            problems.push(format!("{id}: Appendix A2 names a command that is not registered"));
        }
    }
    for spec in reg {
        let actual = a2_cells(spec);
        match rows.get(spec.id) {
            None if actual != a2_default(spec) => problems.push(format!(
                "{} | registry {actual:?} differs from the default {:?} and Appendix A2 has no row",
                spec.id,
                a2_default(spec)
            )),
            None => {}
            Some(row) if *row == a2_default(spec) => {
                problems.push(format!("{} | Appendix A2 row lists only defaults; delete it", spec.id));
            }
            Some(row) if *row != actual => {
                problems.push(format!("{} | catalog {row:?} | registry {actual:?}", spec.id));
            }
            Some(_) => {}
        }
    }
    assert!(
        problems.is_empty(),
        "Appendix A2 (docs/commands.md §10) and the registry disagree:\n{}",
        problems.join("\n")
    );
}

/// A host that reads `Host` from the registration has something to read: every effect other than `None` is
/// declared by at least one command, so a variant cannot go dead, and the neutral value is the one most commands
/// carry.
#[test]
fn every_host_effect_is_declared_by_a_command() {
    let reg = registered();
    let used: BTreeSet<&str> = reg.iter().map(|s| s.host.effect.name()).collect();
    for effect in [
        HostEffect::CompletesRename,
        HostEffect::CompletesRenameThenSaveAs,
        HostEffect::CompletesExport,
        HostEffect::CompletesLibraryAdd,
        HostEffect::ShowsConvertPopup,
        HostEffect::CompletesLinkEdit,
        HostEffect::ReturnsUrlToOpen,
        HostEffect::ShowsSearchResults,
        HostEffect::BeginsGesture,
        HostEffect::EndsGestureOnRefusal,
        HostEffect::EndsGesture,
        HostEffect::EndsEquationEdit,
    ] {
        assert!(used.contains(effect.name()), "no command declares the host effect `{}`", effect.name());
    }
    assert!(reg.iter().filter(|s| s.host.is_none()).count() > reg.len() / 2);
}

// ---------------------------------------------------------------------------------------------------
// 4. Declared refusal codes
// ---------------------------------------------------------------------------------------------------

#[test]
fn declared_refusal_codes_are_catalogued() {
    let md = read_catalog_text();
    let cat = Catalog::parse(&md);
    let mut problems: Vec<String> = Vec::new();
    for spec in registered() {
        if spec.refusals.is_empty() {
            continue;
        }
        let details = cat.details.get(spec.id).map(|cells| cells.join(" | "));
        for code in spec.refusals {
            if !lower_snake(code) {
                problems.push(format!("{}: refusal code `{code}` is not lower snake_case", spec.id));
            }
            if is_shared_code(code) {
                continue;
            }
            match &details {
                None => problems.push(format!("{}: refusal code `{code}`: the command has no details row in docs/commands.md §2", spec.id)),
                Some(text) if !text.contains(code) => problems.push(format!(
                    "{}: refusal code `{code}` is neither a shared code nor named in the command's details row in docs/commands.md §2",
                    spec.id
                )),
                Some(_) => {}
            }
        }
    }
    assert!(problems.is_empty(), "declared refusal codes are not in the catalog:\n{}", problems.join("\n"));
}

// ---------------------------------------------------------------------------------------------------
// 5. The helpers themselves
// ---------------------------------------------------------------------------------------------------

const SYNTHETIC: &str = "\
# Title

| `outside.row` | not in section |

## 2. Families

Intro text with a | pipe that is not a row.

| id | title | undo | keys | source |
|---|---|---|---|---|
| `a.one` | One | Undoable (the step) | `Mod+K` | NEW |
| `a.two` | Two \\| pipe | NoUndo, read-only | none | NEW |

| id | args | availability and refusals | effect |
|---|---|---|---|
| `a.one` | `x: int` | Execute: `bad_thing` | does it |
| `a.two` | `since: \"a\" \\| \"b\"` | Always available. | none |
| `a.three` | none | x | y |

## 3. Next

| `after.row` | not in section |

## 10. Appendix A

| id | mutates | selection | ports | batch | view-only | during gesture | during text edit | redraw | ring | palette | agent |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `a.one` | D,O | req | Files,Storage | ok | no | no | commit | scene | always | y | y |
| `a.two` | - | - | - | ok | ok | ok | ok | - | quiet | n | n |
";

#[test]
fn markdown_helpers_parse_tables() {
    // Row splitting keeps escaped pipes inside the cell and drops the outer pipes.
    assert_eq!(
        split_row("| `a.b` | x \\| y | z |"),
        Some(vec!["`a.b`".to_owned(), "x | y".to_owned(), "z".to_owned()])
    );
    assert_eq!(split_row("  | a | | c |  "), Some(vec!["a".to_owned(), String::new(), "c".to_owned()]));
    assert_eq!(split_row("| a | b"), Some(vec!["a".to_owned(), "b".to_owned()]), "a row without the closing pipe");
    assert_eq!(split_row("not a row | a |"), None);
    assert_eq!(split_row(""), None);

    assert_eq!(backticked("`doc.new`"), Some("doc.new"));
    assert_eq!(backticked("doc.new"), None);
    assert_eq!(backticked("`two words`"), None);
    assert_eq!(backticked("``"), None);
    assert!(
        id_rows(&["| `opacity` | 0 to 1 |", "| `a.b` | x |"]).iter().map(|r| r.0.as_str()).eq(["a.b"]),
        "no dot: not an id"
    );

    let cat = Catalog::parse(SYNTHETIC);
    assert_eq!(cat.ids().into_iter().collect::<Vec<_>>(), ["a.one", "a.three", "a.two"], "rows outside §2 are ignored");
    assert_eq!(cat.counts.get("a.one"), Some(&2));
    assert_eq!(cat.counts.get("a.three"), Some(&1));
    assert_eq!(cat.identity.get("a.two").and_then(|r| r.get(1)).map(String::as_str), Some("Two | pipe"));
    assert_eq!(cat.identity.get("a.one").and_then(|r| r.get(2)).map(String::as_str), Some("Undoable (the step)"));
    assert_eq!(undo_of_cell("NoUndo, read-only"), Some(Undo::NoUndo));
    assert_eq!(undo_of_cell("Undoable (the step)"), Some(Undo::Undoable));
    assert_eq!(undo_of_cell("Gesture"), Some(Undo::Gesture));
    assert_eq!(undo_of_cell("Maybe"), None);
    let details = cat.details.get("a.two").map(|r| r.join(" | ")).unwrap_or_default();
    assert!(details.contains("\"a\" | \"b\""), "{details}");
    assert!(cat.details.get("a.one").is_some_and(|r| r.join(" ").contains("bad_thing")));
    assert!(!cat.details.contains_key("a.three"));

    let (rows, dups) = parse_appendix(SYNTHETIC);
    assert!(dups.is_empty());
    assert_eq!(rows.keys().map(String::as_str).collect::<Vec<_>>(), ["a.one", "a.two"]);
    assert_eq!(rows.get("a.one").map(Vec::len), Some(APPENDIX_COLUMNS));
    assert_eq!(item_set("D, O"), BTreeSet::from(["d".to_owned(), "o".to_owned()]));
    assert!(item_set("-").is_empty());

    let listed = parse_unregistered("# header\n\n a.one \n# more\na.two\n");
    assert_eq!(listed, ["a.one", "a.two"]);
    assert!(is_id_shape("doc.new") && !is_id_shape("doc") && !is_id_shape("a.b.c") && !is_id_shape("Doc.new"));
    assert!(
        lower_snake("no_target")
            && !lower_snake("NoTarget")
            && !lower_snake("a__b")
            && !lower_snake("")
            && !lower_snake("_a")
    );
}
