//! Derived facts: what the old code computes about a document. Format: ../../README.md.

use crate::canon::walk;
use crate::util::Res;
use drafftink_core::canvas::CanvasDocument;
use drafftink_core::crdt::CrdtDocument;
use drafftink_core::shapes::Shape;
use kurbo::{Point, Rect};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

pub const FACTS_VERSION: u32 = 1;
pub const OLD_COMMIT: &str = "8ce40ab";

pub fn type_name(s: &Shape) -> &'static str {
    match s {
        Shape::Rectangle(_) => "Rectangle",
        Shape::Ellipse(_) => "Ellipse",
        Shape::Line(_) => "Line",
        Shape::Arrow(_) => "Arrow",
        Shape::Freehand(_) => "Freehand",
        Shape::Text(_) => "Text",
        Shape::Group(_) => "Group",
        Shape::Image(_) => "Image",
        Shape::Math(_) => "Math",
    }
}

fn rect(r: Rect) -> Value {
    json!([r.x0, r.y0, r.x1, r.y1])
}

fn extra(s: &Shape) -> Value {
    match s {
        Shape::Text(t) => json!({
            "chars": t.content.chars().count(),
            "char_colors_len": t.char_colors.len(),
            "char_colors_set": t.char_colors.iter().filter(|c| c.is_some()).count(),
            "font_family": t.font_family.name(),
            "font_size": t.font_size,
        }),
        Shape::Math(m) => json!({"latex": m.latex, "font_size": m.font_size}),
        Shape::Image(i) => json!({
            "format": i.format.mime_type(),
            "source": [i.source_width, i.source_height],
            "data_bytes": i.data().map(|d| d.len()),
        }),
        Shape::Group(g) => json!({"children": g.children().len()}),
        Shape::Line(l) => json!({"points": 2 + l.intermediate_points.len(), "closed": l.closed}),
        Shape::Arrow(a) => json!({"points": 2 + a.intermediate_points.len(), "head_size": a.head_size}),
        Shape::Freehand(f) => json!({
            "points": f.points.len(),
            "pressures": f.pressures.len(),
            "closed": f.closed,
        }),
        _ => Value::Null,
    }
}

/// True when the shape's bounds come from the old text/math size estimate rather than geometry
/// (the old renderer replaces the estimate with a measured size at draw time, which we do not run).
fn estimated(s: &Shape) -> bool {
    match s {
        Shape::Text(_) | Shape::Math(_) => true,
        Shape::Group(g) => g.children().iter().any(estimated),
        _ => false,
    }
}

/// Probe points: a regular 8x8 grid over the inflated document bounds, plus the top-left corner and
/// centre of the first 40 top-level shapes. Derived from the bounds alone, so it is reproducible.
fn probes(doc: &CanvasDocument, idx_of: &BTreeMap<String, usize>) -> Value {
    let Some(b) = doc.bounds() else {
        return json!({"tolerance": 5.0, "hit": [], "rect": []});
    };
    let tol = 5.0;
    let inflated = b.inflate(20.0, 20.0);
    let mut pts: Vec<Point> = Vec::new();
    for iy in 0..8 {
        for ix in 0..8 {
            pts.push(Point::new(
                inflated.x0 + inflated.width() * (ix as f64 + 0.5) / 8.0,
                inflated.y0 + inflated.height() * (iy as f64 + 0.5) / 8.0,
            ));
        }
    }
    for s in doc.shapes_ordered().take(40) {
        let sb = s.bounds();
        pts.push(Point::new(sb.x0, sb.y0));
        pts.push(sb.center());
    }
    let hit: Vec<Value> = pts
        .iter()
        .map(|p| {
            let hits: Vec<usize> = doc
                .shapes_at_point(*p, tol)
                .iter()
                .filter_map(|id| idx_of.get(&id.to_string()).copied())
                .collect();
            json!({"point": [p.x, p.y], "hits": hits})
        })
        .collect();
    let (cx, cy) = (b.center().x, b.center().y);
    let rects = [
        b.inflate(10.0, 10.0),
        Rect::new(b.x0, b.y0, cx, b.y1),
        Rect::new(cx, b.y0, b.x1, b.y1),
        Rect::new(cx - b.width() / 4.0, cy - b.height() / 4.0, cx + b.width() / 4.0, cy + b.height() / 4.0),
    ];
    let rect_hits: Vec<Value> = rects
        .iter()
        .map(|r| {
            let hits: Vec<usize> = doc
                .shapes_in_rect(*r)
                .iter()
                .filter_map(|id| idx_of.get(&id.to_string()).copied())
                .collect();
            json!({"rect": rect(*r), "hits": hits})
        })
        .collect();
    json!({"tolerance": tol, "hit": hit, "rect": rect_hits})
}

/// Differences (as JSON paths) between a shape and its CRDT round trip. Group `style` is skipped
/// because the CRDT never stores it.
fn diff(path: &str, a: &Value, b: &Value, out: &mut Vec<String>) {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for (k, v) in x {
                let p = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
                if k == "style" && x.contains_key("children") {
                    continue;
                }
                match y.get(k) {
                    Some(w) => diff(&p, v, w, out),
                    None => out.push(format!("{p} (missing after round trip)")),
                }
            }
            for k in y.keys() {
                if !x.contains_key(k) {
                    out.push(format!("{path}.{k} (added by round trip)"));
                }
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                out.push(format!("{path} (len {} -> {})", x.len(), y.len()));
            } else {
                for (i, (v, w)) in x.iter().zip(y).enumerate() {
                    diff(&format!("{path}[{i}]"), v, w, out);
                }
            }
        }
        _ => {
            if a != b {
                out.push(path.to_string());
            }
        }
    }
}

/// What the old CRDT keeps: add every top-level shape to a CrdtDocument in z-order, read back.
fn crdt_roundtrip(doc: &CanvasDocument) -> Res<Value> {
    let mut crdt = CrdtDocument::new();
    for s in doc.shapes_ordered() {
        crdt.add_shape(s)?;
    }
    let back = crdt.shapes_ordered();
    let orig: Vec<&Shape> = doc.shapes_ordered().collect();
    let mut changed = Vec::new();
    for (i, (a, b)) in orig.iter().zip(back.iter()).enumerate() {
        let (va, vb) = (serde_json::to_value(a)?, serde_json::to_value(b)?);
        let mut d = Vec::new();
        diff("", &va, &vb, &mut d);
        if !d.is_empty() {
            changed.push(json!({"top_level_index": i, "type": type_name(a), "fields": d}));
        }
    }
    Ok(json!({
        "shape_count": back.len(),
        "z_order_equal": orig.len() == back.len() && orig.iter().zip(back.iter()).all(|(a, b)| a.id() == b.id()),
        "changed": changed,
    }))
}

/// Everything a checker compares. `idx` is the pre-order position (top level in z-order, group
/// children directly after their group); it is the stable key across old and new ids.
pub fn compute(doc: &CanvasDocument, with_crdt: bool) -> Res<Value> {
    let entries = walk(doc);
    let mut idx_of = BTreeMap::new();
    for (i, (s, _, _)) in entries.iter().enumerate() {
        idx_of.insert(s.id().to_string(), i);
    }
    let mut counts_top: BTreeMap<&str, usize> = BTreeMap::new();
    let mut counts_all: BTreeMap<&str, usize> = BTreeMap::new();
    let mut shapes = Vec::new();
    let mut z_counters: BTreeMap<Option<usize>, usize> = BTreeMap::new();
    let mut groups = Map::new();
    for (i, (s, parent, depth)) in entries.iter().enumerate() {
        *counts_all.entry(type_name(s)).or_default() += 1;
        if parent.is_none() {
            *counts_top.entry(type_name(s)).or_default() += 1;
        }
        let z = z_counters.entry(*parent).or_default();
        let mut o = Map::new();
        o.insert("idx".into(), json!(i));
        o.insert("id".into(), json!(s.id().to_string()));
        o.insert("type".into(), json!(type_name(s)));
        o.insert("parent".into(), json!(parent));
        o.insert("depth".into(), json!(depth));
        o.insert("z".into(), json!(*z));
        o.insert("bounds".into(), rect(s.bounds()));
        o.insert("bounds_estimated".into(), json!(estimated(s)));
        o.insert("rotation".into(), json!(s.rotation()));
        let e = extra(s);
        if !e.is_null() {
            o.insert("extra".into(), e);
        }
        *z += 1;
        shapes.push(Value::Object(o));
        if let Shape::Group(g) = s {
            let kids: Vec<usize> = g
                .children()
                .iter()
                .filter_map(|c| idx_of.get(&c.id().to_string()).copied())
                .collect();
            groups.insert(i.to_string(), json!(kids));
        }
    }
    let top_ids: Vec<usize> = doc
        .shapes_ordered()
        .filter_map(|s| idx_of.get(&s.id().to_string()).copied())
        .collect();
    let mut v = Map::new();
    v.insert("name".into(), json!(doc.name));
    v.insert("shape_count_top_level".into(), json!(top_ids.len()));
    v.insert("shape_count_total".into(), json!(entries.len()));
    v.insert("counts_top_level".into(), json!(counts_top));
    v.insert("counts_total".into(), json!(counts_all));
    v.insert("bounds".into(), doc.bounds().map(rect).unwrap_or(Value::Null));
    v.insert("bounds_estimated".into(), json!(doc.shapes_ordered().any(estimated)));
    v.insert("z_order_idx".into(), json!(top_ids));
    v.insert("shapes".into(), Value::Array(shapes));
    v.insert("groups".into(), Value::Object(groups));
    v.insert("probes".into(), probes(doc, &idx_of));
    v.insert("crdt_roundtrip".into(), if with_crdt { crdt_roundtrip(doc)? } else { Value::Null });
    Ok(Value::Object(v))
}
