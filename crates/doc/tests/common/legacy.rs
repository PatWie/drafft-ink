#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::field_reassign_with_default,
    clippy::cast_possible_truncation
)]
//! The small old-format converter from `crates/scene/tests/old_format.rs` (copied, because test files cannot be
//! imported across crates). It follows the mapping table in docs/model.md section 10 and turns every legacy fixture
//! into a valid `Document`. The real migration is `io`'s job.

use base64::Engine as _;
use drafftink_scene::*;
use kurbo::Point;
use serde_json::Value;

struct Conv {
    ids: SeqIds,
    elements: Vec<Element>,
    files: Vec<(FileId, FileEntry)>,
}

fn f(v: &Value, key: &str, default: f64) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(default)
}

fn color(v: &Value) -> Color {
    let c = |k| v.get(k).and_then(Value::as_u64).unwrap_or(0) as u8;
    Color::rgba(c("r"), c("g"), c("b"), c("a"))
}

fn pt(v: &Value) -> Point {
    Point::new(f(v, "x", 0.0), f(v, "y", 0.0))
}

fn style(v: &Value) -> Style {
    let s = &v["style"];
    let mut st = Style::default();
    st.stroke_color = color(&s["stroke_color"]);
    st.stroke_width = f(s, "stroke_width", 2.0).max(0.0);
    st.fill_color = s.get("fill_color").filter(|c| !c.is_null()).map(color);
    st.fill_pattern = match s.get("fill_pattern").and_then(Value::as_str).unwrap_or("Solid") {
        "Hachure" => FillPattern::Hachure,
        "ZigZag" => FillPattern::ZigZag,
        "CrossHatch" => FillPattern::CrossHatch,
        "Dots" => FillPattern::Dots,
        "Dashed" => FillPattern::Dashed,
        "ZigZagLine" => FillPattern::ZigZagLine,
        _ => FillPattern::Solid,
    };
    st.sloppiness = match s["sloppiness"].as_str().unwrap_or("Artist") {
        "Architect" => Sloppiness::Architect,
        "Cartoonist" => Sloppiness::Cartoonist,
        "Drunk" => Sloppiness::Drunk,
        _ => Sloppiness::Artist,
    };
    st.seed = s.get("seed").and_then(Value::as_u64).unwrap_or(0) as u32;
    st.opacity = f(s, "opacity", 1.0).clamp(0.0, 1.0);
    st
}

fn stroke_style(v: &Value, st: &mut Style) {
    st.stroke_style = match v.get("stroke_style").and_then(Value::as_str) {
        Some("Dashed") => StrokeStyle::Dashed,
        Some("Dotted") => StrokeStyle::Dotted,
        _ => StrokeStyle::Solid,
    };
}

impl Conv {
    fn push(&mut self, kind: ElementKind, bounds: (f64, f64, f64, f64), angle: f64, st: Style, groups: &[GroupId]) {
        let id = self.ids.element_id();
        let mut e = Element::new(id, kind, ZIndex::first()).with_style(st);
        // Negative sizes are normalised: the box is the same rectangle.
        let (x, y, w, h) = bounds;
        e.transform =
            Transform { x: x.min(x + w), y: y.min(y + h), w: w.abs(), h: h.abs(), angle, flip_x: false, flip_y: false };
        e.group_ids = groups.iter().cloned().collect();
        self.elements.push(e);
    }

    fn linear(
        &mut self,
        kind: LinearKind,
        mut pts: Vec<Point>,
        mut data: LinearData,
        angle: f64,
        st: Style,
        groups: &[GroupId],
    ) {
        if data.closed && pts.len() < 3 {
            data.closed = false;
        }
        let mut e = Element::linear(self.ids.element_id(), kind, ZIndex::first(), &pts.split_off(0)).unwrap();
        let (points, transform) = (e.points().unwrap().to_vec(), e.transform);
        data.points = points;
        e.kind = if kind == LinearKind::Arrow { ElementKind::Arrow(data) } else { ElementKind::Line(data) };
        e.transform = Transform { angle, ..transform };
        e.style = st;
        e.group_ids = groups.iter().cloned().collect();
        self.elements.push(e);
    }

    fn shape(&mut self, shape: &Value, groups: &[GroupId]) {
        let Some((tag, v)) = shape.as_object().and_then(|o| o.iter().next()) else { return };
        let mut st = style(v);
        let angle = f(v, "rotation", 0.0);
        match tag.as_str() {
            "Rectangle" => {
                st.corner_radius = f(v, "corner_radius", 0.0).max(0.0);
                let p = pt(&v["position"]);
                self.push(
                    ElementKind::Rectangle,
                    (p.x, p.y, f(v, "width", 0.0), f(v, "height", 0.0)),
                    angle,
                    st,
                    groups,
                );
            }
            "Ellipse" => {
                let (c, rx, ry) = (pt(&v["center"]), f(v, "radius_x", 0.0).abs(), f(v, "radius_y", 0.0).abs());
                self.push(ElementKind::Ellipse, (c.x - rx, c.y - ry, 2.0 * rx, 2.0 * ry), angle, st, groups);
            }
            "Line" | "Arrow" => {
                let is_arrow = tag == "Arrow";
                let mut pts = vec![pt(&v["start"])];
                pts.extend(v["intermediate_points"].as_array().into_iter().flatten().map(pt));
                pts.push(pt(&v["end"]));
                let mut d = LinearData::new(vec![]);
                d.path = match v.get("path_style").and_then(Value::as_str) {
                    Some("Flowing") => PathType::Round,
                    Some("Angular") => PathType::Elbow,
                    _ => PathType::Sharp,
                };
                d.closed = v.get("closed").and_then(Value::as_bool).unwrap_or(false);
                if is_arrow {
                    d.end_head = Head::Arrow;
                    d.head_size = Some(f(v, "head_size", 15.0));
                }
                stroke_style(v, &mut st);
                self.linear(if is_arrow { LinearKind::Arrow } else { LinearKind::Line }, pts, d, 0.0, st, groups);
            }
            "Freehand" => {
                let pts: Vec<Point> = v["points"].as_array().into_iter().flatten().map(pt).collect();
                let mut d = FreehandData::new(vec![]);
                d.pressures = v["pressures"].as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
                if d.pressures.len() != pts.len() {
                    d.pressures.clear();
                }
                d.pressure_mode = if d.pressures.is_empty() { PressureMode::Uniform } else { PressureMode::Device };
                d.closed = v.get("closed").and_then(Value::as_bool).unwrap_or(false);
                let mut e =
                    Element::new(self.ids.element_id(), ElementKind::Freehand(d), ZIndex::first()).with_style(st);
                if let ElementKind::Freehand(d) = &mut e.kind {
                    d.points = pts;
                }
                e.group_ids = groups.iter().cloned().collect();
                if e.refit_points().is_ok() {
                    self.elements.push(e);
                }
            }
            "Text" => {
                let content = v["content"].as_str().unwrap_or("").to_owned();
                let mut t = TextData::new(content);
                t.font.size = f(v, "font_size", 20.0).max(1.0);
                t.font.family = match v["font_family"].as_str() {
                    Some("NotoSans") => FontFamily::NotoSans,
                    Some("GelPenSerif") => FontFamily::GelPenSerif,
                    Some("VanillaExtract") => FontFamily::VanillaExtract,
                    _ => FontFamily::GelPen,
                };
                t.font.weight = match v["font_weight"].as_str() {
                    Some("Light") => FontWeight::Light,
                    Some("Heavy") => FontWeight::Heavy,
                    _ => FontWeight::Regular,
                };
                let colors: Vec<Option<Color>> = v["char_colors"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|c| (!c.is_null()).then(|| color(c)))
                    .collect();
                t.runs = TextData::runs_from_char_colors(&colors, t.char_count());
                let p = pt(&v["position"]);
                // Size is measured by `typeset` later; a placeholder keeps the box valid.
                let lines = t.content.lines().count().max(1) as f64;
                let w = t.content.lines().map(|l| l.chars().count()).max().unwrap_or(0) as f64 * t.font.size * 0.6;
                let h = lines * t.font.size * t.font.line_height;
                self.push(ElementKind::Text(t), (p.x, p.y, w, h), angle, st, groups);
            }
            "Math" => {
                let mut m = MathData::new(v["latex"].as_str().unwrap_or(""));
                m.font_size = f(v, "font_size", 20.0).max(1.0);
                let p = pt(&v["position"]);
                self.push(ElementKind::Math(m), (p.x, p.y, 0.0, 0.0), angle, st, groups);
            }
            "Image" => {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(v["data_base64"].as_str().unwrap_or(""))
                    .unwrap_or_default();
                let mime = match v["format"].as_str() {
                    Some("Jpeg") => "image/jpeg",
                    Some("WebP") => "image/webp",
                    _ => "image/png",
                };
                let (fid, entry) = FileEntry::new(mime, bytes);
                if !self.files.iter().any(|(i, _)| *i == fid) {
                    self.files.push((fid.clone(), entry));
                }
                let img = ImageData {
                    file_id: Some(fid),
                    natural_width: v["source_width"].as_u64().unwrap_or(0) as u32,
                    natural_height: v["source_height"].as_u64().unwrap_or(0) as u32,
                    crop: None,
                };
                let p = pt(&v["position"]);
                self.push(
                    ElementKind::Image(img),
                    (p.x, p.y, f(v, "width", 0.0), f(v, "height", 0.0)),
                    angle,
                    st,
                    groups,
                );
            }
            "Group" => {
                let children = v["children"].as_array().cloned().unwrap_or_default();
                // A group of fewer than two members counts as no group. Children carry the group id after their own.
                let gid = (children.len() >= 2).then(|| self.ids.group_id());
                for c in &children {
                    let mut stack = Vec::new();
                    let before = self.elements.len();
                    self.shape(c, &[]);
                    stack.extend(gid.iter().cloned());
                    for e in &mut self.elements[before..] {
                        e.group_ids.extend(stack.iter().cloned());
                    }
                }
                let _ = groups; // outer groups are appended by the caller's loop above
            }
            other => panic!("unknown old shape {other}"),
        }
    }
}

/// Converts an old document. Only shapes listed in `z_order` are drawn by the old app, so only they are kept.
pub fn convert(old: &Value) -> Document {
    let mut c = Conv { ids: SeqIds::new(), elements: Vec::new(), files: Vec::new() };
    for id in old["z_order"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        if let Some(shape) = old["shapes"].get(id) {
            c.shape(shape, &[]);
        }
    }
    let keys = ZIndex::rebalance(c.elements.len()).unwrap();
    for (e, k) in c.elements.iter_mut().zip(keys) {
        e.index = k;
    }
    let mut doc = Document::new();
    doc.name = old["name"].as_str().filter(|n| !n.is_empty()).unwrap_or("Untitled").to_owned();
    doc.elements = c.elements;
    doc.files = c.files.into_iter().collect();
    doc
}
