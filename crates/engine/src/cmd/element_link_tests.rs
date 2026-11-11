//! Tests of `element.set_link` and `element.open_link`: availability, exact refusal text, effect, a negative case
//! that leaves the fingerprint alone, and undo. No UI, no DOM: a headless session and the fake ports.

use drafftink_scene::{ElementId, SceneTx, StoreError};
use serde_json::{Value, json};

use crate::refusal::Refusal;
use crate::session::Session;
use crate::testing;

fn run(s: &mut Session, id: &str, args: Value) -> Value {
    match s.execute(id, args) {
        Ok(v) => v,
        Err(r) => panic!("{id} refused: {} ({})", r.message, r.code),
    }
}

/// Refuses, and checks that the state did not change.
fn refuse(s: &mut Session, id: &str, args: Value) -> Refusal {
    let before = s.fingerprint();
    match s.execute(id, args) {
        Ok(v) => panic!("{id} should refuse, got {v}"),
        Err(r) => {
            assert_eq!(s.fingerprint(), before, "{id}: a refusal must leave the state unchanged ({})", r.message);
            r
        }
    }
}

fn eid(s: &str) -> ElementId {
    ElementId::new(s).unwrap()
}

/// Three rectangles `e1`, `e2`, `e3`; `e1` selected.
fn shapes() -> Session {
    testing::session_with_shapes().unwrap().0
}

fn link_of(s: &Session, id: &str) -> Option<String> {
    s.element(&eid(id)).unwrap().link.clone()
}

fn setup(s: &mut Session, f: impl FnOnce(&mut crate::store::DocTx<'_, '_>) -> Result<(), StoreError>) {
    s.tab_mut().doc.edit::<(), StoreError>(f).unwrap();
    s.tab_mut().doc.commit_untracked().unwrap();
}

fn set_raw_link(s: &mut Session, id: &str, link: &str) {
    let id = eid(id);
    setup(s, |tx| tx.update(&id, |e| e.link = Some(link.to_owned())));
}

fn lock_it(s: &mut Session, id: &str) {
    let id = eid(id);
    setup(s, |tx| tx.update(&id, |e| e.locked = true));
}

fn selected(s: &Session) -> Vec<String> {
    s.selection().iter().map(|i| i.as_str().to_owned()).collect()
}

// ---- element.set_link ----

#[test]
fn set_link_is_available_and_refuses_in_a_view_only_document() {
    let mut s = shapes();
    assert!(s.availability("element.set_link").is_ok());
    s.view_only = true;
    let r = s.availability("element.set_link").unwrap_err();
    assert_eq!(r.code, "view_only");
}

#[test]
fn set_link_sets_a_web_link_on_the_selected_element_and_undoes() {
    let mut s = shapes();
    let out = run(&mut s, "element.set_link", json!({"link": "HTTPS://example.com/a?b=c"}));
    assert_eq!(out["id"], "e1");
    assert_eq!(out["link"], "https://example.com/a?b=c", "the scheme is lower-cased");
    assert_eq!(out["kind"], "url");
    assert_eq!(out["changed"], true);
    assert_eq!(link_of(&s, "e1").as_deref(), Some("https://example.com/a?b=c"));
    assert_eq!(s.tab().doc.undo_depth(), 1);
    s.undo_steps(1).unwrap();
    assert_eq!(link_of(&s, "e1"), None);
    s.redo_steps(1).unwrap();
    assert_eq!(link_of(&s, "e1").as_deref(), Some("https://example.com/a?b=c"));
}

#[test]
fn set_link_adds_https_to_a_bare_address_and_accepts_mailto() {
    let mut s = shapes();
    assert_eq!(run(&mut s, "element.set_link", json!({"id": "e2", "link": "example.com/page"}))["link"], "https://example.com/page");
    assert_eq!(run(&mut s, "element.set_link", json!({"id": "e3", "link": "mailto:a@b.example"}))["link"], "mailto:a@b.example");
}

#[test]
fn set_link_with_the_same_link_is_ok_without_an_undo_step() {
    let mut s = shapes();
    run(&mut s, "element.set_link", json!({"link": "https://example.com"}));
    let out = run(&mut s, "element.set_link", json!({"link": "https://example.com"}));
    assert_eq!(out["changed"], false);
    assert_eq!(s.tab().doc.undo_depth(), 1);
}

#[test]
fn set_link_removes_a_link_with_null_absent_or_empty() {
    for removal in [json!({"link": null}), json!({}), json!({"link": ""}), json!({"link": "  "})] {
        let mut s = shapes();
        run(&mut s, "element.set_link", json!({"link": "https://example.com"}));
        let out = run(&mut s, "element.set_link", removal.clone());
        assert_eq!((out["link"].clone(), out["kind"].clone(), out["changed"].clone()), (Value::Null, Value::Null, json!(true)), "{removal}");
        assert_eq!(link_of(&s, "e1"), None);
        s.undo_steps(1).unwrap();
        assert!(link_of(&s, "e1").is_some(), "undo brings the link back");
    }
}

#[test]
fn set_link_refuses_schemes_that_run_code_or_read_files_in_plain_words() {
    let mut s = shapes();
    for bad in ["javascript:alert(1)", "JavaScript:alert(1)", "data:text/html,x", "file:///etc/passwd", "vbscript:x", "ftp://example.com"] {
        let r = refuse(&mut s, "element.set_link", json!({"link": bad}));
        assert_eq!(r.code, "invalid_link", "{bad}");
        assert!(r.message.starts_with("Links can only use http://, https:// or mailto:, not `"), "{}", r.message);
        assert!(r.message.ends_with("Use a web address such as https://example.com, or link to an element with `#id` or `@name`."), "{}", r.message);
    }
    let r = refuse(&mut s, "element.set_link", json!({"link": "javascript:alert(1)"}));
    assert_eq!(
        r.message,
        "Links can only use http://, https:// or mailto:, not `javascript:`. Use a web address such as https://example.com, or link to an element with `#id` or `@name`."
    );
}

#[test]
fn set_link_refuses_text_that_is_not_an_address() {
    let mut s = shapes();
    let r = refuse(&mut s, "element.set_link", json!({"link": "hello"}));
    assert_eq!(r.code, "invalid_link");
    assert_eq!(
        r.message,
        "`hello` is not a web address. Write it like https://example.com/page, link to an element with `#id` or `@name`, or pass null to remove the link."
    );
    assert_eq!(refuse(&mut s, "element.set_link", json!({"link": "https://a b.example"})).code, "invalid_link");
    assert_eq!(refuse(&mut s, "element.set_link", json!({"link": "https://user:pw@example.com"})).code, "invalid_link");
    let long = format!("https://example.com/{}", "a".repeat(3000));
    assert!(refuse(&mut s, "element.set_link", json!({"link": long})).message.contains("the most allowed is 2048"));
    assert_eq!(link_of(&s, "e1"), None);
}

#[test]
fn set_link_to_an_element_stores_its_id_and_follows_a_name() {
    let mut s = shapes();
    run(&mut s, "element.set_name", json!({"id": "e3", "name": "target"}));
    let out = run(&mut s, "element.set_link", json!({"link": "#e2"}));
    assert_eq!((out["link"].clone(), out["kind"].clone()), (json!("#e2"), json!("element")));
    let out = run(&mut s, "element.set_link", json!({"id": "e2", "link": "@target"}));
    assert_eq!(out["link"], "#e3", "a name is stored as the id, so renaming the target keeps the link");
    run(&mut s, "element.set_name", json!({"id": "e3", "name": "renamed"}));
    assert_eq!(link_of(&s, "e2").as_deref(), Some("#e3"));
}

#[test]
fn set_link_to_a_missing_or_deleted_element_is_refused() {
    let mut s = shapes();
    let r = refuse(&mut s, "element.set_link", json!({"link": "#nope"}));
    assert_eq!(r.code, "not_found");
    assert!(r.message.starts_with("No element matches `nope`."), "{}", r.message);
    assert_eq!(refuse(&mut s, "element.set_link", json!({"link": "@nobody"})).code, "not_found");
    run(&mut s, "element.delete", json!({"ids": ["e3"]}));
    assert_eq!(refuse(&mut s, "element.set_link", json!({"link": "#e3"})).code, "not_found");
    assert_eq!(refuse(&mut s, "element.set_link", json!({"link": "#"})).code, "invalid_link");
}

#[test]
fn set_link_needs_exactly_one_element() {
    let mut s = shapes();
    testing::select(&mut s, &[]);
    let r = refuse(&mut s, "element.set_link", json!({"link": "https://example.com"}));
    assert_eq!(r.code, "needs_single");
    assert_eq!(
        r.message,
        "`element.set_link` needs exactly one element selected, and 0 are selected. Select one element first, or pass it by id."
    );
    testing::select(&mut s, &[eid("e1"), eid("e2")]);
    assert!(refuse(&mut s, "element.set_link", json!({"link": "https://example.com"})).message.contains("and 2 are selected"));
    run(&mut s, "element.set_link", json!({"id": "e3", "link": "https://example.com"}));
    assert_eq!(link_of(&s, "e3").as_deref(), Some("https://example.com"), "an id works without a selection");
    assert_eq!(refuse(&mut s, "element.set_link", json!({"id": "nope", "link": "https://x.example"})).code, "not_found");
}

#[test]
fn set_link_refuses_a_locked_element() {
    let mut s = shapes();
    lock_it(&mut s, "e2");
    let r = refuse(&mut s, "element.set_link", json!({"id": "e2", "link": "https://example.com"}));
    assert_eq!(r.code, "locked");
    assert!(r.message.contains("`e2`"), "{}", r.message);
}

#[test]
fn set_link_does_not_touch_the_selection() {
    let mut s = shapes();
    run(&mut s, "element.set_link", json!({"id": "e2", "link": "https://example.com"}));
    assert_eq!(selected(&s), ["e1"]);
}

// ---- element.open_link ----

#[test]
fn open_link_is_greyed_out_until_some_element_has_a_link() {
    let mut s = shapes();
    let r = s.availability("element.open_link").unwrap_err();
    assert_eq!(r.code, "nothing_to_do");
    assert_eq!(r.message, "Nothing to open: no element has a link. Add one with `element.set_link`.");
    run(&mut s, "element.set_link", json!({"link": "https://example.com"}));
    assert!(s.availability("element.open_link").is_ok());
}

#[test]
fn open_link_returns_the_web_address_and_changes_nothing() {
    let mut s = shapes();
    run(&mut s, "element.set_link", json!({"link": "https://example.com/x"}));
    let before = s.fingerprint();
    s.take_dirty();
    let out = run(&mut s, "element.open_link", json!({}));
    assert_eq!((out["kind"].clone(), out["url"].clone(), out["id"].clone()), (json!("url"), json!("https://example.com/x"), json!("e1")));
    assert_eq!(out["changed"], false);
    assert_eq!(s.fingerprint(), before);
    assert_eq!(s.take_dirty(), None, "idle means idle: opening a web link redraws nothing");
}

#[test]
fn open_link_works_in_a_view_only_document() {
    let mut s = shapes();
    run(&mut s, "element.set_link", json!({"link": "https://example.com"}));
    s.view_only = true;
    assert_eq!(run(&mut s, "element.open_link", json!({}))["url"], "https://example.com");
}

#[test]
fn open_link_checks_a_stored_link_again() {
    let mut s = shapes();
    for bad in ["javascript:alert(1)", "data:text/html,x", "file:///etc/passwd"] {
        set_raw_link(&mut s, "e1", bad);
        let r = refuse(&mut s, "element.open_link", json!({}));
        assert_eq!(r.code, "invalid_link", "{bad}");
        assert!(r.message.starts_with("Links can only use http://, https:// or mailto:"), "{}", r.message);
    }
}

#[test]
fn open_link_on_an_element_without_a_link() {
    let mut s = shapes();
    set_raw_link(&mut s, "e2", "https://example.com");
    let r = refuse(&mut s, "element.open_link", json!({"id": "e1"}));
    assert_eq!(r.code, "no_link");
    assert_eq!(r.message, "Element `e1` has no link. Add one with `element.set_link`.");
    testing::select(&mut s, &[]);
    assert_eq!(refuse(&mut s, "element.open_link", json!({})).code, "needs_single");
    assert_eq!(refuse(&mut s, "element.open_link", json!({"id": "nope"})).code, "not_found");
}

#[test]
fn open_link_to_an_element_selects_it_and_brings_it_into_view() {
    let mut s = shapes();
    run(&mut s, "view.set_viewport", json!({"width": 800, "height": 600}));
    run(&mut s, "element.set_link", json!({"link": "#e3"}));
    let out = run(&mut s, "element.open_link", json!({}));
    assert_eq!((out["kind"].clone(), out["target"].clone(), out["changed"].clone()), (json!("element"), json!("e3"), json!(true)));
    assert_eq!(selected(&s), ["e3"]);
    // e3 is at (-150, 300), 40 wide: its centre is now the centre of the view.
    let v = s.view();
    assert!((v.scroll.x - -130.0).abs() < 1e-6 && (v.scroll.y - 320.0).abs() < 1e-6, "{:?}", v.scroll);
}

#[test]
fn open_link_to_an_element_never_zooms_in() {
    let mut s = shapes();
    run(&mut s, "view.set_viewport", json!({"width": 800, "height": 600}));
    run(&mut s, "view.set", json!({"zoom": 0.5}));
    run(&mut s, "element.set_link", json!({"link": "#e3"}));
    run(&mut s, "element.open_link", json!({}));
    assert!((s.view().zoom - 0.5).abs() < 1e-9, "a small target does not zoom in: {}", s.view().zoom);
}

#[test]
fn open_link_to_a_locked_element_moves_the_view_without_selecting() {
    let mut s = shapes();
    run(&mut s, "view.set_viewport", json!({"width": 800, "height": 600}));
    run(&mut s, "element.set_link", json!({"link": "#e3"}));
    lock_it(&mut s, "e3");
    let out = run(&mut s, "element.open_link", json!({}));
    assert_eq!(out["target"], "e3");
    assert_eq!(selected(&s), ["e1"], "a locked element cannot be selected");
    assert!((s.view().scroll.y - 320.0).abs() < 1e-6);
}

#[test]
fn open_link_to_a_deleted_element_is_refused() {
    let mut s = shapes();
    run(&mut s, "element.set_link", json!({"link": "#e3"}));
    run(&mut s, "element.delete", json!({"ids": ["e3"]}));
    testing::select(&mut s, &[eid("e1")]);
    let r = refuse(&mut s, "element.open_link", json!({}));
    assert_eq!(r.code, "not_found");
    assert!(r.message.starts_with("No element matches `e3`."), "{}", r.message);
}

#[test]
fn links_are_a_query_field() {
    let mut s = shapes();
    run(&mut s, "element.set_link", json!({"link": "https://example.com"}));
    let out = run(&mut s, "element.query", json!({"filter": {"ids": ["e1", "e2"]}, "fields": ["id", "link"]}));
    assert_eq!(out["items"][0]["link"], "https://example.com");
    assert_eq!(out["items"][1]["link"], Value::Null);
}

#[test]
fn a_link_survives_a_duplicate() {
    let mut s = shapes();
    run(&mut s, "element.set_link", json!({"link": "https://example.com"}));
    let out = run(&mut s, "element.duplicate", json!({}));
    let copy = out["ids"][0].as_str().unwrap().to_owned();
    assert_eq!(link_of(&s, &copy).as_deref(), Some("https://example.com"));
}
