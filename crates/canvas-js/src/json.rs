//! The JSON layer of the shape model.
//!
//! A shape is the scene's own element layout (`docs/model.md`): `{ id, kind: { type, ... }, transform: { x, y, w,
//! h, angle }, style, group_ids, name, ... }`. Writing it out in full is tedious, so `add` and `update` accept a
//! flat shorthand that [`expand`] turns into the full layout:
//!
//! * `x`, `y`, `w`, `h`, `angle`, `flip_x`, `flip_y` at the top level go to `transform`;
//! * `type` and every key that is not an element field go to `kind`, so a line is
//!   `{ type: "line", x: 0, y: 0, points: [[0, 0], [100, 50]] }`.
//!
//! `update` takes a merge patch (RFC 7396): objects merge key by key, anything else replaces, `null` removes.

use serde_json::{Map, Value};

use crate::error::ApiError;

/// A JSON object.
pub(crate) type Obj = Map<String, Value>;

/// The fields of an element itself; any other top-level key of a shape belongs to its kind.
const ELEMENT_FIELDS: &[&str] = &[
    "id",
    "kind",
    "transform",
    "style",
    "index",
    "group_ids",
    "frame_id",
    "locked",
    "deleted",
    "name",
    "link",
    "custom",
    "version",
];

/// The shorthand keys that belong to `transform`.
const TRANSFORM_KEYS: &[&str] = &["x", "y", "w", "h", "angle", "flip_x", "flip_y"];

/// Whether the shorthand starts a new element or patches one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Expand {
    /// A new element: the type is required and a missing box is zero.
    Create,
    /// A patch: only what is given.
    Patch,
}

fn object(value: Value, what: &str) -> Result<Obj, ApiError> {
    match value {
        Value::Object(o) => Ok(o),
        other => Err(ApiError::invalid(format!(
            "{what} must be an object such as {{ type: \"rectangle\", x: 0, y: 0, w: 100, h: 60 }}, not {}",
            kind_name(&other)
        ))),
    }
}

fn kind_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

fn take_object(obj: &mut Obj, key: &str) -> Result<Obj, ApiError> {
    match obj.remove(key) {
        None => Ok(Obj::new()),
        Some(v) => object(v, &format!("`{key}`")),
    }
}

/// Turns the shorthand into the element layout. See the module docs.
pub(crate) fn expand(value: Value, mode: Expand) -> Result<Obj, ApiError> {
    let mut obj = object(value, "a shape")?;
    let mut transform = take_object(&mut obj, "transform")?;
    for key in TRANSFORM_KEYS {
        if let Some(v) = obj.remove(*key) {
            transform.insert((*key).to_owned(), v);
        }
    }
    let mut kind = take_object(&mut obj, "kind")?;
    if let Some(t) = obj.remove("type") {
        kind.insert("type".to_owned(), t);
    }
    let loose: Vec<String> = obj.keys().filter(|k| !ELEMENT_FIELDS.contains(&k.as_str())).cloned().collect();
    for key in loose {
        if let Some(v) = obj.remove(&key) {
            kind.insert(key, v);
        }
    }
    if mode == Expand::Create {
        if !kind.contains_key("type") {
            return Err(ApiError::invalid(
                "a shape needs a type: rectangle, diamond, ellipse, line, arrow, freehand, text, math, image or frame",
            ));
        }
        for key in ["x", "y", "w", "h"] {
            transform.entry(key).or_insert(Value::from(0.0));
        }
    }
    if !transform.is_empty() {
        obj.insert("transform".to_owned(), Value::Object(transform));
    }
    if !kind.is_empty() {
        obj.insert("kind".to_owned(), Value::Object(kind));
    }
    Ok(obj)
}

/// Rejects fields on the kinds that have none. The element layout would drop them silently, and a typo such as
/// `colour` on a rectangle should be told.
pub(crate) fn check_unit_kind(shape: &Value) -> Result<(), ApiError> {
    let Some(Value::Object(kind)) = shape.get("kind") else { return Ok(()) };
    let tag = kind.get("type").and_then(Value::as_str).unwrap_or_default();
    if !matches!(tag, "rectangle" | "diamond" | "ellipse") {
        return Ok(());
    }
    match kind.keys().find(|k| *k != "type") {
        Some(extra) => Err(ApiError::invalid(format!(
            "a {tag} has no field `{extra}`. Its box is x, y, w, h and angle, and its look goes in `style`"
        ))),
        None => Ok(()),
    }
}

/// RFC 7396 merge patch: objects merge, `null` removes a key, anything else replaces.
pub(crate) fn merge_patch(target: &mut Value, patch: Value) {
    let Value::Object(patch) = patch else {
        *target = patch;
        return;
    };
    if !target.is_object() {
        *target = Value::Object(Obj::new());
    }
    let Value::Object(target) = target else { return };
    for (key, value) in patch {
        if value.is_null() {
            target.remove(&key);
        } else {
            merge_patch(target.entry(key).or_insert(Value::Null), value);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn shorthand_moves_box_keys_into_transform_and_the_rest_into_kind() {
        let out = expand(
            json!({"type": "line", "x": 5, "points": [[0, 0], [10, 10]], "style": {"stroke_width": 3}}),
            Expand::Create,
        )
        .unwrap();
        assert_eq!(out["transform"], json!({"x": 5, "y": 0.0, "w": 0.0, "h": 0.0}));
        assert_eq!(out["kind"], json!({"type": "line", "points": [[0, 0], [10, 10]]}));
        assert_eq!(out["style"], json!({"stroke_width": 3}));
    }

    #[test]
    fn the_full_layout_passes_through() {
        let full = json!({"kind": {"type": "rectangle"}, "transform": {"x": 1, "y": 2, "w": 3, "h": 4}, "name": "a"});
        let out = expand(full.clone(), Expand::Create).unwrap();
        assert_eq!(Value::Object(out), full);
    }

    #[test]
    fn element_fields_win_over_kind_fields_and_create_needs_a_type() {
        let out = expand(json!({"type": "frame", "name": "card"}), Expand::Create).unwrap();
        assert_eq!(out["name"], "card");
        assert!(out["kind"].get("name").is_none());
        let err = expand(json!({"x": 1}), Expand::Create).unwrap_err();
        assert!(err.to_string().contains("needs a type"), "{err}");
        assert!(expand(json!({"x": 1}), Expand::Patch).is_ok());
    }

    #[test]
    fn a_patch_does_not_invent_a_box() {
        let out = expand(json!({"x": 9}), Expand::Patch).unwrap();
        assert_eq!(out["transform"], json!({"x": 9}));
    }

    #[test]
    fn non_objects_are_refused_with_advice() {
        let err = expand(json!([1]), Expand::Create).unwrap_err();
        assert!(err.to_string().contains("must be an object"), "{err}");
        assert!(expand(json!({"type": "rectangle", "transform": 3}), Expand::Create).is_err());
    }

    #[test]
    fn fields_on_a_unit_kind_are_told() {
        let shape = json!({"kind": {"type": "rectangle", "colour": "red"}});
        let err = check_unit_kind(&shape).unwrap_err().to_string();
        assert!(err.contains("`colour`") && err.contains("style"), "{err}");
        assert!(check_unit_kind(&json!({"kind": {"type": "rectangle"}})).is_ok());
        assert!(check_unit_kind(&json!({"kind": {"type": "line", "points": []}})).is_ok());
    }

    #[test]
    fn merge_patch_follows_rfc_7396() {
        let mut v = json!({"a": {"b": 1, "c": 2}, "d": [1, 2], "e": 5});
        merge_patch(&mut v, json!({"a": {"b": null, "x": 9}, "d": [3], "e": 6}));
        assert_eq!(v, json!({"a": {"c": 2, "x": 9}, "d": [3], "e": 6}));
        let mut v = json!(null);
        merge_patch(&mut v, json!({"a": 1}));
        assert_eq!(v, json!({"a": 1}));
    }
}
