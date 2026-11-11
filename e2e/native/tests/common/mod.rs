//! Helpers shared by the native e2e tests.
#![allow(dead_code)]

use drafftink_e2e::{E2eError, NativeApp};
use serde_json::{Value, json};

/// A string at a JSON pointer, or an error that names the pointer.
pub fn text<'a>(v: &'a Value, pointer: &str) -> Result<&'a str, E2eError> {
    v.pointer(pointer).and_then(Value::as_str).ok_or_else(|| E2eError::Shape(format!("no string at {pointer} in {v}")))
}

/// A number at a JSON pointer.
pub fn num(v: &Value, pointer: &str) -> Result<f64, E2eError> {
    v.pointer(pointer).and_then(Value::as_f64).ok_or_else(|| E2eError::Shape(format!("no number at {pointer} in {v}")))
}

/// An array at a JSON pointer.
pub fn list<'a>(v: &'a Value, pointer: &str) -> Result<&'a Vec<Value>, E2eError> {
    v.pointer(pointer).and_then(Value::as_array).ok_or_else(|| E2eError::Shape(format!("no array at {pointer} in {v}")))
}

/// Creates a box (rectangle by default) and returns its id.
pub fn make_box(
    app: &mut NativeApp,
    kind: &str,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    fill: Option<&str>,
) -> Result<String, E2eError> {
    let mut args = json!({ "kind": kind, "bounds": { "x": x, "y": y, "w": w, "h": h } });
    if let (Some(fill), Some(map)) = (fill, args.as_object_mut()) {
        map.insert("style".into(), json!({ "fill_color": fill, "fill_pattern": "solid" }));
    }
    let made = app.exec("element.create", args)?;
    Ok(text(&made, "/id")?.to_owned())
}

/// The row of one element: `bounds`, `bindings`, `kind`, `points`.
pub fn row(app: &mut NativeApp, id: &str) -> Result<Value, E2eError> {
    let r = app.exec("element.query", json!({ "filter": { "ids": [id] }, "fields": ["id", "kind", "bounds", "bindings", "points", "text", "group_ids", "frame"] }))?;
    list(&r, "/items")?.first().cloned().ok_or_else(|| E2eError::Shape(format!("element {id} is missing")))
}

/// How many elements the document has.
pub fn count(app: &mut NativeApp) -> Result<u64, E2eError> {
    let r = app.exec("element.query", json!({ "limit": 1 }))?;
    r.get("total").and_then(Value::as_u64).ok_or_else(|| E2eError::Shape("no total".into()))
}

/// Runs `history.undo` for `steps` steps.
pub fn undo(app: &mut NativeApp, steps: u32) -> Result<Value, E2eError> {
    app.exec("history.undo", json!({ "steps": steps }))
}
