//! The keymap cannot drift: the default bindings of `docs/commands.md` §6.1 are compared with the shortcuts the
//! registered commands declare (docs/commands.md §1.2, registry test 7).
//!
//! The rules, so a family author knows what is compared:
//!
//! * Only registered commands take part. A §6.1 row whose commands are all unregistered is skipped, and so is a
//!   UI verb (`ui.*`).
//! * For every registered command, on both platforms, the set of chords in §6.1 must equal the set of chords in its
//!   `shortcuts`, with the same scope (`canvas`, `text`, `always`). A row that lists several commands for one key
//!   with a condition ("`arrow.delete_point` while ..., else `element.delete`") is satisfied when any one registered
//!   command of the row has the chord, and no command may add a chord the row does not list.
//! * A cell `mac ... / other ...` gives the two platforms their own chords; any other cell serves both.
//! * `A/B/C` at the end of a chord and `Arrow keys` stand for the four directions (`Left/Right/Up/Down`), on the
//!   registry side too, where `ArrowLeft` and `Left` are the same key.
//! * Fixed arguments written as `key=value` (or `key=a, b, c`, one value per chord in order) must appear in the
//!   binding's `args`. A value that starts with `$` is a runtime value and is not compared.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::catalog_tests::{section, split_row};
use crate::registry::Registry;
use crate::spec::{CommandSpec, Scope};

const COMMANDS_MD: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/commands.md");
const KEYMAP_HEADING: &str = "### 6.1 Default bindings";
const KEYMAP_END_PREFIX: &str = "### 6.2";

/// One row of §6.1.
#[derive(Debug, Clone, PartialEq)]
struct Row {
    /// Chords on macOS, in cell order.
    mac: Vec<String>,
    /// Chords elsewhere, in cell order.
    other: Vec<String>,
    /// Command ids in the cell (anything that looks like `family.verb`, UI verbs included).
    ids: Vec<String>,
    /// Fixed arguments: key to the values listed for it.
    args: BTreeMap<String, Vec<String>>,
    /// The row picks one of several commands by state.
    conditional: bool,
    /// `canvas`, `text` or `always`.
    scope: String,
    /// The row as written, for messages.
    text: String,
}

/// What one binding declares, per chord.
#[derive(Debug, Clone, PartialEq)]
struct Declared {
    scope: &'static str,
    args: Value,
    /// Which binding of the command this chord is in, for messages.
    chord: String,
}

fn backticks(cell: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = cell;
    while let Some(a) = rest.find('`') {
        let after = &rest[a + 1..];
        let Some(b) = after.find('`') else { break };
        out.push(after[..b].to_owned());
        rest = &after[b + 1..];
    }
    out
}

const ARROWS: [&str; 4] = ["Left", "Right", "Up", "Down"];

/// A chord as the table or a binding writes it, as one or more canonical chords.
fn expand(token: &str) -> Vec<String> {
    let token = token.trim();
    if let Some(prefix) = token.strip_suffix("Arrow keys") {
        return ARROWS.iter().map(|d| canonical(&format!("{prefix}{d}"))).collect();
    }
    if let Some(i) = token.find("Left/Right/Up/Down") {
        let prefix = &token[..i];
        return ARROWS.iter().map(|d| canonical(&format!("{prefix}{d}"))).collect();
    }
    vec![canonical(token)]
}

/// Modifiers in a fixed order, key lower-case, `ArrowLeft` as `left`.
fn canonical(chord: &str) -> String {
    let mut rest = chord.trim();
    let (mut m, mut c, mut a, mut sh) = (false, false, false, false);
    loop {
        if let Some(r) = rest.strip_prefix("Mod+") {
            m = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("Ctrl+") {
            c = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("Alt+") {
            a = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("Shift+") {
            sh = true;
            rest = r;
        } else {
            break;
        }
    }
    let key = rest.to_lowercase();
    let key = key.strip_prefix("arrow").filter(|k| ARROWS.iter().any(|d| d.eq_ignore_ascii_case(k))).unwrap_or(&key);
    format!(
        "{}{}{}{}{}",
        if m { "mod+" } else { "" },
        if c { "ctrl+" } else { "" },
        if a { "alt+" } else { "" },
        if sh { "shift+" } else { "" },
        key
    )
}

fn id_shape(s: &str) -> bool {
    let mut parts = s.split('.');
    let (Some(a), Some(b), None) = (parts.next(), parts.next(), parts.next()) else { return false };
    let ok = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase() || c == '_');
    ok(a) && ok(b)
}

/// The `key=value` pairs in the text of a command cell, outside the backticked ids. Bare words after a pair
/// (`op=forward, backward`) are more values of that key.
fn parse_args(cell: &str) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut key: Option<String> = None;
    let flat = cell.replace('`', "").replace(',', " ");
    for w in flat.split_whitespace() {
        if let Some((k, v)) = w.split_once('=') {
            key = Some(k.to_owned());
            out.entry(k.to_owned()).or_default().push(v.to_owned());
        } else if let Some(k) = &key {
            if w.chars().all(|c| c.is_ascii_lowercase() || c == '_') && !id_shape(w) {
                out.entry(k.clone()).or_default().push(w.to_owned());
            } else {
                key = None;
            }
        }
    }
    out
}

fn parse_row(line: &str) -> Option<Row> {
    let cells = split_row(line)?;
    let [keys, cmd, when, ..] = cells.as_slice() else { return None };
    if !cmd.contains('`') || keys.starts_with("---") || keys.starts_with("Keys") {
        return None;
    }
    let (mac_text, other_text) = match (keys.find("mac "), keys.find("/ other")) {
        (Some(m), Some(o)) if m < o => (&keys[m..o], &keys[o..]),
        _ => (keys.as_str(), keys.as_str()),
    };
    let chords = |t: &str| backticks(t).iter().flat_map(|c| expand(c)).collect::<Vec<_>>();
    let ids: Vec<String> = backticks(cmd).into_iter().filter(|t| id_shape(t)).collect();
    // `$cursor` and friends are backticked values, not ids; the id filter drops them. A UI verb is not an id here.
    let ids = ids.into_iter().filter(|i| !cmd.contains(&format!("UI verb `{i}`"))).collect();
    Some(Row {
        mac: chords(mac_text),
        other: chords(other_text),
        ids,
        args: parse_args(cmd),
        conditional: cmd.contains(" else ") || cmd.contains(" while "),
        scope: when.split_whitespace().next().unwrap_or("").to_owned(),
        text: line.trim().to_owned(),
    })
}

fn parse_keymap(md: &str) -> Vec<Row> {
    section(md, KEYMAP_HEADING, KEYMAP_END_PREFIX).into_iter().filter_map(parse_row).collect()
}

fn scope_word(s: Scope) -> &'static str {
    match s {
        Scope::Canvas => "canvas",
        Scope::Text => "text",
        Scope::Always => "always",
    }
}

/// The chords a command declares for one platform.
fn declared(spec: &CommandSpec, mac: bool) -> BTreeMap<String, Declared> {
    let mut out = BTreeMap::new();
    for b in spec.shortcuts {
        let args: Value =
            if b.args.is_empty() { Value::Null } else { serde_json::from_str(b.args).unwrap_or(Value::Null) };
        for raw in b.chords(mac) {
            for chord in expand(raw) {
                out.insert(chord, Declared { scope: scope_word(b.when), args: args.clone(), chord: (*raw).to_owned() });
            }
        }
    }
    out
}

fn arg_matches(actual: Option<&Value>, want: &str) -> bool {
    match actual {
        Some(Value::String(s)) => s == want,
        Some(Value::Bool(b)) => (if *b { "true" } else { "false" }) == want,
        Some(Value::Number(n)) => n.to_string().as_str() == want,
        Some(_) => false,
        None => false,
    }
}

/// Every difference between the table and the registrations, in words.
fn differences(rows: &[Row], specs: &[&CommandSpec]) -> Vec<String> {
    let by_id: BTreeMap<&str, &CommandSpec> = specs.iter().map(|s| (s.id, *s)).collect();
    let mut problems = Vec::new();
    for mac in [false, true] {
        let plat = if mac { "macOS" } else { "other platforms" };
        // Chords the table gives each registered command, with the row they come from.
        let mut wanted: BTreeMap<&str, BTreeMap<String, &Row>> = BTreeMap::new();
        for row in rows {
            let chords = if mac { &row.mac } else { &row.other };
            let registered: Vec<&str> = row.ids.iter().map(String::as_str).filter(|i| by_id.contains_key(i)).collect();
            if registered.is_empty() {
                continue;
            }
            for chord in chords {
                let holders: Vec<&str> = registered
                    .iter()
                    .copied()
                    .filter(|id| by_id.get(id).is_some_and(|s| declared(s, mac).contains_key(chord)))
                    .collect();
                if row.conditional {
                    if holders.is_empty() {
                        problems.push(format!(
                            "{plat}: §6.1 binds `{chord}` to one of {} but none of them declares it: {}",
                            registered.join(", "),
                            row.text
                        ));
                    }
                    for id in &holders {
                        wanted.entry(id).or_default().insert(chord.clone(), row);
                    }
                } else {
                    for id in &registered {
                        wanted.entry(id).or_default().insert(chord.clone(), row);
                    }
                }
            }
        }
        for (id, spec) in &by_id {
            let have = declared(spec, mac);
            let want = wanted.get(id).cloned().unwrap_or_default();
            for (chord, d) in &have {
                match want.get(chord) {
                    None => problems.push(format!(
                        "{plat}: `{id}` declares the shortcut `{}` but §6.1 has no row for it. Add the row or remove the shortcut",
                        d.chord
                    )),
                    Some(row) => {
                        if row.scope != d.scope {
                            problems.push(format!(
                                "{plat}: `{id}` binds `{}` in scope `{}` but §6.1 says `{}`",
                                d.chord, d.scope, row.scope
                            ));
                        }
                        let list = if mac { &row.mac } else { &row.other };
                        for (key, values) in &row.args {
                            let expected = match values.as_slice() {
                                [one] => Some(one.as_str()),
                                many if many.len() == list.len() => {
                                    list.iter().position(|c| c == chord).and_then(|i| many.get(i)).map(String::as_str)
                                }
                                _ => None,
                            };
                            let Some(expected) = expected.filter(|e| !e.starts_with('$')) else { continue };
                            if !arg_matches(d.args.get(key), expected) {
                                problems.push(format!(
                                    "{plat}: `{id}` binds `{}` without {key}={expected} (binding args: {})",
                                    d.chord, d.args
                                ));
                            }
                        }
                    }
                }
            }
            for chord in want.keys().filter(|c| !have.contains_key(*c)) {
                problems.push(format!(
                    "{plat}: §6.1 binds `{chord}` to `{id}` but `{id}` declares no such shortcut. Add it to the command's shortcuts"
                ));
            }
        }
    }
    problems.sort();
    problems.dedup();
    problems
}

fn registered() -> Vec<&'static CommandSpec> {
    Registry::global().all().to_vec()
}

#[test]
fn the_registered_shortcuts_are_the_keymap_of_the_catalog() {
    let md = std::fs::read_to_string(COMMANDS_MD).unwrap();
    let rows = parse_keymap(&md);
    assert!(rows.len() > 40, "§6.1 should hold dozens of rows, parsed {}", rows.len());
    let problems = differences(&rows, &registered());
    assert!(problems.is_empty(), "the keymap drifted:\n{}", problems.join("\n"));
}

#[test]
fn the_comparison_covers_registered_commands() {
    // A guard against a parser that silently matches nothing: these registered commands are in §6.1.
    let md = std::fs::read_to_string(COMMANDS_MD).unwrap();
    let rows = parse_keymap(&md);
    let in_table: BTreeSet<&str> = rows.iter().flat_map(|r| r.ids.iter().map(String::as_str)).collect();
    let specs = registered();
    let compared: Vec<&str> =
        specs.iter().filter(|s| in_table.contains(s.id) && !s.shortcuts.is_empty()).map(|s| s.id).collect();
    for id in ["history.undo", "history.redo", "view.zoom_in", "view.zoom_to_fit", "view.scroll_page"] {
        assert!(compared.contains(&id), "`{id}` should be compared with §6.1, compared: {compared:?}");
    }
}

// ---- the parser and the comparison themselves ----

fn row(line: &str) -> Row {
    parse_row(line).unwrap_or_else(|| panic!("not a row: {line}"))
}

#[test]
fn rows_are_parsed_into_chords_ids_arguments_and_scope() {
    let r = row("| `Mod+Shift+Z`, `Mod+Y` | `history.redo` | canvas | A:4944 |");
    assert_eq!(r.other, ["mod+shift+z", "mod+y"]);
    assert_eq!((r.ids.as_slice(), r.scope.as_str()), (["history.redo".to_owned()].as_slice(), "canvas"));

    let r = row("| `Shift+2` `Shift+3` | `view.zoom_to_fit` scope=selection | canvas | NEW |");
    assert_eq!(r.args["scope"], ["selection"]);

    let r = row("| `Mod+Shift+Left/Right/Up/Down` | `arrange.align` edge=left, right, top, bottom | canvas | NEW |");
    assert_eq!(r.other, ["mod+shift+left", "mod+shift+right", "mod+shift+up", "mod+shift+down"]);
    assert_eq!(r.args["edge"], ["left", "right", "top", "bottom"]);

    let r = row("| `Shift+Arrow keys` | `element.nudge` dir, fine=true | canvas | NEW |");
    assert_eq!(r.other, ["shift+left", "shift+right", "shift+up", "shift+down"]);
    assert_eq!(r.args["fine"], ["true"]);

    let r = row(
        "| mac `Mod+Alt+]` `Mod+Alt+[` / other `Mod+Shift+]` `Mod+Shift+[` | `arrange.z_order` op=front, back | canvas | NEW |",
    );
    assert_eq!(r.mac, ["mod+alt+]", "mod+alt+["]);
    assert_eq!(r.other, ["mod+shift+]", "mod+shift+["]);

    let r = row(
        "| `Delete`, `Backspace` | `arrow.delete_point` while the point editor has a selected point, else `element.delete` | canvas | A:5299 |",
    );
    assert!(r.conditional);
    assert_eq!(r.ids, ["arrow.delete_point", "element.delete"]);

    let r = row("| `Mod+V` | `clipboard.paste` at=`$cursor` | canvas | A:5106 |");
    assert_eq!(
        (r.ids.as_slice(), r.args["at"].as_slice()),
        (["clipboard.paste".to_owned()].as_slice(), ["$cursor".to_owned()].as_slice())
    );

    let r = row("| `Mod+O` | UI verb `ui.open_file` | always | A:4790 |");
    assert!(r.ids.is_empty(), "a UI verb is not a command");

    assert!(parse_row("|---|---|---|---|").is_none());
    assert!(parse_row("| Keys (mac / other) | Command and fixed args | When | Source |").is_none());
    assert_eq!(canonical("Mod++"), "mod++");
    assert_eq!(canonical("Shift+ArrowLeft"), "shift+left");
}

#[test]
fn the_comparison_names_every_kind_of_drift() {
    use crate::spec::{Binding, Facts, NoArgs, Typed};
    fn noop(_: &mut crate::Session, _: NoArgs) -> Result<Value, crate::Refusal> {
        Ok(Value::Null)
    }
    const KEYS_A: &[Binding] = &[Binding::new(&["Mod+Z"], Scope::Canvas), Binding::new(&["Mod+Q"], Scope::Canvas)];
    const KEYS_B: &[Binding] = &[Binding::new(&["Mod+Shift+Z"], Scope::Always).with_args(r#"{"op":"back"}"#)];
    static A: CommandSpec = CommandSpec::new(
        "k.a",
        "k",
        "A",
        "A synthetic command for the keymap test.",
        &Typed::new(noop),
        Facts::READ_ONLY,
    )
    .shortcuts(KEYS_A);
    static B: CommandSpec = CommandSpec::new(
        "k.b",
        "k",
        "B",
        "A synthetic command for the keymap test.",
        &Typed::new(noop),
        Facts::READ_ONLY,
    )
    .shortcuts(KEYS_B);
    static C: CommandSpec = CommandSpec::new(
        "k.c",
        "k",
        "C",
        "A synthetic command for the keymap test.",
        &Typed::new(noop),
        Facts::READ_ONLY,
    );
    let rows = vec![
        row("| `Mod+Z` | `k.a` | canvas | x |"),
        row("| `Mod+Shift+Z` | `k.b` op=front | canvas | x |"),
        row("| `Mod+K` | `k.c` | canvas | x |"),
        row("| `Mod+U` | `k.unregistered` | canvas | x |"),
    ];
    let p = differences(&rows, &[&A, &B, &C]).join("\n");
    assert!(p.contains("`k.a` declares the shortcut `Mod+Q` but §6.1 has no row"), "{p}");
    assert!(p.contains("`k.b` binds `Mod+Shift+Z` in scope `always` but §6.1 says `canvas`"), "{p}");
    assert!(p.contains("`k.b` binds `Mod+Shift+Z` without op=front"), "{p}");
    assert!(p.contains("§6.1 binds `mod+k` to `k.c` but `k.c` declares no such shortcut"), "{p}");
    assert!(!p.contains("unregistered"), "unregistered commands are skipped: {p}");
    assert!(!p.contains("`k.a` binds `Mod+Z`"), "a matching binding is not reported: {p}");

    // A conditional row is satisfied by any one registered command that has the chord.
    let rows = vec![row("| `Mod+Z` | `k.a` while editing, else `k.c` | canvas | x |")];
    let p = differences(&rows, &[&A, &C]).join("\n");
    assert!(!p.contains("k.c"), "{p}");
    assert!(p.contains("`k.a` declares the shortcut `Mod+Q`"), "{p}");
}
