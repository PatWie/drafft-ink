//! Tests of `tool.set_marker` and `highlight.apply`, through a stand-in writer that keeps the highlight in the
//! `custom` data of the element (the real writer is the `highlight` style property, see the module docs).

use std::collections::BTreeMap;
use std::ops::Range;

use drafftink_edit::highlight::{HighlightError, HighlightWriter, MarkerColor};
use drafftink_scene::{ElementId, SceneTx};
use serde_json::{Value, json};

use crate::ring::Origin;
use crate::session::Session;
use crate::testing;

/// Keeps the highlights in `custom["highlight"]`: the colour of the whole element, and `custom["highlight_chars"]`
/// the `[start, end, colour]` runs of a text.
#[derive(Default)]
pub(super) struct CustomWriter;

fn word(color: Option<MarkerColor>) -> Value {
    serde_json::to_value(color).unwrap_or(Value::Null)
}

fn put<T: SceneTx>(tx: &mut T, id: &ElementId, key: &str, value: Value) -> Result<bool, HighlightError> {
    let before = tx.get(id).and_then(|e| e.custom.as_ref().and_then(|c| c.get(key).cloned()));
    let after = (!value.is_null()).then_some(value);
    if before == after {
        return Ok(false);
    }
    tx.update(id, |e| {
        let custom = e.custom.get_or_insert_with(BTreeMap::new);
        match after {
            Some(v) => custom.insert(key.to_owned(), v),
            None => custom.remove(key),
        };
    })?;
    Ok(true)
}

impl HighlightWriter for CustomWriter {
    fn set_element<T: SceneTx>(&self, tx: &mut T, id: &ElementId, color: Option<MarkerColor>) -> Result<bool, HighlightError> {
        put(tx, id, "highlight", word(color))
    }

    fn set_chars<T: SceneTx>(
        &self,
        tx: &mut T,
        id: &ElementId,
        chars: Range<usize>,
        color: Option<MarkerColor>,
    ) -> Result<bool, HighlightError> {
        let mut runs: Vec<Value> = tx
            .get(id)
            .and_then(|e| e.custom.as_ref().and_then(|c| c.get("highlight_chars").and_then(|v| v.as_array().cloned())))
            .unwrap_or_default();
        runs.retain(|r| r.get(0).and_then(Value::as_u64) != Some(chars.start as u64));
        if color.is_some() {
            runs.push(json!([chars.start, chars.end, word(color)]));
        }
        put(tx, id, "highlight_chars", if runs.is_empty() { Value::Null } else { Value::Array(runs) })
    }
}

fn run(s: &mut Session, id: &str, args: Value) -> Value {
    match s.execute_from(Origin::Test, id, args) {
        Ok(v) => v,
        Err(r) => panic!("{id} refused: {} ({})", r.message, r.code),
    }
}

fn custom(s: &Session, id: &ElementId, key: &str) -> Option<Value> {
    s.element(id).and_then(|e| e.custom.as_ref().and_then(|c| c.get(key).cloned()))
}

#[test]
fn the_marker_starts_yellow_and_none_is_a_colour_of_its_own() {
    let mut s = testing::headless_session();
    assert_eq!(s.tool.marker, Some(MarkerColor::Yellow));
    let out = run(&mut s, "tool.set_marker", json!({ "color": "pink" }));
    assert_eq!((out["changed"].clone(), out["color"].clone()), (json!(true), json!("pink")));
    let out = run(&mut s, "tool.set_marker", json!({ "color": null }));
    assert_eq!((out["changed"].clone(), out["color"].clone()), (json!(true), Value::Null));
    assert_eq!(s.tool.marker, None);
    assert_eq!(run(&mut s, "tool.set_marker", json!({})), json!({ "changed": false, "color": null, "warnings": [] }));
}

#[test]
fn one_apply_is_one_undo_step_for_all_targets() {
    let mut s = testing::headless_session();
    let a = testing::add_rect(&mut s, 0.0, 0.0, 50.0, 50.0).unwrap();
    let b = testing::add_rect(&mut s, 100.0, 0.0, 50.0, 50.0).unwrap();
    let depth = s.tab().doc.undo_depth();
    let out = run(&mut s, "highlight.apply", json!({ "targets": [{ "id": a.as_str() }, { "id": b.as_str() }], "color": "green" }));
    assert_eq!(out["elements"], json!([a.as_str(), b.as_str()]));
    assert_eq!(s.tab().doc.undo_depth(), depth + 1, "one step");
    assert_eq!(custom(&s, &a, "highlight"), Some(json!("green")));
    assert_eq!(custom(&s, &b, "highlight"), Some(json!("green")));
    run(&mut s, "history.undo", json!({}));
    assert_eq!((custom(&s, &a, "highlight"), custom(&s, &b, "highlight")), (None, None), "one undo takes both back");
}

#[test]
fn none_removes_and_removing_what_is_not_there_changes_nothing() {
    let mut s = testing::headless_session();
    let a = testing::add_rect(&mut s, 0.0, 0.0, 50.0, 50.0).unwrap();
    run(&mut s, "highlight.apply", json!({ "targets": [{ "id": a.as_str() }], "color": "blue" }));
    let out = run(&mut s, "highlight.apply", json!({ "targets": [{ "id": a.as_str() }], "color": null }));
    assert_eq!(out["elements"], json!([a.as_str()]));
    assert_eq!(custom(&s, &a, "highlight"), None);
    let out = run(&mut s, "highlight.apply", json!({ "targets": [{ "id": a.as_str() }], "color": null }));
    assert_eq!(out["elements"], json!([]));
}

#[test]
fn a_refusal_changes_nothing_and_says_why() {
    let mut s = testing::headless_session();
    let a = testing::add_rect(&mut s, 0.0, 0.0, 50.0, 50.0).unwrap();
    let depth = s.tab().doc.undo_depth();
    for (args, code) in [
        (json!({ "targets": [] }), "bad_args"),
        (json!({ "targets": [{ "id": a.as_str() }, { "id": a.as_str(), "chars": [[0, 3]] }], "color": "pink" }), "not_highlightable"),
    ] {
        let r = s.execute_from(Origin::Test, "highlight.apply", args).unwrap_err();
        assert_eq!(r.code, code, "{}", r.message);
    }
    assert_eq!((s.tab().doc.undo_depth(), custom(&s, &a, "highlight")), (depth, None));
}
