#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The state-change fence for `crates/ui` and `crates/app`.
//!
//! Everything a user can do is a command, so these two crates may change the session, a tab, a document or a
//! store only by `execute`, `execute_from`, `execute_typed` and `execute_batch`, by the gesture fast path of
//! `docs/exceptions.md`, or through an entry of `allowlist.txt`. This test reads the source of both crates with
//! `syn` (see `checker.rs` for what counts as a state change) and fails on anything else.
//!
//! The allowlist is the one list of exceptions. Each entry has a reason. It can only shrink: the test fails when an
//! entry matches nothing any more, and when the number of entries is not the ceiling below, so that adding one
//! needs a reviewed change to this file, and removing one needs the ceiling lowered.
//!
//! The negative tests feed the checker code that bypasses the fence, among it the bypass that the design audit
//! found (`s.tool.lock = true; s.tabs.truncate(1);` in the UI model). They fail when the checker stops seeing it.

mod checker;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use checker::{EngineFacts, Finding, Kind, Universe, engine_facts, rust_files, scan};

/// The number of entries in `allowlist.txt`. Lower it when an entry goes; never raise it without a review.
const CEILING: usize = 20;

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// `(path relative to crates/, source)` of every file of the two crates.
fn sources() -> Vec<(String, String)> {
    let root = crates_dir();
    ["ui/src", "app/src"]
        .iter()
        .flat_map(|dir| rust_files(&root.join(dir)))
        .map(|p| {
            let rel = p.strip_prefix(&root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
            let text = fs::read_to_string(&p).unwrap_or_else(|e| panic!("{rel}: {e}"));
            (rel, text)
        })
        .collect()
}

/// The engine's facts, read once: parsing its sources takes a while.
fn engine() -> &'static EngineFacts {
    static FACTS: OnceLock<EngineFacts> = OnceLock::new();
    FACTS.get_or_init(|| engine_facts(&["engine/src", "scene/src", "doc/src"].map(|d| crates_dir().join(d))))
}

fn findings() -> Vec<Finding> {
    let files = sources();
    let engine = engine();
    let universe = Universe::of(&files);
    files
        .iter()
        .flat_map(|(name, src)| scan(name, src, engine, &universe).unwrap_or_else(|e| panic!("{name}: {e}")))
        .collect()
}

/// One line of `allowlist.txt`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    file: String,
    kind: String,
    name: String,
    in_fn: String,
    gate: String,
    reason: String,
}

impl Entry {
    fn matches(&self, f: &Finding) -> bool {
        self.file == f.file
            && self.kind == f.kind.word()
            && self.name == f.name
            && (self.in_fn == "*" || self.in_fn == f.in_fn)
    }
}

fn parse_allowlist(text: &str) -> Result<Vec<Entry>, String> {
    let mut out = Vec::new();
    for (no, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.splitn(6, '|').map(str::trim).collect();
        let [file, kind, name, in_fn, gate, reason] = parts.as_slice() else {
            return Err(format!("line {}: expected `file | kind | name | in_fn | gate | reason`", no + 1));
        };
        if reason.split_whitespace().count() < 4 {
            return Err(format!("line {}: the reason must say why, in a sentence", no + 1));
        }
        if !matches!(*gate, "-" | "scenarios") {
            return Err(format!("line {}: the gate is `-` or `scenarios`", no + 1));
        }
        out.push(Entry {
            file: (*file).into(),
            kind: (*kind).into(),
            name: (*name).into(),
            in_fn: (*in_fn).into(),
            gate: (*gate).into(),
            reason: (*reason).into(),
        });
    }
    Ok(out)
}

fn allowlist() -> Vec<Entry> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fences/allowlist.txt");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    parse_allowlist(&text).unwrap_or_else(|e| panic!("allowlist.txt: {e}"))
}

fn describe(f: &Finding) -> String {
    format!("{} | {} | {} | {} | - | <reason>", f.file, f.kind.word(), f.name, f.in_fn)
}

/// Findings that no entry covers.
fn unlisted<'a>(found: &'a [Finding], list: &[Entry]) -> Vec<&'a Finding> {
    found.iter().filter(|f| !list.iter().any(|e| e.matches(f))).collect()
}

#[test]
fn ui_and_app_change_state_only_through_commands_the_gesture_path_and_the_listed_exceptions() {
    let found = findings();
    let rest = unlisted(&found, &allowlist());
    assert!(
        rest.is_empty(),
        "these places can change session, tab, document or store state outside a command. Run it as a command \
         (`Session::execute_from`), or, if it truly is not a user action, add a line with a reason to \
         crates/app/tests/fences/allowlist.txt:\n  {}",
        rest.iter().map(|f| describe(f)).collect::<Vec<_>>().join("\n  ")
    );
}

#[test]
fn every_allowlist_entry_still_matches_something_and_is_gated_as_it_says() {
    let found = findings();
    for e in allowlist() {
        let hits: Vec<&Finding> = found.iter().filter(|f| e.matches(f)).collect();
        assert!(!hits.is_empty(), "stale entry, remove it (and lower CEILING): {e:?}");
        if e.gate == "scenarios" {
            assert!(
                hits.iter().all(|f| f.scenarios_only),
                "{e:?} says `scenarios`, but the code is not behind #[cfg(feature = \"scenarios\")]"
            );
        }
    }
}

#[test]
fn the_allowlist_can_only_shrink() {
    let list = allowlist();
    let unique: BTreeSet<(&str, &str, &str, &str)> =
        list.iter().map(|e| (e.file.as_str(), e.kind.as_str(), e.name.as_str(), e.in_fn.as_str())).collect();
    assert_eq!(unique.len(), list.len(), "an entry is listed twice");
    assert_eq!(
        list.len(),
        CEILING,
        "allowlist.txt has {} entries and the ceiling is {CEILING}. Fewer entries: lower CEILING in main.rs. More: \
         a state change outside commands needs a reviewed reason and a new ceiling",
        list.len()
    );
}

#[test]
fn the_state_types_exist_in_the_engine() {
    let facts = engine();
    let missing: Vec<&&str> = checker::STATE_TYPES.iter().filter(|t| !facts.declared.contains(**t)).collect();
    assert!(missing.is_empty(), "the engine no longer declares {missing:?}: update STATE_TYPES in checker.rs");
    for known in ["tab_mut", "set_active_index", "clear_history", "queue_write", "undo_steps", "mark_dirty"] {
        assert!(facts.mutators.contains(known), "`{known}` is a mutator of the session; the derivation lost it");
    }
    for known in ["tab", "tab_mut", "store"] {
        assert!(facts.getters.contains(known), "`{known}` hands out state; the derivation lost it");
    }
}

// ---- the negative fixtures -------------------------------------------------------------------------------------

/// Scans `src` as if it were a file of the UI, against the real engine.
fn scan_fixture(src: &str) -> Vec<Finding> {
    let files = vec![("ui/src/fixture.rs".to_owned(), src.to_owned())];
    let universe = Universe::of(&files);
    scan("ui/src/fixture.rs", src, engine(), &universe).unwrap_or_else(|e| panic!("fixture: {e}"))
}

fn kinds(found: &[Finding]) -> Vec<(Kind, &str)> {
    found.iter().map(|f| (f.kind, f.name.as_str())).collect()
}

/// The bypass of the design audit: three writes added to `ui/src/model.rs`, which the text grep of
/// `crates/ui/tests/thin.rs` did not see.
#[test]
fn the_bypass_the_audit_found_is_caught() {
    let found = scan_fixture(
        r#"
        impl Model {
            pub fn refresh(&mut self, s: &mut Session) -> bool {
                s.tool.lock = true;
                s.tabs.truncate(1);
                s.tool.active = Tool::Hand;
                true
            }
        }
        "#,
    );
    let k = kinds(&found);
    assert!(k.contains(&(Kind::Assign, "lock")), "{k:?}");
    assert!(k.contains(&(Kind::Method, "truncate")), "{k:?}");
    assert!(k.contains(&(Kind::Assign, "active")), "{k:?}");
}

#[test]
fn each_way_around_the_fence_is_caught() {
    let cases: &[(&str, Kind, &str)] = &[
        ("fn f(s: &mut Session) { s.tab_mut(); }", Kind::Method, "tab_mut"),
        ("fn f(s: &mut Session) { s.set_active_index(1); }", Kind::Method, "set_active_index"),
        ("fn f(s: &mut Session) { s.clear_history(); }", Kind::Method, "clear_history"),
        ("fn f(s: &mut Session) { let t = s.tab_mut(); t.saved_revision = 3; }", Kind::Assign, "saved_revision"),
        ("fn f(s: &mut Session) { let d = &mut s.tab_mut().doc; d.touch(); }", Kind::MutBorrow, "doc"),
        ("fn f(s: &mut Session) { s.settings.set(\"a\", 1); }", Kind::Method, "set"),
        ("fn f(s: &mut Session) { s.tabs[0].doc = other(); }", Kind::Assign, "doc"),
        ("fn f(s: &mut Session) { s.view_only |= true; }", Kind::Assign, "view_only"),
        ("fn f(s: &mut Session) { Session::clear_history(s); }", Kind::PathCall, "clear_history"),
        ("fn f(s: &mut Session) { Session::clear_history(&mut *s); }", Kind::PathCall, "clear_history"),
        ("fn f(s: &mut Session) { elsewhere::poke(&mut s.tool); }", Kind::MutBorrow, "tool"),
        ("fn f(s: &mut Session) { elsewhere::poke(s); }", Kind::HandedOut, "poke"),
        ("fn f(s: &mut Session) { helper.poke(&mut *s); }", Kind::HandedOut, "poke"),
        ("fn f(s: &mut Session) { sneaky!(s, 1); }", Kind::Macro, "sneaky"),
        ("fn f(s: &mut Session) { println!(\"{}\", s.tabs.pop().is_some()); }", Kind::Method, "pop"),
        ("fn f(t: &mut Tab) { t.doc.rollback_to(1); }", Kind::Method, "rollback_to"),
        ("struct App { session: Session } fn f(app: &mut App) { app.session.tabs.clear(); }", Kind::Method, "clear"),
        ("fn f<S: CommandSink>(sink: &mut S) { sink.tabs.clear(); }", Kind::Method, "clear"),
        (
            "struct App { session: Session } impl App { fn f(&mut self) { self.session.tool.lock = false; } }",
            Kind::Assign,
            "lock",
        ),
        (
            "struct App { session: Session } impl App { fn f(&mut self) { self.session.queue_write(w()); } }",
            Kind::Method,
            "queue_write",
        ),
        (
            "struct App { session: Session } impl App { fn f(&mut self) { host.poll(&mut self.session); } }",
            Kind::HandedOut,
            "poll",
        ),
    ];
    for (src, kind, name) in cases {
        let found = scan_fixture(src);
        assert!(
            found.iter().any(|f| f.kind == *kind && f.name == *name),
            "not caught: {src}\n  found: {:?}",
            kinds(&found)
        );
    }
}

#[test]
fn commands_reads_and_unrelated_code_are_not_findings() {
    let found = scan_fixture(
        r#"
        struct Model { rows: Vec<u32>, scratch: Vec<u32> }
        impl Model {
            fn refresh(&mut self, s: &mut Session) -> bool {
                let n = s.tabs.len();
                let name = s.tab().doc.store().meta().name.clone();
                let tool = s.tool.active;
                let on = s.settings.get("a").and_then(|v| v.as_bool()).unwrap_or(false);
                self.rows.clear();
                self.rows.push(n as u32);
                self.scratch.truncate(0);
                let mut local = Vec::new();
                local.push(name);
                let _ = (tool, on);
                s.execute_from(Origin::Ui, "view.zoom_in", json!({}));
                s.execute("tool.set", json!({}));
                s.execute_typed(Origin::Ui, "view.set_viewport", &args);
                s.execute_batch(steps);
                self.read(s);
                println!("{}", s.tabs.len());
                n > 1
            }
            fn read(&mut self, s: &mut Session) {}
        }
        "#,
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn an_allowlist_entry_covers_only_what_it_names() {
    let found =
        scan_fixture("fn a(s: &mut Session) { s.clear_history(); }\nfn b(s: &mut Session) { s.clear_history(); }");
    let list = parse_allowlist(
        "ui/src/fixture.rs | call | clear_history | a | - | a fixture that clears history on purpose here",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let rest = unlisted(&found, &list);
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0].in_fn, "b");
}

#[test]
fn a_malformed_allowlist_line_is_refused() {
    assert!(parse_allowlist("a | call | b | c | - | ").is_err(), "no reason");
    assert!(parse_allowlist("a | call | b | c | - | short").is_err(), "reason too short");
    assert!(parse_allowlist("a | call | b | c | maybe | a reason with enough words in it").is_err(), "bad gate");
    assert!(parse_allowlist("a | call | b").is_err(), "too few fields");
    assert!(parse_allowlist("# only a comment\n\n").unwrap_or_default().is_empty());
}

#[test]
fn a_scenarios_only_function_is_recognised() {
    let found = scan_fixture(
        "#[cfg(feature = \"scenarios\")]\nfn a(s: &mut Session) { s.clear_history(); }\nfn b(s: &mut Session) { s.clear_history(); }",
    );
    let a = found.iter().find(|f| f.in_fn == "a").map(|f| f.scenarios_only);
    let b = found.iter().find(|f| f.in_fn == "b").map(|f| f.scenarios_only);
    assert_eq!((a, b), (Some(true), Some(false)));
}
