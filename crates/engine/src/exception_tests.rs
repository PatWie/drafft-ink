//! docs/exceptions.md: the registry side of `cargo xtask exceptions --check`, and the tests of the writes
//! that are not commands (section 5 of that file).
//!
//! The gate in `xtask/src/exceptions.rs` checks the file against the source text (named tests exist, every
//! `Non-command write` marker has a row). The tests here check it against the registry, which xtask cannot
//! see without linking the engine.

use std::collections::BTreeSet;

use crate::registry::Registry;
use crate::spec::Undo;
use crate::testing;

const EXCEPTIONS_MD: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/exceptions.md");

/// One `### E<n>` section: its id, its class cell and the command ids its table names.
struct Row {
    id: String,
    class: String,
    commands: BTreeSet<String>,
    final_commands: BTreeSet<String>,
}

fn read() -> String {
    std::fs::read_to_string(EXCEPTIONS_MD).unwrap_or_else(|e| panic!("cannot read {EXCEPTIONS_MD}: {e}"))
}

/// The backticked spans of a line.
fn spans(line: &str) -> impl Iterator<Item = &str> {
    line.split('`').skip(1).step_by(2)
}

/// `family.verb` when the span starts with one (`"element.move { ids }"` gives `element.move`).
fn command_id(span: &str) -> Option<&str> {
    let word = span.split_whitespace().next()?;
    let (family, verb) = word.split_once('.')?;
    let lower = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase() || b == b'_');
    (lower(family) && lower(verb)).then_some(word)
}

fn parse_rows(md: &str) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    // Only the lines between a `### E<n>` heading and the next heading belong to a row.
    let mut open = false;
    for line in md.lines() {
        if line.starts_with('#') {
            let id = line.strip_prefix("### ").and_then(|r| r.split_whitespace().next()).unwrap_or_default();
            open = id.strip_prefix('E').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
            if open {
                rows.push(Row {
                    id: id.to_owned(),
                    class: String::new(),
                    commands: BTreeSet::new(),
                    final_commands: BTreeSet::new(),
                });
            }
            continue;
        }
        let (true, Some(row)) = (open, rows.last_mut()) else { continue };
        if line.starts_with("| Class |") {
            row.class = line.split('|').nth(2).unwrap_or_default().trim().to_owned();
        }
        let is_final = line.starts_with("| Final state |");
        for id in spans(line).filter_map(command_id) {
            row.commands.insert(id.to_owned());
            if is_final {
                row.final_commands.insert(id.to_owned());
            }
        }
    }
    rows
}

#[test]
fn exceptions_doc_matches_the_registry() {
    let rows = parse_rows(&read());
    assert!(rows.len() >= 15, "docs/exceptions.md: found {} rows `### E<n>`", rows.len());
    let registered: BTreeSet<&str> = Registry::global().all().iter().map(|s| s.id).collect();

    // Every final command is registered.
    let missing: Vec<String> = rows
        .iter()
        .flat_map(|r| r.final_commands.iter().map(move |c| (r.id.as_str(), c)))
        .filter(|(_, c)| !registered.contains(c.as_str()))
        .map(|(row, c)| format!("{row}: `{c}`"))
        .collect();
    assert!(missing.is_empty(), "rows name a final command that is not registered: {missing:?}");

    // Every gesture command is named in a class A row.
    let class_a: BTreeSet<&str> =
        rows.iter().filter(|r| r.class == "A").flat_map(|r| r.commands.iter().map(String::as_str)).collect();
    let unlisted: Vec<&str> = Registry::global()
        .all()
        .iter()
        .filter(|s| s.undo == Undo::Gesture && !class_a.contains(s.id))
        .map(|s| s.id)
        .collect();
    assert!(unlisted.is_empty(), "commands with `undo = Gesture` that no class A row of docs/exceptions.md names: {unlisted:?}");

    // Every class A row names at least one gesture command (a class A row for something else is a stale row).
    for r in rows.iter().filter(|r| r.class == "A") {
        let has = r.commands.iter().any(|c| Registry::global().find(c).is_some_and(|s| s.undo == Undo::Gesture));
        assert!(has, "{} is class A and names no command with `undo = Gesture`", r.id);
    }
}

#[test]
fn rows_have_unique_ids() {
    let rows = parse_rows(&read());
    let ids: BTreeSet<&str> = rows.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids.len(), rows.len(), "duplicate row ids in docs/exceptions.md");
}

// ---- section 5: writes that are not commands ----

/// N2: a collaborator's update changes the document and the redraw state without a command, a ring entry of a
/// user action, or an undo step.
#[test]
fn exception_N2_a_remote_update_changes_the_document_without_a_command() {
    let mut a = testing::headless_session();
    let mut b = testing::headless_session();
    let id = testing::add_rect(&mut a, 5.0, 6.0, 70.0, 80.0).unwrap();
    let vv = b.tab().doc.replicated().version_vector().unwrap();
    let bytes = a.tab().doc.replicated().export_updates_since(&vv).unwrap();
    let tab = b.tab().id;
    b.apply_remote(tab, &bytes).unwrap();
    assert!(drafftink_scene::SceneRead::get(b.store(), &id).is_some());
    assert_eq!(b.tab().doc.undo_depth(), 0);
    assert_eq!(b.ring().iter().last().map(|e| e.id.as_str()), Some(crate::remote::REMOTE_RING_ID));
}
