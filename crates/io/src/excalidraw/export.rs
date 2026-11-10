//! Scene elements to Excalidraw JSON.

use std::collections::{BTreeMap, HashMap, HashSet};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use drafftink_scene::{
    BindMode, Binding, Element, ElementId, ElementKind, FileEntry, FileId, FontFamily, FontWeight, LinearData,
    PathType, PressureMode,
};
use serde_json::{Map, Value, json};

use super::map::{self, Roundness};
use super::notes::{self, Note};
use super::read::warn;
use crate::ImportReport;

/// What the `source` field of a file we write says.
pub(super) const SOURCE: &str = "https://drafft.ink";

/// Exported elements and the picture table they refer to.
pub(super) struct Exported {
    pub elements: Vec<Value>,
    pub files: Map<String, Value>,
}

/// Turns live elements into Excalidraw elements. Deleted elements are not written. `files` is the blob table of the
/// document; with `allow_images` false (a library has no picture table) image elements are skipped with a warning.
pub(super) fn export_elements(
    elements: &[Element],
    files: &BTreeMap<FileId, FileEntry>,
    allow_images: bool,
    rep: &mut ImportReport,
) -> Exported {
    let live: Vec<&Element> = elements.iter().filter(|e| !e.deleted).collect();
    let by_id: HashMap<&ElementId, &Element> = live.iter().map(|e| (&e.id, *e)).collect();

    // Images that cannot be written are left out first, so that nothing refers to them afterwards.
    let mut skipped: HashSet<&ElementId> = HashSet::new();
    for e in &live {
        if let ElementKind::Image(img) = &e.kind {
            let reason = match &img.file_id {
                _ if !allow_images => Some("a library cannot hold pictures"),
                None => Some("it has no picture yet"),
                Some(f) if !files.contains_key(f) => Some("its picture is missing from the document"),
                Some(_) => None,
            };
            if let Some(reason) = reason {
                warn(rep, || format!("image {} was not exported: {reason}", e.id));
                skipped.insert(&e.id);
            }
        }
    }
    let present = |id: &ElementId| by_id.contains_key(id) && !skipped.contains(id);

    // `boundElements` is derived: texts on their containers, arrows on both of their targets.
    let mut bound: HashMap<&ElementId, Vec<Value>> = HashMap::new();
    for e in &live {
        if skipped.contains(&e.id) {
            continue;
        }
        if let Some(c) = e.container_id().filter(|c| present(c)) {
            bound.entry(c).or_default().push(json!({"id": e.id.as_str(), "type": "text"}));
        }
        if let Some(d) = e.linear_data() {
            let mut seen: Vec<&ElementId> = Vec::new();
            for b in [&d.start_binding, &d.end_binding].into_iter().flatten() {
                if present(&b.target) && !seen.contains(&&b.target) {
                    seen.push(&b.target);
                    bound.entry(&b.target).or_default().push(json!({"id": e.id.as_str(), "type": "arrow"}));
                }
            }
        }
    }

    let mut out = Exported { elements: Vec::with_capacity(live.len()), files: Map::new() };
    for e in &live {
        if skipped.contains(&e.id) {
            continue;
        }
        let mut cx = Cx { present: &present, files, out_files: &mut out.files, rep };
        let v = element_json(e, bound.get(&e.id).map(Vec::as_slice), &mut cx);
        out.elements.push(v);
    }
    out
}

struct Cx<'a> {
    present: &'a dyn Fn(&ElementId) -> bool,
    files: &'a BTreeMap<FileId, FileEntry>,
    out_files: &'a mut Map<String, Value>,
    rep: &'a mut ImportReport,
}

/// A stable stand-in for `versionNonce`: a hash of the id, so the same document always writes the same file.
fn nonce(id: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in id.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h & 0x7fff_ffff
}

fn roundness_value(e: &Element) -> Option<(Roundness, Value)> {
    let radius = e.style.corner_radius;
    match &e.kind {
        ElementKind::Rectangle if radius > 0.0 => {
            Some((Roundness { kind: map::ROUNDNESS_ADAPTIVE, value: None }, json!({"type": map::ROUNDNESS_ADAPTIVE})))
        }
        ElementKind::Diamond if radius > 0.0 => Some((
            Roundness { kind: map::ROUNDNESS_PROPORTIONAL, value: None },
            json!({"type": map::ROUNDNESS_PROPORTIONAL}),
        )),
        ElementKind::Line(d) | ElementKind::Arrow(d) if d.path == PathType::Round => Some((
            Roundness { kind: map::ROUNDNESS_PROPORTIONAL, value: None },
            json!({"type": map::ROUNDNESS_PROPORTIONAL}),
        )),
        _ => None,
    }
}

/// The corner radius the importer derives from what the exporter wrote.
fn radius_read_back(e: &Element, r: Option<Roundness>) -> f64 {
    match (&e.kind, r) {
        (ElementKind::Rectangle | ElementKind::Diamond, Some(r)) => {
            map::corner_radius(r, e.transform.w.min(e.transform.h))
        }
        _ => 0.0,
    }
}

fn binding_json(b: &Binding) -> Value {
    json!({
        "elementId": b.target.as_str(),
        "fixedPoint": [b.fixed_point[0], b.fixed_point[1]],
        "mode": match b.mode { BindMode::Inside => "inside", BindMode::Orbit => "orbit" },
    })
}

fn element_json(e: &Element, bound: Option<&[Value]>, cx: &mut Cx) -> Value {
    let mut note = Note::default();
    let s = &e.style;
    if map::fill_from_name(map::fill_to_name(s.fill_pattern)) != Some(s.fill_pattern) {
        note.fill_pattern = Some(s.fill_pattern);
    }
    if map::sloppiness_from_roughness(map::roughness_of(s.sloppiness)) != s.sloppiness {
        note.sloppiness = Some(s.sloppiness);
    }
    let rounding = roundness_value(e);
    if (radius_read_back(e, rounding.as_ref().map(|r| r.0)) - s.corner_radius).abs() > 1e-12 {
        note.corner_radius = Some(s.corner_radius);
    }
    note.name = e.name.clone();

    let t = &e.transform;
    let (type_name, mut m) = (e.tag().name(), Map::new());
    let mut put = |k: &str, v: Value| {
        m.insert(k.to_owned(), v);
    };
    put("id", json!(e.id.as_str()));
    put("angle", json!(t.angle));
    put("strokeColor", json!(s.stroke_color.to_hex()));
    put("backgroundColor", json!(s.fill_color.map_or_else(|| "transparent".to_owned(), |c| c.to_hex())));
    put("fillStyle", json!(map::fill_to_name(s.fill_pattern)));
    put("strokeWidth", json!(s.stroke_width));
    put("strokeStyle", json!(map::stroke_style_name(s.stroke_style)));
    put("roughness", json!(map::roughness_of(s.sloppiness)));
    put("opacity", json!(s.opacity * 100.0));
    put("seed", json!(s.seed));
    put("version", json!(1));
    put("versionNonce", json!(nonce(e.id.as_str())));
    put("index", json!(e.index.as_str()));
    put("isDeleted", json!(false));
    put("groupIds", Value::Array(e.group_ids.iter().map(|g| json!(g.as_str())).collect()));
    let frame = e.frame_id.as_ref().filter(|f| (cx.present)(f));
    put("frameId", frame.map_or(Value::Null, |f| json!(f.as_str())));
    put("roundness", rounding.map_or(Value::Null, |r| r.1));
    put("boundElements", bound.map_or(Value::Null, |b| Value::Array(b.to_vec())));
    put("updated", json!(0));
    put("link", e.link.as_ref().map_or(Value::Null, |l| json!(l)));
    put("locked", json!(e.locked));

    // Kind-specific part. `(x, y, width, height)` are set here because linear elements differ.
    let mut geometry = (t.x, t.y, t.w, t.h);
    let type_written;
    match &e.kind {
        ElementKind::Rectangle | ElementKind::Diamond | ElementKind::Ellipse => type_written = type_name,
        ElementKind::Frame(f) => {
            type_written = "frame";
            put("name", f.name.as_ref().map_or(Value::Null, |n| json!(n)));
            if !f.clip {
                note.frame_clip = Some(false);
            }
        }
        ElementKind::Line(d) | ElementKind::Arrow(d) => {
            type_written = type_name;
            let is_arrow = matches!(e.kind, ElementKind::Arrow(_));
            let first = d.points.first().copied().unwrap_or_default();
            geometry.0 = t.x + first.x;
            geometry.1 = t.y + first.y;
            let mut points: Vec<Value> = d.points.iter().map(|p| json!([p.x - first.x, p.y - first.y])).collect();
            if d.closed {
                points.push(json!([0.0, 0.0]));
            }
            put("points", Value::Array(points));
            put("lastCommittedPoint", Value::Null);
            put(
                "startBinding",
                d.start_binding.as_ref().filter(|b| (cx.present)(&b.target)).map_or(Value::Null, binding_json),
            );
            put(
                "endBinding",
                d.end_binding.as_ref().filter(|b| (cx.present)(&b.target)).map_or(Value::Null, binding_json),
            );
            put("startArrowhead", map::head_to_name(d.start_head).map_or(Value::Null, |n| json!(n)));
            put("endArrowhead", map::head_to_name(d.end_head).map_or(Value::Null, |n| json!(n)));
            note.head_size = d.head_size;
            let elbow = d.path == PathType::Elbow;
            if is_arrow || elbow {
                put("elbowed", json!(elbow));
            }
            if elbow {
                put("fixedSegments", fixed_segments_json(d, first));
                put("startIsSpecial", json!(d.start_is_special));
                put("endIsSpecial", json!(d.end_is_special));
            }
            if !is_arrow {
                put("polygon", json!(d.closed));
            }
        }
        ElementKind::Freehand(d) => {
            type_written = "freedraw";
            let first = d.points.first().copied().unwrap_or_default();
            geometry.0 = t.x + first.x;
            geometry.1 = t.y + first.y;
            put("points", Value::Array(d.points.iter().map(|p| json!([p.x - first.x, p.y - first.y])).collect()));
            put(
                "pressures",
                json!(if d.pressure_mode == PressureMode::Device { d.pressures.clone() } else { Vec::new() }),
            );
            put("simulatePressure", json!(d.pressure_mode == PressureMode::Simulated));
            put("lastCommittedPoint", Value::Null);
            if d.brush != drafftink_scene::Brush::Normal {
                note.brush = Some(d.brush);
            }
            if d.highlighter {
                note.highlighter = Some(true);
            }
            if d.closed {
                note.closed = Some(true);
            }
        }
        ElementKind::Text(d) => {
            type_written = "text";
            put("text", json!(d.content));
            put("originalText", json!(d.content));
            put("fontSize", json!(d.font.size));
            let n = map::font_to_number(d.font.family, d.font.weight);
            put("fontFamily", json!(n));
            if let Some((rf, rw)) = map::font_from_number(n) {
                if rf != d.font.family {
                    note.font_family = Some(d.font.family);
                }
                if rw != d.font.weight {
                    note.font_weight = Some(d.font.weight);
                }
            }
            put("textAlign", json!(align_name(d.font.align)));
            put("verticalAlign", json!(valign_name(d.font.vertical_align)));
            let container = d.container_id.as_ref().filter(|c| (cx.present)(c));
            put("containerId", container.map_or(Value::Null, |c| json!(c.as_str())));
            put("lineHeight", json!(d.font.line_height));
            put("autoResize", json!(d.auto_resize));
            if let (Some(_), Some(lp)) = (container, d.label_position) {
                put("labelPosition", json!(lp));
            }
            note.runs = d.runs.clone();
        }
        ElementKind::Math(d) => {
            // Excalidraw has no formula element: the source goes out as text and the note marks it.
            type_written = "text";
            put("text", json!(d.latex));
            put("originalText", json!(d.latex));
            put("fontSize", json!(d.font_size));
            put("fontFamily", json!(map::font_to_number(FontFamily::GelPen, FontWeight::Regular)));
            put("textAlign", json!("left"));
            put("verticalAlign", json!("top"));
            put("containerId", Value::Null);
            put("lineHeight", json!(1.25));
            put("autoResize", json!(true));
            note.math = true;
        }
        ElementKind::Image(d) => {
            type_written = "image";
            let file_id = d.file_id.as_ref();
            put("fileId", file_id.map_or(Value::Null, |f| json!(f.as_str())));
            put("status", json!("saved"));
            put("scale", json!([if t.flip_x { -1 } else { 1 }, if t.flip_y { -1 } else { 1 }]));
            put(
                "crop",
                d.crop.as_ref().map_or(Value::Null, |c| {
                    json!({
                        "x": c.x, "y": c.y, "width": c.width, "height": c.height,
                        "naturalWidth": d.natural_width, "naturalHeight": d.natural_height,
                    })
                }),
            );
            if let Some((id, entry)) = file_id.and_then(|f| cx.files.get(f).map(|e| (f, e))) {
                cx.out_files.entry(id.as_str().to_owned()).or_insert_with(|| {
                    json!({
                        "id": id.as_str(),
                        "mimeType": entry.mime,
                        "dataURL": format!("data:{};base64,{}", entry.mime, STANDARD.encode(&entry.bytes)),
                        "created": 0,
                    })
                });
            }
        }
    }
    put("type", json!(type_written));
    put("x", json!(geometry.0));
    put("y", json!(geometry.1));
    put("width", json!(geometry.2));
    put("height", json!(geometry.3));

    // customData: the element's own map plus the note.
    let mut custom: Map<String, Value> = e.custom.iter().flatten().map(|(k, v)| (k.clone(), v.clone())).collect();
    if let Some(n) = note.to_value() {
        if custom.contains_key(notes::KEY) {
            warn(cx.rep, || {
                format!(
                    "element {} has its own `customData.{}`, which is replaced by the export note",
                    e.id,
                    notes::KEY
                )
            });
        }
        custom.insert(notes::KEY.to_owned(), n);
    }
    if !custom.is_empty() {
        m.insert("customData".to_owned(), Value::Object(custom));
    }
    Value::Object(m)
}

fn fixed_segments_json(d: &LinearData, first: kurbo::Point) -> Value {
    if d.fixed_segments.is_empty() {
        return Value::Null;
    }
    Value::Array(
        d.fixed_segments
            .iter()
            .map(|s| {
                json!({
                    "index": s.index,
                    "start": [s.start.x - first.x, s.start.y - first.y],
                    "end": [s.end.x - first.x, s.end.y - first.y],
                })
            })
            .collect(),
    )
}

fn align_name(a: drafftink_scene::TextAlign) -> &'static str {
    match a {
        drafftink_scene::TextAlign::Left => "left",
        drafftink_scene::TextAlign::Center => "center",
        drafftink_scene::TextAlign::Right => "right",
    }
}

fn valign_name(a: drafftink_scene::VerticalAlign) -> &'static str {
    match a {
        drafftink_scene::VerticalAlign::Top => "top",
        drafftink_scene::VerticalAlign::Middle => "middle",
        drafftink_scene::VerticalAlign::Bottom => "bottom",
    }
}
