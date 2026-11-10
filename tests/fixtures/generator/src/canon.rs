//! Canonical documents: the old serializer's output with ids and map order made deterministic.
//!
//! The old code assigns random UUIDv4 ids and keeps shapes in a HashMap, so two runs never produce
//! the same bytes. We serialize with the old `to_json`, then (a) rename every id (shapes, group
//! children) to a counter in depth-first z-order, (b) write the `shapes` map in z-order, and
//! (c) give the document a fixed id and the fixture name. Nothing else is touched. The result is
//! parsed back with the old `from_json` and compared with the old serializer's own output.

use crate::util::{fnv32, Res};
use drafftink_core::canvas::CanvasDocument;
use drafftink_core::shapes::Shape;
use serde_json::{Map, Value};
use std::collections::HashMap;

pub fn canon_id(n: u64) -> String {
    format!("00000000-0000-4000-8000-{n:012x}")
}

fn rename(shape: &mut Value, map: &mut HashMap<String, String>, next: &mut u64) -> Res<String> {
    let (variant, inner) = shape
        .as_object_mut()
        .and_then(|o| o.iter_mut().next())
        .map(|(k, v)| (k.clone(), v))
        .ok_or("shape is not a single-key object")?;
    let obj = inner.as_object_mut().ok_or("shape body is not an object")?;
    let old = obj
        .get("id")
        .and_then(Value::as_str)
        .ok_or("shape without id")?
        .to_string();
    let new = canon_id(*next);
    *next += 1;
    map.insert(old, new.clone());
    obj.insert("id".into(), Value::String(new.clone()));
    if variant == "Group" {
        // The old Group has a private ShapeStyle seeded from a process-wide counter; pin it.
        if let Some(seed) = obj.get_mut("style").and_then(|st| st.get_mut("seed")) {
            *seed = Value::from(fnv32(&new));
        }
        if let Some(Value::Array(children)) = obj.get_mut("children") {
            for c in children.iter_mut() {
                rename(c, map, next)?;
            }
        }
    }
    Ok(new)
}

/// Serialize `doc` with the old serializer and canonicalize it. Returns the file text.
pub fn canonical_json(doc: &CanvasDocument, name: &str) -> Res<String> {
    let raw: Value = serde_json::from_str(&doc.to_json()?)?;
    let z: Vec<String> = raw["z_order"]
        .as_array()
        .ok_or("no z_order")?
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    let shapes = raw["shapes"].as_object().ok_or("no shapes")?;
    if shapes.len() != z.len() {
        return Err(format!("shapes ({}) and z_order ({}) differ", shapes.len(), z.len()).into());
    }
    let mut map = HashMap::new();
    let mut next = 1u64;
    let mut new_shapes = Map::new();
    let mut new_z = Vec::new();
    for old in &z {
        let mut sv = shapes.get(old).cloned().ok_or("z_order id missing from shapes")?;
        let new = rename(&mut sv, &mut map, &mut next)?;
        new_shapes.insert(new.clone(), sv);
        new_z.push(Value::String(new));
    }
    let mut top = Map::new();
    top.insert(
        "id".into(),
        Value::String(format!("00000000-0000-4000-9000-{:012x}", fnv32(name))),
    );
    top.insert("name".into(), Value::String(name.to_string()));
    top.insert("shapes".into(), Value::Object(new_shapes));
    top.insert("z_order".into(), Value::Array(new_z));
    let text = serde_json::to_string_pretty(&Value::Object(top.clone()))?;

    // Sanity: the old loader accepts it and re-serializes to the same data (map order aside).
    let reloaded = CanvasDocument::from_json(&text)?;
    let again: Value = serde_json::from_str(&reloaded.to_json()?)?;
    let expect = Value::Object(top);
    if again != expect {
        return Err("canonical document differs from the old serializer's re-serialization".into());
    }
    Ok(text + "\n")
}

/// Pre-order index of every shape (top level in z-order, children after their group).
pub fn walk<'a>(doc: &'a CanvasDocument) -> Vec<(&'a Shape, Option<usize>, usize)> {
    fn rec<'a>(
        s: &'a Shape,
        parent: Option<usize>,
        depth: usize,
        out: &mut Vec<(&'a Shape, Option<usize>, usize)>,
    ) {
        let me = out.len();
        out.push((s, parent, depth));
        if let Shape::Group(g) = s {
            for c in g.children() {
                rec(c, Some(me), depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    for s in doc.shapes_ordered() {
        rec(s, None, 0, &mut out);
    }
    out
}
