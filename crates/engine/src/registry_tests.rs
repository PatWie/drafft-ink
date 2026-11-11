//! Registry-wide tests (docs/commands.md §1.2, "Registry tests"). They run over every registered command,
//! so a new family is covered the moment its name is added to `cmd/mod.rs`.
//!
//! The catalog tests (`catalog_matches_registry`, the facts of Appendix A, declared refusal codes) are in
//! `catalog_tests.rs`. The layering test is `tests/layering.rs`.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::refusal::is_shared_code;
use crate::registry::Registry;
use crate::session::Session;
use crate::spec::{Batch, CommandSpec, DocWrite, Gate, Mutates, Scope, Selection, Status, TextGate, Undo};
use crate::testing;

fn specs() -> Vec<&'static CommandSpec> {
    Registry::global().all().to_vec()
}

fn fixture() -> Session {
    testing::rich_session().unwrap()
}

// ---- identity and facts ----

#[test]
fn ids_are_unique_and_well_formed() {
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for s in specs() {
        *seen.entry(s.id).or_default() += 1;
        let (fam, verb) = s.id.split_once('.').unwrap_or_else(|| panic!("`{}` has no dot", s.id));
        assert!(!fam.is_empty() && fam.bytes().all(|b| b.is_ascii_lowercase()), "family part of `{}`", s.id);
        assert!(
            !verb.is_empty() && verb.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
            "verb part of `{}`",
            s.id
        );
        assert_eq!(s.family, fam, "`{}` declares family `{}`", s.id, s.family);
    }
    let dups: Vec<&&str> = seen.iter().filter(|(_, n)| **n > 1).map(|(id, _)| id).collect();
    assert!(dups.is_empty(), "duplicate ids: {dups:?}");
}

#[test]
fn words_are_plain() {
    for s in specs() {
        assert!(
            !s.title.is_empty() && s.title.chars().next().is_some_and(char::is_uppercase),
            "title of `{}`: {:?}",
            s.id,
            s.title
        );
        assert!(!s.title.ends_with('.'), "title of `{}` ends with a period", s.id);
        assert!(
            s.summary.len() >= 20 && s.summary.ends_with('.'),
            "summary of `{}` must be a sentence: {:?}",
            s.id,
            s.summary
        );
        for b in s.shortcuts {
            if let Some(t) = b.title {
                assert!(t.chars().next().is_some_and(char::is_uppercase), "binding title {t:?} of `{}`", s.id);
            }
        }
    }
}

#[test]
fn facts_are_consistent() {
    for s in specs() {
        let id = s.id;
        if s.reads_only {
            assert!(s.mutates.is_empty(), "`{id}` is reads_only but mutates {:?}", s.mutates);
            assert_eq!(s.undo, Undo::NoUndo, "`{id}` is reads_only, so it has no undo");
            assert_eq!(s.view_only, Gate::Allowed, "`{id}` is reads_only, so it runs in view-only mode");
            assert_eq!(s.while_gesture, Gate::Allowed, "`{id}` is reads_only, so it runs during a gesture");
            assert_ne!(s.while_text_edit, TextGate::Refused, "`{id}` is reads_only, so it runs during a text edit");
        } else {
            assert!(!s.mutates.is_empty(), "`{id}` is not reads_only but declares no state class it changes");
        }
        // The document rules come from the declared `doc_write`, not from a list of ids.
        assert_eq!(
            s.mutates.contains(Mutates::DOC),
            s.doc_write != DocWrite::NotDocument,
            "`{id}` declares doc_write `{}` but mutates {:?}",
            s.doc_write.name(),
            s.mutates
        );
        match s.doc_write {
            DocWrite::NotDocument => {}
            DocWrite::Edit => {
                assert_eq!(
                    s.view_only,
                    Gate::Refused,
                    "`{id}` edits the document, so it must refuse in view-only mode"
                );
                assert_ne!(s.undo, Undo::NoUndo, "`{id}` edits the document, so it needs an undo class");
            }
            DocWrite::Replaces => {
                assert_eq!(s.view_only, Gate::Refused, "`{id}` replaces the document, so it refuses in view-only mode");
                assert_ne!(s.undo, Undo::Gesture, "`{id}` replaces the document, so it is not a gesture");
            }
            DocWrite::Managed => {}
        }
        if s.undo == Undo::Gesture {
            assert_eq!(s.doc_write, DocWrite::Edit, "`{id}` is a gesture command, so it is an ordinary edit");
        }
        if s.coalesce.is_some() {
            assert_ne!(s.undo, Undo::NoUndo, "`{id}` coalesces undo steps but has no undo");
        }
        if s.needs_selection == Selection::Required {
            assert!(!s.reads_only || s.family == "selection", "`{id}` needs a selection but is read-only");
        }
        if s.is_own_transaction() {
            assert!(
                matches!(s.doc_write, DocWrite::Managed | DocWrite::NotDocument),
                "`{id}` manages its own transaction, so it is not an ordinary edit or a replacement (doc_write `{}`)",
                s.doc_write.name()
            );
        }
        if s.own_agent_tool {
            assert!(s.agent && s.reads_only, "`{id}` has an agent tool of its own, so it is a read-only agent command");
        }
        if s.host.text_editor_takes_style {
            assert_eq!(
                s.while_text_edit,
                TextGate::Refused,
                "`{id}`: the editor takes the style only where the command refuses"
            );
        }
        if let Status::Planned(reason) = s.status {
            assert!(reason.len() > 8, "the reason for planned `{id}` must say something");
        }
    }
}

/// `Executor::commit_text_edit` runs `text.edit_end`; the command must therefore not commit a text edit itself.
#[test]
fn the_command_that_commits_a_text_edit_runs_during_one() {
    let spec = Registry::global().find(crate::executor::TEXT_EDIT_END).unwrap();
    assert_eq!(spec.while_text_edit, TextGate::Allowed);
}

#[test]
fn titles_are_unique() {
    let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
    for s in specs() {
        if let Some(prev) = seen.insert(s.title, s.id) {
            panic!("`{}` and `{}` share the title {:?}", prev, s.id, s.title);
        }
    }
}

/// A chord in a canonical form: modifiers in a fixed order, key lower-case.
fn canonical(chord: &str) -> String {
    let mut rest = chord;
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
    format!(
        "{}{}{}{}{}",
        if m { "mod+" } else { "" },
        if c { "ctrl+" } else { "" },
        if a { "alt+" } else { "" },
        if sh { "shift+" } else { "" },
        rest.to_lowercase()
    )
}

#[test]
fn default_shortcuts_do_not_collide_within_a_scope() {
    for mac in [false, true] {
        let mut seen: Vec<(Scope, String, String)> = Vec::new();
        for s in specs() {
            for b in s.shortcuts {
                for chord in b.chords(mac) {
                    let key = canonical(chord);
                    for (scope, other_key, other_id) in &seen {
                        let overlap = *scope == b.when || *scope == Scope::Always || b.when == Scope::Always;
                        assert!(
                            !(overlap && *other_key == key),
                            "`{}` and `{}` both bind {chord} ({}) for scope {:?} / {:?}",
                            other_id,
                            s.id,
                            if mac { "mac" } else { "other" },
                            scope,
                            b.when
                        );
                    }
                    seen.push((b.when, key, s.id.to_owned()));
                }
            }
        }
    }
}

#[test]
fn shortcut_arguments_are_valid_for_their_command() {
    for s in specs() {
        for b in s.shortcuts {
            assert!(!b.keys.is_empty(), "a binding of `{}` has no keys", s.id);
            if b.args.is_empty() {
                continue;
            }
            let v: Value =
                serde_json::from_str(b.args).unwrap_or_else(|e| panic!("binding args of `{}` are not JSON: {e}", s.id));
            if let Err(r) = s.handler.parse(s.id, &v) {
                panic!("binding args {v} of `{}` do not parse: {}", s.id, r.message);
            }
        }
    }
}

// ---- schemas and examples ----

#[test]
fn every_command_publishes_closed_argument_and_output_schemas() {
    for s in specs() {
        let args = (s.args)().to_value();
        assert_eq!(args.get("type"), Some(&json!("object")), "args schema of `{}` is not an object", s.id);
        assert_eq!(
            args.get("additionalProperties"),
            Some(&json!(false)),
            "args of `{}` must use #[serde(deny_unknown_fields)]: unknown keys have to be refused",
            s.id
        );
        let out = (s.output)().to_value();
        assert!(out.is_object() || out.is_boolean(), "output schema of `{}`", s.id);
    }
}

#[test]
fn examples_parse_and_agent_commands_with_required_arguments_have_one() {
    for s in specs() {
        if !s.example.is_empty() {
            let v: Value =
                serde_json::from_str(s.example).unwrap_or_else(|e| panic!("example of `{}` is not JSON: {e}", s.id));
            if let Err(r) = s.handler.parse(s.id, &v) {
                panic!("example {v} of `{}` does not parse: {}", s.id, r.message);
            }
        }
        let required = crate::args::schema_required(&(s.args)());
        if s.agent && !required.is_empty() {
            assert!(
                !s.example.is_empty(),
                "`{}` is for agents and has required arguments {required:?}: give it an `.example(...)`",
                s.id
            );
        }
    }
}

#[test]
fn engine_describe_and_commands_cover_every_command() {
    let mut s = fixture();
    let all = s.execute("engine.commands", json!({"include_schemas": true})).unwrap();
    let listed = all["commands"].as_array().unwrap();
    assert_eq!(listed.len(), specs().len());
    for spec in specs() {
        let d = s.execute("engine.describe", json!({"id": spec.id})).unwrap();
        assert_eq!(d["id"], spec.id);
        assert!(d["args_schema"].is_object() && d["refusals"].is_array() && d["example"].is_object(), "{}", spec.id);
        assert!(listed.iter().any(|c| c["id"] == spec.id), "{} missing from engine.commands", spec.id);
    }
    let r = s.execute("engine.describe", json!({"id": "view.pann"})).unwrap_err();
    assert_eq!(r.code, "unknown_command");
    assert!(r.message.contains("`view.pan`"), "{}", r.message);
}

// ---- hostile input ----

fn hostile_values() -> Vec<Value> {
    let long = "x".repeat(5000);
    let deep = (0..60).fold(json!(1), |acc, _| json!([acc]));
    vec![
        json!(null),
        json!(true),
        json!(false),
        json!(0),
        json!(-1),
        json!(1),
        json!(0.5),
        json!(1e308),
        json!(-1e308),
        json!(1e-308),
        json!(9_007_199_254_740_993_i64),
        json!(u64::MAX),
        json!(i64::MIN),
        json!(""),
        json!("x"),
        json!(long),
        json!("\u{0}"),
        json!("\n\t"),
        json!("emoji 🙂 and ünïcode"),
        json!("$name"),
        json!("@name"),
        json!("e1"),
        json!("../../etc/passwd"),
        json!([]),
        json!([null]),
        json!([[0, 0]]),
        json!([0, 0]),
        json!([1e308, -1e308]),
        json!(["e1", "e1"]),
        json!({}),
        json!({"x": 1}),
        json!({"x": 0, "y": 0, "w": -1, "h": 1e308}),
        json!({"base64": "AAAA"}),
        deep,
    ]
}

/// Keys tried on every command besides its own: they must be refused as unknown, whatever the value.
const GENERIC_KEYS: &[&str] = &["ids", "x", "at", "args"];

fn keys_for(spec: &CommandSpec) -> Vec<String> {
    let mut keys: Vec<String> = GENERIC_KEYS.iter().map(|k| (*k).to_owned()).collect();
    for k in crate::args::schema_keys(&(spec.args)()) {
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
    keys
}

/// Puts the session back into plain modes after a hostile call turned one on.
fn reset_modes(s: &mut Session) {
    s.cancel_gesture_if_running();
    s.view_only = false;
    s.text_edit = None;
    s.take_dirty();
}

/// Runs one hostile call. Returns true when the command succeeded, so the caller starts from a fresh
/// fixture: only refusals are known to leave the state alone.
fn check_call(spec: &CommandSpec, s: &mut Session, before: &mut String, args: Value) -> bool {
    match s.execute_from(crate::ring::Origin::Test, spec.id, args.clone()) {
        Ok(_) => {
            reset_modes(s);
            *before = s.fingerprint();
            true
        }
        Err(r) => {
            assert_ne!(r.code, "internal", "`{}` panicked or broke an invariant on {args}: {}", spec.id, r.message);
            assert!(
                !r.message.is_empty() && r.message.chars().count() < 1500,
                "`{}` refusal text on {args}: {:?}",
                spec.id,
                r.message
            );
            assert_eq!(r.command, spec.id, "refusal of `{}` names another command", spec.id);
            assert!(
                is_shared_code(r.code) || spec.refusals.contains(&r.code),
                "`{}` returned the code `{}` for {args}; list it in `.refusals(&[..])` and in docs/commands.md",
                spec.id,
                r.code
            );
            let after = s.fingerprint();
            assert!(after == *before, "`{}` refused {args} with `{}` but changed state", spec.id, r.code);
            reset_modes(s);
            false
        }
    }
}

#[test]
fn hostile_params_never_panic() {
    let values = hostile_values();
    for spec in specs() {
        let mut s = fixture();
        let mut before = s.fingerprint();
        for v in &values {
            if check_call(spec, &mut s, &mut before, v.clone()) {
                s = fixture();
                before = s.fingerprint();
            }
        }
        for key in keys_for(spec) {
            for v in &values {
                if check_call(spec, &mut s, &mut before, json!({ key.clone(): v.clone() })) {
                    s = fixture();
                    before = s.fingerprint();
                }
            }
        }
    }
}

#[test]
fn a_command_a_batch_may_run_anywhere_leaves_the_active_tab_alone() {
    // A batch holds one journal, the one of the tab it started in. A command that moves the active tab must be
    // `Batch::Last` (or `Never`); the executor also checks at run time, for commands that do it only sometimes.
    for spec in specs().into_iter().filter(|s| s.batch == Batch::Allowed && s.mutates.contains(Mutates::TABS)) {
        let mut s = fixture();
        s.execute("tab.new", json!({})).unwrap();
        s.execute("tab.switch", json!({"tab": 0})).unwrap();
        let before = s.tab().id;
        let args = serde_json::from_str(spec.example).unwrap_or_else(|_| json!({}));
        let _ = s.execute(spec.id, args);
        assert_eq!(s.tab().id, before, "`{}` changed the active tab but is `batch: allowed`", spec.id);
    }
}

#[test]
fn hostile_params_never_panic_in_a_batch() {
    // The same values reach commands as batch steps; preflight or the step refuses.
    let values = hostile_values();
    for spec in specs().into_iter().filter(|s| s.batch != Batch::Never) {
        for v in &values {
            let mut s = fixture();
            let before = s.fingerprint();
            let step = crate::batch::BatchStep::new(spec.id, v.clone());
            let out = s.execute_batch(crate::ring::Origin::Test, &[step], &crate::batch::BatchOpts::default());
            if let Some(f) = &out.failed {
                assert_ne!(
                    f.refusal.code, "internal",
                    "`{}` panicked in a batch on {v}: {}",
                    spec.id, f.refusal.message
                );
                assert_eq!(s.fingerprint(), before, "`{}` failed in a batch on {v} but changed state", spec.id);
            }
        }
    }
}

#[test]
fn refusal_leaves_state_unchanged() {
    for spec in specs() {
        // 1. An unknown key.
        let mut s = fixture();
        check_refusal(spec, &mut s, json!({"no_such_key_at_all": 1}), true);
        // 2. View-only mode.
        let mut s = fixture();
        s.view_only = true;
        check_refusal(spec, &mut s, json!({}), spec.view_only == Gate::Refused);
        // 3. A text edit in progress.
        let mut s = fixture();
        s.text_edit = Some(crate::state::TextEdit { element: None, draft: true });
        check_refusal(spec, &mut s, json!({}), spec.while_text_edit == TextGate::Refused);
        // 4. No selection.
        let mut s = fixture();
        s.tab_mut().observed.selection.clear();
        check_refusal(spec, &mut s, json!({}), false);
        // 5. No optional port at all.
        let mut s = testing::headless_session();
        check_refusal(spec, &mut s, json!({}), false);
        // 6. A gesture in progress, when some registered command can start one.
        if let Some(g) = specs().into_iter().find(|g| g.is_gesture()) {
            let mut s = fixture();
            let example = serde_json::from_str::<Value>(g.example).unwrap_or(json!({}));
            let began = s.execute("gesture.begin", json!({"command": g.id, "args": example})).is_ok();
            if began {
                check_refusal(spec, &mut s, json!({}), spec.while_gesture == Gate::Refused);
            }
        }
    }
}

/// Runs the command; every refusal must leave the fingerprint alone and be recorded in the ring. When
/// `must_refuse`, success is an error.
fn check_refusal(spec: &CommandSpec, s: &mut Session, args: Value, must_refuse: bool) {
    let before = s.fingerprint();
    let ring_len = s.ring().len();
    let result = s.execute_from(crate::ring::Origin::Test, spec.id, args.clone());
    match result {
        Ok(_) => assert!(!must_refuse, "`{}` should have refused {args} here", spec.id),
        Err(r) => {
            assert_ne!(r.code, "internal", "`{}` failed unexpectedly on {args}: {}", spec.id, r.message);
            assert!(s.fingerprint() == before, "`{}` refused {args} with `{}` but changed state", spec.id, r.code);
            assert!(
                s.ring().len() > ring_len || s.ring().len() == s.ring().capacity(),
                "the refusal of `{}` was not recorded in the ring",
                spec.id
            );
            let last = s.ring().iter().last();
            assert_eq!(last.and_then(|e| e.outcome.code()), Some(r.code), "ring entry of `{}`", spec.id);
        }
    }
}

#[test]
fn availability_and_execute_agree_on_state_refusals() {
    // What a greyed-out control says is what execute says.
    for spec in specs() {
        let mut s = fixture();
        s.view_only = true;
        if let Err(a) = s.availability(spec.id)
            && matches!(a.code, "view_only" | "not_supported")
        {
            let e = s.execute(spec.id, json!({})).unwrap_err();
            assert_eq!((a.code, a.message.as_str()), (e.code, e.message.as_str()), "`{}`", spec.id);
        }
    }
}
