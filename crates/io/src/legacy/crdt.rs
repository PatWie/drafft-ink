//! Reads an old Loro snapshot (`docs/formats.md` section 2).
//!
//! The old app only ever used the CRDT for live collaboration, so these snapshots come from sync traffic and from
//! the fixtures. The schema is a `shapes` map of flat per-shape maps, a `z_order` list and a `name` text.

use std::collections::HashSet;

use drafftink_scene::limits::MAX_COORD;
use drafftink_scene::{Color, FillPattern, FontFamily, FontWeight, PathType, Sloppiness, StrokeStyle};
use kurbo::Point;
use loro::{LoroDoc, LoroValue};
use serde_json::{Map, Value};

use super::MAX_NEST;
use super::json::{num, points};
use super::model::{Body, ImageFormat, OImage, OLinear, OShape, OStyle, OText, OldDoc};
use crate::error::{ImportReport, IoError};

/// Deepest Loro value tree this reader walks.
const MAX_VALUE_DEPTH: usize = 160;

/// Decodes a snapshot (or update) into an old document.
pub(crate) fn read_snapshot(bytes: &[u8], report: &mut ImportReport) -> Result<OldDoc, IoError> {
    let doc = LoroDoc::new();
    doc.import(bytes).map_err(|e| IoError::parse("loro snapshot", e.to_string()))?;
    let root = to_json(&doc.get_deep_value(), 0);
    let Some(obj) = root.as_object() else {
        return Err(IoError::WrongFormat {
            expected: "an old drafft.ink collaboration snapshot",
            hint: "the snapshot has no root containers".into(),
        });
    };
    let (Some(shapes), Some(order)) =
        (obj.get("shapes").and_then(Value::as_object), obj.get("z_order").and_then(Value::as_array))
    else {
        return Err(IoError::WrongFormat {
            expected: "an old drafft.ink collaboration snapshot",
            hint: "it needs a `shapes` map and a `z_order` list".into(),
        });
    };
    let mut out = OldDoc {
        name: obj.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()).map(str::to_owned),
        ..OldDoc::default()
    };
    let mut listed: HashSet<&str> = HashSet::new();
    for key in order.iter().filter_map(Value::as_str) {
        let Some(m) = shapes.get(key).and_then(Value::as_object) else {
            report.warn(format!(
                "z_order lists {:?}, which has no shape; skipped",
                key.chars().take(40).collect::<String>()
            ));
            continue;
        };
        if !listed.insert(key) {
            continue;
        }
        if let Some(s) = read_shape(m, 0, report) {
            out.shapes.push(s);
        }
    }
    out.undrawn = shapes.keys().filter(|k| !listed.contains(k.as_str())).count();
    if out.undrawn > 0 {
        report.warn(format!("{} stored shape(s) are not in z_order and are dropped", out.undrawn));
    }
    Ok(out)
}

fn to_json(v: &LoroValue, depth: usize) -> Value {
    if depth > MAX_VALUE_DEPTH {
        return Value::Null;
    }
    match v {
        LoroValue::Null | LoroValue::Binary(_) | LoroValue::Container(_) => Value::Null,
        LoroValue::Bool(b) => Value::Bool(*b),
        LoroValue::Double(d) => serde_json::Number::from_f64(*d).map_or(Value::Null, Value::Number),
        LoroValue::I64(i) => Value::from(*i),
        LoroValue::String(s) => Value::String(s.to_string()),
        LoroValue::List(l) => Value::Array(l.iter().map(|x| to_json(x, depth + 1)).collect()),
        LoroValue::Map(m) => Value::Object(m.iter().map(|(k, x)| (k.clone(), to_json(x, depth + 1))).collect()),
    }
}

fn dbl(m: &Map<String, Value>, key: &str) -> Option<f64> {
    m.get(key).and_then(Value::as_f64).filter(|x| x.is_finite()).map(|x| x.clamp(-MAX_COORD, MAX_COORD))
}

/// The old `get_i64`: integers, and doubles truncated.
fn int(m: &Map<String, Value>, key: &str) -> Option<i64> {
    let v = m.get(key)?;
    v.as_i64().or_else(|| v.as_f64().filter(|x| x.is_finite()).map(|x| x as i64))
}

fn chan(v: i64) -> u8 {
    // The old reader cast with `as u8`, which wraps.
    v as u8
}

fn style(m: &Map<String, Value>) -> Option<OStyle> {
    let stroke = Color::rgba(
        chan(int(m, "stroke_r")?),
        chan(int(m, "stroke_g")?),
        chan(int(m, "stroke_b")?),
        chan(int(m, "stroke_a")?),
    );
    let stroke_width = dbl(m, "stroke_width")?;
    let has_fill = m.get("has_fill").and_then(Value::as_bool).unwrap_or(false);
    let fill = has_fill.then(|| {
        Color::rgba(
            chan(int(m, "fill_r").unwrap_or(0)),
            chan(int(m, "fill_g").unwrap_or(0)),
            chan(int(m, "fill_b").unwrap_or(0)),
            chan(int(m, "fill_a").unwrap_or(255)),
        )
    });
    Some(OStyle {
        stroke,
        stroke_width,
        fill,
        fill_pattern: fill_pattern(int(m, "fill_pattern").unwrap_or(0)),
        sloppiness: sloppiness(int(m, "sloppiness").unwrap_or(1)),
        seed: int(m, "seed").map(|s| s as u32),
        opacity: dbl(m, "opacity").unwrap_or(1.0).clamp(0.0, 1.0),
    })
}

// The old reverse mappings send every out-of-range code to the LAST variant.
fn sloppiness(c: i64) -> Sloppiness {
    match c {
        0 => Sloppiness::Architect,
        1 => Sloppiness::Artist,
        2 => Sloppiness::Cartoonist,
        _ => Sloppiness::Drunk,
    }
}

fn fill_pattern(c: i64) -> FillPattern {
    match c {
        0 => FillPattern::Solid,
        1 => FillPattern::Hachure,
        2 => FillPattern::ZigZag,
        3 => FillPattern::CrossHatch,
        4 => FillPattern::Dots,
        5 => FillPattern::Dashed,
        _ => FillPattern::ZigZagLine,
    }
}

fn stroke_style(c: i64) -> StrokeStyle {
    match c {
        0 => StrokeStyle::Solid,
        1 => StrokeStyle::Dashed,
        _ => StrokeStyle::Dotted,
    }
}

fn path_style(c: i64) -> PathType {
    match c {
        0 => PathType::Sharp,
        1 => PathType::Round,
        _ => PathType::Elbow,
    }
}

fn family(c: i64) -> FontFamily {
    match c {
        0 => FontFamily::GelPen,
        1 => FontFamily::NotoSans,
        2 => FontFamily::GelPenSerif,
        _ => FontFamily::VanillaExtract,
    }
}

fn weight(c: i64) -> FontWeight {
    match c {
        0 => FontWeight::Light,
        1 => FontWeight::Regular,
        _ => FontWeight::Heavy,
    }
}

fn image_format(c: i64) -> ImageFormat {
    match c {
        0 => ImageFormat::Png,
        1 => ImageFormat::Jpeg,
        _ => ImageFormat::WebP,
    }
}

fn linear(m: &Map<String, Value>) -> Option<OLinear> {
    let mut pts = vec![Point::new(dbl(m, "start_x")?, dbl(m, "start_y")?)];
    pts.extend(points(m.get("intermediate_points")));
    pts.push(Point::new(dbl(m, "end_x")?, dbl(m, "end_y")?));
    Some(OLinear {
        points: pts,
        path: path_style(int(m, "path_style").unwrap_or(0)),
        stroke_style: stroke_style(int(m, "stroke_style").unwrap_or(0)),
        // The old CRDT does not store `closed`.
        closed: false,
    })
}

fn read_shape(m: &Map<String, Value>, depth: usize, report: &mut ImportReport) -> Option<OShape> {
    let kind = m.get("type").and_then(Value::as_str)?;
    let id = m.get("id").and_then(Value::as_str).map(str::to_owned);
    let rotation = dbl(m, "rotation").unwrap_or(0.0);
    if kind == "group" {
        if depth >= MAX_NEST {
            report.warn("a group is nested too deeply; its content is dropped");
            return None;
        }
        let children = m.get("children")?.as_array()?;
        let children =
            children.iter().filter_map(Value::as_object).filter_map(|c| read_shape(c, depth + 1, report)).collect();
        // The old CRDT stores neither the group's rotation nor its style.
        return Some(OShape { id, style: OStyle::default(), body: Body::Group { children, rotation: 0.0 } });
    }
    let Some(st) = style(m) else {
        report.warn(format!("a {kind} shape lacks its stroke fields; skipped"));
        return None;
    };
    let body = match kind {
        "rectangle" => Body::Rect {
            pos: Point::new(dbl(m, "x")?, dbl(m, "y")?),
            w: dbl(m, "width")?,
            h: dbl(m, "height")?,
            radius: dbl(m, "corner_radius").unwrap_or(0.0),
            rotation,
        },
        // The keys are reused with radius semantics: x, y is the centre, width and height the radii.
        "ellipse" => Body::Ellipse {
            center: Point::new(dbl(m, "x")?, dbl(m, "y")?),
            rx: dbl(m, "width")?,
            ry: dbl(m, "height")?,
            rotation,
        },
        "line" => Body::Line(linear(m)?),
        "arrow" => Body::Arrow(linear(m)?, dbl(m, "head_size").unwrap_or(15.0)),
        "freehand" => Body::Freehand {
            points: points(m.get("points")),
            pressures: m
                .get("pressures")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|p| num(Some(p), 1.0).clamp(0.0, 1.0)).collect())
                .unwrap_or_default(),
            closed: false,
        },
        "text" => Body::Text(OText {
            pos: Point::new(dbl(m, "x")?, dbl(m, "y")?),
            content: m.get("content")?.as_str()?.to_owned(),
            size: dbl(m, "font_size").unwrap_or(20.0),
            family: family(int(m, "font_family").unwrap_or(0)),
            weight: weight(int(m, "font_weight").unwrap_or(1)),
            rotation,
            char_colors: m
                .get("char_colors")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(char_color).collect())
                .unwrap_or_default(),
        }),
        "math" => Body::Math {
            pos: Point::new(dbl(m, "x")?, dbl(m, "y")?),
            latex: m.get("content")?.as_str()?.to_owned(),
            size: dbl(m, "font_size").unwrap_or(20.0),
            rotation,
        },
        "image" => Body::Image(image(m, rotation, report)?),
        other => {
            report.warn(format!("unknown shape type {:?}; skipped", other.chars().take(40).collect::<String>()));
            return None;
        }
    };
    Some(OShape { id, style: st, body })
}

fn char_color(v: &Value) -> Option<Color> {
    let a = v.as_array()?;
    let c = |i: usize| a.get(i).and_then(Value::as_f64).filter(|x| x.is_finite()).map(|x| chan(x as i64));
    Some(Color::rgba(c(0)?, c(1)?, c(2)?, c(3)?))
}

fn image(m: &Map<String, Value>, rotation: f64, report: &mut ImportReport) -> Option<OImage> {
    use base64::Engine as _;
    let data =
        base64::engine::general_purpose::STANDARD.decode(m.get("data_base64")?.as_str()?.trim()).unwrap_or_else(|_| {
            report.warn("an image has invalid base64 data; the picture is dropped");
            Vec::new()
        });
    let px = |key: &str| int(m, key).map(|v| u32::try_from(v.max(0)).unwrap_or(u32::MAX));
    Some(OImage {
        pos: Point::new(dbl(m, "x")?, dbl(m, "y")?),
        w: dbl(m, "width")?,
        h: dbl(m, "height")?,
        source_w: px("source_width")?,
        source_h: px("source_height")?,
        format: Some(image_format(int(m, "format").unwrap_or(0))),
        data,
        rotation,
    })
}
