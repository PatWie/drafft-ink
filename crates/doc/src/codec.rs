//! Element <-> Loro fields. See `docs/crdt.md` for the schema; this file is its implementation.
//!
//! An element is one `LoroMap` of flat fields. Nested model structs are flattened with dotted keys
//! (`style.stroke_color`, `kind.font.size`) so that concurrent edits of different fields merge. A few values stay
//! whole on purpose (a binding, a crop, `custom`, the point list): they are only meaningful as a unit.
//! The flat form is produced from the model's own serde output, so the model and the schema cannot drift apart:
//! a new field in `scene` shows up here without a code change.

use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

use drafftink_scene::limits::{MAX_POINTS, MAX_TEXT_CHARS};
use drafftink_scene::{Element, ElementId, ElementKind, Fields, Style, TextStyle, validate_element};
use loro::{Container, LoroMap, LoroValue, ValueOrContainer};
use serde_json::{Map, Number, Value};

use crate::error::DocError;
use crate::text;

/// Deepest nesting accepted in a stored value. Hostile peers can write arbitrarily deep values.
const MAX_DEPTH: usize = 24;
/// Keys (last path segment) whose object value is stored whole.
const ATOMIC: [&str; 4] = ["start_binding", "end_binding", "crop", "custom"];
/// Keys (last path segment) whose array of numbers is packed into little-endian f64 bytes.
const PACKED_PAIRS: &str = "points";
const PACKED_SINGLES: &str = "pressures";

pub(crate) const K_KIND: &str = "kind";
pub(crate) const K_CONTENT: &str = "kind.content";
const K_RUNS_JSON: &str = "runs";

/// One stored field.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Flat {
    /// A plain Loro value (last writer wins).
    Value(LoroValue),
    /// A text element's content and colour runs, held in a `LoroText`.
    Text { content: String, runs: Vec<drafftink_scene::TextRun> },
}

pub(crate) type FlatMap = BTreeMap<String, Flat>;

/// Which group of the model a stored key belongs to. Unknown keys belong to none, so the store never touches or
/// deletes fields written by a newer minor version.
pub(crate) fn group_of(key: &str) -> Fields {
    match key {
        "x" | "y" => Fields::POSITION,
        "w" | "h" => Fields::SIZE,
        "angle" => Fields::ANGLE,
        "flip_x" | "flip_y" => Fields::FLIP,
        K_KIND => Fields::KIND,
        "index" => Fields::INDEX,
        "group_ids" => Fields::GROUPS,
        "frame_id" => Fields::FRAME,
        "locked" => Fields::LOCKED,
        "deleted" => Fields::DELETED,
        "name" => Fields::NAME,
        "link" => Fields::LINK,
        "custom" => Fields::CUSTOM,
        k if k.starts_with("kind.") => Fields::KIND,
        k if k.starts_with("style.") => Fields::STYLE,
        _ => Fields::empty(),
    }
}

fn last_segment(key: &str) -> &str {
    key.rsplit('.').next().unwrap_or(key)
}

// ---------------------------------------------------------------------------------------------------------------
// JSON <-> LoroValue
// ---------------------------------------------------------------------------------------------------------------

fn bad(msg: impl Into<String>) -> DocError {
    DocError::Backend(msg.into())
}

pub(crate) fn json_to_loro(v: &Value, depth: usize) -> Result<LoroValue, DocError> {
    if depth > MAX_DEPTH {
        return Err(bad("value is nested too deeply"));
    }
    Ok(match v {
        Value::Null => LoroValue::Null,
        Value::Bool(b) => LoroValue::Bool(*b),
        Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => LoroValue::I64(i),
            (None, Some(f)) => LoroValue::Double(f),
            _ => return Err(bad("number out of range")),
        },
        Value::String(s) => LoroValue::from(s.as_str()),
        Value::Array(a) => {
            let items = a.iter().map(|x| json_to_loro(x, depth + 1)).collect::<Result<Vec<_>, _>>()?;
            LoroValue::from(items)
        }
        Value::Object(o) => {
            let mut m: HashMap<String, LoroValue> = HashMap::with_capacity(o.len());
            for (k, x) in o {
                m.insert(k.clone(), json_to_loro(x, depth + 1)?);
            }
            LoroValue::from(m)
        }
    })
}

pub(crate) fn loro_to_json(v: &LoroValue, depth: usize) -> Result<Value, DocError> {
    if depth > MAX_DEPTH {
        return Err(bad("value is nested too deeply"));
    }
    Ok(match v {
        LoroValue::Null => Value::Null,
        LoroValue::Bool(b) => Value::Bool(*b),
        LoroValue::I64(i) => Value::Number(Number::from(*i)),
        LoroValue::Double(f) => Value::Number(Number::from_f64(*f).ok_or_else(|| bad("number is not finite"))?),
        LoroValue::String(s) => Value::String(s.to_string()),
        LoroValue::List(l) => {
            Value::Array(l.iter().map(|x| loro_to_json(x, depth + 1)).collect::<Result<Vec<_>, _>>()?)
        }
        LoroValue::Map(m) => {
            let mut o = Map::new();
            for (k, x) in m.iter() {
                o.insert(k.clone(), loro_to_json(x, depth + 1)?);
            }
            Value::Object(o)
        }
        LoroValue::Binary(_) | LoroValue::Container(_) => return Err(bad("unexpected binary or container value")),
    })
}

fn pack(values: impl Iterator<Item = f64>) -> LoroValue {
    let mut bytes = Vec::new();
    for v in values {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    LoroValue::from(bytes)
}

fn unpack(bytes: &[u8], limit: usize) -> Result<Vec<f64>, DocError> {
    if !bytes.len().is_multiple_of(8) || bytes.len() / 8 > limit {
        return Err(bad("packed number list has a bad length"));
    }
    Ok(bytes
        .chunks_exact(8)
        .map(|c| {
            let mut a = [0u8; 8];
            a.copy_from_slice(c);
            f64::from_le_bytes(a)
        })
        .collect())
}

fn num(f: f64) -> Result<Value, DocError> {
    Number::from_f64(f).map(Value::Number).ok_or_else(|| bad("number is not finite"))
}

/// `[[x, y], ...]` -> packed bytes.
fn pack_pairs(arr: &[Value]) -> Result<LoroValue, DocError> {
    let mut flat = Vec::with_capacity(arr.len() * 2);
    for p in arr {
        let pair = p.as_array().ok_or_else(|| bad("a point is not a pair"))?;
        for c in pair {
            flat.push(c.as_f64().ok_or_else(|| bad("a coordinate is not a number"))?);
        }
    }
    Ok(pack(flat.into_iter()))
}

fn pack_singles(arr: &[Value]) -> Result<LoroValue, DocError> {
    let flat =
        arr.iter().map(|c| c.as_f64().ok_or_else(|| bad("a value is not a number"))).collect::<Result<Vec<_>, _>>()?;
    Ok(pack(flat.into_iter()))
}

// ---------------------------------------------------------------------------------------------------------------
// Element -> fields
// ---------------------------------------------------------------------------------------------------------------

/// The flat entries of a default `Style` / `TextStyle`: stored fields equal to these are left out.
fn defaults() -> &'static BTreeMap<String, Value> {
    static DEFAULTS: OnceLock<BTreeMap<String, Value>> = OnceLock::new();
    DEFAULTS.get_or_init(|| {
        let mut out = BTreeMap::new();
        let mut put = |prefix: &str, v: Result<Value, serde_json::Error>| {
            if let Ok(Value::Object(o)) = v {
                for (k, x) in o {
                    out.insert(format!("{prefix}.{k}"), x);
                }
            }
        };
        put("style", serde_json::to_value(Style::default()));
        put("kind.font", serde_json::to_value(TextStyle::default()));
        out
    })
}

fn flatten_value(out: &mut FlatMap, key: String, v: &Value) -> Result<(), DocError> {
    if let Some(d) = defaults().get(&key)
        && d == v
    {
        return Ok(());
    }
    match v {
        Value::Object(o) if !ATOMIC.contains(&last_segment(&key)) => {
            for (k, x) in o {
                flatten_value(out, format!("{key}.{k}"), x)?;
            }
        }
        Value::Array(a) if last_segment(&key) == PACKED_PAIRS => {
            out.insert(key, Flat::Value(pack_pairs(a)?));
        }
        Value::Array(a) if last_segment(&key) == PACKED_SINGLES => {
            out.insert(key, Flat::Value(pack_singles(a)?));
        }
        other => {
            out.insert(key, Flat::Value(json_to_loro(other, 0)?));
        }
    }
    Ok(())
}

fn flatten_kind(out: &mut FlatMap, kind: &ElementKind) -> Result<(), DocError> {
    let value = serde_json::to_value(kind).map_err(|e| bad(e.to_string()))?;
    let Value::Object(obj) = value else { return Err(bad("kind did not serialise to an object")) };
    for (k, v) in &obj {
        if k == "type" {
            out.insert(K_KIND.to_owned(), Flat::Value(json_to_loro(v, 0)?));
        } else if let ElementKind::Text(_) = kind
            && (k == "content" || k == K_RUNS_JSON)
        {
            // Both live in the LoroText, added below.
        } else {
            flatten_value(out, format!("kind.{k}"), v)?;
        }
    }
    if let ElementKind::Text(t) = kind {
        let mut runs = t.runs.clone();
        text::normalize_runs(&mut runs);
        out.insert(K_CONTENT.to_owned(), Flat::Text { content: t.content.clone(), runs });
    }
    Ok(())
}

/// The stored fields of `e` for the groups in `fields`. Absent keys mean "default"; the writer deletes stored
/// keys of these groups that are absent here.
pub(crate) fn flatten(e: &Element, fields: Fields) -> Result<FlatMap, DocError> {
    let mut out = FlatMap::new();
    let f = |x: f64| Flat::Value(LoroValue::Double(x));
    let b = |x: bool| Flat::Value(LoroValue::Bool(x));
    let s = |x: &str| Flat::Value(LoroValue::from(x));
    let t = &e.transform;
    if fields.intersects(Fields::POSITION) {
        out.insert("x".into(), f(t.x));
        out.insert("y".into(), f(t.y));
    }
    if fields.intersects(Fields::SIZE) {
        out.insert("w".into(), f(t.w));
        out.insert("h".into(), f(t.h));
    }
    if fields.intersects(Fields::ANGLE) && t.angle != 0.0 {
        out.insert("angle".into(), f(t.angle));
    }
    if fields.intersects(Fields::FLIP) {
        if t.flip_x {
            out.insert("flip_x".into(), b(true));
        }
        if t.flip_y {
            out.insert("flip_y".into(), b(true));
        }
    }
    if fields.intersects(Fields::KIND) {
        flatten_kind(&mut out, &e.kind)?;
    }
    if fields.intersects(Fields::STYLE) {
        let v = serde_json::to_value(&e.style).map_err(|er| bad(er.to_string()))?;
        if let Value::Object(o) = v {
            for (k, x) in &o {
                flatten_value(&mut out, format!("style.{k}"), x)?;
            }
        }
    }
    if fields.intersects(Fields::INDEX) {
        out.insert("index".into(), s(e.index.as_str()));
    }
    if fields.intersects(Fields::GROUPS) && !e.group_ids.is_empty() {
        let items: Vec<LoroValue> = e.group_ids.iter().map(|g| LoroValue::from(g.as_str())).collect();
        out.insert("group_ids".into(), Flat::Value(LoroValue::from(items)));
    }
    if fields.intersects(Fields::FRAME)
        && let Some(fr) = &e.frame_id
    {
        out.insert("frame_id".into(), s(fr.as_str()));
    }
    if fields.intersects(Fields::LOCKED) && e.locked {
        out.insert("locked".into(), b(true));
    }
    if fields.intersects(Fields::DELETED) && e.deleted {
        out.insert("deleted".into(), b(true));
    }
    if fields.intersects(Fields::NAME)
        && let Some(n) = &e.name
    {
        out.insert("name".into(), s(n));
    }
    if fields.intersects(Fields::LINK)
        && let Some(l) = &e.link
    {
        out.insert("link".into(), s(l));
    }
    if fields.intersects(Fields::CUSTOM)
        && let Some(c) = &e.custom
    {
        let v = serde_json::to_value(c).map_err(|er| bad(er.to_string()))?;
        out.insert("custom".into(), Flat::Value(json_to_loro(&v, 0)?));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------------------------
// Fields -> element
// ---------------------------------------------------------------------------------------------------------------

fn set_path(obj: &mut Map<String, Value>, segs: &[&str], v: Value) -> Result<(), DocError> {
    let Some((last, parents)) = segs.split_last() else { return Err(bad("empty field name")) };
    let mut cur = obj;
    for seg in parents {
        let slot = cur.entry((*seg).to_owned()).or_insert_with(|| Value::Object(Map::new()));
        cur = slot.as_object_mut().ok_or_else(|| bad("a field is both a value and a group"))?;
    }
    if cur.contains_key(*last) {
        return Err(bad("a field is both a value and a group"));
    }
    cur.insert((*last).to_owned(), v);
    Ok(())
}

fn runs_json(runs: &[drafftink_scene::TextRun]) -> Result<Value, DocError> {
    serde_json::to_value(runs).map_err(|e| bad(e.to_string()))
}

/// Decodes the element stored in `map`. Fails (the caller quarantines the element) when a field has the wrong
/// type, a value is out of range, or the element breaks a rule that cannot be repaired.
pub(crate) fn read_element(id: &ElementId, map: &LoroMap) -> Result<Element, DocError> {
    let mut top = Map::new();
    let mut transform = Map::new();
    let mut kind = Map::new();
    let mut style = Map::new();
    let mut failure: Option<DocError> = None;
    map.for_each(|key, voc| {
        if failure.is_some() {
            return;
        }
        if let Err(e) = absorb(key, voc, &mut top, &mut transform, &mut kind, &mut style) {
            failure = Some(e);
        }
    });
    if let Some(e) = failure {
        return Err(e);
    }
    top.insert("id".into(), Value::String(id.as_str().to_owned()));
    top.insert("transform".into(), Value::Object(transform));
    top.insert("kind".into(), Value::Object(kind));
    top.insert("style".into(), Value::Object(style));
    let mut e: Element = serde_json::from_value(Value::Object(top)).map_err(|er| bad(er.to_string()))?;
    if validate_element(&e).is_err() {
        // A merge of two edits of a line (one changed the points, one the box) can leave the box and the points
        // out of step. Every peer repairs the same way, from the merged points.
        if !(e.refit_points() && validate_element(&e).is_ok()) {
            let why = validate_element(&e).err().map(|x| x.to_string()).unwrap_or_default();
            return Err(bad(format!("element {id} is invalid: {why}")));
        }
    }
    Ok(e)
}

fn absorb(
    key: &str,
    voc: ValueOrContainer,
    top: &mut Map<String, Value>,
    transform: &mut Map<String, Value>,
    kind: &mut Map<String, Value>,
    style: &mut Map<String, Value>,
) -> Result<(), DocError> {
    if let Some(rest) = key.strip_prefix("kind.") {
        if rest == "content" {
            let (content, runs) = match voc {
                ValueOrContainer::Container(Container::Text(t)) => {
                    if t.len_unicode() > MAX_TEXT_CHARS {
                        return Err(bad("text is too long"));
                    }
                    (t.to_string(), text::read_runs(&t))
                }
                ValueOrContainer::Value(LoroValue::String(s)) => (s.to_string(), Vec::new()),
                _ => return Err(bad("text content has the wrong type")),
            };
            kind.insert("content".into(), Value::String(content));
            if !runs.is_empty() {
                kind.insert(K_RUNS_JSON.into(), runs_json(&runs)?);
            }
            return Ok(());
        }
        let segs: Vec<&str> = rest.split('.').collect();
        return set_path(kind, &segs, field_json(rest, voc)?);
    }
    if let Some(rest) = key.strip_prefix("style.") {
        let segs: Vec<&str> = rest.split('.').collect();
        return set_path(style, &segs, field_json(rest, voc)?);
    }
    match key {
        "x" | "y" | "w" | "h" | "angle" | "flip_x" | "flip_y" => {
            transform.insert(key.to_owned(), field_json(key, voc)?);
        }
        K_KIND => {
            kind.insert("type".into(), field_json(key, voc)?);
        }
        "index" | "group_ids" | "frame_id" | "locked" | "deleted" | "name" | "link" | "custom" => {
            top.insert(key.to_owned(), field_json(key, voc)?);
        }
        _ => {}
    }
    Ok(())
}

fn field_json(key: &str, voc: ValueOrContainer) -> Result<Value, DocError> {
    let ValueOrContainer::Value(v) = voc else { return Err(bad(format!("field {key} holds a container"))) };
    match (&v, last_segment(key)) {
        (LoroValue::Binary(b), PACKED_PAIRS) => {
            let nums = unpack(b, MAX_POINTS.saturating_mul(2))?;
            let mut pts = Vec::with_capacity(nums.len() / 2);
            for p in nums.chunks_exact(2) {
                let (x, y) = (p.first().copied().unwrap_or(0.0), p.get(1).copied().unwrap_or(0.0));
                pts.push(Value::Array(vec![num(x)?, num(y)?]));
            }
            if nums.len() % 2 != 0 {
                return Err(bad("odd number of coordinates"));
            }
            Ok(Value::Array(pts))
        }
        (LoroValue::Binary(b), PACKED_SINGLES) => {
            let nums = unpack(b, MAX_POINTS)?;
            Ok(Value::Array(nums.into_iter().map(num).collect::<Result<Vec<_>, _>>()?))
        }
        _ => loro_to_json(&v, 0),
    }
}
