//! Reads the old `CanvasDocument` JSON (`docs/formats.md` section 1) leniently.
//!
//! The old loader failed the whole file on one bad shape. This one keeps every shape it can, defaults what the old
//! serde attributes defaulted (and what the old file revisions lacked), and reports the rest as warnings.

use std::collections::HashSet;

use drafftink_scene::limits::MAX_COORD;
use drafftink_scene::{Color, FillPattern, FontFamily, FontWeight, PathType, Sloppiness, StrokeStyle};
use kurbo::Point;
use serde_json::{Map, Value};

use super::MAX_NEST;
use super::model::{Body, ImageFormat, OImage, OLinear, OShape, OStyle, OText, OldDoc};
use crate::error::{Expected, ImportReport, IoError};

/// True when `v` has the structure of an old document (`shapes` object and `z_order` array).
pub(crate) fn looks_like_old_document(v: &Value) -> bool {
    v.get("shapes").is_some_and(Value::is_object) && v.get("z_order").is_some_and(Value::is_array)
}

/// Reads a whole old document.
pub(crate) fn read_document(v: &Value, report: &mut ImportReport) -> Result<OldDoc, IoError> {
    let Some(obj) = v.as_object() else {
        return Err(IoError::WrongFormat {
            expected: Expected::LegacyDocument,
            hint: "the top level is not an object".into(),
        });
    };
    let (Some(shapes), Some(order)) =
        (obj.get("shapes").and_then(Value::as_object), obj.get("z_order").and_then(Value::as_array))
    else {
        return Err(IoError::WrongFormat {
            expected: Expected::LegacyDocument,
            hint: "it needs a `shapes` object and a `z_order` array".into(),
        });
    };
    let mut out = OldDoc { name: obj.get("name").and_then(Value::as_str).map(str::to_owned), ..OldDoc::default() };
    let mut listed: HashSet<&str> = HashSet::new();
    for key in order.iter().filter_map(Value::as_str) {
        let Some(shape) = shapes.get(key) else {
            report.warn(format!("z_order lists {key:?}, which has no shape; skipped"));
            continue;
        };
        if !listed.insert(key) {
            report.warn(format!("z_order lists {key:?} twice; the later entry is skipped"));
            continue;
        }
        if let Some(mut s) = read_shape(shape, 0, report) {
            // The old app used the map key for lookups and the inner id for the shape. Prefer the inner id.
            if s.id.is_none() {
                s.id = Some(key.to_owned());
            }
            out.shapes.push(s);
        }
    }
    out.undrawn = shapes.keys().filter(|k| !listed.contains(k.as_str())).count();
    if out.undrawn > 0 {
        report.warn(format!(
            "{} stored shape(s) are not in z_order; the old app never drew them, so they are dropped",
            out.undrawn
        ));
    }
    Ok(out)
}

/// Reads a bare list of shapes (the old internal clipboard form).
pub(crate) fn read_shape_list(v: &Value, report: &mut ImportReport) -> Vec<OShape> {
    v.as_array().into_iter().flatten().filter_map(|s| read_shape(s, 0, report)).collect()
}

/// A finite number clamped to the coordinate range, or `default`.
pub(crate) fn num(v: Option<&Value>, default: f64) -> f64 {
    match v.and_then(Value::as_f64) {
        Some(x) if x.is_finite() => x.clamp(-MAX_COORD, MAX_COORD),
        _ => default,
    }
}

fn field(obj: &Map<String, Value>, key: &str, default: f64) -> f64 {
    num(obj.get(key), default)
}

pub(crate) fn point(v: Option<&Value>) -> Point {
    match v {
        Some(Value::Object(o)) => Point::new(field(o, "x", 0.0), field(o, "y", 0.0)),
        Some(Value::Array(a)) if a.len() >= 2 => Point::new(num(a.first(), 0.0), num(a.get(1), 0.0)),
        _ => Point::ZERO,
    }
}

pub(crate) fn points(v: Option<&Value>) -> Vec<Point> {
    v.and_then(Value::as_array).map(|a| a.iter().map(|p| point(Some(p))).collect()).unwrap_or_default()
}

fn channel(v: Option<&Value>, default: u8) -> u8 {
    match v.and_then(Value::as_f64) {
        Some(x) if x.is_finite() => x.round().clamp(0.0, 255.0) as u8,
        _ => default,
    }
}

fn color(v: Option<&Value>) -> Option<Color> {
    let o = v?.as_object()?;
    Some(Color::rgba(channel(o.get("r"), 0), channel(o.get("g"), 0), channel(o.get("b"), 0), channel(o.get("a"), 255)))
}

fn text_of(obj: &Map<String, Value>, key: &str) -> Option<String> {
    obj.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn read_style(obj: &Map<String, Value>, report: &mut ImportReport) -> OStyle {
    let empty = Map::new();
    let s = obj.get("style").and_then(Value::as_object).unwrap_or(&empty);
    let mut st = OStyle::default();
    if let Some(c) = color(s.get("stroke_color")) {
        st.stroke = c;
    }
    st.stroke_width = field(s, "stroke_width", 2.0);
    st.fill = color(s.get("fill_color"));
    st.fill_pattern = match s.get("fill_pattern").and_then(Value::as_str) {
        None | Some("Solid") => FillPattern::Solid,
        Some("Hachure") => FillPattern::Hachure,
        Some("ZigZag") => FillPattern::ZigZag,
        Some("CrossHatch") => FillPattern::CrossHatch,
        Some("Dots") => FillPattern::Dots,
        Some("Dashed") => FillPattern::Dashed,
        Some("ZigZagLine") => FillPattern::ZigZagLine,
        Some(other) => {
            report.warn(format!("unknown fill pattern {:?}; used solid", clip(other)));
            FillPattern::Solid
        }
    };
    st.sloppiness = match s.get("sloppiness").and_then(Value::as_str) {
        None | Some("Artist") => Sloppiness::Artist,
        Some("Architect") => Sloppiness::Architect,
        Some("Cartoonist") => Sloppiness::Cartoonist,
        Some("Drunk") => Sloppiness::Drunk,
        Some(other) => {
            report.warn(format!("unknown sloppiness {:?}; used artist", clip(other)));
            Sloppiness::Artist
        }
    };
    st.seed = s.get("seed").and_then(Value::as_f64).filter(|x| x.is_finite() && *x >= 0.0).map(|x| x as u64 as u32);
    st.opacity = s.get("opacity").and_then(Value::as_f64).filter(|x| x.is_finite()).unwrap_or(1.0).clamp(0.0, 1.0);
    st
}

fn clip(s: &str) -> String {
    s.chars().take(40).collect()
}

fn path_style(obj: &Map<String, Value>) -> PathType {
    match obj.get("path_style").and_then(Value::as_str) {
        Some("Flowing") => PathType::Round,
        Some("Angular") => PathType::Elbow,
        _ => PathType::Sharp,
    }
}

fn stroke_style(obj: &Map<String, Value>) -> StrokeStyle {
    match obj.get("stroke_style").and_then(Value::as_str) {
        Some("Dashed") => StrokeStyle::Dashed,
        Some("Dotted") => StrokeStyle::Dotted,
        _ => StrokeStyle::Solid,
    }
}

fn read_linear(obj: &Map<String, Value>, with_closed: bool) -> OLinear {
    let mut pts = vec![point(obj.get("start"))];
    pts.extend(points(obj.get("intermediate_points")));
    pts.push(point(obj.get("end")));
    OLinear {
        points: pts,
        path: path_style(obj),
        stroke_style: stroke_style(obj),
        closed: with_closed && obj.get("closed").and_then(Value::as_bool).unwrap_or(false),
    }
}

/// Reads one externally tagged shape (`{"Rectangle": {...}}`). `None` for anything that is not a shape.
pub(crate) fn read_shape(v: &Value, depth: usize, report: &mut ImportReport) -> Option<OShape> {
    let Some((tag, inner)) = v.as_object().filter(|o| o.len() == 1).and_then(|o| o.iter().next()) else {
        report.warn("a shape is not a single-key object; skipped");
        return None;
    };
    let Some(obj) = inner.as_object() else {
        report.warn(format!("the {:?} shape has no fields; skipped", clip(tag)));
        return None;
    };
    let style = read_style(obj, report);
    let rotation = field(obj, "rotation", 0.0);
    let body = match tag.as_str() {
        "Rectangle" => Body::Rect {
            pos: point(obj.get("position")),
            w: field(obj, "width", 0.0),
            h: field(obj, "height", 0.0),
            radius: field(obj, "corner_radius", 0.0),
            rotation,
        },
        "Ellipse" => Body::Ellipse {
            center: point(obj.get("center")),
            rx: field(obj, "radius_x", 0.0),
            ry: field(obj, "radius_y", 0.0),
            rotation,
        },
        "Line" => Body::Line(read_linear(obj, true)),
        "Arrow" => Body::Arrow(read_linear(obj, false), field(obj, "head_size", 15.0)),
        "Freehand" => Body::Freehand {
            points: points(obj.get("points")),
            pressures: obj
                .get("pressures")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|p| num(Some(p), 1.0).clamp(0.0, 1.0)).collect())
                .unwrap_or_default(),
            closed: obj.get("closed").and_then(Value::as_bool).unwrap_or(false),
        },
        "Text" => Body::Text(OText {
            pos: point(obj.get("position")),
            content: text_of(obj, "content").unwrap_or_default(),
            size: field(obj, "font_size", 20.0),
            family: match obj.get("font_family").and_then(Value::as_str) {
                Some("NotoSans") => FontFamily::NotoSans,
                Some("GelPenSerif") => FontFamily::GelPenSerif,
                Some("VanillaExtract") => FontFamily::VanillaExtract,
                _ => FontFamily::GelPen,
            },
            weight: match obj.get("font_weight").and_then(Value::as_str) {
                Some("Light") => FontWeight::Light,
                Some("Heavy") => FontWeight::Heavy,
                _ => FontWeight::Regular,
            },
            rotation,
            char_colors: obj
                .get("char_colors")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|c| color(Some(c))).collect())
                .unwrap_or_default(),
        }),
        "Math" => Body::Math {
            pos: point(obj.get("position")),
            latex: text_of(obj, "latex").unwrap_or_default(),
            size: field(obj, "font_size", 20.0),
            rotation,
        },
        "Image" => Body::Image(read_image(obj, rotation, report)),
        "Group" => {
            if depth >= MAX_NEST {
                report.warn("a group is nested too deeply; its content is dropped");
                return None;
            }
            let children = obj
                .get("children")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|c| read_shape(c, depth + 1, report)).collect())
                .unwrap_or_default();
            Body::Group { children, rotation }
        }
        other => {
            report.warn(format!("unknown shape type {:?}; skipped", clip(other)));
            return None;
        }
    };
    Some(OShape { id: text_of(obj, "id"), style, body })
}

fn read_image(obj: &Map<String, Value>, rotation: f64, report: &mut ImportReport) -> OImage {
    use base64::Engine as _;
    let data = match obj.get("data_base64").and_then(Value::as_str) {
        Some(s) => base64::engine::general_purpose::STANDARD
            .decode(s.trim())
            .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(s.trim().trim_end_matches('=')))
            .unwrap_or_else(|_| {
                report.warn("an image has invalid base64 data; the picture is dropped");
                Vec::new()
            }),
        None => Vec::new(),
    };
    let px = |key: &str| {
        obj.get(key)
            .and_then(Value::as_f64)
            .filter(|x| x.is_finite() && *x >= 0.0)
            .map_or(0, |x| x.min(f64::from(u32::MAX)) as u32)
    };
    OImage {
        pos: point(obj.get("position")),
        w: field(obj, "width", 0.0),
        h: field(obj, "height", 0.0),
        source_w: px("source_width"),
        source_h: px("source_height"),
        format: match obj.get("format").and_then(Value::as_str) {
            Some("Png") => Some(ImageFormat::Png),
            Some("Jpeg") => Some(ImageFormat::Jpeg),
            Some("WebP") => Some(ImageFormat::WebP),
            _ => None,
        },
        data,
        rotation,
    }
}
